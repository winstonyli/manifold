---
status: draft
---

# One background-worker pool (ARC2)

## Why

Four background workers each hand-roll a queue: `FaviconFetcher`, `RedirectFetcher`, `ImageDecoder`, `PdfRenderer`. The second survey (restructuring-cleanup plan, item ARC2) tabled how they differ and found five defects that come from the differences, not from any one worker's job:

1. A PDF render panic kills its worker with no result. The card spins for ever, and once both workers are dead `request` drops jobs silently.
2. Two jobs for one id finish in either order, and results carry no job identity, so an old result or `Err` overwrites the new state.
3. In PDF `State`, a second `begin` overwrites the first `Cancel` and the first `finish` removes the second's, so that render can't be cancelled.
4. `Err("cancelled")` is toasted as "Couldn't open PDF" if a node with that id exists by then.
5. `cancel` of a queued PDF then `request` leaves two queue entries.

Defects 2, 3 and 4 have failing tests in the tree (`9298e7e6`, ignored; PARKED "Worker pool defect tests"). Queued favicon and redirect jobs also start only from `tick`, which a modal file dialog stops and headless mode runs only sporadically.

## Decided

All 2026-11-05, by the user's Decides (d1, l1, n1, o1, q1, r1):

- `worker.rs` holds `Pool<K, J, R>`: threads spawned on demand up to a cap, draining the pool's jobs, exiting when none is queued.
- `request` is a no-op while the key is live. `cancel` drops a queued job or discards a running job's result.
- Image and PDF wrappers call `cancel` then `request`, so the latest request wins with no policy flag.
- PDF keeps a plain UI-thread map of `Cancel` handles, with the `Inbox` made at request time and carried in the job.
- `catch_unwind` lives in the pool, a panic reaches the wrapper as `None` with its job handed back, and each wrapper words its own failure. Each pool has its own cap.
- The pool's state is one list of entries, each with a result cell its thread fills; a cancel orphans the cell, so no token is needed.
- Four typed wrappers stay. `new` takes the work function, and `tick` returns tuples.

## Goals

- All five defects gone, each proved by a test.
- One mechanism for queueing, capping, dedupe, cancel, panic and result delivery.
- `tick` delivers results and starts nothing: queued jobs run with the event loop stopped.
- Same caps as today: favicon 4, redirect 4, image 2, PDF 2. One pool per kind, because a redirect can hold a slot for about 13 minutes.

## Non-goals

- `SuggestionFetcher`, JSON search, `search/service.rs`, `view_install` downloads and tab spawn stay as they are (not keyed, or not capped, or reporting through the event proxy).
- Redirect and favicon get no cancel call. A deleted node's redirect result is handled at `tick`, as today.
- No change to what `tick_fetchers` does with a result, beyond what follows from stale results no longer reaching it.

## The pool

Home: `crates/manifold/src/worker.rs`, tests in `worker/tests.rs`, `mod worker;` in `main.rs`, `"worker"` in `CORE_MODULES` (`crates/gate/src/tier.rs`) beside `net`, `redirect_fetch` and `favicon_fetch`, so an edit to the pool gets the full gate. It stays out of `CORE_IMPORTS`: the pool touches no store, session or engine, so importing it doesn't raise a UI file's tier.

```rust
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

impl<K, J, R> Pool<K, J, R> where K: PartialEq + Send + 'static, J: Send + 'static, R: Send + 'static {
    pub(crate) fn new(cap: usize, work: impl Fn(&J) -> R + Send + Sync + 'static) -> Self;
    /// Queues `job` under `key`; `false` (and nothing done) while `key` is live.
    pub(crate) fn request(&self, key: K, job: J) -> bool;
    /// Forgets `key`'s job: a queued one never runs, a running or finished one's result is never returned.
    pub(crate) fn cancel(&self, key: &K);
    /// Every job finished since the last call, in request order: its key, the job and the result (`None`: it
    /// panicked). Its key stops being live.
    pub(crate) fn tick(&self) -> Vec<(K, J, Option<R>)>;
    /// Live keys. A cancelled job isn't one, so its thread can outlive this reaching 0.
    #[cfg(test)]
    pub(crate) fn pending(&self) -> usize;
    /// Polls until at most `n` threads are left, panicking with the count after 20 s. A thread leaves after filling
    /// its cell, so this is how a test knows a cancelled job has ended. The test releases its gates first.
    #[cfg(test)]
    pub(crate) fn wait_for_threads(&self, n: usize);
}
```

Everything is `&self`. An entry's lifecycle: queued (`queued` is `Some`), running (`queued` is `None`, the cell empty), finished (the cell filled). Only the thread that took a job fills its cell, and it took the job under the state lock, so no other combination arises. An entry exists exactly while its key is live.

There are two lock levels, in one order: state, then a cell. Only `tick` holds both. A thread takes the cell alone, after releasing the state lock. `work` runs under neither.

- **`request`:** under the state lock, return `false` if any entry has `key`. Otherwise push an entry with a new cell, and if `threads < cap` count one more thread. Spawn after releasing the lock.
- **Worker thread:** loop. Under the state lock, take the first `queued` job and clone its cell; if there is none, subtract one from `threads` and return (same lock, so a concurrent `request` either has its job taken by this thread or sees the new count and spawns). With no lock held, run `catch_unwind(AssertUnwindSafe(|| work(&job))).ok()`. Then store `(job, result)` in the cell. For a cancelled job the cell is orphaned: nobody will read it, and it is freed when the thread drops it, outside every lock.
- **`cancel`:** under the state lock, remove `key`'s entry with `Vec::remove` (not `swap_remove`: `tick`'s order is request order), and drop it after releasing the lock, since a result can be a whole image. A running job is left to finish into its orphaned cell. The cap is on threads, not live keys: a new job for that key may run beside the old one, and the old one's thread counts against the cap until it ends.
- **`tick`:** under the state lock, `extract_if` the entries whose cell is filled and take each cell's contents. Results are in request order; no caller depends on finish order.
- **Lock poisoning:** recovered with `into_inner` through one small helper over a `LockResult`, as `State::lock` does today. `work` never runs under a lock.

There is no `mpsc`: results stay reachable from the entry, which is what lets `cancel` reach a finished job the UI has not taken. Nothing is lost by it: no result wakes the event loop today either (the four workers are polled from `drain_tab_events`), so the pool needs no notify hook.

The panic hook already prints a panic, so the pool logs nothing of its own; `ImageDecoder::tick` adds the path, as `guarded` does today. A job's or result's `Drop` runs outside `catch_unwind`, so one that panicked would lose its thread's slot; no job here can panic there. `thread::spawn` stays: it panics on thread exhaustion on the UI thread, as `favicon_fetch` and `redirect_fetch` do today. No production code catches a panic on that thread, so the app ends there, and the count `request` raised before spawning doesn't matter.

### Why a panic is `None` with the job handed back

The pool catches a panic but can't word it: image's message names the file, redirect's carries the job. So `work` borrows `J` and `tick` returns the job beside `Option<R>`, and each wrapper turns `None` into its own failure where it already maps results. Nothing is built per request, and no job type needs `Clone`.

## The four wrappers

Each keeps its public type and its name in `Workers`, so `tick_fetchers` keeps its four loops. The key is derived inside the wrapper, so a caller can't pass one that disagrees with its job.

| | `K` | `J` | `R` | on `None` | repeat request |
|---|---|---|---|---|---|
| `FaviconFetcher` | `String` origin | `String` origin | `Option<FaviconImage>` | `None` (`flatten`) | dropped |
| `RedirectFetcher` | `NodeId` (`job.node_id`) | `FetchJob` | `Option<WebBody>` | `None` body (`flatten`) | dropped |
| `ImageDecoder` | `NodeId` | `String` path | `Result<Decoded, String>` | `Err("the decoder crashed")`, named by `tick` | cancel, then run |
| `PdfRenderer` | `NodeId` | `(String, Cell<Option<Inbox>>)` | `Result<PdfRenderResult, String>` | `Err("the renderer crashed")` | cancel, then run |

`tick_fetchers` prefixes PDF errors with "Couldn't open PDF {name}: ", so the PDF failure reads "Couldn't open PDF d.pdf: the renderer crashed".

Redirect's key stays `job.node_id`, not the target: for `FetchTarget::Tab` that id is freshly allocated, so repeated events for one tab aren't deduped today, and keying on the tab would silently add that.

**`new` takes the work function.** The `new`/`with_decode` and `new`/`with_render` pairs become one `new`, and favicon's and redirect's `fetch` fields, which tests assign, become `new` arguments. `Workers::new` passes the real functions (`favicon_fetch::fetch_favicon`, `redirect_fetch::fetch`, `image_decode::decode`, `pdf::render::render`; all but `fetch`, already `pub`, become `pub(crate)`), and tests pass stubs:

```rust
FaviconFetcher::new(cache_dir: Option<PathBuf>, fetch: fn(&str) -> Option<FaviconImage>)
RedirectFetcher::new(dir: Option<PathBuf>, fetch: fn(&Path, NodeId, &str, ContentKind) -> Option<WebBody>)
ImageDecoder::new(decode: impl Fn(&str) -> Result<Decoded, String> + Send + Sync + 'static)
PdfRenderer::new(render: impl Fn(&str, Inbox) -> Result<PdfRenderResult, String> + Send + Sync + 'static)
```

The two `fn` pointers keep the existing `|_| None` stubs compiling; the image and PDF stubs capture a gate, so they need `impl Fn`, and `Sync` where today's seams need `Clone`: `work` is shared between threads.

`tick` is `&self` on the first three, as `Pool::tick` is; only `PdfRenderer` needs `&mut self`.

**`FaviconFetcher`:** `work` closes over `cache_dir` and `fetch`; `new` still spawns its `prune_cache` thread. `tick` maps `(origin, _, image)` to `(origin, image.flatten())`. `in_flight` and `throttle` go. `request` becomes `&self`.

**`RedirectFetcher`:** `tick` returns `Vec<(FetchJob, Option<WebBody>)>`, mapping `(_, job, body)` to `(job, body.flatten())`. `FetchResult` goes; `tick_fetchers` and `drain_fetches` read the tuple. `in_flight` and `throttle` go. `request` keeps its `bool`, which tests read, and becomes `&self`. Bodies don't pile up while `tick` is stopped: a finished job holds a path or a note's text, not the file.

**`ImageDecoder`:** `request` is `pool.cancel(&id); pool.request(id, path);`. Gains `cancel`. `tick` is the one place an error is named: `None` logs "image: decoder panicked on {path}" and becomes `Err("the decoder crashed")`, and every `Err` becomes "Couldn't open image {name}: {why}". `decode` keeps its `eprintln` and returns the bare reason, or the name would appear twice. `Finished`, `guarded`, `pending: Arc<AtomicUsize>` and the two workers' loop go; `pending` delegates to the pool.

**`PdfRenderer`:** gains `cancels: HashMap<NodeId, Cancel>`, UI-thread only. `run_child` keeps taking its `Inbox` by value, so the job holds it in a `Cell` and `work` takes it out: `render(path, inbox.take().expect("a pool runs a job once"))`. `Cell` is `Send`, which is all `J` needs. The protocol crate doesn't change.

```rust
pub fn request(&mut self, id: NodeId, path: String) {
    self.cancel(id);
    let (cancel, inbox) = manifold_view_protocol::render::channel();
    self.cancels.insert(id, cancel);
    self.pool.request(id, (path, Cell::new(Some(inbox))));
}

pub fn cancel(&mut self, id: NodeId) {
    self.pool.cancel(&id);
    if let Some(cancel) = self.cancels.remove(&id) {
        cancel.cancel();
    }
}

pub fn tick(&mut self) -> Vec<(NodeId, Result<PdfRenderResult, String>)> {
    let done = self.pool.tick();
    done.into_iter()
        .map(|(id, _, result)| {
            self.cancels.remove(&id);
            (id, result.unwrap_or_else(|| Err("the renderer crashed".into())))
        })
        .collect()
}
```

An entry leaves `cancels` when its result is delivered or its job is cancelled, and every live job ends one of those two ways. A `Cancel` sent to a queued job's `Inbox` is harmless: `cancel` removes that job, so nothing reads it. `run_child` still returns `Err("cancelled")` for a killed child, but `pool.cancel` always runs first and `PdfRenderer` is the only user of `Cancel`, so that result lands in an orphaned cell. `State`, `Inner` and `is_cancelled` go; `#[cfg(test)] pending` delegates to the pool, as `ImageDecoder`'s does. `request`, `cancel` and `tick` become `&mut self`.

## One cancel for a node

Image decodes gain the cancel PDFs have, and every path that drops a node calls one helper rather than asking what kind it was:

```rust
impl Workers {
    /// Node `id` left the live graph: nothing queued or running for it may report.
    pub(crate) fn cancel_node(&mut self, id: NodeId) { .. }  // image_decoder, then pdf_renderer
}
```

- `unshow` (`node_store.rs`): replaces the `as_pdf` check, unconditionally.
- `teardown_running_session` (`session_files.rs`): replaces the PDF filter, over every id in `self.nodes`.
- `reload_fetched_node` (`fetched.rs`): at its `self.nodes.remove(&id)`, so a reload that changes the node's kind leaves no job of the old kind running. A same-kind reload cancels through `request` anyway.

Cancelling on the hide path is safe: every re-show runs `apply_visibility`, `materialize`, `insert_materialized`, then `insert_image_node_with_id` or its PDF twin, which requests a new job. `reopen_node` takes the same route, and a missing file with a known url inserts a placeholder with no job.

This closes the image half of defect 4, which the survey didn't list: a session replaced by one with the same ids could toast a stale decode error.

## Tests

The defect tests:

- `the_newest_request_for_a_node_decides_its_image` and `a_cancelled_render_says_nothing_to_a_card_that_took_its_id` are un-ignored. `with_decode` and `with_render` become `new`, and their one-second wait for a result that may never come becomes `wait_for_threads(0)` and a `tick`.
- `a_finished_render_leaves_a_later_render_of_its_id_cancellable` tests `State`, which goes. It is rewritten against `PdfRenderer` with its cap of 2 and stubs gated per path, each signalling when it starts: request, wait for the first stub's start, request again, wait for the second's, release the first, `wait_for_threads(1)`, `tick`, then assert the tick returned nothing and `cancels` still holds the id; then `cancel` leaves `cancels` empty and `pending` 0. A stale result delivered by that `tick` would have taken the id out of `cancels`, so the test can fail. That `PdfRenderer::cancel` calls `Cancel::cancel` has no unit test: an `Inbox` shows nothing without a real child, and the protocol crate's `render_child` tests cover what follows the call.
- `a_cancelled_queued_render_is_not_queued_twice` is new and proves defect 5 through `PdfRenderer`; the pool-level `cancel` test doesn't reach it. Two gated jobs fill both slots (the test waits for both to start), then `request(3, "a")`, `cancel(3)`, `request(3, "b")`, release, `wait_for_threads(0)`: the stub saw "b" once and never "a".
- `hiding_a_node_cancels_its_decode` and `reloading_a_node_cancels_its_decode` are new, for the two `cancel_node` callers the teardown test doesn't reach: a gated image stub on `App::default()`, then `pending` is 0 after `unshow`, or after `reload_fetched_node`. The reload's node needs a source url, or the function returns early, and its body is a `ContentKind::Json` file that is missing: an image or PDF body starts a job and an image body cancels through `request`, so only this body can show the call. Without the call the job stays live, so both can fail.

New in `worker/tests.rs`, with gated stubs that signal their start (a test that means a running job waits for that signal) and no fixed sleeps or timed absence waits (the prototype's 100 ms waits go). A cap is checked on the count itself: `request` raises `threads` under the lock before it returns, so with every running job gated, `threads == cap` and the extra job's `queued` still `Some` right after the over-cap `request` is deterministic. As a child module these tests read `pool.shared.state` directly, and a peak-concurrency counter stays as a second check. "This result never comes" is `wait_for_threads(0)`, then `tick`, here and in every wrapper test that checks for silence; `ImageDecoder` and `PdfRenderer` pass `wait_for_threads` through under `cfg(test)`, as they do `pending`, since a cancelled job is invisible to `pending`:

- at most `cap` jobs run at once, and queued jobs start without a `tick` (replaces `a_throttle_runs_at_most_its_cap_and_hands_on_in_order`, `at_most_four_favicon_fetches_run_at_once` and `at_most_four_redirect_fetches_run_at_once`; each wrapper's cap is a named constant passed to `new`, and a test of it would only restate it);
- a live key's repeat is dropped, and the key can be requested again after `tick` returns it;
- `cancel` of a queued job never runs it; of a running job, drops its result; of a finished job not yet taken, drops its result;
- at a cap of 1, jobs start in request order, and after a middle entry is cancelled `tick` returns the rest in request order;
- `cap_holds_when_a_running_job_is_cancelled`: once the job has signalled its start, the cancelled job's thread still counts, so `threads` is 1 right after `cancel` and again after the next `request`;
- a panic delivers `None` with its job, and the pool keeps working (defect 1; replaces `a_panicking_fetch_gives_its_slot_back`);
- threads exit when no job is queued, and a later `request` still runs;
- `request_racing_a_thread_exit_is_never_stranded`: 20,000 requests against a cap of 1, each waited for. It stranded a job in 3 of 3 runs of a shape that counted threads wrongly, which the other tests passed;
- a model test: a fixed-seed random run of `request`, `cancel`, cancel-then-request and `tick`, with gated and panicking jobs, checked after each step against a plain model (a delivered job is the key's newest live request, every accepted job is delivered or cancelled exactly once, `request` returns "key not live", at most `cap` jobs run, and after a drain `threads` is 0 and a later request runs). About 3,000 seeds, sized to take about a second under `cargo unit` (`model-best`: 0.3 ms a seed in an optimized build, so the plan measures it in the test profile). The survey's version adds a check that nothing drops under a lock, which needs a hook in `lock`, so that check stays out.

Each wrapper keeps one test of its own `None` mapping, since the pool can't see its shape: `a_panicking_decode_is_an_error_for_that_image_only` (rewritten through `new`, as `guarded` goes), a new PDF one reading "the renderer crashed" and leaving `cancels` empty, `a_panicking_fetch_still_frees_its_node` (checking the tuple carries its job), and a new favicon one, `a_panicking_fetch_is_no_favicon`. `a_cancelled_decode_reports_nothing` is new, for `ImageDecoder::cancel`.

Deleted with `State`, their cases now in the `cancel` tests above: `a_cancel_before_the_render_starts_skips_it`, `a_cancel_while_running_kills_instead_of_marking_and_a_finish_clears_it`, `a_new_request_clears_a_stale_cancel`.

Rewritten:

- `a_decode_comes_back_named_and_a_bad_file_comes_back_as_an_error`: its `starts_with` on the bad file becomes an `assert_eq` of the whole message, with the reason computed from `image::load_from_memory` rather than hard-coding the library's text, so a doubled name fails.
- `teardown_cancels_pending_pdf_renders` becomes `teardown_cancels_image_decodes_and_pdf_renders`: the mark is gone, so it swaps in gated stubs (a plain `request` on `App::default()` would launch the real PDF host), requests one of each, tears down, and asserts both `pending`s are 0. Without the cancel, even a finished job stays live until `tick`, so the test can fail.
- Every test that builds a wrapper passes a work function to `new`, including those that assign nothing today (`RedirectFetcher::new(None)`, `ImageDecoder::new()`): a stub, or the real `image_decode::decode` where the test decodes real bytes. Every `let mut fetcher` in `favicon_fetch` and `redirect_fetch` loses its `mut`, since `request` and `tick` are `&self` and `-D warnings` rejects the leftover.
- Tests reading `FetchResult` fields read the tuple (`session_sync/tests/misc.rs`, `remote.rs`), and `session_sync/tests.rs` drops `FetchResult` from its import, with `drain_fetches` returning `Vec<(FetchJob, Option<WebBody>)>`.

Stubs capture a `Receiver` as `Arc<Mutex<Receiver>>` or `Mutex<Receiver>`, as the defect tests do: `work` must be `Sync`.

`finish_image_decodes` and `drain_fetches` keep working through `pending` and `tick`.

## Guard

`crates/gate/src/limits.rs` gains `catch_unwind_lives_only_in_the_worker_pool`. `sources()` walks every crate and keeps inline test blocks, so the check filters on `manifold/src/` (the slash keeps `manifold-view-*` out), takes the first `measure(..).0` lines of each file, skips comment lines, matches `catch_unwind(`, and asserts the users are exactly `manifold/src/worker.rs`, so it also fails if the pool loses its own. It fails on the likeliest form of the drift, a new worker with its own panic guard. No check can spot a hand-written queue in general; that half rests on the module doc of `worker.rs` naming itself as the place for keyed background work.

## Considered and rejected

Each was built in a throwaway worktree on 2026-11-05 unless marked. Names in brackets are runs, `2026-11-05-<name>` under `claude-runs/manifold`. Sizes are production code lines of `worker.rs`, without blanks or comments, to within a line or two; the chosen shape is 82.

- **Prebuilt `failed` value per request.** Allocates on every request, and `FetchJob` needs `Clone`.
- **`failed` closure at `new`.** One more concept for what `tick` can do. It would earn its place only with the wrapper ticks gone (next item). Not built.
- **`run_child` borrowing its `Inbox`.** Loses the compile-time one-use guarantee, changes a public signature and four call sites, and leaves a late frame in a kept `Inbox` until `tick`.
- **Wrappers collapsed into `Pool` aliases** (`proto-m1`). 75 fewer production lines, but callers pass the key twice, results turn into positional `Option<Option<_>>`, `tick_fetchers` grows 19 lines, crash wording splits between it and `PdfRenderer::tick`, and PDF stays a struct, so there are two worker shapes instead of one.
- **Declarative kind** (cap, work, panic meaning and repeat policy in one description, PDF's cancels in the pool). Brings back the policy flag, and cancelling a running job needs shared access to it, which `Cell` forbids. Read, not built.
- **Key derived from the job** (`t3-key`). 16 more lines: favicon's duplicate goes, but three wrappers gain a key closure.
- **A wrapper shared by image and PDF** (`t5-nodepool`). 25 more lines.
- **Three collections, `live`, `queue` and `done`** (`pool-tests`, 105 lines). O(1) at any size, but three collections to keep in step, a token, and `K: Hash + Clone`. Its `live` can't be derived from the others without a fourth list (112 lines).
- **`Slot` enum with a token** (`proto-final-v1`, 118 lines). States named by the type, one lock, but the most lines, and a `Vec` allocated every `tick` unless it uses `extract_if`.
- **`queued` and `done` options with a token** (96 lines). One lock, but keeps the token, its stale-result branch and a second state lock per job.
- **Counting threads from `Arc::strong_count`.** Stranded a job in 3 of 3 stress runs: the `Arc` drops after the lock is released.
- **Running entries as the thread count** (127 lines). Needs a tombstone for a cancelled running job, or the cap fails.

All of base, the entry list, `Slot`, the options shape and the chosen cell passed a randomized model test with 0 failures in 300,000 seeds each (`pool-model`, `model-best`, `model-v1`, `model-v2`). It caught every mutation that applied, except a thread-exit race it caught only with a `yield_now` added. The chosen shape passed 2,027 tests (`proto-final`).

## End-state fit

| Change | Target | Replaces | Deletes |
|---|---|---|---|
| Queue, cap, dedupe, cancel, panic, delivery | `worker.rs` `Pool` | four hand-rolled loops | `net::Throttle` and its test, both `in_flight` sets, both `Mutex<Receiver>` queues, PDF `State`/`Inner`, `guarded`, `ImageDecoder`'s atomic, three `catch_unwind`s |
| Seams and result types | `new` parameters, tuples | `with_decode`, `with_render`, the `fetch` fields | `FetchResult` (and its import in `main.rs`), `Finished`, the `new`/`with_*` pairs |
| Cancel on node removal | `Workers::cancel_node` | the two `as_pdf` checks | `PdfRenderer::is_cancelled` |
| Panic guard placement | `limits.rs` check | nothing | nothing |

Docs and leftovers:

- PARKED "Worker pool defect tests" goes, and ARC2 leaves "Shared-owner refactors". PARKED "Pool scans" (trigger: about 5,000 live entries) stays.
- PARKED GATE-FETCHERS is rewritten, and its name stays in the list at the head of that section: it decided against unifying these until a fifth fetcher, and `image_decode` is that fifth. New text: "**GATE-FETCHERS.** `favicon_fetch`, `redirect_fetch`, `image_decode` and `pdf/render` share `worker.rs` (ARC2). `suggestions` stays out: it is not keyed or capped. *Decided against*; *trigger:* a second unkeyed fetcher."
- The restructuring-cleanup plan already links this spec (item ARC2, "Own plan"); its survey paragraph stays as history.
- README's module list ("and the fetchers") names `worker`.
- Every doc comment naming `Throttle`, a shared queue, fixed workers or `tick` starting jobs is rewritten to point at `worker.rs`: the module docs of `net.rs`, `favicon_fetch.rs`, `redirect_fetch.rs`, `image_decode.rs` and `pdf/render.rs`, and the item comments on `unshow` and `teardown_running_session`. A grep for `net::Throttle`, `Throttle::` and `throttle:` in `crates/manifold/src` then returns nothing; other crates' "throttle" comments are about something else.
- Imports only the deleted code used go; `-D warnings` names each (for example `VecDeque` in `net.rs`, `HashSet` in the three wrappers that had one, `mpsc` throughout, `AtomicUsize` in `image_decode.rs`).

## Risks

- **Every pool operation scans its entries, under the state lock.** Nothing caps nodes, and restore requests every shown image and PDF at once, so a pool can hold a whole session: realistically tens to hundreds. Measured on a shared machine with no timing lease (`pool-cost5`), `tick` costs 5 us at 1,000 entries and 60 us at 10,000 per frame, about 0.4% of a 60 Hz frame; a burst of 10,000 requests about 22 ms on the UI thread; draining 10,000 trivial jobs at a cap of 4 takes 134 ms against about 6 ms for three collections. PARKED "Pool scans" holds the trigger.
- **Results pile up while `tick` is stopped.** A modal dialog stops `tick`, and finished decodes wait whole, up to about 256 MB each. Image and PDF do this on `main` too, since their workers never wait for `tick`. Favicon and redirect now run every queued job rather than four, but their results are small.
- **An idle app now has no worker threads, and a burst pays for spawning them.** Up to `cap` spawns per burst, against a decode or a network fetch: not measurable next to the job.
- **A cancelled image decode still runs to its end and holds its slot.** Same as today's slot use, but a reload now has old and new decoding at once, on the two slots. Both only read the file.
- **A re-request goes to the back of the queue.** In a restore burst, a reload of the card in view waits behind the restored ones. Today it does too.
- **A re-request now kills the old render,** where before both ran. If that render was doing the first-run PDFium download, the new render starts the download again. Once per machine, and only on a reload during that first run.
- **`request`'s `&mut self` on `PdfRenderer`** needs the callers' borrows to be disjoint from `workers`; `fetched.rs:320` holds `pdf` from `self.nodes`, a different field.
