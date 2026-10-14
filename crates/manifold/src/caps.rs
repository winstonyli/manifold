//! Capability and edge tables keyed by MIME type (spec 2026-10-04-node-model-design.md, sections 1 to 3).

mod edges;
mod pattern;
mod rows;

pub(crate) use edges::{Scope, Storage, CONTAINS, JAR_EDGE, LINK, OPENED_FROM};
pub(crate) use pattern::{Tag, TagSet};
use rows::{builtin_rows, Registry};
pub(crate) use rows::{Caps, DRAW_LAYERS};
use std::sync::OnceLock;

/// The builtin capability rows.
pub(crate) fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut registry = Registry::new();
        for row in builtin_rows() {
            registry.register(row).expect("builtin rows are distinct");
        }
        registry
    })
}

/// The builtin edge rows.
pub(crate) fn edges() -> &'static edges::EdgeTable {
    static EDGES: OnceLock<edges::EdgeTable> = OnceLock::new();
    EDGES.get_or_init(edges::builtin_edges)
}
