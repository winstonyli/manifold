use std::collections::{HashMap, HashSet};

/// Pure force-directed layout for the M5 tab graph: nodes repel each other,
/// parent→child edges pull toward a rest length, everything damps and
/// settles. No `wgpu`/`winit` types here — this only manipulates plain
/// positions/velocities so it can be unit-tested without a live window.
///
/// The graph is structurally a forest (each tab has at most one parent —
/// the tab it was opened from), but the layout itself doesn't need to know
/// that; it only sees a flat edge list.
const REPULSION_STRENGTH: f32 = 900_000.0;
/// Guards the spring's direction against dividing by zero.
const MIN_DISTANCE: f32 = 1.0;
/// Repulsion stops growing below this distance: closer nodes push with the
/// force they'd have at this distance (900,000 / 40² ≈ 560). An inverse-square
/// force clamped only at `MIN_DISTANCE` pushed two nodes on one point
/// (two files dropped on the same spot, a card dragged onto another) with
/// 900,000 on the first tick; that alone saturates `MAX_SPEED`, and under
/// `DAMPING` each card then coasted ~670 px, off the canvas. Capped here,
/// the pair builds up at most ~180 px/s, so it eases apart to the usual
/// spacing (~230 px for two free nodes) near where it was, and nodes
/// further apart than this feel exactly the force they did before.
const REPULSION_SOFT_DISTANCE: f32 = 40.0;
/// Spring constant (force per px of overlap) pushing overlapping cards apart.
const OVERLAP_STRENGTH: f32 = 40.0;
/// Overlap depth beyond which the push stops growing (max force ~640).
const OVERLAP_MAX: f32 = 16.0;
/// Clear space kept between two cards' rects.
const CARD_GAP: f32 = 8.0;
const SPRING_STRENGTH: f32 = 8.0;
const REST_LENGTH: f32 = 320.0;
const CENTERING_STRENGTH: f32 = 0.15;
// Damping trades off two things: the fraction of velocity kept each frame,
// and (less obviously) how much of a given force actually shows up as
// motion at all. Under this integrator's semi-implicit-Euler-then-damp
// step, a force sustained over many frames converges to a terminal
// velocity of `force * dt * damping / (1 - damping)` — at the old 0.85
// that coefficient was small enough that two overlapping siblings (a
// spawn-jitter hash collision, or several tabs opened in a burst) could
// sit almost fully overlapped for multiple real seconds before visibly
// separating, even though the raw repulsion force between them was far
// from zero. 0.95 (combined with the `REPULSION_STRENGTH` bump above)
// roughly triples that coefficient (0.85's ~5.7 to 0.95's ~19), so a
// genuine overlap resolves within well under a second instead of visibly
// lingering.
const DAMPING: f32 = 0.95;
const MAX_SPEED: f32 = 2000.0;
/// Below this speed a node is considered settled and snapped to a full
/// stop, rather than asymptotically approaching zero forever under
/// damping alone — without this the graph never visibly stops moving.
const SETTLE_SPEED: f32 = 1.0;

/// One force-directed simulation tick.
///
/// `positions`/`velocities` are keyed by node id and updated in place.
/// `edges` are `(parent_id, child_id)` pairs. `pinned` holds node ids being
/// dragged: their position is the caller's and they don't move, but they
/// still push the others (a dragged card shoulders its neighbours aside).
/// Ids and edges are processed in sorted order, so the result doesn't depend
/// on the maps' iteration order or the order of `edges`.
#[cfg(test)]
pub fn step(
    positions: &mut HashMap<String, egui::Vec2>,
    velocities: &mut HashMap<String, egui::Vec2>,
    edges: &[(String, String)],
    pinned: &HashSet<String>,
    center: egui::Vec2,
    dt: f32,
) {
    step_sized(positions, velocities, edges, pinned, &HashMap::new(), center, dt);
}

/// `step` where cards have extents: `sizes` maps a node id to its card size
/// (a missing id is a point). Cards whose rects, padded by `CARD_GAP`, overlap
/// are pushed apart along the axis of least overlap, so a pair never settles
/// with centers a repulsion-length apart but corners on top of each other.
pub fn step_sized(
    positions: &mut HashMap<String, egui::Vec2>,
    velocities: &mut HashMap<String, egui::Vec2>,
    edges: &[(String, String)],
    pinned: &HashSet<String>,
    sizes: &HashMap<String, egui::Vec2>,
    center: egui::Vec2,
    dt: f32,
) {
    if dt <= 0.0 {
        return;
    }
    // Owned ids: `positions` is mutated below.
    let mut ids: Vec<String> = positions.keys().cloned().collect();
    ids.sort_unstable();
    let index: HashMap<&str, usize> = ids.iter().enumerate().map(|(i, id)| (id.as_str(), i)).collect();
    let pos: Vec<egui::Vec2> = ids.iter().map(|id| positions[id]).collect();
    // Each card's size, indexed like `pos`: looked up once per step, not twice per pair.
    let size: Vec<Option<egui::Vec2>> = ids.iter().map(|id| sizes.get(id).copied()).collect();
    let mut forces = vec![egui::Vec2::ZERO; ids.len()];

    // Pairwise repulsion: O(n^2) over plain vectors.
    for i in 0..pos.len() {
        for j in (i + 1)..pos.len() {
            let delta = pos[i] - pos[j];
            let length = delta.length();
            // Two nodes on one point have no direction between them: pick a
            // fixed one from their ids so they still push apart, gently (see
            // `REPULSION_SOFT_DISTANCE`).
            let dir = if length > f32::EPSILON {
                delta / length
            } else {
                spawn_jitter(&format!("{}\u{0}{}", ids[i], ids[j])).normalized()
            };
            let dist = length.max(REPULSION_SOFT_DISTANCE);
            let force = dir * (REPULSION_STRENGTH / (dist * dist));
            forces[i] += force;
            forces[j] -= force;

            // Card extents: push apart along the axis that overlaps least.
            let (Some(a), Some(b)) = (size[i], size[j]) else { continue };
            let reach = (a + b) / 2.0 + egui::Vec2::splat(CARD_GAP);
            let overlap = egui::vec2(reach.x - delta.x.abs(), reach.y - delta.y.abs());
            if overlap.x > 0.0 && overlap.y > 0.0 {
                // Capped like the soft repulsion: a card dropped deep inside
                // another must ease out, not coast across the canvas.
                let push = if overlap.x < overlap.y {
                    egui::vec2(if delta.x < 0.0 { -1.0 } else { 1.0 } * overlap.x.min(OVERLAP_MAX), 0.0)
                } else {
                    egui::vec2(0.0, if delta.y < 0.0 { -1.0 } else { 1.0 } * overlap.y.min(OVERLAP_MAX))
                } * OVERLAP_STRENGTH;
                forces[i] += push;
                forces[j] -= push;
            }
        }
    }

    // Spring attraction along parent->child edges toward a rest length. An
    // edge to a since-removed node is ignored; the rest are applied in sorted
    // order, so the sums don't depend on the order of `edges` either.
    let mut springs: Vec<(usize, usize)> = edges
        .iter()
        .filter_map(|(parent, child)| Some((*index.get(parent.as_str())?, *index.get(child.as_str())?)))
        .collect();
    springs.sort_unstable();
    for (p, c) in springs {
        let delta = pos[c] - pos[p];
        let dist = delta.length().max(MIN_DISTANCE);
        let force = delta / dist * (SPRING_STRENGTH * (dist - REST_LENGTH));
        forces[p] += force;
        forces[c] -= force;
    }

    for (i, id) in ids.iter().enumerate() {
        // Pinned (dragged) nodes stay exactly where the caller put them, with
        // no velocity, so releasing the drag doesn't fling them.
        if pinned.contains(id) {
            velocities.insert(id.clone(), egui::Vec2::ZERO);
            continue;
        }
        // Gentle pull toward the canvas center, so a disconnected node
        // doesn't drift away.
        let force = forces[i] + (center - pos[i]) * CENTERING_STRENGTH;
        let mut vel = velocities.get(id).copied().unwrap_or(egui::Vec2::ZERO);
        vel = (vel + force * dt) * DAMPING;
        if vel.length() > MAX_SPEED {
            vel *= MAX_SPEED / vel.length();
        }
        if vel.length() < SETTLE_SPEED {
            vel = egui::Vec2::ZERO;
        }
        if let Some(p) = positions.get_mut(id) {
            *p += vel * dt;
        }
        match velocities.get_mut(id) {
            Some(v) => *v = vel,
            None => {
                velocities.insert(id.clone(), vel);
            }
        }
    }
}

/// Deterministic pseudo-random-ish spawn offset for a newly created node,
/// derived from its own id so repeated spawns don't all land exactly on
/// top of their parent (where repulsion is at its cap and still has to
/// push them a full spacing apart — see `REPULSION_SOFT_DISTANCE`). Radius is
/// close to `REST_LENGTH` itself (not a small fixed offset) so a new child
/// spawns near where the spring will settle it anyway, instead of starting
/// deep inside its parent's/siblings' repulsion field and relying on the
/// simulation to slowly push it out from near-zero distance.
pub fn spawn_jitter(seed_str: &str) -> egui::Vec2 {
    let mut hash: u32 = 2166136261;
    for b in seed_str.bytes() {
        hash ^= b as u32;
        hash = hash.wrapping_mul(16777619);
    }
    let angle = (hash % 360) as f32 * std::f32::consts::PI / 180.0;
    let radius = REST_LENGTH * 0.8 + (hash % (REST_LENGTH as u32 / 4)) as f32;
    egui::Vec2::new(angle.cos(), angle.sin()) * radius
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Today's `step_sized`, kept as the golden test's reference (wave 2, H3).
    fn step_sized_reference(
        positions: &mut HashMap<String, egui::Vec2>,
        velocities: &mut HashMap<String, egui::Vec2>,
        edges: &[(String, String)],
        pinned: &HashSet<String>,
        sizes: &HashMap<String, egui::Vec2>,
        center: egui::Vec2,
        dt: f32,
    ) {
        if dt <= 0.0 {
            return;
        }
        // Owned ids: `positions` is mutated below.
        let mut ids: Vec<String> = positions.keys().cloned().collect();
        ids.sort_unstable();
        let index: HashMap<&str, usize> = ids.iter().enumerate().map(|(i, id)| (id.as_str(), i)).collect();
        let pos: Vec<egui::Vec2> = ids.iter().map(|id| positions[id]).collect();
        let mut forces = vec![egui::Vec2::ZERO; ids.len()];

        // Pairwise repulsion: O(n^2) over plain vectors.
        for i in 0..pos.len() {
            for j in (i + 1)..pos.len() {
                let delta = pos[i] - pos[j];
                let length = delta.length();
                let dir = if length > f32::EPSILON {
                    delta / length
                } else {
                    spawn_jitter(&format!("{}\u{0}{}", ids[i], ids[j])).normalized()
                };
                let dist = length.max(REPULSION_SOFT_DISTANCE);
                let force = dir * (REPULSION_STRENGTH / (dist * dist));
                forces[i] += force;
                forces[j] -= force;

                // Card extents: push apart along the axis that overlaps least.
                let (Some(a), Some(b)) = (sizes.get(&ids[i]), sizes.get(&ids[j])) else { continue };
                let reach = (*a + *b) / 2.0 + egui::Vec2::splat(CARD_GAP);
                let overlap = egui::vec2(reach.x - delta.x.abs(), reach.y - delta.y.abs());
                if overlap.x > 0.0 && overlap.y > 0.0 {
                    let push = if overlap.x < overlap.y {
                        egui::vec2(if delta.x < 0.0 { -1.0 } else { 1.0 } * overlap.x.min(OVERLAP_MAX), 0.0)
                    } else {
                        egui::vec2(0.0, if delta.y < 0.0 { -1.0 } else { 1.0 } * overlap.y.min(OVERLAP_MAX))
                    } * OVERLAP_STRENGTH;
                    forces[i] += push;
                    forces[j] -= push;
                }
            }
        }

        let mut springs: Vec<(usize, usize)> = edges
            .iter()
            .filter_map(|(parent, child)| Some((*index.get(parent.as_str())?, *index.get(child.as_str())?)))
            .collect();
        springs.sort_unstable();
        for (p, c) in springs {
            let delta = pos[c] - pos[p];
            let dist = delta.length().max(MIN_DISTANCE);
            let force = delta / dist * (SPRING_STRENGTH * (dist - REST_LENGTH));
            forces[p] += force;
            forces[c] -= force;
        }

        for (i, id) in ids.iter().enumerate() {
            if pinned.contains(id) {
                velocities.insert(id.clone(), egui::Vec2::ZERO);
                continue;
            }
            let force = forces[i] + (center - pos[i]) * CENTERING_STRENGTH;
            let mut vel = velocities.get(id).copied().unwrap_or(egui::Vec2::ZERO);
            vel = (vel + force * dt) * DAMPING;
            if vel.length() > MAX_SPEED {
                vel *= MAX_SPEED / vel.length();
            }
            if vel.length() < SETTLE_SPEED {
                vel = egui::Vec2::ZERO;
            }
            if let Some(p) = positions.get_mut(id) {
                *p += vel * dt;
            }
            match velocities.get_mut(id) {
                Some(v) => *v = vel,
                None => {
                    velocities.insert(id.clone(), vel);
                }
            }
        }
    }

    /// Wave 2 (H3): `step_sized` over 40 cards for 90 settle steps matches today's copy bit for bit: sized and unsized
    /// cards, two on one point (the `spawn_jitter` direction), springs, and one pinned card.
    #[test]
    fn step_sized_matches_reference_bit_for_bit() {
        let ids: Vec<String> = (0..40).map(|i| format!("c{i:02}")).collect();
        let mut positions: HashMap<String, egui::Vec2> = ids
            .iter()
            .enumerate()
            .map(|(i, id)| (id.clone(), egui::vec2((i % 8) as f32 * 150.0, (i / 8) as f32 * 90.0)))
            .collect();
        let c00 = positions["c00"];
        positions.insert("c01".into(), c00);
        let mut velocities: HashMap<String, egui::Vec2> = ids.iter().map(|id| (id.clone(), egui::Vec2::ZERO)).collect();
        let sizes: HashMap<String, egui::Vec2> = (0..40)
            .filter(|i: &usize| !i.is_multiple_of(5))
            .map(|i| (ids[i].clone(), egui::vec2(160.0 + i as f32, 100.0)))
            .collect();
        let edges: Vec<(String, String)> = (1..13).map(|i| ("c00".to_string(), ids[3 * i].clone())).collect();
        let pinned = HashSet::from(["c07".to_string()]);
        let (mut ref_positions, mut ref_velocities) = (positions.clone(), velocities.clone());
        for _ in 0..90 {
            step_sized(&mut positions, &mut velocities, &edges, &pinned, &sizes, egui::Vec2::ZERO, 1.0 / 60.0);
            step_sized_reference(
                &mut ref_positions,
                &mut ref_velocities,
                &edges,
                &pinned,
                &sizes,
                egui::Vec2::ZERO,
                1.0 / 60.0,
            );
        }
        let bits = |map: &HashMap<String, egui::Vec2>| {
            let mut all: Vec<(String, u32, u32)> =
                map.iter().map(|(id, v)| (id.clone(), v.x.to_bits(), v.y.to_bits())).collect();
            all.sort();
            all
        };
        assert_eq!(bits(&positions), bits(&ref_positions));
        assert_eq!(bits(&velocities), bits(&ref_velocities));
        assert_ne!(positions["c00"], positions["c01"], "the pair on one point pushed apart");
    }

    #[test]
    fn connected_pair_settles_near_rest_length() {
        let mut positions = HashMap::new();
        let mut velocities = HashMap::new();
        positions.insert("a".to_string(), egui::Vec2::new(-500.0, 0.0));
        positions.insert("b".to_string(), egui::Vec2::new(500.0, 0.0));
        velocities.insert("a".to_string(), egui::Vec2::ZERO);
        velocities.insert("b".to_string(), egui::Vec2::ZERO);
        let edges = vec![("a".to_string(), "b".to_string())];
        let pinned = HashSet::new();

        for _ in 0..2000 {
            step(&mut positions, &mut velocities, &edges, &pinned, egui::Vec2::ZERO, 1.0 / 60.0);
        }

        let dist = (positions["a"] - positions["b"]).length();
        assert!((dist - REST_LENGTH).abs() < 20.0, "expected settling near rest length {REST_LENGTH}, got {dist}");
    }

    #[test]
    fn simulation_settles_to_zero_velocity() {
        let mut positions = HashMap::new();
        let mut velocities = HashMap::new();
        positions.insert("a".to_string(), egui::Vec2::new(-50.0, 0.0));
        positions.insert("b".to_string(), egui::Vec2::new(50.0, 0.0));
        velocities.insert("a".to_string(), egui::Vec2::ZERO);
        velocities.insert("b".to_string(), egui::Vec2::ZERO);
        let edges = vec![("a".to_string(), "b".to_string())];
        let pinned = HashSet::new();

        for _ in 0..5000 {
            step(&mut positions, &mut velocities, &edges, &pinned, egui::Vec2::ZERO, 1.0 / 60.0);
        }

        assert_eq!(velocities["a"], egui::Vec2::ZERO);
        assert_eq!(velocities["b"], egui::Vec2::ZERO);
    }

    fn pair_at(a: egui::Vec2, b: egui::Vec2) -> (HashMap<String, egui::Vec2>, HashMap<String, egui::Vec2>) {
        let positions = HashMap::from([("a".to_string(), a), ("b".to_string(), b)]);
        let velocities = HashMap::from([("a".to_string(), egui::Vec2::ZERO), ("b".to_string(), egui::Vec2::ZERO)]);
        (positions, velocities)
    }

    /// Review C-B9: two nodes on one point push apart.
    #[test]
    fn coincident_nodes_separate() {
        let (mut positions, mut velocities) = pair_at(egui::Vec2::ZERO, egui::Vec2::ZERO);
        for _ in 0..600 {
            step(&mut positions, &mut velocities, &[], &HashSet::new(), egui::Vec2::ZERO, 1.0 / 60.0);
        }
        assert!((positions["a"] - positions["b"]).length() > 100.0, "{positions:?}");
    }

    /// Two files dropped on one spot (or a card dragged onto another) ease
    /// apart to about a card's width, near the drop: never flung off-canvas,
    /// as an unbounded repulsion at distance ~0 did (~670 px each). Four
    /// 90-tick settles, as `settle_current_level` runs them.
    #[test]
    fn stacked_nodes_settle_a_card_apart_near_the_drop() {
        let drop = egui::vec2(400.0, 300.0);
        for pinned in [HashSet::new(), HashSet::from(["a".to_string()])] {
            let (mut positions, mut velocities) = pair_at(drop, drop);
            let mut farthest: f32 = 0.0;
            for tick in 1..=360 {
                step(&mut positions, &mut velocities, &[], &pinned, drop, 1.0 / 60.0);
                farthest = farthest.max((positions["a"] - drop).length()).max((positions["b"] - drop).length());
                if tick == 90 {
                    let gap = (positions["a"] - positions["b"]).length();
                    assert!(gap > 100.0, "one settle already parts them: {gap} ({pinned:?})");
                }
            }
            let gap = (positions["a"] - positions["b"]).length();
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
        for n in 0..40 {
            let (a, b) = (format!("a{n}"), format!("b{n}"));
            let mut positions = HashMap::from([(a.clone(), drop), (b.clone(), drop)]);
            let mut velocities = HashMap::from([(a.clone(), egui::Vec2::ZERO), (b.clone(), egui::Vec2::ZERO)]);
            let sizes = HashMap::from([(a.clone(), egui::vec2(250.0, 200.0)), (b.clone(), egui::vec2(225.0, 90.0))]);
            for _ in 0..360 {
                step_sized(&mut positions, &mut velocities, &[], &HashSet::new(), &sizes, drop, 1.0 / 60.0);
            }
            for id in [&a, &b] {
                let off = (positions[id] - drop).length();
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
        let ids: Vec<String> = (0..30).map(|i| format!("n{i}")).collect();
        // A chain plus a few cross edges, so several springs act on one node.
        let mut edges: Vec<(String, String)> = (0..29).map(|i| (format!("n{i}"), format!("n{}", i + 1))).collect();
        edges.extend([("n0", "n7"), ("n3", "n20"), ("n7", "n15"), ("n20", "n15")].map(|(a, b)| (a.into(), b.into())));
        let run = |order: &[String], edges: &[(String, String)]| {
            let mut positions = HashMap::new();
            let mut velocities = HashMap::new();
            for id in order {
                let i: f32 = id[1..].parse().unwrap();
                // n1 sits on n0 (at the origin): a stacked pair takes its push
                // direction from the ids, not from the maps.
                let at = if i == 1.0 { egui::Vec2::ZERO } else { egui::vec2(i * 7.0 % 50.0, i * 13.0 % 40.0) };
                positions.insert(id.clone(), at);
                velocities.insert(id.clone(), egui::Vec2::ZERO);
            }
            for _ in 0..90 {
                step(&mut positions, &mut velocities, edges, &HashSet::new(), egui::Vec2::ZERO, 1.0 / 60.0);
            }
            let mut out: Vec<_> = positions.into_iter().collect();
            out.sort_by(|a, b| a.0.cmp(&b.0));
            out
        };
        let reversed: Vec<String> = ids.iter().rev().cloned().collect();
        let reversed_edges: Vec<(String, String)> = edges.iter().rev().cloned().collect();
        assert_eq!(run(&ids, &edges), run(&reversed, &reversed_edges), "edge order must not matter");
        assert_eq!(run(&ids, &edges), run(&reversed, &edges), "map order must not matter");
    }

    /// A pinned (dragged) node still pushes its neighbours aside.
    #[test]
    fn a_pinned_node_still_pushes() {
        let (mut positions, mut velocities) = pair_at(egui::Vec2::ZERO, egui::vec2(10.0, 0.0));
        step(&mut positions, &mut velocities, &[], &HashSet::from(["a".to_string()]), egui::vec2(5.0, 0.0), 1.0 / 60.0);
        assert!(positions["b"].x > 10.0, "{positions:?}");
    }

    #[test]
    fn pinned_node_is_not_moved_by_simulation() {
        let mut positions = HashMap::new();
        let mut velocities = HashMap::new();
        positions.insert("a".to_string(), egui::Vec2::new(0.0, 0.0));
        positions.insert("b".to_string(), egui::Vec2::new(10.0, 0.0));
        velocities.insert("a".to_string(), egui::Vec2::ZERO);
        velocities.insert("b".to_string(), egui::Vec2::ZERO);
        let edges = vec![];
        let mut pinned = HashSet::new();
        pinned.insert("a".to_string());

        step(&mut positions, &mut velocities, &edges, &pinned, egui::Vec2::ZERO, 1.0 / 60.0);

        assert_eq!(positions["a"], egui::Vec2::new(0.0, 0.0));
    }
}
