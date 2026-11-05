use std::collections::{HashMap, HashSet};

use crate::store::model::NodeId;

/// A spring between two cards: `weight` scales its pull toward `REST_LENGTH` (1.0 for a structural edge, less for a
/// weak one, 0.0 for none).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edge {
    pub from: NodeId,
    pub to: NodeId,
    pub weight: f32,
}

/// The graph's world-space center that the physics centering force pulls
/// toward — independent of screen/window coordinates now that the graph is
/// pannable/zoomable (`egui::Scene`), unlike pre-pan/zoom where node
/// positions doubled as screen coordinates directly.
pub(crate) const GRAPH_WORLD_CENTER: egui::Vec2 = egui::Vec2::ZERO;

/// Pure force-directed layout for the tab graph: nodes repel each other,
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
/// `edges` are springs from a parent to a child (or any pair). `pinned` holds node ids being
/// dragged: their position is the caller's and they don't move, but they
/// still push the others (a dragged card shoulders its neighbours aside).
/// Ids and edges are processed in sorted order, so the result doesn't depend
/// on the maps' iteration order or the order of `edges`.
#[cfg(test)]
pub fn step(
    positions: &mut HashMap<NodeId, egui::Vec2>,
    velocities: &mut HashMap<NodeId, egui::Vec2>,
    edges: &[Edge],
    pinned: &HashSet<NodeId>,
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
    positions: &mut HashMap<NodeId, egui::Vec2>,
    velocities: &mut HashMap<NodeId, egui::Vec2>,
    edges: &[Edge],
    pinned: &HashSet<NodeId>,
    sizes: &HashMap<NodeId, egui::Vec2>,
    center: egui::Vec2,
    dt: f32,
) {
    if dt <= 0.0 {
        return;
    }
    // Owned ids: `positions` is mutated below.
    let mut ids: Vec<NodeId> = positions.keys().copied().collect();
    ids.sort_unstable();
    let index: HashMap<NodeId, usize> = ids.iter().enumerate().map(|(i, &id)| (id, i)).collect();
    let pos: Vec<egui::Vec2> = ids.iter().map(|id| positions[id]).collect();
    // Each card's size, indexed like `pos`: looked up once per step, not twice per pair.
    let size: Vec<Option<egui::Vec2>> = ids.iter().map(|id| sizes.get(id).copied()).collect();
    let mut forces = vec![ExactSum::default(); ids.len()];

    // Pairwise repulsion: O(n^2) over plain vectors.
    for i in 0..pos.len() {
        for j in (i + 1)..pos.len() {
            let delta = pos[i] - pos[j];
            let length = delta.length();
            // Two nodes on one point have no direction between them: pick a
            // fixed one from their ids so they still push apart, gently (see
            // `REPULSION_SOFT_DISTANCE`).
            let tie = || pair_direction(ids[i], ids[j]);
            let dir = if length > f32::EPSILON { delta / length } else { tie() };
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
                // another must ease out, not coast across the canvas. Cards level on that axis part along the pair's
                // fixed direction.
                let away = |d: f32, axis: usize| if d != 0.0 { d.signum() } else { tie()[axis].signum() };
                let push = if overlap.x < overlap.y {
                    egui::vec2(away(delta.x, 0) * overlap.x.min(OVERLAP_MAX), 0.0)
                } else {
                    egui::vec2(0.0, away(delta.y, 1) * overlap.y.min(OVERLAP_MAX))
                } * OVERLAP_STRENGTH;
                forces[i] += push;
                forces[j] -= push;
            }
        }
    }

    // Spring attraction along parent->child edges toward a rest length. An
    // edge to a since-removed node is ignored; the rest are applied in sorted
    // order, so the sums don't depend on the order of `edges` either.
    let mut springs: Vec<(usize, usize, f32)> =
        edges.iter().filter_map(|e| Some((*index.get(&e.from)?, *index.get(&e.to)?, e.weight))).collect();
    springs.sort_unstable_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)).then(a.2.total_cmp(&b.2)));
    for (p, c, weight) in springs {
        let delta = pos[c] - pos[p];
        let dist = delta.length().max(MIN_DISTANCE);
        let force = delta / dist * (SPRING_STRENGTH * weight * (dist - REST_LENGTH));
        forces[p] += force;
        forces[c] -= force;
    }

    for (i, id) in ids.iter().enumerate() {
        // Pinned (dragged) nodes stay exactly where the caller put them, with
        // no velocity, so releasing the drag doesn't fling them.
        if pinned.contains(id) {
            velocities.insert(*id, egui::Vec2::ZERO);
            continue;
        }
        // Gentle pull toward the canvas center, so a disconnected node
        // doesn't drift away.
        let force = forces[i].get() + (center - pos[i]) * CENTERING_STRENGTH;
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
                velocities.insert(*id, vel);
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
///
/// Only the id's counter feeds it, not its actor (random per launch), so the same steps place cards the same way in
/// every run (`gate chrome-shots` pixel-diffs two runs).
pub fn spawn_jitter(id: NodeId) -> egui::Vec2 {
    jitter_from_text(&id.counter.to_string())
}

/// `spawn_jitter` for a card placed before it has an id, seeded by its name.
pub fn named_jitter(name: &str) -> egui::Vec2 {
    jitter_from_text(name)
}

/// The fixed direction `a` is pushed from `b` when nothing else picks one, from their counters (see `spawn_jitter`).
/// `pair_direction(b, a)` is its opposite, so it doesn't matter which card of the pair sorts first. Only two cards
/// with one counter, from two launches, are told apart by their random actors.
fn pair_direction(a: NodeId, b: NodeId) -> egui::Vec2 {
    let flip = (a.counter, a.actor) > (b.counter, b.actor);
    let (first, second) = if flip { (b, a) } else { (a, b) };
    let dir = jitter_from_text(&format!("{}\u{0}{}", first.counter, second.counter)).normalized();
    if flip {
        -dir
    } else {
        dir
    }
}

/// A force summed exactly, so the total doesn't depend on the order its terms come in: ids sort by their launch's
/// random actor first, and float addition rounds differently in another order. Each term is cut to a multiple of
/// `2^-FRACTION_BITS` on its own, then added as an integer.
#[derive(Clone, Copy, Default)]
struct ExactSum {
    x: i128,
    y: i128,
}

/// 2^-60 is far below an f32's precision at any force the layout makes, and leaves 2^67 of headroom.
const FRACTION_BITS: i32 = 60;

impl ExactSum {
    fn fixed(v: f32) -> i128 {
        // Exact: f32 to f64 and a power-of-two scale both are; the cast truncates and saturates.
        (f64::from(v) * 2f64.powi(FRACTION_BITS)) as i128
    }

    fn get(self) -> egui::Vec2 {
        let scale = 2f64.powi(-FRACTION_BITS);
        egui::vec2((self.x as f64 * scale) as f32, (self.y as f64 * scale) as f32)
    }
}

impl std::ops::AddAssign<egui::Vec2> for ExactSum {
    fn add_assign(&mut self, v: egui::Vec2) {
        self.x = self.x.saturating_add(Self::fixed(v.x));
        self.y = self.y.saturating_add(Self::fixed(v.y));
    }
}

impl std::ops::SubAssign<egui::Vec2> for ExactSum {
    fn sub_assign(&mut self, v: egui::Vec2) {
        *self += -v;
    }
}

fn jitter_from_text(seed_str: &str) -> egui::Vec2 {
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
mod tests;
