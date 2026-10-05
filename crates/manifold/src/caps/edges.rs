//! The edge-type table: which node types an edge may join (spec section 3). Endpoints only for now; cardinality,
//! `acyclic`, removal variants and drawing arrive with the code that reads them.

use super::pattern::Pattern;
use crate::store::model::{TYPE_GROUP, TYPE_JSON_LEAF, TYPE_NOTE, TYPE_TAB};

/// The containment edge: a node is shown inside a group.
pub(crate) const CONTAINS: &str = "contains";

/// One edge type and the node types allowed at each end.
#[derive(Debug, Clone)]
pub(crate) struct EdgeRow {
    id: &'static str,
    sources: Vec<Pattern>,
    targets: Vec<Pattern>,
}

impl EdgeRow {
    pub(crate) fn new(id: &'static str, sources: &[&str], targets: &[&str]) -> EdgeRow {
        EdgeRow {
            id,
            sources: sources.iter().map(|p| Pattern::parse(p)).collect(),
            targets: targets.iter().map(|p| Pattern::parse(p)).collect(),
        }
    }
}

/// Every edge type, looked up by id.
#[derive(Debug, Default)]
pub(crate) struct EdgeTable {
    rows: Vec<EdgeRow>,
}

impl EdgeTable {
    pub(crate) fn new(rows: Vec<EdgeRow>) -> EdgeTable {
        EdgeTable { rows }
    }

    fn row(&self, id: &str) -> Option<&EdgeRow> {
        self.rows.iter().find(|r| r.id == id)
    }

    /// Whether a `source`-typed node may have a `target`-typed one on the other end of an `id` edge.
    pub(crate) fn can_connect(&self, id: &str, source: &str, target: &str) -> bool {
        self.row(id)
            .is_some_and(|r| r.sources.iter().any(|p| p.matches(source)) && r.targets.iter().any(|p| p.matches(target)))
    }

    /// Whether any node may be the target of an `id` edge (a group, for `contains`).
    pub(crate) fn target_accepts(&self, id: &str, target: &str) -> bool {
        self.row(id).is_some_and(|r| r.targets.iter().any(|p| p.matches(target)))
    }
}

/// The builtin edge rows: just `contains` for now.
pub(crate) fn builtin_edges() -> EdgeTable {
    EdgeTable::new(vec![EdgeRow::new(
        CONTAINS,
        &[
            TYPE_TAB,
            TYPE_GROUP,
            TYPE_NOTE,
            "image/*",
            "application/pdf",
            "application/json",
            TYPE_JSON_LEAF,
            "application/octet-stream",
        ],
        &[TYPE_GROUP],
    )])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::edges;

    fn table() -> EdgeTable {
        EdgeTable::new(vec![
            EdgeRow::new("contains", &["application/vnd.manifold.tab", "image/*"], &["application/vnd.manifold.group"]),
            EdgeRow::new("sees", &["*/*"], &["*/*+json"]),
        ])
    }

    #[test]
    fn can_connect_checks_both_endpoints_against_the_row() {
        let t = table();
        assert!(t.can_connect("contains", "application/vnd.manifold.tab", "application/vnd.manifold.group"));
        assert!(t.can_connect("contains", "image/gif", "application/vnd.manifold.group"));
        assert!(!t.can_connect("contains", "application/vnd.manifold.group", "application/vnd.manifold.tab"));
        assert!(!t.can_connect("contains", "video/mp4", "application/vnd.manifold.group"));
    }

    #[test]
    fn an_unknown_edge_type_connects_nothing() {
        assert!(!table().can_connect("nope", "image/gif", "image/gif"));
        assert!(!table().target_accepts("nope", "image/gif"));
    }

    #[test]
    fn target_accepts_looks_at_the_target_alone() {
        let t = table();
        assert!(t.target_accepts("sees", "application/ld+json"));
        assert!(!t.target_accepts("sees", "application/json"));
    }

    #[test]
    fn builtin_contains_takes_every_card_into_a_group_only() {
        use crate::store::model::{TYPE_GROUP, TYPE_TAB};
        let t = edges();
        for source in [
            TYPE_TAB,
            TYPE_GROUP,
            "text/markdown",
            "image/png",
            "application/pdf",
            "application/json",
            "application/octet-stream",
        ] {
            assert!(t.can_connect(CONTAINS, source, TYPE_GROUP), "{source}");
            assert!(!t.can_connect(CONTAINS, source, TYPE_TAB), "{source}");
        }
        assert!(!t.can_connect(CONTAINS, "application/vnd.manifold.jar", TYPE_GROUP));
    }
}
