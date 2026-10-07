//! The edge-type table: which node types an edge may join (spec section 3), where it is stored, whether it is
//! acyclic or moves cards, and the verb that draws it. Cardinality and removal variants arrive with the code that
//! reads them.

use super::pattern::Pattern;
use crate::store::model::{field, TYPE_GROUP, TYPE_JSON_LEAF, TYPE_LINK, TYPE_NOTE, TYPE_TAB};

/// The containment edge: a node is shown inside a group.
pub(crate) const CONTAINS: &str = "contains";
/// The edge from a card to the card it was opened from.
pub(crate) const OPENED_FROM: &str = "opened-from";
/// A user-drawn link between two cards, stored as a link node.
pub(crate) const LINK: &str = "link";

/// Where an edge lives in the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Storage {
    /// A field on the source node, by field number.
    Field(u32),
    /// A node of this type whose endpoints are the edge's.
    Node(&'static str),
    /// The tree edge (a node's group).
    Tree,
}

/// Every card type that may sit inside a group.
const CARD_TYPES: [&str; 8] = [
    TYPE_TAB,
    TYPE_GROUP,
    TYPE_NOTE,
    "image/*",
    "application/pdf",
    "application/json",
    TYPE_JSON_LEAF,
    "application/octet-stream",
];
/// The card types a link or opened-from edge may join: all but the JSON leaf.
const LINKABLE: [&str; 7] =
    [TYPE_TAB, TYPE_GROUP, TYPE_NOTE, "image/*", "application/pdf", "application/json", "application/octet-stream"];

/// One edge type and the node types allowed at each end.
#[derive(Debug, Clone)]
pub(crate) struct EdgeRow {
    id: &'static str,
    sources: Vec<Pattern>,
    targets: Vec<Pattern>,
    storage: Storage,
    acyclic: bool,
    affects_layout: bool,
    verb: Option<&'static str>,
}

impl EdgeRow {
    pub(crate) fn new(id: &'static str, sources: &[&str], targets: &[&str]) -> EdgeRow {
        EdgeRow {
            id,
            sources: sources.iter().map(|p| Pattern::parse(p)).collect(),
            targets: targets.iter().map(|p| Pattern::parse(p)).collect(),
            storage: Storage::Tree,
            acyclic: false,
            affects_layout: false,
            verb: None,
        }
    }

    pub(crate) fn stored_in(mut self, storage: Storage) -> EdgeRow {
        self.storage = storage;
        self
    }

    pub(crate) fn acyclic(mut self) -> EdgeRow {
        self.acyclic = true;
        self
    }

    pub(crate) fn layout(mut self) -> EdgeRow {
        self.affects_layout = true;
        self
    }

    /// Mark the edge as one the user can draw, with the menu verb that names it.
    pub(crate) fn user_creatable(mut self, verb: &'static str) -> EdgeRow {
        self.verb = Some(verb);
        self
    }

    // The accessors below are read by later links tasks.
    #[allow(dead_code)]
    pub(crate) fn id(&self) -> &'static str {
        self.id
    }

    #[allow(dead_code)]
    pub(crate) fn storage(&self) -> Storage {
        self.storage
    }

    #[allow(dead_code)]
    pub(crate) fn is_acyclic(&self) -> bool {
        self.acyclic
    }

    #[allow(dead_code)]
    pub(crate) fn affects_layout(&self) -> bool {
        self.affects_layout
    }

    #[allow(dead_code)]
    pub(crate) fn verb(&self) -> Option<&'static str> {
        self.verb
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

    pub(crate) fn row(&self, id: &str) -> Option<&EdgeRow> {
        self.rows.iter().find(|r| r.id == id)
    }

    /// Whether a `source`-typed node may have a `target`-typed one on the other end of an `id` edge.
    pub(crate) fn can_connect(&self, id: &str, source: &str, target: &str) -> bool {
        self.row(id)
            .is_some_and(|r| r.sources.iter().any(|p| p.matches(source)) && r.targets.iter().any(|p| p.matches(target)))
    }

    /// The rows the user can draw, in table order.
    #[allow(dead_code)]
    pub(crate) fn user_creatable(&self) -> impl Iterator<Item = &EdgeRow> {
        self.rows.iter().filter(|r| r.verb.is_some())
    }

    /// Whether any node may be the target of an `id` edge (a group, for `contains`).
    pub(crate) fn target_accepts(&self, id: &str, target: &str) -> bool {
        self.row(id).is_some_and(|r| r.targets.iter().any(|p| p.matches(target)))
    }
}

/// The builtin edge rows: `contains`, `opened-from` and `link`.
pub(crate) fn builtin_edges() -> EdgeTable {
    EdgeTable::new(vec![
        EdgeRow::new(CONTAINS, &CARD_TYPES, &[TYPE_GROUP]).stored_in(Storage::Tree).acyclic(),
        EdgeRow::new(OPENED_FROM, &LINKABLE, &LINKABLE)
            .stored_in(Storage::Field(field::PARENT))
            .acyclic()
            .layout()
            .user_creatable("Opened from\u{2026}"),
        EdgeRow::new(LINK, &LINKABLE, &LINKABLE).stored_in(Storage::Node(TYPE_LINK)).user_creatable("Link to\u{2026}"),
    ])
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

    #[test]
    fn opened_from_and_link_take_any_card_to_any_card_but_never_a_jar_or_a_link_node() {
        let t = edges();
        for a in [
            TYPE_TAB,
            TYPE_GROUP,
            "text/markdown",
            "image/png",
            "application/pdf",
            "application/json",
            "application/octet-stream",
        ] {
            for b in [TYPE_TAB, TYPE_GROUP, "image/png"] {
                assert!(t.can_connect(LINK, a, b), "{a} -> {b}");
                assert!(t.can_connect(OPENED_FROM, a, b), "{a} -> {b}");
            }
        }
        assert!(!t.can_connect(LINK, "application/vnd.manifold.jar", TYPE_TAB));
        assert!(!t.can_connect(LINK, TYPE_TAB, "application/vnd.manifold.link"));
        assert!(!t.can_connect(LINK, TYPE_JSON_LEAF, TYPE_TAB));
    }

    #[test]
    fn the_columns_say_what_each_edge_does() {
        let t = edges();
        assert!(t.row(OPENED_FROM).unwrap().affects_layout() && t.row(OPENED_FROM).unwrap().is_acyclic());
        assert!(!t.row(LINK).unwrap().affects_layout() && !t.row(LINK).unwrap().is_acyclic());
        assert_eq!(t.user_creatable().map(|r| r.id()).collect::<Vec<_>>(), [OPENED_FROM, LINK]);
        assert_eq!(t.row(LINK).unwrap().verb(), Some("Link to\u{2026}"));
        assert_eq!(t.row(CONTAINS).unwrap().storage(), Storage::Tree);
        assert_eq!(t.row(OPENED_FROM).unwrap().storage(), Storage::Field(field::PARENT));
        assert_eq!(t.row(LINK).unwrap().storage(), Storage::Node(TYPE_LINK));
    }
}
