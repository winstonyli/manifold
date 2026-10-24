//! Local page search: the text the hosts return for each tab (`text_store`), the tokenizer and the BM25 `Index`
//! built from it (`service`, on a worker thread), and `sync`, which keeps the index equal to what the document and
//! the text log hold.

mod arrange;
mod cluster;
mod index;
mod lsa;
mod service;
mod suggest;
mod sync;
pub(crate) mod text_store;
mod tokenize;

pub(crate) use index::{DocKey, Index};
pub(crate) use service::{page_doc, SearchService};
pub(crate) use text_store::TextStore;

use crate::store::model::NodeId;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Instant;

/// The cards whose document matched a query, best first, at one index generation.
pub(crate) struct Hits {
    pub(crate) ids: Vec<NodeId>,
    pub(crate) set: HashSet<NodeId>,
}

/// What `App` keeps for search.
pub(crate) struct Search {
    pub(crate) texts: TextStore,
    pub(crate) service: SearchService,
    /// The fingerprint of each document as last queued, so `sync` queues only what changed.
    indexed: HashMap<DocKey, u64>,
    /// When `sync` next runs (the debounce for note edits and the pace of everything else).
    pub(crate) next_sync: Option<Instant>,
    /// The last query's hits, keyed by its text and the index generation.
    cache: RefCell<Option<(String, u64, Rc<Hits>)>>,
    /// The groups offered by the last "Suggest groups" (`App::suggestions` says which still stand).
    pub(crate) suggestions: Option<suggest::Suggestions>,
}

impl Search {
    /// Opens the text log in the profile (memory only under test) and starts the index from it.
    pub(crate) fn open() -> Self {
        let mut texts = TextStore::open(crate::profile::search_dir().as_deref());
        texts.compact_if_wasteful();
        Self::from_texts(texts)
    }

    /// Starts the index from `texts`. `indexed` lists what that build holds, with a fingerprint nothing matches, so
    /// the first `sync` re-queues each and its sweep removes any whose document is gone.
    pub(super) fn from_texts(texts: TextStore) -> Self {
        let service = SearchService::start(&texts);
        let indexed = texts.ids().map(|id| (DocKey::Node(id), 0)).collect();
        Search { texts, service, indexed, next_sync: None, cache: RefCell::new(None), suggestions: None }
    }

    /// Forgets every stored text and the index ("Clear search index"). Titles and urls of what is open come back at
    /// the next `sync`.
    pub(crate) fn clear(&mut self) {
        self.texts.clear();
        self.service.queue_clear();
        self.indexed.clear();
        self.next_sync = None;
    }

    /// Whether a sync pass has run since the last change that resets it.
    pub(crate) fn synced(&self) -> bool {
        self.next_sync.is_some()
    }

    /// The cards whose document holds every word of `text`.
    pub(crate) fn hits(&self, text: &str) -> Rc<Hits> {
        let generation = self.service.generation();
        if let Some((t, g, hits)) = &*self.cache.borrow() {
            if t == text && *g == generation {
                return hits.clone();
            }
        }
        let ids: Vec<NodeId> = self
            .service
            .search(text)
            .into_iter()
            .filter_map(|(key, _)| if let DocKey::Node(id) = key { Some(id) } else { None })
            .collect();
        let hits = Rc::new(Hits { set: ids.iter().copied().collect(), ids });
        *self.cache.borrow_mut() = Some((text.to_owned(), generation, hits.clone()));
        hits
    }
}
