//! The two walks over a parent edge (a node's group, a tab's parent), each owning its cycle guard: two replicas'
//! moves can race into a cycle that exists until the next save shows the document's groups, and a walk that loops
//! on one would freeze the frame it runs in. The edge is a closure, so the mirror and the document share them.

use std::collections::HashSet;
use std::hash::Hash;

/// The nodes above `start`, nearest first (`start` itself excluded), ending where the edge ends or on the first
/// node already seen (`looped`).
pub(crate) struct Ancestors<K, F> {
    next: Option<K>,
    seen: HashSet<K>,
    edge: F,
    looped: bool,
}

pub(crate) fn ancestors<K: Clone + Eq + Hash, F: FnMut(&K) -> Option<K>>(start: K, mut edge: F) -> Ancestors<K, F> {
    let next = edge(&start);
    Ancestors { next, seen: HashSet::from([start]), edge, looped: false }
}

impl<K, F> Ancestors<K, F> {
    /// Whether the walk ended on a cycle rather than at the top. Meaningful once the iterator is exhausted.
    pub(crate) fn looped(&self) -> bool {
        self.looped
    }
}

impl<K: Clone + Eq + Hash, F: FnMut(&K) -> Option<K>> Iterator for Ancestors<K, F> {
    type Item = K;

    fn next(&mut self) -> Option<K> {
        let cur = self.next.take()?;
        if !self.seen.insert(cur.clone()) {
            self.looped = true;
            return None;
        }
        self.next = (self.edge)(&cur);
        Some(cur)
    }
}

/// Every node below `root` at any depth (`root` excluded), each once, in a stack order. `children` lists a node's direct members; only those `expands` accepts are
/// searched further (a group, not a tab). A cycle can't hang it.
pub(crate) fn descendants<K: Clone + Eq + Hash>(
    root: K,
    children: impl Fn(&K) -> Vec<K>,
    expands: impl Fn(&K) -> bool,
) -> Vec<K> {
    let (mut out, mut seen, mut levels) = (Vec::new(), HashSet::from([root.clone()]), vec![root]);
    while let Some(level) = levels.pop() {
        for child in children(&level) {
            if seen.insert(child.clone()) {
                if expands(&child) {
                    levels.push(child.clone());
                }
                out.push(child);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ancestors_stop_at_the_top_or_on_a_cycle() {
        let parent = |k: &u32| (*k < 4).then_some(k + 1);
        let mut up = ancestors(1, parent);
        assert_eq!(up.by_ref().collect::<Vec<_>>(), [2, 3, 4]);
        assert!(!up.looped(), "ended at the top");
        let mut up = ancestors(1, |k: &u32| Some(k % 3 + 1));
        assert_eq!(up.by_ref().collect::<Vec<_>>(), [2, 3], "1 is not yielded again");
        assert!(up.looped());
    }

    #[test]
    fn descendants_visit_each_node_once_and_expand_only_groups() {
        // 1 holds 2 and 3; 2 holds 1 (a cycle) and 4; 4 is a leaf that "holds" 5 but is not expanded.
        let children = |k: &u32| match k {
            1 => vec![2, 3],
            2 => vec![1, 4],
            4 => vec![5],
            _ => vec![],
        };
        let mut out = descendants(1, children, |k| *k < 4);
        out.sort();
        assert_eq!(out, [2, 3, 4]);
    }
}
