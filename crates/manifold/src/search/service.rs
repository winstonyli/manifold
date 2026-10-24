//! The index on a worker thread: `queue_add`/`queue_remove` return at once, `search` locks the index for the length
//! of one query. Priority is the launcher's business, not set here.

use super::{DocKey, Index, TextStore};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex, MutexGuard};

enum Job {
    Add(DocKey, String),
    Remove(DocKey),
    /// Replaces the whole index with one built from these documents (built off the lock, then swapped in).
    Rebuild(Vec<(DocKey, String)>),
    Clear,
}

pub(crate) struct SearchService {
    index: Arc<Mutex<Index>>,
    tx: Sender<Job>,
    /// Bumps after each job, so a cache keyed by it knows when its answers may have changed.
    generation: Arc<AtomicU64>,
    /// Jobs queued and not finished.
    pending: Arc<AtomicUsize>,
}

/// What a tab's document holds: title, url and the stored text.
pub(crate) fn page_doc(title: &str, url: &str, text: &str) -> String {
    format!("{title} {url} {text}")
}

fn lock(index: &Mutex<Index>) -> MutexGuard<'_, Index> {
    index.lock().unwrap_or_else(|e| e.into_inner())
}

impl SearchService {
    /// Spawns the worker and queues a build of every record in `texts`. The worker ends when the service drops.
    pub(crate) fn start(texts: &TextStore) -> Self {
        let index = Arc::new(Mutex::new(Index::new()));
        let (tx, rx) = channel::<Job>();
        let generation = Arc::new(AtomicU64::new(0));
        let pending = Arc::new(AtomicUsize::new(0));
        let service =
            SearchService { index: index.clone(), tx, generation: generation.clone(), pending: pending.clone() };
        let docs = texts
            .ids()
            .filter_map(|id| texts.get(id))
            .map(|r| (DocKey::Node(r.id), page_doc(&r.title, &r.url, &r.text)))
            .collect();
        service.send(Job::Rebuild(docs));
        let builder = std::thread::Builder::new().name("search-index".into());
        let spawned = builder.spawn(move || {
            while let Ok(job) = rx.recv() {
                match job {
                    Job::Add(key, text) => lock(&index).add(key, &text),
                    Job::Remove(key) => lock(&index).remove(key),
                    Job::Rebuild(mut docs) => {
                        docs.sort_by_key(|&(key, _)| key); // ascending keys append to each posting list
                        let mut fresh = Index::new();
                        for (key, text) in docs {
                            fresh.add(key, &text);
                        }
                        *lock(&index) = fresh;
                    }
                    Job::Clear => *lock(&index) = Index::new(),
                }
                generation.fetch_add(1, Ordering::SeqCst);
                pending.fetch_sub(1, Ordering::SeqCst);
            }
        });
        if let Err(e) = spawned {
            eprintln!("search: could not start the index worker: {e}");
            service.pending.store(0, Ordering::SeqCst);
        }
        service
    }

    fn send(&self, job: Job) {
        self.pending.fetch_add(1, Ordering::SeqCst);
        if self.tx.send(job).is_err() {
            self.pending.fetch_sub(1, Ordering::SeqCst);
        }
    }

    pub(crate) fn queue_add(&self, key: DocKey, text: String) {
        self.send(Job::Add(key, text));
    }

    pub(crate) fn queue_remove(&self, key: DocKey) {
        self.send(Job::Remove(key));
    }

    /// Forgets every document.
    pub(crate) fn queue_clear(&self) {
        self.send(Job::Clear);
    }

    pub(crate) fn search(&self, query: &str) -> Vec<(DocKey, f32)> {
        lock(&self.index).search(query)
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    pub(crate) fn busy(&self) -> bool {
        self.pending.load(Ordering::SeqCst) > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::text_store::TextRecord;
    use crate::store::model::{ActorId, NodeId};
    use std::time::{Duration, Instant};

    fn id(n: u64) -> NodeId {
        NodeId { actor: ActorId(1), counter: n }
    }

    fn settle(service: &SearchService) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while service.busy() {
            assert!(Instant::now() < deadline, "the worker did not finish");
            std::thread::yield_now();
        }
    }

    fn keys(service: &SearchService, query: &str) -> Vec<DocKey> {
        service.search(query).into_iter().map(|(k, _)| k).collect()
    }

    fn record(n: u64, text: &str) -> TextRecord {
        TextRecord {
            id: id(n),
            url: format!("https://{n}.test/"),
            title: format!("Page {n}"),
            text: text.into(),
            at: 1,
        }
    }

    #[test]
    fn queued_adds_become_searchable_and_generation_advances() {
        let service = SearchService::start(&TextStore::open(None));
        settle(&service);
        let before = service.generation();
        service.queue_add(DocKey::Node(id(1)), "needle in a haystack".into());
        settle(&service);
        assert_eq!(keys(&service, "needle"), [DocKey::Node(id(1))]);
        assert!(service.generation() > before);
    }

    #[test]
    fn start_builds_from_the_text_store() {
        let mut texts = TextStore::open(None);
        texts.put(record(1, "alpha beta"));
        texts.put(record(2, "gamma"));
        let service = SearchService::start(&texts);
        settle(&service);
        assert_eq!(keys(&service, "alpha"), [DocKey::Node(id(1))]);
        assert_eq!(keys(&service, "gamma"), [DocKey::Node(id(2))]);
        let mut by_title = keys(&service, "page");
        by_title.sort();
        assert_eq!(by_title, [DocKey::Node(id(1)), DocKey::Node(id(2))], "the title is in the document");
    }

    #[test]
    fn removing_a_node_removes_its_hits() {
        let service = SearchService::start(&TextStore::open(None));
        service.queue_add(DocKey::Node(id(1)), "needle".into());
        service.queue_remove(DocKey::Node(id(1)));
        settle(&service);
        assert!(keys(&service, "needle").is_empty());
    }

    #[test]
    fn clear_empties_the_index() {
        let service = SearchService::start(&TextStore::open(None));
        service.queue_add(DocKey::Node(id(1)), "needle".into());
        service.queue_clear();
        settle(&service);
        assert!(keys(&service, "needle").is_empty());
    }

    /// Run by hand in release: `cargo test -p manifold --release -- --ignored --nocapture startup_build`. The time
    /// to load a 50 MB `text.log` and build the index from it (decision P1: no saved index while this is under 1 s).
    #[test]
    #[ignore = "a timing probe, run by hand"]
    fn startup_build_of_a_fifty_megabyte_log() {
        let dir = crate::store::testutil::temp_dir("startup-build");
        let mut state = 99u64;
        let mut next = || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (state >> 33) as usize
        };
        let words: Vec<String> = (0..20_000).map(|i| format!("word{i}")).collect();
        let mut texts = TextStore::open(Some(&dir));
        for n in 0..5_000u64 {
            let text: Vec<&str> = (0..1_500).map(|_| words[next() % words.len()].as_str()).collect();
            texts.put(record(n, &text.join(" ")));
        }
        let bytes = std::fs::metadata(dir.join("text.log")).map_or(0, |m| m.len());
        let start = Instant::now();
        let reopened = TextStore::open(Some(&dir));
        let loaded = start.elapsed();
        let service = SearchService::start(&reopened);
        settle(&service);
        println!("{} MB log: load {loaded:?}, load + index {:?}", bytes / 1_000_000, start.elapsed());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
