//! Suggested groups: the cards of the current level clustered by what their pages say (`cluster`), offered one
//! group at a time to accept or dismiss. Nothing is grouped until the user accepts.

use super::cluster::{cluster, CLUSTER_MAX_CARDS, CLUSTER_MIN_SIMILARITY};
use super::lsa::{Embedder, LsaEmbedder};
use super::{tokenize::tokens, DocKey};
use crate::store::model::NodeId;
use crate::undo::Moved;
use crate::App;
use std::collections::BTreeMap;

/// One offered group.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Suggestion {
    /// The words the cards share most, as the group's name.
    pub(crate) label: String,
    pub(crate) cards: Vec<NodeId>,
}

/// What `suggest_groups` produced, and for which cards: it stands only while those are still the level's cards.
pub(crate) struct Suggestions {
    level: Option<NodeId>,
    /// The cards that were embedded, sorted.
    candidates: Vec<NodeId>,
    pub(crate) items: Vec<Suggestion>,
}

impl App {
    /// The cards `suggest_groups` considers: the current level's non-container cards, sorted.
    fn suggestion_candidates(&self) -> Vec<NodeId> {
        let scan = self.level_scan();
        let mut cards: Vec<NodeId> =
            scan.layers.iter().flatten().copied().filter(|id| !scan.groups.contains(id)).collect();
        cards.sort_unstable();
        cards
    }

    /// Clusters the current level by topic and keeps the clusters as suggestions (none when the level has more than
    /// `CLUSTER_MAX_CARDS` cards, or nothing in common). Returns how many.
    pub(crate) fn suggest_groups(&mut self) -> usize {
        let candidates = self.suggestion_candidates();
        let mut items = Vec::new();
        if candidates.len() <= CLUSTER_MAX_CARDS {
            let texts: Vec<(NodeId, String)> =
                candidates.iter().map(|&id| (id, self.index_text(DocKey::Node(id)))).collect();
            let counted: Vec<(DocKey, BTreeMap<String, u32>)> = texts
                .iter()
                .map(|(id, text)| {
                    let mut counts = BTreeMap::new();
                    for token in tokens(text) {
                        *counts.entry(token).or_default() += 1;
                    }
                    (DocKey::Node(*id), counts)
                })
                .collect();
            let docs: Vec<(DocKey, Vec<(&str, u32)>)> = counted
                .iter()
                .map(|(key, counts)| (*key, counts.iter().map(|(t, &n)| (t.as_str(), n)).collect()))
                .collect();
            let embedding = LsaEmbedder::default().embed(&docs);
            for members in cluster(&embedding, CLUSTER_MIN_SIMILARITY) {
                let mut cards: Vec<NodeId> = members
                    .iter()
                    .filter_map(|&i| if let DocKey::Node(id) = embedding.keys[i] { Some(id) } else { None })
                    .collect();
                cards.sort_unstable();
                items.push(Suggestion { label: embedding.top_terms(&members, 2).join(", "), cards });
            }
        }
        let count = items.len();
        self.search.suggestions = Some(Suggestions { level: self.current_group, candidates, items });
        count
    }

    /// The suggestions still standing: none once the level or its set of cards has changed since.
    pub(crate) fn suggestions(&self) -> &[Suggestion] {
        match &self.search.suggestions {
            Some(s) if s.level == self.current_group && s.candidates == self.suggestion_candidates() => &s.items,
            _ => &[],
        }
    }

    /// Groups suggestion `index`'s cards in one new group named by its label, as one undo step.
    pub(crate) fn accept_suggestion(&mut self, index: usize) {
        let Some(suggestion) = self.suggestions().get(index).cloned() else { return };
        let at = suggestion.cards.first().map_or(egui::Vec2::ZERO, |&id| self.layout_pos(id));
        let before = self.membership_snapshot();
        let group = self.group_cards(suggestion.cards.iter().copied(), at);
        if !suggestion.label.is_empty() {
            self.edit_name(group, &suggestion.label);
        }
        let moved: Vec<Moved> = self.moved_since(before, &[]);
        if !moved.is_empty() {
            let message = format!("Grouped {} cards as {}", moved.len(), suggestion.label);
            self.record_undo(message, crate::undo::UndoKind::Revert { moved, created: Some(group) });
        }
        if let Some(s) = &mut self.search.suggestions {
            s.items.retain(|i| *i != suggestion);
        }
        self.save_session();
    }

    /// Drops suggestion `index` without grouping.
    pub(crate) fn dismiss_suggestion(&mut self, index: usize) {
        let Some(suggestion) = self.suggestions().get(index).cloned() else { return };
        if let Some(s) = &mut self.search.suggestions {
            s.items.retain(|i| *i != suggestion);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_sync::tests::add_note;

    const CATS: [&str; 3] = ["cat milk fur purr cat", "cat fur kitten milk", "kitten purr milk cat fur"];
    const RUST: [&str; 3] =
        ["rust cargo crate compile rust", "cargo crate rust build", "compile build crate rust cargo"];

    /// Six notes, three about cats and three about rust; returns (cats, rust).
    fn two_topic_app() -> (App, Vec<NodeId>, Vec<NodeId>) {
        let mut app = App::default();
        let cats = CATS.iter().map(|t| add_note(&mut app, t)).collect();
        let rust = RUST.iter().map(|t| add_note(&mut app, t)).collect();
        (app, cats, rust)
    }

    fn sorted(mut ids: Vec<NodeId>) -> Vec<NodeId> {
        ids.sort_unstable();
        ids
    }

    #[test]
    fn suggestions_group_the_cards_by_topic_and_are_labelled_by_their_words() {
        let (mut app, cats, rust) = two_topic_app();
        assert_eq!(app.suggest_groups(), 2);
        let found = app.suggestions();
        let by_cards = |cards: &Vec<NodeId>| found.iter().find(|s| s.cards == sorted(cards.clone())).unwrap();
        assert!(["cat", "milk", "fur", "purr", "kitten"].iter().any(|w| by_cards(&cats).label.contains(w)));
        assert!(["rust", "cargo", "crate", "compile", "build"].iter().any(|w| by_cards(&rust).label.contains(w)));
    }

    #[test]
    fn accepting_creates_the_group_and_records_one_revert() {
        let (mut app, cats, _) = two_topic_app();
        app.suggest_groups();
        let index = app.suggestions().iter().position(|s| s.cards == sorted(cats.clone())).unwrap();
        let serial = |app: &App| app.toasts.undo.as_ref().map(|u| u.1);
        let before = serial(&app);
        app.accept_suggestion(index);
        let group = app.doc.mirror.group(cats[0]).expect("grouped");
        assert!(cats.iter().all(|&c| app.doc.mirror.group(c) == Some(group)));
        assert_ne!(serial(&app), before, "one undo step recorded");
        assert!(app.suggestions().is_empty(), "the cards left the level, so the offers are stale");
    }

    #[test]
    fn undoing_an_accepted_suggestion_restores_membership_and_closes_the_empty_group() {
        let (mut app, cats, _) = two_topic_app();
        app.suggest_groups();
        let index = app.suggestions().iter().position(|s| s.cards == sorted(cats.clone())).unwrap();
        app.accept_suggestion(index);
        let group = app.doc.mirror.group(cats[0]).unwrap();
        app.step_history(None, false);
        assert!(cats.iter().all(|&c| app.doc.mirror.group(c).is_none()));
        assert!(!app.nodes.contains_key(&group), "the empty group is closed");
    }

    #[test]
    fn undo_skips_a_card_the_user_moved_again() {
        let (mut app, cats, _) = two_topic_app();
        app.suggest_groups();
        let index = app.suggestions().iter().position(|s| s.cards == sorted(cats.clone())).unwrap();
        app.accept_suggestion(index);
        let group = app.doc.mirror.group(cats[0]).unwrap();
        let other = crate::session_sync::tests::add_group(&mut app, "Elsewhere");
        app.edit_group(cats[0], Some(other));
        app.step_history(None, false);
        assert_eq!(app.doc.mirror.group(cats[0]), Some(other), "left where the user put it");
        assert!(cats[1..].iter().all(|&c| app.doc.mirror.group(c).is_none()));
        assert!(!app.nodes.contains_key(&group), "nothing is left in it, so undo closes it");
    }

    #[test]
    fn a_change_of_cards_clears_the_suggestions() {
        let (mut app, _, _) = two_topic_app();
        app.suggest_groups();
        assert_eq!(app.suggestions().len(), 2);
        add_note(&mut app, "a new card");
        assert!(app.suggestions().is_empty());
    }

    #[test]
    fn dismissing_removes_only_that_suggestion() {
        let (mut app, _, _) = two_topic_app();
        app.suggest_groups();
        app.dismiss_suggestion(0);
        assert_eq!(app.suggestions().len(), 1);
    }
}
