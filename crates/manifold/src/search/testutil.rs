//! Shared fixtures for the search tests: a document of word counts and the six-document two-topic corpus.

use std::collections::BTreeMap;

use crate::search::DocKey;
use crate::store::model::{ActorId, NodeId};

pub(super) fn key(n: u64) -> DocKey {
    DocKey::Node(NodeId { actor: ActorId(1), counter: n })
}

pub(super) fn doc(n: u64, words: &str) -> (DocKey, Vec<(&str, u32)>) {
    let mut counts: BTreeMap<&str, u32> = BTreeMap::new();
    for w in words.split_whitespace() {
        *counts.entry(w).or_default() += 1;
    }
    (key(n), counts.into_iter().collect())
}

/// Three documents about cats and three about Rust.
pub(super) fn topics() -> Vec<(DocKey, Vec<(&'static str, u32)>)> {
    vec![
        doc(1, "cat milk fur purr cat"),
        doc(2, "cat fur kitten milk"),
        doc(3, "kitten purr milk cat fur"),
        doc(4, "rust cargo crate compile rust"),
        doc(5, "cargo crate rust build"),
        doc(6, "compile build crate rust cargo"),
    ]
}
