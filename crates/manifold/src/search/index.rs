//! The BM25 index: documents are page texts (`DocKey::Node`) and history titles (`DocKey::History`), queried as an AND
//! of words whose last word is a prefix.

use super::tokenize::tokens;
use crate::store::model::NodeId;
use std::collections::{BTreeMap, HashMap};

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub(crate) enum DocKey {
    Node(NodeId),
    History(u32),
}

const K1: f32 = 1.2;
const B: f32 = 0.75;

/// Term postings with each term's count per document, and document lengths for BM25. `terms` is the forward view of
/// the same data, so replacing or removing a document touches only its own terms.
pub(crate) struct Index {
    postings: BTreeMap<String, Vec<(DocKey, u32)>>,
    len: HashMap<DocKey, u32>,
    terms: HashMap<DocKey, Vec<(String, u32)>>,
    total_len: u64,
}

impl Index {
    pub(crate) fn new() -> Self {
        Index { postings: BTreeMap::new(), len: HashMap::new(), terms: HashMap::new(), total_len: 0 }
    }

    /// Indexes `text` under `key`, replacing a document already there.
    pub(crate) fn add(&mut self, key: DocKey, text: &str) {
        self.remove(key);
        let words = tokens(text);
        let mut counts: BTreeMap<String, u32> = BTreeMap::new();
        for word in &words {
            *counts.entry(word.clone()).or_default() += 1;
        }
        for (term, count) in &counts {
            let list = self.postings.entry(term.clone()).or_default();
            let at = list.binary_search_by_key(&key, |&(k, _)| k).unwrap_or_else(|at| at);
            list.insert(at, (key, *count));
        }
        self.len.insert(key, words.len() as u32);
        self.terms.insert(key, counts.into_iter().collect());
        self.total_len += words.len() as u64;
    }

    pub(crate) fn remove(&mut self, key: DocKey) {
        let Some(len) = self.len.remove(&key) else { return };
        self.total_len -= u64::from(len);
        for (term, _) in self.terms.remove(&key).unwrap_or_default() {
            if let Some(list) = self.postings.get_mut(&term) {
                list.retain(|&(k, _)| k != key);
                if list.is_empty() {
                    self.postings.remove(&term);
                }
            }
        }
    }

    /// Documents containing every word of `query` (the last as a prefix), best BM25 score first.
    pub(crate) fn search(&self, query: &str) -> Vec<(DocKey, f32)> {
        let words = tokens(query);
        let Some((last, _)) = words.iter().enumerate().next_back() else { return Vec::new() };
        let n = self.len.len() as f32;
        let avg_len = (self.total_len as f32 / n.max(1.0)).max(1.0);
        let mut scores: Option<HashMap<DocKey, f32>> = None;
        for (i, word) in words.iter().enumerate() {
            let matched: Vec<&Vec<(DocKey, u32)>> = if i == last {
                self.postings
                    .range(word.clone()..)
                    .take_while(|(t, _)| t.starts_with(word.as_str()))
                    .map(|(_, l)| l)
                    .collect()
            } else {
                self.postings.get(word).into_iter().collect()
            };
            let mut word_scores: HashMap<DocKey, f32> = HashMap::new();
            for list in matched {
                let df = list.len() as f32;
                let idf = (1.0 + (n - df + 0.5) / (df + 0.5)).ln();
                for &(key, tf) in list {
                    let tf = tf as f32;
                    let len = self.len[&key] as f32;
                    *word_scores.entry(key).or_default() +=
                        idf * tf * (K1 + 1.0) / (tf + K1 * (1.0 - B + B * len / avg_len));
                }
            }
            scores = Some(match scores {
                None => word_scores,
                Some(prev) => word_scores.into_iter().filter_map(|(k, s)| prev.get(&k).map(|p| (k, p + s))).collect(),
            });
        }
        let mut hits: Vec<(DocKey, f32)> = scores.unwrap_or_default().into_iter().collect();
        hits.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        hits
    }

    /// About `width` characters of `text` around the first query word it contains.
    pub(crate) fn snippet(text: &str, query: &str, width: usize) -> Option<String> {
        let chars: Vec<char> = text.chars().collect();
        // One lowercase char per char keeps indices aligned with `chars`.
        let lower: Vec<char> = chars.iter().map(|c| c.to_lowercase().next().unwrap_or(*c)).collect();
        let (at, hit_len) = tokens(query)
            .iter()
            .filter_map(|word| {
                let w: Vec<char> = word.chars().collect();
                lower.windows(w.len()).position(|win| win == w.as_slice()).map(|at| (at, w.len()))
            })
            .min_by_key(|&(at, _)| at)?;
        let mut start = at.saturating_sub(width.saturating_sub(hit_len) / 2);
        let end = (start + width).min(chars.len());
        start = end.saturating_sub(width);
        let body: String = chars[start..end].iter().collect();
        let lead = if start > 0 { "…" } else { "" };
        let tail = if end < chars.len() { "…" } else { "" };
        Some(format!("{lead}{}{tail}", body.trim()))
    }

    #[cfg(test)]
    pub(crate) fn docs(&self) -> usize {
        self.len.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::model::ActorId;

    fn node(n: u64) -> DocKey {
        DocKey::Node(NodeId { actor: ActorId(1), counter: n })
    }

    fn keys(hits: Vec<(DocKey, f32)>) -> Vec<DocKey> {
        hits.into_iter().map(|(k, _)| k).collect()
    }

    #[test]
    fn bm25_prefers_the_doc_with_more_and_rarer_matches() {
        let mut index = Index::new();
        index.add(node(1), "rust rust rust and a little more text");
        index.add(node(2), "rust is one of the languages here and a little more text");
        index.add(node(3), "nothing to see in this document at all");
        assert_eq!(keys(index.search("rust")), [node(1), node(2)]);
    }

    #[test]
    fn a_rarer_word_outweighs_a_common_one() {
        let mut index = Index::new();
        index.add(node(1), "common common common");
        index.add(node(2), "common rare");
        index.add(node(3), "common common");
        assert_eq!(keys(index.search("common rare")), [node(2)], "the AND of both words");
        let hits = index.search("rare");
        assert_eq!(keys(hits), [node(2)]);
    }

    #[test]
    fn last_word_matches_as_a_prefix_others_exactly() {
        let mut index = Index::new();
        index.add(node(1), "rust programming language");
        assert_eq!(keys(index.search("rus")), [node(1)]);
        assert_eq!(keys(index.search("rust prog")), [node(1)]);
        assert!(index.search("ru prog").is_empty(), "an earlier word is not a prefix");
    }

    #[test]
    fn a_query_with_no_words_finds_nothing() {
        let mut index = Index::new();
        index.add(node(1), "rust");
        assert!(index.search("  ,, ").is_empty());
    }

    #[test]
    fn add_replaces_and_remove_forgets() {
        let mut index = Index::new();
        index.add(node(1), "alpha beta");
        index.add(node(1), "gamma");
        assert!(index.search("alpha").is_empty(), "the old text is gone");
        assert_eq!(keys(index.search("gamma")), [node(1)]);
        assert_eq!(index.docs(), 1);
        index.remove(node(1));
        assert!(index.search("gamma").is_empty());
        assert_eq!(index.docs(), 0);
        index.remove(node(1)); // forgetting what is not there is fine
    }

    #[test]
    fn rebuild_equals_incremental() {
        let texts = [(1, "red green blue"), (2, "green blue yellow"), (3, "blue yellow red red"), (4, "purple")];
        let mut built = Index::new();
        for (n, text) in texts {
            built.add(node(n), text);
        }
        let mut edited = Index::new();
        edited.add(node(4), "something else entirely");
        edited.add(node(3), "blue yellow red red");
        edited.add(node(9), "green blue");
        edited.add(node(1), "stale");
        edited.remove(node(9));
        edited.add(node(1), "red green blue");
        edited.add(node(2), "green blue yellow");
        edited.add(node(4), "purple");
        for query in ["blue", "red", "green bl", "yel", "purple", "stale", "else"] {
            assert_eq!(built.search(query), edited.search(query), "{query}");
        }
        assert_eq!(built.docs(), edited.docs());
    }

    #[test]
    fn history_and_node_documents_share_one_index() {
        let mut index = Index::new();
        index.add(DocKey::History(7), "Rust blog");
        index.add(node(1), "rust book");
        assert_eq!(index.search("rust").len(), 2);
    }

    #[test]
    fn snippet_is_a_window_around_the_first_hit() {
        let text = "x".repeat(100) + " Needle in a haystack " + &"é".repeat(100);
        let snippet = Index::snippet(&text, "needle", 40).unwrap();
        assert!(snippet.to_lowercase().contains("needle"), "{snippet}");
        assert!(snippet.chars().count() <= 40 + 2, "{snippet}"); // plus the ellipses
        assert_eq!(Index::snippet("short text", "absent", 40), None);
        assert_eq!(Index::snippet("short text", "text", 40).as_deref(), Some("short text"));
    }

    /// Run once with `-- --ignored --nocapture`: the cost of a query over 10 000 documents of 200 words.
    #[test]
    #[ignore = "a timing probe, run by hand"]
    fn search_is_fast_on_ten_thousand_docs() {
        let words: Vec<String> = (0..2000).map(|i| format!("word{i}")).collect();
        let mut index = Index::new();
        let mut state = 12345u64;
        let mut next = || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (state >> 33) as usize
        };
        for n in 0..10_000u64 {
            let text: Vec<&str> = (0..200).map(|_| words[next() % words.len()].as_str()).collect();
            index.add(node(n), &text.join(" "));
        }
        for query in ["word17", "word17 word300", "word1"] {
            let start = std::time::Instant::now();
            let hits = index.search(query);
            println!("{query:?}: {} hits in {:?}", hits.len(), start.elapsed());
        }
    }
}
