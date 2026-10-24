//! Local page search: the text the hosts return for each tab (`text_store`) and, later, the index built from it.

pub(crate) mod text_store;

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
