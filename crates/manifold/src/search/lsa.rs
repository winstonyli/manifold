//! Latent semantic analysis behind the `Embedder` trait: tf-idf rows, a seeded randomized range finder, and an exact
//! eigendecomposition of the small projected matrix (cyclic Jacobi). Each document becomes a unit vector whose cosine
//! with another says how much they talk about the same things. All arithmetic runs in sorted-key order and the
//! random numbers depend only on `(seed, term, column)`, so the same documents give the same bits in any input order.

use super::DocKey;
use std::collections::BTreeMap;

/// Dimensions of an embedding.
pub(crate) const LSA_K: usize = 64;
/// The vocabulary is the most frequent terms, at most this many.
const MAX_TERMS: usize = 20_000;
/// Extra columns the range finder carries beyond `k`.
const OVERSAMPLE: usize = 8;
const POWER_ITERATIONS: usize = 2;

pub(crate) trait Embedder {
    /// Embeds documents given as their term counts (`App::suggest_groups` counts the tokens of each card's text).
    fn embed(&self, docs: &[(DocKey, Vec<(&str, u32)>)]) -> Embedding;
}

pub(crate) struct Embedding {
    /// The documents, in key order.
    pub keys: Vec<DocKey>,
    /// One L2-normalised vector per document (all zeros, or empty, when the document shares no term with another).
    pub vectors: Vec<Vec<f32>>,
    /// The vocabulary, alphabetical.
    pub terms: Vec<String>,
    /// Each document's tf-idf row as `(term index, weight)`, L2-normalised.
    pub weights: Vec<Vec<(u32, f32)>>,
}

impl Embedding {
    pub(crate) fn cosine(&self, a: usize, b: usize) -> f32 {
        self.vectors[a].iter().zip(&self.vectors[b]).map(|(x, y)| x * y).sum()
    }

    /// The `n` terms carrying the most tf-idf weight across `members`, leaving out terms found in over 60 % of all
    /// documents.
    pub(crate) fn top_terms(&self, members: &[usize], n: usize) -> Vec<String> {
        let mut df = vec![0u32; self.terms.len()];
        for row in &self.weights {
            for &(t, _) in row {
                df[t as usize] += 1;
            }
        }
        let mut mass = vec![0f32; self.terms.len()];
        for &m in members {
            for &(t, w) in &self.weights[m] {
                mass[t as usize] += w;
            }
        }
        let common = (self.keys.len() as f32 * 0.6) as u32;
        let mut order: Vec<usize> = (0..self.terms.len()).filter(|&t| mass[t] > 0.0 && df[t] <= common).collect();
        if order.is_empty() {
            // Every word of these members is common (a level of a few cards): name them by the common ones.
            order = (0..self.terms.len()).filter(|&t| mass[t] > 0.0).collect();
        }
        order.sort_by(|&a, &b| mass[b].total_cmp(&mass[a]).then(a.cmp(&b)));
        order.into_iter().take(n).map(|t| self.terms[t].clone()).collect()
    }
}

pub(crate) struct LsaEmbedder {
    pub k: usize,
    pub seed: u64,
}

impl Default for LsaEmbedder {
    fn default() -> Self {
        LsaEmbedder { k: LSA_K, seed: 0x6d61_6e69_666f_6c64 }
    }
}

type Row = Vec<(u32, f64)>;

impl Embedder for LsaEmbedder {
    fn embed(&self, docs: &[(DocKey, Vec<(&str, u32)>)]) -> Embedding {
        let mut order: Vec<&(DocKey, Vec<(&str, u32)>)> = docs.iter().collect();
        order.sort_by_key(|d| d.0);
        let n = order.len();
        let keys: Vec<DocKey> = order.iter().map(|d| d.0).collect();

        // The vocabulary: terms in at least two documents, the most frequent kept, then alphabetical.
        let mut df: BTreeMap<&str, u32> = BTreeMap::new();
        for (_, terms) in &order {
            for &(term, count) in terms {
                if count > 0 {
                    *df.entry(term).or_default() += 1;
                }
            }
        }
        let mut frequent: Vec<(&str, u32)> = df.into_iter().filter(|&(_, d)| d >= 2).collect();
        frequent.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        frequent.truncate(MAX_TERMS);
        frequent.sort_by(|a, b| a.0.cmp(b.0));
        let index: BTreeMap<&str, u32> = frequent.iter().enumerate().map(|(i, &(t, _))| (t, i as u32)).collect();
        let terms: Vec<String> = frequent.iter().map(|&(t, _)| t.to_owned()).collect();

        // tf-idf rows, each L2-normalised.
        let mut rows: Vec<Row> = Vec::with_capacity(n);
        for (_, doc_terms) in &order {
            let mut row: Row = doc_terms
                .iter()
                .filter(|&&(_, c)| c > 0)
                .filter_map(|&(t, c)| {
                    let at = *index.get(t)?;
                    let idf = (1.0 + n as f64 / f64::from(frequent[at as usize].1)).ln();
                    Some((at, (1.0 + f64::from(c).ln()) * idf))
                })
                .collect();
            row.sort_by_key(|&(t, _)| t);
            let norm = row.iter().map(|&(_, w)| w * w).sum::<f64>().sqrt();
            if norm > 0.0 {
                for entry in &mut row {
                    entry.1 /= norm;
                }
            }
            rows.push(row);
        }
        let weights: Vec<Vec<(u32, f32)>> =
            rows.iter().map(|r| r.iter().map(|&(t, w)| (t, w as f32)).collect()).collect();

        let vectors = project(&rows, &terms, self.k, self.seed)
            .into_iter()
            .map(|v| v.iter().map(|&x| x as f32).collect())
            .collect();
        Embedding { keys, vectors, terms, weights }
    }
}

/// Each row's unit vector in the top `k` singular directions of the matrix `rows` (`terms.len()` columns).
fn project(rows: &[Row], terms: &[String], k: usize, seed: u64) -> Vec<Vec<f64>> {
    let svd = svd(rows, terms, k, seed);
    let mut out: Vec<Vec<f64>> = (0..rows.len())
        .map(|i| {
            (0..svd.sigma.len())
                .map(|j| svd.q.iter().zip(&svd.u).map(|(q, u)| q[i] * u[j]).sum::<f64>() * svd.sigma[j])
                .collect()
        })
        .collect();
    for v in &mut out {
        let norm = v.iter().map(|x| x * x).sum::<f64>().sqrt();
        if norm > 1e-12 {
            v.iter_mut().for_each(|x| *x /= norm);
        } else {
            v.iter_mut().for_each(|x| *x = 0.0);
        }
    }
    out
}

/// A truncated SVD of the sparse matrix `rows`: `q` is an orthonormal basis (columns) of its range, `u[c][j]` the `j`th
/// left singular vector of `Qᵀ A` in that basis, `sigma` the singular values, largest first.
struct Svd {
    q: Vec<Vec<f64>>,
    u: Vec<Vec<f64>>,
    sigma: Vec<f64>,
}

fn svd(rows: &[Row], terms: &[String], k: usize, seed: u64) -> Svd {
    let (n, v) = (rows.len(), terms.len());
    let l = (k + OVERSAMPLE).min(n).min(v);
    if l == 0 {
        return Svd { q: Vec::new(), u: Vec::new(), sigma: Vec::new() };
    }
    // Y = A Ω, one column per random vector.
    let omega: Vec<Vec<f64>> = (0..l).map(|c| terms.iter().map(|t| random(seed, t, c as u64)).collect()).collect();
    let mut y: Vec<Vec<f64>> = omega.iter().map(|w| times(rows, w)).collect();
    orthonormalise(&mut y);
    for _ in 0..POWER_ITERATIONS {
        let mut z: Vec<Vec<f64>> = y.iter().map(|c| times_transposed(rows, c, v)).collect();
        orthonormalise(&mut z);
        y = z.iter().map(|c| times(rows, c)).collect();
        orthonormalise(&mut y);
    }
    // B = Qᵀ A (l × V), then the eigenpairs of B Bᵀ.
    let b: Vec<Vec<f64>> = y.iter().map(|c| times_transposed(rows, c, v)).collect();
    let gram: Vec<Vec<f64>> =
        b.iter().map(|x| b.iter().map(|w| x.iter().zip(w).map(|(p, q)| p * q).sum()).collect()).collect();
    let (values, vectors) = jacobi(gram);
    let mut order: Vec<usize> = (0..l).collect();
    order.sort_by(|&a, &c| values[c].total_cmp(&values[a]).then(a.cmp(&c)));
    order.truncate(k.min(l));
    let sigma: Vec<f64> = order.iter().map(|&j| values[j].max(0.0).sqrt()).collect();
    // u[c][j]: row c of the chosen eigenvectors.
    let u: Vec<Vec<f64>> = (0..l).map(|c| order.iter().map(|&j| vectors[c][j]).collect()).collect();
    Svd { q: y, u, sigma }
}

/// `A w` for a vector `w` over the columns: one value per row.
fn times(rows: &[Row], w: &[f64]) -> Vec<f64> {
    rows.iter().map(|row| row.iter().map(|&(t, x)| x * w[t as usize]).sum()).collect()
}

/// `Aᵀ y` for a vector `y` over the rows: one value per column.
fn times_transposed(rows: &[Row], y: &[f64], columns: usize) -> Vec<f64> {
    let mut out = vec![0.0; columns];
    for (row, &yi) in rows.iter().zip(y) {
        for &(t, x) in row {
            out[t as usize] += x * yi;
        }
    }
    out
}

/// Modified Gram-Schmidt on the columns, twice; a column that falls to nothing becomes zero.
fn orthonormalise(columns: &mut [Vec<f64>]) {
    for _ in 0..2 {
        for c in 0..columns.len() {
            let (done, rest) = columns.split_at_mut(c);
            let current = &mut rest[0];
            for earlier in done.iter() {
                let dot: f64 = earlier.iter().zip(current.iter()).map(|(a, b)| a * b).sum();
                current.iter_mut().zip(earlier).for_each(|(x, e)| *x -= dot * e);
            }
            let norm = current.iter().map(|x| x * x).sum::<f64>().sqrt();
            if norm > 1e-10 {
                current.iter_mut().for_each(|x| *x /= norm);
            } else {
                current.iter_mut().for_each(|x| *x = 0.0);
            }
        }
    }
}

/// A value in `[-1, 1]` from `(seed, term, column)`: FNV-1a over the three, then xorshift64* steps.
fn random(seed: u64, term: &str, column: u64) -> f64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ seed;
    for byte in term.bytes().chain(column.to_le_bytes()) {
        h = (h ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    let mut x = h | 1;
    for _ in 0..3 {
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
    }
    let bits = x.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 11;
    bits as f64 / (1u64 << 52) as f64 - 1.0
}

/// Eigenvalues and eigenvectors (`vectors[row][j]` is component `row` of eigenvector `j`) of a symmetric matrix by
/// cyclic Jacobi rotations.
fn jacobi(mut a: Vec<Vec<f64>>) -> (Vec<f64>, Vec<Vec<f64>>) {
    let n = a.len();
    let mut v: Vec<Vec<f64>> = (0..n).map(|i| (0..n).map(|j| f64::from(u8::from(i == j))).collect()).collect();
    for _ in 0..60 {
        let off: f64 = (0..n).flat_map(|p| (p + 1..n).map(move |q| (p, q))).map(|(p, q)| a[p][q] * a[p][q]).sum();
        if off.sqrt() < 1e-9 {
            break;
        }
        for p in 0..n.saturating_sub(1) {
            for q in p + 1..n {
                if a[p][q].abs() < 1e-300 {
                    continue;
                }
                let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
                let t = if theta >= 0.0 { 1.0 } else { -1.0 } / (theta.abs() + (theta * theta + 1.0).sqrt());
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                for row in a.iter_mut() {
                    let (rp, rq) = (row[p], row[q]);
                    row[p] = c * rp - s * rq;
                    row[q] = s * rp + c * rq;
                }
                for k in 0..n {
                    let (pk, qk) = (a[p][k], a[q][k]);
                    a[p][k] = c * pk - s * qk;
                    a[q][k] = s * pk + c * qk;
                }
                for row in v.iter_mut() {
                    let (rp, rq) = (row[p], row[q]);
                    row[p] = c * rp - s * rq;
                    row[q] = s * rp + c * rq;
                }
            }
        }
    }
    ((0..n).map(|i| a[i][i]).collect(), v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::model::{ActorId, NodeId};

    fn key(n: u64) -> DocKey {
        DocKey::Node(NodeId { actor: ActorId(1), counter: n })
    }

    fn doc(n: u64, words: &str) -> (DocKey, Vec<(&str, u32)>) {
        let mut counts: BTreeMap<&str, u32> = BTreeMap::new();
        for w in words.split_whitespace() {
            *counts.entry(w).or_default() += 1;
        }
        (key(n), counts.into_iter().collect())
    }

    fn topics() -> Vec<(DocKey, Vec<(&'static str, u32)>)> {
        vec![
            doc(1, "cat milk fur purr cat"),
            doc(2, "cat fur kitten milk"),
            doc(3, "kitten purr milk cat fur"),
            doc(4, "rust cargo crate compile rust"),
            doc(5, "cargo crate rust build"),
            doc(6, "compile build crate rust cargo"),
        ]
    }

    fn mean(values: &[f32]) -> f32 {
        values.iter().sum::<f32>() / values.len() as f32
    }

    #[test]
    fn two_topics_separate() {
        let embedding = LsaEmbedder::default().embed(&topics());
        let (mut within, mut across) = (Vec::new(), Vec::new());
        for a in 0..6 {
            for b in a + 1..6 {
                let c = embedding.cosine(a, b);
                if (a < 3) == (b < 3) {
                    within.push(c);
                } else {
                    across.push(c);
                }
            }
        }
        let (within, across) = (mean(&within), mean(&across));
        assert!(within > 0.5, "in-topic {within}");
        assert!(within > across + 0.3, "in-topic {within} vs cross-topic {across}");
    }

    #[test]
    fn the_result_is_deterministic() {
        let a = LsaEmbedder::default().embed(&topics());
        let b = LsaEmbedder::default().embed(&topics());
        assert!(a
            .vectors
            .iter()
            .zip(&b.vectors)
            .all(|(x, y)| x.iter().zip(y).all(|(p, q)| p.to_bits() == q.to_bits())));
    }

    #[test]
    fn input_order_does_not_matter() {
        let mut shuffled = topics();
        shuffled.swap(0, 5);
        shuffled.swap(1, 3);
        let (a, b) = (LsaEmbedder::default().embed(&topics()), LsaEmbedder::default().embed(&shuffled));
        assert_eq!(a.keys, b.keys);
        assert_eq!(a.cosine(0, 1).to_bits(), b.cosine(0, 1).to_bits());
        assert_eq!(a.cosine(2, 4).to_bits(), b.cosine(2, 4).to_bits());
    }

    #[test]
    fn one_doc_and_empty_input_do_not_panic() {
        let lsa = LsaEmbedder::default();
        assert!(lsa.embed(&[]).keys.is_empty());
        let one = lsa.embed(&[doc(1, "alone words here")]);
        assert_eq!(one.keys.len(), 1);
        assert_eq!(one.cosine(0, 0), 0.0, "no term is shared, so the row is zero");
        let two = lsa.embed(&[doc(1, "same"), doc(2, "same")]);
        assert!((two.cosine(0, 1) - 1.0).abs() < 1e-4, "{}", two.cosine(0, 1));
    }

    #[test]
    fn top_terms_of_a_small_level_are_never_empty() {
        let embedding = LsaEmbedder::default().embed(&[doc(1, "cat milk"), doc(2, "cat fur"), doc(3, "cat purr")]);
        assert!(
            !embedding.top_terms(&[0, 1, 2], 3).is_empty(),
            "every shared word is common, the label still names something"
        );
    }

    #[test]
    fn top_terms_name_the_topic() {
        let embedding = LsaEmbedder::default().embed(&topics());
        let cats = embedding.top_terms(&[0, 1, 2], 3);
        assert!(cats.iter().any(|t| t == "cat" || t == "milk"), "{cats:?}");
        let rust = embedding.top_terms(&[3, 4, 5], 3);
        assert!(rust.iter().any(|t| t == "rust" || t == "crate" || t == "cargo"), "{rust:?}");
    }

    #[test]
    fn svd_recovers_a_known_rank_one_matrix() {
        let u = [1.0, 2.0, -1.0, 0.5];
        let v = [3.0, 0.0, 4.0, 1.0, -2.0];
        let rows: Vec<Row> =
            u.iter().map(|&x| v.iter().enumerate().map(|(t, &y)| (t as u32, x * y)).collect()).collect();
        let terms: Vec<String> = (0..v.len()).map(|t| format!("t{t}")).collect();
        let result = svd(&rows, &terms, 2, 7);
        let (nu, nv) = (u.iter().map(|x| x * x).sum::<f64>().sqrt(), v.iter().map(|x| x * x).sum::<f64>().sqrt());
        assert!((result.sigma[0] - nu * nv).abs() < 1e-3, "{} vs {}", result.sigma[0], nu * nv);
        assert!(result.sigma[1].abs() < 1e-3, "rank one: {}", result.sigma[1]);
    }

    /// Run by hand in release: `cargo test -p manifold --release -- --ignored --nocapture lsa_time`.
    #[test]
    #[ignore = "a timing probe, run by hand"]
    fn lsa_time_on_two_thousand_docs() {
        let mut state = 5u64;
        let mut next = || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (state >> 33) as usize
        };
        let words: Vec<String> = (0..20_000).map(|i| format!("w{i}")).collect();
        let docs: Vec<(DocKey, Vec<(&str, u32)>)> = (0..2_000u64)
            .map(|n| {
                let mut counts: BTreeMap<&str, u32> = BTreeMap::new();
                let topic = next() % 10;
                for _ in 0..300 {
                    let w = if next() % 2 == 0 { topic * 1000 + next() % 1000 } else { next() % 20_000 };
                    *counts.entry(words[w].as_str()).or_default() += 1;
                }
                (key(n), counts.into_iter().collect())
            })
            .collect();
        let start = std::time::Instant::now();
        let embedding = LsaEmbedder::default().embed(&docs);
        println!("embedded {} docs, {} terms in {:?}", embedding.keys.len(), embedding.terms.len(), start.elapsed());
    }
}
