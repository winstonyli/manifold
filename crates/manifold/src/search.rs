//! Local page search: the text the hosts return for each tab (`text_store`), the tokenizer and the BM25 `Index` built
//! from it.

#[allow(dead_code, reason = "the index service (Task 7) is the first user")]
mod index;
pub(crate) mod text_store;
#[allow(dead_code, reason = "the index service (Task 7) is the first user")]
mod tokenize;

#[allow(unused_imports, reason = "the index service (Task 7) is the first user")]
pub(crate) use index::{DocKey, Index};
#[allow(unused_imports, reason = "the index service (Task 7) is the first user")]
pub(crate) use tokenize::tokens;

/// What `App` keeps for search.
pub(crate) struct Search {
    pub(crate) texts: text_store::TextStore,
}

impl Search {
    /// Opens the text log in the profile (memory only under test).
    pub(crate) fn open() -> Self {
        let mut texts = text_store::TextStore::open(crate::profile::search_dir().as_deref());
        texts.compact_if_wasteful();
        Search { texts }
    }
}
