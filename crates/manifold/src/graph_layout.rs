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
const MIN_DISTANCE: f32 = 1.0;
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
/// `edges` are `(parent_id, child_id)` pairs. `pinned` holds node ids
/// currently being dragged by the user: their position is assumed already
/// set by the caller (e.g. to the cursor position) and they're excluded
/// from force accumulation and integration entirely — they neither push
/// nor get pushed by the simulation while held.
pub fn step(
    positions: &mut HashMap<String, egui::Vec2>,
    velocities: &mut HashMap<String, egui::Vec2>,
    edges: &[(String, String)],
    pinned: &HashSet<String>,
    center: egui::Vec2,
    dt: f32,
) {
    if dt <= 0.0 {
        return;
    }

    let ids: Vec<String> = positions.keys().cloned().collect();
    let mut forces: HashMap<String, egui::Vec2> = ids.iter().map(|id| (id.clone(), egui::Vec2::ZERO)).collect();

    // Pairwise repulsion — O(n^2), fine for the tens-of-tabs scale this UI
    // is meant for.
    for i in 0..ids.len() {
        for j in (i + 1)..ids.len() {
            let (id_a, id_b) = (&ids[i], &ids[j]);
            let pos_a = positions[id_a];
            let pos_b = positions[id_b];
            let delta = pos_a - pos_b;
            let dist = delta.length().max(MIN_DISTANCE);
            let dir = delta / dist;
            let force = dir * (REPULSION_STRENGTH / (dist * dist));
            *forces.get_mut(id_a).unwrap() += force;
            *forces.get_mut(id_b).unwrap() -= force;
        }
    }

    // Spring attraction along parent->child edges toward a rest length.
    for (parent, child) in edges {
        let (Some(&pos_p), Some(&pos_c)) = (positions.get(parent), positions.get(child)) else {
            continue; // an edge referencing a since-removed node; ignore
        };
        let delta = pos_c - pos_p;
        let dist = delta.length().max(MIN_DISTANCE);
        let dir = delta / dist;
        let stretch = dist - REST_LENGTH;
        let force = dir * (SPRING_STRENGTH * stretch);
        if let Some(f) = forces.get_mut(parent) {
            *f += force;
        }
        if let Some(f) = forces.get_mut(child) {
            *f -= force;
        }
    }

    // Gentle pull toward the canvas center so a disconnected node (or an
    // entire orphaned subtree) doesn't drift off-screen indefinitely.
    for id in &ids {
        let pos = positions[id];
        let force = (center - pos) * CENTERING_STRENGTH;
        *forces.get_mut(id).unwrap() += force;
    }

    // Integrate. Pinned (dragged) nodes are left exactly where the caller
    // placed them, with zero velocity, so releasing the drag doesn't fling
    // the node from accumulated velocity it never actually had control over.
    for id in &ids {
        if pinned.contains(id) {
            velocities.insert(id.clone(), egui::Vec2::ZERO);
            continue;
        }
        let force = forces[id];
        let mut vel = velocities.get(id).copied().unwrap_or(egui::Vec2::ZERO);
        vel = (vel + force * dt) * DAMPING;
        if vel.length() > MAX_SPEED {
            vel = vel * (MAX_SPEED / vel.length());
        }
        if vel.length() < SETTLE_SPEED {
            vel = egui::Vec2::ZERO;
        }
        let pos = positions.get_mut(id).unwrap();
        *pos += vel * dt;
        velocities.insert(id.clone(), vel);
    }
}

/// Deterministic pseudo-random-ish spawn offset for a newly created node,
/// derived from its own id so repeated spawns don't all land exactly on
/// top of their parent (which would otherwise produce a near-infinite
/// repulsion force on the very first tick — see `MIN_DISTANCE`). Radius is
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
