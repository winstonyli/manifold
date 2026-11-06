//! The one place keyed background work is queued, capped, deduplicated, cancelled and delivered: `Pool`. The favicon,
//! redirect, image and PDF workers each wrap one, and a new worker of that shape should too (`limits.rs` fails a
//! second `catch_unwind` in production code). Threads are spawned on demand, up to the pool's cap, and exit when the
//! queue is empty, so an idle app has none and a queued job runs with the event loop stopped; `tick` only collects.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, LockResult, Mutex, MutexGuard, PoisonError};

/// Jobs of type `J` keyed by `K`, run by `work` on at most `cap` threads at once. A key is live from `request` until
/// `tick` returns it or `cancel` forgets it.
pub(crate) struct Pool<K, J, R> {
    shared: Arc<Shared<K, J, R>>,
}

struct Shared<K, J, R> {
    cap: usize,
    work: Box<dyn Fn(&J) -> R + Send + Sync>,
    state: Mutex<State<K, J, R>>,
}

struct State<K, J, R> {
    /// Every live job, in request order, at most one per key. Restored sessions can hold hundreds; lookups scan.
    entries: Vec<Entry<K, J, R>>,
    threads: usize,
}

struct Entry<K, J, R> {
    key: K,
    /// The job until a thread takes it.
    queued: Option<J>,
    /// Where the thread that took the job leaves it. Shared with that thread, so removing the entry (`cancel`)
    /// orphans the cell: the thread fills it unseen and frees it on its own thread, outside every lock.
    out: Out<J, R>,
}

/// What a finished job leaves: the job and the result (`None`: `work` panicked). Empty while queued or running.
type Out<J, R> = Arc<Mutex<Option<(J, Option<R>)>>>;

/// Poisoning is recovered: `work` never runs under a lock, so nothing is half-updated.
fn unpoison<T>(lock: LockResult<MutexGuard<'_, T>>) -> MutexGuard<'_, T> {
    lock.unwrap_or_else(PoisonError::into_inner)
}

impl<K, J, R> Pool<K, J, R>
where
    K: PartialEq + Send + 'static,
    J: Send + 'static,
    R: Send + 'static,
{
    pub(crate) fn new(cap: usize, work: impl Fn(&J) -> R + Send + Sync + 'static) -> Self {
        let state = State { entries: Vec::new(), threads: 0 };
        Self { shared: Arc::new(Shared { cap, work: Box::new(work), state: Mutex::new(state) }) }
    }

    /// Queues `job` under `key`; `false` (and nothing done) while `key` is live.
    pub(crate) fn request(&self, key: K, job: J) -> bool {
        let spawn = {
            let mut state = unpoison(self.shared.state.lock());
            if state.entries.iter().any(|e| e.key == key) {
                return false;
            }
            state.entries.push(Entry { key, queued: Some(job), out: Out::default() });
            let spawn = state.threads < self.shared.cap;
            state.threads += usize::from(spawn);
            spawn
        };
        if spawn {
            let shared = Arc::clone(&self.shared);
            std::thread::spawn(move || run(&shared));
        }
        true
    }

    /// Forgets `key`'s job: a queued one never runs, a running or finished one's result is never returned.
    pub(crate) fn cancel(&self, key: &K) {
        // Dropped after the lock is released: a result can be a whole image.
        let _removed = {
            let mut state = unpoison(self.shared.state.lock());
            state.entries.iter().position(|e| e.key == *key).map(|i| state.entries.remove(i))
        };
    }

    /// Every job finished since the last call, in request order: its key, the job and the result (`None`: it
    /// panicked). Its key stops being live.
    pub(crate) fn tick(&self) -> Vec<(K, J, Option<R>)> {
        let mut state = unpoison(self.shared.state.lock());
        let finished = state.entries.extract_if(.., |e| unpoison(e.out.lock()).is_some());
        finished.filter_map(|e| unpoison(e.out.lock()).take().map(|(job, result)| (e.key, job, result))).collect()
    }

    /// Live keys. A cancelled job isn't one, so its thread can outlive this reaching 0.
    #[cfg(test)]
    pub(crate) fn pending(&self) -> usize {
        unpoison(self.shared.state.lock()).entries.len()
    }

    /// Polls until at most `n` threads are left, panicking with the count after 20 s. A thread leaves after filling
    /// its cell, so this is how a test knows a cancelled job has ended. The test releases its gates first.
    #[cfg(test)]
    pub(crate) fn wait_for_threads(&self, n: usize) {
        let end = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            let threads = unpoison(self.shared.state.lock()).threads;
            if threads <= n {
                return;
            }
            assert!(std::time::Instant::now() < end, "{threads} threads left, waiting for {n}");
            std::thread::yield_now();
        }
    }
}

/// A pool thread: runs queued jobs until none is queued.
fn run<K, J, R>(shared: &Shared<K, J, R>) {
    loop {
        let (job, out) = {
            let mut state = unpoison(shared.state.lock());
            match state.entries.iter_mut().find_map(|e| e.queued.take().map(|job| (job, Arc::clone(&e.out)))) {
                Some(next) => next,
                // Under the same lock as `request`'s count, so a concurrent request either finds its entry taken by
                // this thread or sees the new count and spawns its own.
                None => {
                    state.threads -= 1;
                    return;
                }
            }
        };
        let result = catch_unwind(AssertUnwindSafe(|| (shared.work)(&job))).ok();
        *unpoison(out.lock()) = Some((job, result));
    }
}

#[cfg(test)]
mod tests;
