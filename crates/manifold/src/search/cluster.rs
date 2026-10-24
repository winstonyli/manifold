//! Grouping documents by topic: average-linkage clustering over an `Embedding`'s cosines.

use super::lsa::Embedding;

/// Documents closer than this on average do not share a cluster.
pub(crate) const CLUSTER_MIN_SIMILARITY: f32 = 0.2;
/// A cluster smaller than this is not worth suggesting as a group.
pub(crate) const CLUSTER_MIN_SIZE: usize = 3;
/// The most clusters offered at once.
pub(crate) const CLUSTER_MAX: usize = 8;
/// The most cards embedded at once (clustering is cubic).
pub(crate) const CLUSTER_MAX_CARDS: usize = 400;

/// Clusters of document indices (into `emb.keys`), each of at least `CLUSTER_MIN_SIZE`, largest first and at most
/// `CLUSTER_MAX` of them. Documents whose vector is zero (nothing in common with any other) are left out. Merges the
/// pair of clusters with the highest average cosine while that is at least `min_similarity`; ties go to the lowest
/// indices, so the result does not depend on the order the documents came in.
pub(crate) fn cluster(emb: &Embedding, min_similarity: f32) -> Vec<Vec<usize>> {
    let mut clusters: Vec<Vec<usize>> =
        (0..emb.keys.len()).filter(|&i| emb.vectors[i].iter().any(|&x| x != 0.0)).map(|i| vec![i]).collect();
    let cosines: Vec<Vec<f32>> = (0..emb.keys.len())
        .map(|a| (0..emb.keys.len()).map(|b| if a == b { 1.0 } else { emb.cosine(a, b) }).collect())
        .collect();
    let average = |x: &[usize], y: &[usize]| {
        let sum: f32 = x.iter().map(|&a| y.iter().map(|&b| cosines[a][b]).sum::<f32>()).sum();
        sum / (x.len() * y.len()) as f32
    };
    loop {
        let mut best: Option<(f32, usize, usize)> = None;
        for i in 0..clusters.len() {
            for j in i + 1..clusters.len() {
                let score = average(&clusters[i], &clusters[j]);
                if score >= min_similarity && best.is_none_or(|(top, _, _)| score > top) {
                    best = Some((score, i, j));
                }
            }
        }
        let Some((_, i, j)) = best else { break };
        let merged = clusters.remove(j);
        clusters[i].extend(merged);
        clusters[i].sort_unstable();
        clusters.sort_by_key(|c| c[0]);
    }
    clusters.retain(|c| c.len() >= CLUSTER_MIN_SIZE);
    clusters.sort_by(|a, b| b.len().cmp(&a.len()).then(a[0].cmp(&b[0])));
    clusters.truncate(CLUSTER_MAX);
    clusters
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::lsa::{Embedder, LsaEmbedder};
    use crate::search::DocKey;
    use crate::store::model::{ActorId, NodeId};
    use std::collections::BTreeMap;

    fn doc(n: u64, words: &str) -> (DocKey, Vec<(&str, u32)>) {
        let mut counts: BTreeMap<&str, u32> = BTreeMap::new();
        for w in words.split_whitespace() {
            *counts.entry(w).or_default() += 1;
        }
        (DocKey::Node(NodeId { actor: ActorId(1), counter: n }), counts.into_iter().collect())
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

    #[test]
    fn average_linkage_splits_two_topics() {
        let emb = LsaEmbedder::default().embed(&topics());
        assert_eq!(cluster(&emb, CLUSTER_MIN_SIMILARITY), [vec![0, 1, 2], vec![3, 4, 5]]);
    }

    #[test]
    fn clusters_below_min_size_are_dropped_and_at_most_cluster_max_returned() {
        let mut docs = topics();
        docs.truncate(5); // the rust topic is now two documents
        let emb = LsaEmbedder::default().embed(&docs);
        assert_eq!(cluster(&emb, CLUSTER_MIN_SIMILARITY), [vec![0, 1, 2]]);

        // Ten topics of three documents each: only CLUSTER_MAX come back.
        let texts: Vec<String> =
            (0..10).map(|t| (0..4).map(|w| format!("topic{t}word{w}")).collect::<Vec<_>>().join(" ")).collect();
        let many: Vec<(DocKey, Vec<(&str, u32)>)> = (0..30u64).map(|n| doc(n, &texts[(n / 3) as usize])).collect();
        let emb = LsaEmbedder::default().embed(&many);
        let found = cluster(&emb, CLUSTER_MIN_SIMILARITY);
        assert_eq!(found.len(), CLUSTER_MAX);
        assert!(found.iter().all(|c| c.len() == 3));
    }

    #[test]
    fn singletons_and_zero_vectors_are_never_clustered() {
        let mut docs = topics();
        docs.push(doc(7, "unrelated lonely words"));
        docs.push(doc(8, ""));
        let emb = LsaEmbedder::default().embed(&docs);
        let found = cluster(&emb, CLUSTER_MIN_SIMILARITY);
        assert!(found.iter().flatten().all(|&i| i < 6), "{found:?}");
        assert!(cluster(&LsaEmbedder::default().embed(&[]), CLUSTER_MIN_SIMILARITY).is_empty());
    }

    #[test]
    fn cluster_result_is_deterministic_and_independent_of_input_order() {
        let mut shuffled = topics();
        shuffled.reverse();
        shuffled.swap(1, 4);
        let (a, b) = (LsaEmbedder::default().embed(&topics()), LsaEmbedder::default().embed(&shuffled));
        let keys = |emb: &Embedding, found: Vec<Vec<usize>>| -> Vec<Vec<DocKey>> {
            found.into_iter().map(|c| c.into_iter().map(|i| emb.keys[i]).collect()).collect()
        };
        assert_eq!(keys(&a, cluster(&a, CLUSTER_MIN_SIMILARITY)), keys(&b, cluster(&b, CLUSTER_MIN_SIMILARITY)));
        assert_eq!(cluster(&a, CLUSTER_MIN_SIMILARITY), cluster(&a, CLUSTER_MIN_SIMILARITY));
    }
}

#[cfg(test)]
mod tuning {
    use super::*;
    use crate::search::lsa::{Embedder, LsaEmbedder};
    use crate::search::DocKey;
    use crate::store::model::{ActorId, NodeId};
    use std::collections::BTreeMap;
    use std::time::Instant;

    /// `topics` topics of `per` documents: 12 words from the topic's 30-word vocabulary and 4 from 40 shared ones.
    fn corpus(topics: usize, per: usize) -> Vec<(DocKey, BTreeMap<String, u32>)> {
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as usize
        };
        let mut docs = Vec::new();
        for t in 0..topics {
            for d in 0..per {
                let mut counts = BTreeMap::new();
                for _ in 0..12 {
                    *counts.entry(format!("t{t}w{}", next(30))).or_default() += 1;
                }
                for _ in 0..4 {
                    *counts.entry(format!("common{}", next(40))).or_default() += 1;
                }
                docs.push((DocKey::Node(NodeId { actor: ActorId(1), counter: (t * per + d) as u64 }), counts));
            }
        }
        docs
    }

    fn embed(docs: &[(DocKey, BTreeMap<String, u32>)]) -> crate::search::lsa::Embedding {
        let borrowed: Vec<(DocKey, Vec<(&str, u32)>)> =
            docs.iter().map(|(k, c)| (*k, c.iter().map(|(t, &n)| (t.as_str(), n)).collect())).collect();
        LsaEmbedder::default().embed(&borrowed)
    }

    /// Prints, per threshold, the clusters found in 3 topics x 20 documents and how pure they are.
    #[test]
    #[ignore = "tuning table for CLUSTER_MIN_SIMILARITY; run with --nocapture"]
    fn threshold_table() {
        let docs = corpus(3, 20);
        let emb = embed(&docs);
        for threshold in [0.2, 0.25, 0.3, 0.35, 0.4, 0.45, 0.5, 0.6] {
            let found = cluster(&emb, threshold);
            let summary: Vec<String> = found
                .iter()
                .map(|c| {
                    let topic = |i: usize| match emb.keys[i] {
                        DocKey::Node(id) => id.counter as usize / 20,
                        _ => unreachable!(),
                    };
                    let majority = (0..3).map(|t| c.iter().filter(|&&i| topic(i) == t).count()).max().unwrap();
                    format!("{}/{}", majority, c.len())
                })
                .collect();
            println!("threshold {threshold}: {} clusters, pure/size {summary:?}", found.len());
        }
    }

    /// The cost of a "Suggest groups" at the cap (`CLUSTER_MAX_CARDS` documents), embedding and clustering.
    #[test]
    #[ignore = "timing for the inline-vs-worker decision; run with --release --nocapture"]
    fn cap_timing() {
        for n in [100, 200, CLUSTER_MAX_CARDS] {
            let docs = corpus(8, n / 8);
            let t0 = Instant::now();
            let emb = embed(&docs);
            let embedded = t0.elapsed();
            let t1 = Instant::now();
            let found = cluster(&emb, CLUSTER_MIN_SIMILARITY);
            println!("{n} docs: embed {embedded:?}, cluster {:?} ({} clusters)", t1.elapsed(), found.len());
        }
    }
}
