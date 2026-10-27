//! Throwaway prototype for "Arrange by topic": does `step_sized` with similarity edges pull
//! same-topic cards together, and how far does an ordinary settle move them afterwards (OQ1)? Run with
//! `cargo test -p manifold --release -- --ignored --nocapture prototype`.

use super::*;
use crate::card_id::testutil::node;
use std::time::Instant;

const ITERATIONS: u32 = 300;
const EDGE_MIN: f32 = 0.30;

struct Rng(u64);
impl Rng {
    fn unit(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f32 / (1u64 << 53) as f32
    }
    /// Uniform in `mean ± spread`.
    fn around(&mut self, mean: f32, spread: f32) -> f32 {
        mean + (self.unit() * 2.0 - 1.0) * spread
    }
}

struct Run {
    ratio: f32,
    drift: f32,
    arrange_ms: u128,
}

/// `topics` topics of `per` cards plus `noise` cards on no topic; `seed_lsa` starts each topic in its own corner.
fn run(topics: usize, per: usize, noise: usize, seed_lsa: bool) -> Run {
    let n = topics * per + noise;
    let topic_of = |i: usize| (i < topics * per).then(|| i / per);
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut cos = vec![vec![0.0f32; n]; n];
    for i in 0..n {
        for j in (i + 1)..n {
            let same = topic_of(i).is_some() && topic_of(i) == topic_of(j);
            let c = if same { rng.around(0.6, 0.15) } else { rng.around(0.1, 0.1) };
            cos[i][j] = c;
            cos[j][i] = c;
        }
    }
    let mut edges = Vec::new();
    for i in 0..n {
        let mut order: Vec<usize> = (0..n).filter(|&j| j != i && cos[i][j] >= EDGE_MIN).collect();
        order.sort_by(|&a, &b| cos[i][b].total_cmp(&cos[i][a]).then(a.cmp(&b)));
        for &j in order.iter().take(3) {
            let (a, b) = (i.min(j), i.max(j));
            let w = cos[a][b];
            match edges.iter_mut().find(|e: &&mut Edge| e.from == node(a as u64 + 1) && e.to == node(b as u64 + 1)) {
                Some(e) => e.weight = e.weight.max(w),
                None => edges.push(Edge { from: node(a as u64 + 1), to: node(b as u64 + 1), weight: w }),
            }
        }
    }
    let mut positions: HashMap<NodeId, egui::Vec2> = HashMap::new();
    for i in 0..n {
        let at = match (seed_lsa, topic_of(i)) {
            (true, Some(t)) => {
                let corner =
                    egui::vec2(if t % 2 == 0 { -1500.0 } else { 1500.0 }, if t / 2 == 0 { -1500.0 } else { 1500.0 });
                corner + egui::vec2(rng.around(0.0, 400.0), rng.around(0.0, 400.0))
            }
            _ => egui::vec2(rng.around(0.0, 2000.0), rng.around(0.0, 2000.0)),
        };
        positions.insert(node(i as u64 + 1), at);
    }
    let mut velocities: HashMap<NodeId, egui::Vec2> = positions.keys().map(|&k| (k, egui::Vec2::ZERO)).collect();
    let sizes: HashMap<NodeId, egui::Vec2> = positions.keys().map(|&k| (k, egui::vec2(220.0, 140.0))).collect();
    let pinned = HashSet::new();
    let t0 = Instant::now();
    for _ in 0..ITERATIONS {
        step_sized(&mut positions, &mut velocities, &edges, &pinned, &sizes, GRAPH_WORLD_CENTER, 1.0 / 60.0);
    }
    let arrange_ms = t0.elapsed().as_millis();
    let at = |i: usize| positions[&node(i as u64 + 1)];
    let (mut within, mut across, mut nw, mut na) = (0.0, 0.0, 0usize, 0usize);
    for i in 0..topics * per {
        for j in (i + 1)..topics * per {
            let d = (at(i) - at(j)).length();
            if topic_of(i) == topic_of(j) {
                within += d;
                nw += 1;
            } else {
                across += d;
                na += 1;
            }
        }
    }
    let ratio = (within / nw as f32) / (across / na as f32);
    // OQ1: the ordinary settle afterwards, structural edges only (none here).
    let before = positions.clone();
    for _ in 0..90 {
        step_sized(&mut positions, &mut velocities, &[], &pinned, &sizes, GRAPH_WORLD_CENTER, 1.0 / 60.0);
    }
    let drift =
        (0..n).map(|i| (positions[&node(i as u64 + 1)] - before[&node(i as u64 + 1)]).length()).sum::<f32>() / n as f32;
    Run { ratio, drift, arrange_ms }
}

#[test]
#[ignore = "prototype; run with --release --nocapture"]
fn prototype() {
    for (topics, per, noise, seed) in [(4, 40, 40, false), (4, 40, 40, true), (4, 60, 60, false), (4, 25, 20, false)] {
        let r = run(topics, per, noise, seed);
        println!(
            "{} cards (seeded {seed}): in/cross distance ratio {:.3}, mean drift in a 90-step settle {:.1} ({:.0}% of REST_LENGTH), 300 iterations {} ms",
            topics * per + noise,
            r.ratio,
            r.drift,
            100.0 * r.drift / REST_LENGTH,
            r.arrange_ms
        );
    }
}
