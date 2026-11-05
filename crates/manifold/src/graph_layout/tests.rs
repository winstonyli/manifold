use super::*;
use crate::card_id::testutil::node;

#[test]
fn spawn_jitter_ignores_the_launchs_random_actor() {
    let other_launch = NodeId { actor: crate::store::model::ActorId(0x8465_e505_d55c_d396), counter: 5 };
    assert_eq!(spawn_jitter(node(5)), spawn_jitter(other_launch));
    assert_ne!(spawn_jitter(node(5)), spawn_jitter(node(6)));
}

fn edge(from: NodeId, to: NodeId) -> Edge {
    Edge { from, to, weight: 1.0 }
}

/// Ids come from two launches (a session imported, a jar made at startup), and each launch's actor is random, so
/// which group sorts first changes from run to run. The settled layout must not: two runs of `gate shots-aa`
/// differed by one colour level where a force sum took the other order (run `shots-aa-drift-state-2`).
#[test]
fn the_layout_does_not_depend_on_which_launch_sorts_first() {
    use crate::store::model::ActorId;
    let (low, high) = (ActorId(0x1111), ActorId(0xffff_0000));
    // Six cards from each launch, counters 0..6 in both, spread out and some overlapping, two springs.
    let settle = |first: ActorId, second: ActorId| {
        let cards: Vec<(NodeId, egui::Vec2)> = (0..12u64)
            .map(|i| {
                let actor = if i < 6 { first } else { second };
                let at = egui::vec2((i % 4) as f32 * 97.3 - 120.0, (i / 4) as f32 * 61.7 + (i as f32).sin() * 13.0);
                (NodeId { actor, counter: i % 6 }, at)
            })
            .collect();
        let mut positions: HashMap<NodeId, egui::Vec2> = cards.iter().copied().collect();
        // Ties, where the pair's order picks a direction: one card on another, and one beside another in a row.
        positions.insert(cards[1].0, cards[8].1);
        positions.insert(cards[2].0, cards[9].1 + egui::vec2(30.0, 0.0));
        let mut velocities = HashMap::new();
        let sizes: HashMap<NodeId, egui::Vec2> = cards.iter().map(|(id, _)| (*id, egui::vec2(160.0, 100.0))).collect();
        let edges = [edge(cards[0].0, cards[7].0), edge(cards[3].0, cards[10].0)];
        for _ in 0..90 {
            step_sized(&mut positions, &mut velocities, &edges, &HashSet::new(), &sizes, egui::Vec2::ZERO, 1.0 / 60.0);
        }
        // By card: the first launch's then the second's, each by counter.
        cards.iter().map(|(id, _)| (positions[id].x.to_bits(), positions[id].y.to_bits())).collect::<Vec<_>>()
    };
    assert_eq!(settle(low, high), settle(high, low));
}

#[test]
fn connected_pair_settles_near_rest_length() {
    let mut positions = HashMap::new();
    let mut velocities = HashMap::new();
    positions.insert(node(1), egui::Vec2::new(-500.0, 0.0));
    positions.insert(node(2), egui::Vec2::new(500.0, 0.0));
    velocities.insert(node(1), egui::Vec2::ZERO);
    velocities.insert(node(2), egui::Vec2::ZERO);
    let edges = vec![edge(node(1), node(2))];
    let pinned = HashSet::new();

    for _ in 0..2000 {
        step(&mut positions, &mut velocities, &edges, &pinned, egui::Vec2::ZERO, 1.0 / 60.0);
    }

    let dist = (positions[&node(1)] - positions[&node(2)]).length();
    assert!((dist - REST_LENGTH).abs() < 20.0, "expected settling near rest length {REST_LENGTH}, got {dist}");
}

/// Two cards 3000 apart joined by a spring of `weight`, 30 steps on: their distance.
fn settled_distance(weight: f32) -> f32 {
    let mut positions = HashMap::from([(node(1), egui::vec2(-1500.0, 0.0)), (node(2), egui::vec2(1500.0, 0.0))]);
    let mut velocities = HashMap::from([(node(1), egui::Vec2::ZERO), (node(2), egui::Vec2::ZERO)]);
    let edges = [Edge { from: node(1), to: node(2), weight }];
    for _ in 0..30 {
        step(&mut positions, &mut velocities, &edges, &HashSet::new(), egui::Vec2::ZERO, 1.0 / 60.0);
    }
    (positions[&node(1)] - positions[&node(2)]).length()
}

#[test]
fn a_zero_weight_edge_exerts_no_force() {
    let mut with = (HashMap::new(), HashMap::new());
    let mut without = (HashMap::new(), HashMap::new());
    for (p, v) in [&mut with, &mut without] {
        p.insert(node(1), egui::vec2(-400.0, 0.0));
        p.insert(node(2), egui::vec2(400.0, 0.0));
        v.insert(node(1), egui::Vec2::ZERO);
        v.insert(node(2), egui::Vec2::ZERO);
    }
    let dead = [Edge { from: node(1), to: node(2), weight: 0.0 }];
    for _ in 0..50 {
        step(&mut with.0, &mut with.1, &dead, &HashSet::new(), egui::Vec2::ZERO, 1.0 / 60.0);
        step(&mut without.0, &mut without.1, &[], &HashSet::new(), egui::Vec2::ZERO, 1.0 / 60.0);
    }
    assert_eq!(with.0, without.0);
}

#[test]
fn a_heavier_edge_pulls_closer_than_a_lighter_one() {
    let (light, heavy) = (settled_distance(0.2), settled_distance(1.0));
    assert!(heavy < light, "heavy {heavy}, light {light}");
}

#[test]
fn simulation_settles_to_zero_velocity() {
    let mut positions = HashMap::new();
    let mut velocities = HashMap::new();
    positions.insert(node(1), egui::Vec2::new(-50.0, 0.0));
    positions.insert(node(2), egui::Vec2::new(50.0, 0.0));
    velocities.insert(node(1), egui::Vec2::ZERO);
    velocities.insert(node(2), egui::Vec2::ZERO);
    let edges = vec![edge(node(1), node(2))];
    let pinned = HashSet::new();

    for _ in 0..5000 {
        step(&mut positions, &mut velocities, &edges, &pinned, egui::Vec2::ZERO, 1.0 / 60.0);
    }

    assert_eq!(velocities[&node(1)], egui::Vec2::ZERO);
    assert_eq!(velocities[&node(2)], egui::Vec2::ZERO);
}

fn pair_at(a: egui::Vec2, b: egui::Vec2) -> (HashMap<NodeId, egui::Vec2>, HashMap<NodeId, egui::Vec2>) {
    let positions = HashMap::from([(node(1), a), (node(2), b)]);
    let velocities = HashMap::from([(node(1), egui::Vec2::ZERO), (node(2), egui::Vec2::ZERO)]);
    (positions, velocities)
}

/// Two nodes on one point push apart.
#[test]
fn coincident_nodes_separate() {
    let (mut positions, mut velocities) = pair_at(egui::Vec2::ZERO, egui::Vec2::ZERO);
    for _ in 0..600 {
        step(&mut positions, &mut velocities, &[], &HashSet::new(), egui::Vec2::ZERO, 1.0 / 60.0);
    }
    assert!((positions[&node(1)] - positions[&node(2)]).length() > 100.0, "{positions:?}");
}

/// Two files dropped on one spot (or a card dragged onto another) ease
/// apart to about a card's width, near the drop: never flung off-canvas,
/// as an unbounded repulsion at distance ~0 did (~670 px each). Four
/// 90-tick settles, as `settle_current_level` runs them.
#[test]
fn stacked_nodes_settle_a_card_apart_near_the_drop() {
    let drop = egui::vec2(400.0, 300.0);
    for pinned in [HashSet::new(), HashSet::from([node(1)])] {
        let (mut positions, mut velocities) = pair_at(drop, drop);
        let mut farthest: f32 = 0.0;
        for tick in 1..=360 {
            step(&mut positions, &mut velocities, &[], &pinned, drop, 1.0 / 60.0);
            farthest = farthest.max((positions[&node(1)] - drop).length()).max((positions[&node(2)] - drop).length());
            if tick == 90 {
                let gap = (positions[&node(1)] - positions[&node(2)]).length();
                assert!(gap > 100.0, "one settle already parts them: {gap} ({pinned:?})");
            }
        }
        let gap = (positions[&node(1)] - positions[&node(2)]).length();
        // About a card's width (`card::GRAPH_NODE_SIZE` is 160 wide): ~200 for a free
        // pair, ~160 when one is held in place.
        assert!(gap > 150.0, "too close: {gap} ({pinned:?})");
        assert!(farthest < 200.0, "a node strayed {farthest} px from the drop ({pinned:?})");
    }
}

/// Wide cards dropped on one point end with disjoint rects whatever the
/// ids (the jitter direction comes from them), not just distant centers.
#[test]
fn wide_cards_settle_without_overlap() {
    let drop = egui::vec2(400.0, 300.0);
    for n in 0..40u64 {
        let (a, b) = (node(2 * n + 1), node(2 * n + 2));
        let mut positions = HashMap::from([(a, drop), (b, drop)]);
        let mut velocities = HashMap::from([(a, egui::Vec2::ZERO), (b, egui::Vec2::ZERO)]);
        let sizes = HashMap::from([(a, egui::vec2(250.0, 200.0)), (b, egui::vec2(225.0, 90.0))]);
        for _ in 0..360 {
            step_sized(&mut positions, &mut velocities, &[], &HashSet::new(), &sizes, drop, 1.0 / 60.0);
        }
        for id in [a, b] {
            let off = (positions[&id] - drop).length();
            assert!(off < 200.0, "seed {n}: {id} strayed {off} px from the drop");
        }
        let d = (positions[&a] - positions[&b]).abs();
        let reach = (sizes[&a] + sizes[&b]) / 2.0;
        assert!(d.x >= reach.x || d.y >= reach.y, "seed {n}: overlap, delta {d:?} reach {reach:?}");
    }
}

/// Same input, same output, whatever order the maps iterate in.
#[test]
fn the_layout_is_deterministic() {
    let ids: Vec<NodeId> = (0..30).map(|i| node(i + 1)).collect();
    // A chain plus a few cross edges, so several springs act on one node.
    let mut edges: Vec<Edge> = (0..29).map(|i| edge(node(i + 1), node(i + 2))).collect();
    edges.extend([(1, 8), (4, 21), (8, 16), (21, 16)].map(|(a, b)| edge(node(a), node(b))));
    let run = |order: &[NodeId], edges: &[Edge]| {
        let mut positions = HashMap::new();
        let mut velocities = HashMap::new();
        for id in order {
            let i = (id.counter - 1) as f32;
            // n1 sits on n0 (at the origin): a stacked pair takes its push
            // direction from the ids, not from the maps.
            let at = if i == 1.0 { egui::Vec2::ZERO } else { egui::vec2(i * 7.0 % 50.0, i * 13.0 % 40.0) };
            positions.insert(*id, at);
            velocities.insert(*id, egui::Vec2::ZERO);
        }
        for _ in 0..90 {
            step(&mut positions, &mut velocities, edges, &HashSet::new(), egui::Vec2::ZERO, 1.0 / 60.0);
        }
        let mut out: Vec<_> = positions.into_iter().collect();
        out.sort_by_key(|a| a.0);
        out
    };
    let reversed: Vec<NodeId> = ids.iter().rev().copied().collect();
    let reversed_edges: Vec<Edge> = edges.iter().rev().copied().collect();
    assert_eq!(run(&ids, &edges), run(&reversed, &reversed_edges), "edge order must not matter");
    assert_eq!(run(&ids, &edges), run(&reversed, &edges), "map order must not matter");
}

/// A pinned (dragged) node still pushes its neighbours aside.
#[test]
fn a_pinned_node_still_pushes() {
    let (mut positions, mut velocities) = pair_at(egui::Vec2::ZERO, egui::vec2(10.0, 0.0));
    step(&mut positions, &mut velocities, &[], &HashSet::from([node(1)]), egui::vec2(5.0, 0.0), 1.0 / 60.0);
    assert!(positions[&node(2)].x > 10.0, "{positions:?}");
}

#[test]
fn pinned_node_is_not_moved_by_simulation() {
    let mut positions = HashMap::new();
    let mut velocities = HashMap::new();
    positions.insert(node(1), egui::Vec2::new(0.0, 0.0));
    positions.insert(node(2), egui::Vec2::new(10.0, 0.0));
    velocities.insert(node(1), egui::Vec2::ZERO);
    velocities.insert(node(2), egui::Vec2::ZERO);
    let edges: Vec<Edge> = vec![];
    let mut pinned = HashSet::new();
    pinned.insert(node(1));

    step(&mut positions, &mut velocities, &edges, &pinned, egui::Vec2::ZERO, 1.0 / 60.0);

    assert_eq!(positions[&node(1)], egui::Vec2::new(0.0, 0.0));
}
