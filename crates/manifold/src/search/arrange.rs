//! "Arrange by topic": the current level's cards laid out so those about the same thing sit together. The cards start
//! at their first two embedding components, then the layout settles with an extra spring between each card and its
//! most similar neighbours. The result is written to the document as one undo step.

use super::lsa::Embedding;
use crate::graph_layout::{self, Edge};
use crate::store::model::NodeId;
use crate::undo::{Moved, UndoKind};
use crate::App;
use std::collections::{HashMap, HashSet};

/// `step_sized` iterations an arrangement runs (the ordinary settle runs 90).
pub(crate) const ARRANGE_ITERATIONS: u32 = 300;
/// The most cards arranged at once.
pub(crate) const ARRANGE_MAX_CARDS: usize = 300;
/// Cards closer than this are not pulled together.
pub(crate) const SIMILARITY_EDGE_MIN: f32 = 0.30;
/// Each card is pulled toward this many of its nearest neighbours.
const NEIGHBOURS: usize = 3;
/// Half the width of the square the seed positions fill.
const SEED_RADIUS: f32 = 1500.0;

impl App {
    /// Springs between each of `ids` (indices into `emb.keys`' cards) and its `NEIGHBOURS` most similar others whose
    /// cosine is at least `SIMILARITY_EDGE_MIN`, weighted by the cosine; a pair found from both ends keeps the larger.
    pub(crate) fn similarity_edges(&self, emb: &Embedding) -> Vec<Edge> {
        let id_of = |i: usize| match emb.keys[i] {
            super::DocKey::Node(id) => id,
            super::DocKey::History(_) => unreachable!("arrange embeds cards only"),
        };
        let mut best: HashMap<(NodeId, NodeId), f32> = HashMap::new();
        for a in 0..emb.keys.len() {
            let mut near: Vec<(f32, usize)> =
                (0..emb.keys.len()).filter(|&b| b != a).map(|b| (emb.cosine(a, b), b)).collect();
            near.sort_by(|x, y| y.0.total_cmp(&x.0).then(x.1.cmp(&y.1)));
            for &(cosine, b) in near.iter().take(NEIGHBOURS).filter(|(c, _)| *c >= SIMILARITY_EDGE_MIN) {
                let key = if id_of(a) < id_of(b) { (id_of(a), id_of(b)) } else { (id_of(b), id_of(a)) };
                let weight = best.entry(key).or_insert(cosine);
                *weight = weight.max(cosine);
            }
        }
        let mut edges: Vec<Edge> = best.into_iter().map(|((from, to), weight)| Edge { from, to, weight }).collect();
        edges.sort_by_key(|e| (e.from, e.to));
        edges
    }

    /// Lays the current level out by topic (see the module comment). A pinned card stays, and so does a group.
    /// Refused with a toast above `ARRANGE_MAX_CARDS` cards.
    pub(crate) fn arrange_by_topic(&mut self) {
        let cards = self.suggestion_candidates();
        if cards.len() > ARRANGE_MAX_CARDS {
            self.queue_toast(format!("Too many cards to arrange by topic (the limit is {ARRANGE_MAX_CARDS})"));
            return;
        }
        let emb = self.embed_cards(&cards);
        let level: Vec<NodeId> = self.nodes.keys().copied().filter(|&id| self.in_level(id)).collect();
        let movable: HashSet<NodeId> = cards.iter().copied().filter(|&id| !self.doc.mirror.pinned(id)).collect();
        let pinned: HashSet<NodeId> = level.iter().copied().filter(|id| !movable.contains(id)).collect();
        let mut positions: HashMap<NodeId, egui::Vec2> = level.iter().map(|&id| (id, self.layout_pos(id))).collect();
        let mut velocities: HashMap<NodeId, egui::Vec2> = level.iter().map(|&id| (id, egui::Vec2::ZERO)).collect();
        for (i, key) in emb.keys.iter().enumerate() {
            if let (super::DocKey::Node(id), [x, y, ..]) = (key, emb.vectors[i].as_slice()) {
                if movable.contains(id) && (*x != 0.0 || *y != 0.0) {
                    positions.insert(*id, graph_layout::GRAPH_WORLD_CENTER + egui::vec2(*x, *y) * SEED_RADIUS);
                }
            }
        }
        let mut edges = self.spring_edges();
        edges.extend(self.similarity_edges(&emb));
        let sizes: HashMap<NodeId, egui::Vec2> = level
            .iter()
            .filter_map(|id| Some((*id, crate::card::node_card_size(&self.nodes.get(id)?.runtime))))
            .collect();
        for _ in 0..ARRANGE_ITERATIONS {
            graph_layout::step_sized(
                &mut positions,
                &mut velocities,
                &edges,
                &pinned,
                &sizes,
                graph_layout::GRAPH_WORLD_CENTER,
                1.0 / 60.0,
            );
        }
        let mut moved = Vec::new();
        for &id in &cards {
            if !movable.contains(&id) {
                continue;
            }
            let after = positions[&id];
            let (group, parent) = (self.doc.mirror.group(id), self.doc.mirror.parent(id));
            moved.push(Moved {
                id,
                group: (group, group),
                parent: (parent, parent),
                pos: (self.doc.mirror.pos(id), Some(after)),
            });
            self.edit_pos(id, Some(after));
            self.snap_layout(id, after);
        }
        if !moved.is_empty() {
            self.record_undo("Arranged by topic".to_owned(), UndoKind::Revert { moved, created: None });
            self.save_session();
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

    /// Six notes (three about cats, three about rust) spread over the field at fixed, interleaved spots.
    fn spread_app() -> (App, Vec<NodeId>, Vec<NodeId>) {
        let mut app = App::default();
        let cats: Vec<NodeId> = CATS.iter().map(|t| add_note(&mut app, t)).collect();
        let rust: Vec<NodeId> = RUST.iter().map(|t| add_note(&mut app, t)).collect();
        let spots = [
            (-2000.0, -1500.0),
            (2000.0, 1500.0),
            (-1900.0, 1800.0),
            (1800.0, -1700.0),
            (100.0, 2200.0),
            (-200.0, -2300.0),
        ];
        for (id, (x, y)) in cats.iter().chain(&rust).zip(spots) {
            app.edit_pos(*id, Some(egui::vec2(x, y)));
            app.snap_layout(*id, egui::vec2(x, y));
        }
        (app, cats, rust)
    }

    fn mean_distance(app: &App, pairs: impl Iterator<Item = (NodeId, NodeId)>) -> f32 {
        let d: Vec<f32> = pairs.map(|(a, b)| (app.layout_pos(a) - app.layout_pos(b)).length()).collect();
        d.iter().sum::<f32>() / d.len() as f32
    }

    fn within_and_across(app: &App, cats: &[NodeId], rust: &[NodeId]) -> (f32, f32) {
        let pairs = |x: &[NodeId]| -> Vec<(NodeId, NodeId)> {
            x.iter().enumerate().flat_map(|(i, a)| x[i + 1..].iter().map(move |b| (*a, *b))).collect()
        };
        let within = mean_distance(app, pairs(cats).into_iter().chain(pairs(rust)));
        let across = mean_distance(app, cats.iter().flat_map(|a| rust.iter().map(move |b| (*a, *b))));
        (within, across)
    }

    #[test]
    fn arrange_moves_same_topic_cards_closer_than_different_topic_cards() {
        let (mut app, cats, rust) = spread_app();
        let (before_in, before_across) = within_and_across(&app, &cats, &rust);
        assert!(before_in > before_across, "the fixture starts interleaved");
        app.arrange_by_topic();
        let (within, across) = within_and_across(&app, &cats, &rust);
        assert!(within < 0.6 * across, "within {within}, across {across}");
    }

    #[test]
    fn arrange_never_moves_a_pinned_card() {
        let (mut app, cats, _) = spread_app();
        app.toggle_pin(cats[0]);
        let (layout, doc) = (app.layout_pos(cats[0]), app.doc.mirror.pos(cats[0]));
        app.arrange_by_topic();
        assert_eq!((app.layout_pos(cats[0]), app.doc.mirror.pos(cats[0])), (layout, doc));
        assert_ne!(app.layout_pos(cats[1]), egui::vec2(2000.0, 1500.0), "the others did move");
    }

    #[test]
    fn arrange_writes_document_positions_and_one_undo_restores_them_all() {
        let (mut app, cats, rust) = spread_app();
        let all: Vec<NodeId> = cats.iter().chain(&rust).copied().collect();
        let before: Vec<_> = all.iter().map(|&id| app.doc.mirror.pos(id)).collect();
        app.arrange_by_topic();
        for &id in &all {
            assert_eq!(app.doc.mirror.pos(id), Some(app.layout_pos(id)), "document pos follows the layout");
        }
        assert_eq!(app.toasts.undo.as_ref().map(|u| u.1), Some(1), "exactly one step recorded");
        let serial = app.toasts.undo.as_ref().map(|u| u.1);
        app.step_history(serial, false);
        let after: Vec<_> = all.iter().map(|&id| app.doc.mirror.pos(id)).collect();
        assert_eq!(after, before);
        assert!(all.iter().all(|&id| Some(app.layout_pos(id)) == app.doc.mirror.pos(id)));
    }

    #[test]
    fn undo_after_arrange_skips_a_card_moved_since() {
        let (mut app, cats, _) = spread_app();
        let original = app.doc.mirror.pos(cats[1]);
        app.arrange_by_topic();
        let dragged = egui::vec2(777.0, 888.0);
        app.edit_pos(cats[2], Some(dragged));
        app.snap_layout(cats[2], dragged);
        let serial = app.toasts.undo.as_ref().map(|u| u.1);
        app.step_history(serial, false);
        assert_eq!(app.doc.mirror.pos(cats[2]), Some(dragged), "the card moved since keeps its place");
        assert_eq!(app.doc.mirror.pos(cats[1]), original, "the others return");
    }

    #[test]
    fn arrange_refuses_more_than_arrange_max_cards_with_a_toast() {
        let mut app = App::default();
        let ids: Vec<NodeId> =
            (0..=ARRANGE_MAX_CARDS).map(|i| add_note(&mut app, &format!("word{i} shared"))).collect();
        let before: Vec<_> = ids.iter().map(|&id| app.doc.mirror.pos(id)).collect();
        app.arrange_by_topic();
        assert_eq!(ids.iter().map(|&id| app.doc.mirror.pos(id)).collect::<Vec<_>>(), before);
        assert!(app.toasts.undo.is_none(), "no undo step");
        let said = app.toasts.save_toast_queue.iter().any(|m| m.contains(&ARRANGE_MAX_CARDS.to_string()));
        assert!(said, "{:?}", app.toasts.save_toast_queue);
    }
}
