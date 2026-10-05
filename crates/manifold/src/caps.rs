//! Capability and edge tables keyed by MIME type (spec 2026-10-04-node-model-design.md, sections 1 to 3).

pub(crate) mod bridge;
mod pattern;
mod rows;

use rows::{builtin_rows, Registry};
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
