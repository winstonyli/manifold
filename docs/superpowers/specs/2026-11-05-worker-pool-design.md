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

The user chose the self-draining pool (Decide d1, 2026-11-05):

- `worker.rs` holds `Pool<K, J, R>`: threads spawned on demand up to a cap, draining a queue under one small `Mutex`, exiting when it is empty.
- `request` is a no-op while the key is in flight. `cancel` drops a queued job or discards a running job's result.
- Image and PDF wrappers call `cancel` then `request`, so the latest request wins with no policy flag.
- PDF keeps a plain UI-thread map of `Cancel` handles, with the `Inbox` made at request time and carried in the job.
- `catch_unwind` lives in the pool, a panic reaches the wrapper as `None` with its job handed back, and each wrapper words its own failure (Decide l1). Each pool has its own cap.

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

Home: `crates/manifold/src/worker.rs`, `mod worker;` in `main.rs`, `"worker"` in `CORE_MODULES` (`crates/gate/src/tier.rs`) beside `net`, `redirect_fetch` and `favicon_fetch`, so an edit to the pool gets the full gate. It stays out of `CORE_IMPORTS`: the pool touches no store, session or engine, so importing it doesn't raise a UI file's tier.

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
    /// The current job of every key: queued, running, or done and not yet taken by `tick`.
    live: HashMap<K, u64>,
    queue: VecDeque<Ticket<K, J>>,
    /// `None`: `work` panicked.
    done: Vec<(K, J, Option<R>)>,
    threads: usize,
    next_token: u64,
}

struct Ticket<K, J> {
    key: K,
    token: u64,
    job: J,
}

impl<K, J, R> Pool<K, J, R> where K: Eq + Hash + Clone + Send + 'static, J: Send + 'static, R: Send + 'static {
    pub(crate) fn new(cap: usize, work: impl Fn(&J) -> R + Send + Sync + 'static) -> Self;
    /// Queues `job` under `key`; `false` (and nothing done) while `key` is live.
    pub(crate) fn request(&self, key: K, job: J) -> bool;
    /// Forgets `key`'s job: a queued one never runs, a running or finished one's result is never returned.
    pub(crate) fn cancel(&self, key: &K);
    /// Every job finished since the last call, with its result (`None`: it panicked). Its keys stop being live.
    pub(crate) fn tick(&self) -> Vec<(K, J, Option<R>)>;
    /// Live keys. A cancelled job isn't one, so its thread can outlive this reaching 0.
    #[cfg(test)]
    pub(crate) fn pending(&self) -> usize;
}
```

Everything is `&self`. One mutex guards all of `State`, held only for map and queue operations, never across `work`.

- **`request`:** under the lock, return `false` if `key` is in `live`. Otherwise take a token, record it in `live`, push the ticket, and if `threads < cap` count one more thread. Spawn after releasing the lock.
- **Worker thread:** loop. Under the lock, pop a ticket; on an empty queue subtract one from `threads` and return (same lock, so a concurrent `request` either sees the old count and its ticket is taken by this thread, or sees the new count and spawns). Outside the lock, run `catch_unwind(AssertUnwindSafe(|| work(&job))).ok()`. Under the lock, push `(key, job, result)` to `done` only if `live[key]` is still this token; a stale job and result drop after the lock is released.
- **`cancel`:** remove `key` from `live`, from `queue`, and from `done`, collecting what comes out and dropping it after the lock is released, since a result can be a whole image. A running job is left to finish; its token no longer matches, so its result is dropped. The cap is on threads, not live keys: a new job for that key may run beside the old one, and the old one's thread counts against the cap until it ends.
- **`tick`:** take `done` and remove each key from `live`.
- **Lock poisoning:** recovered with `into_inner`, as `State::lock` does today. `work` never runs under the lock.

There is no `mpsc`: `done` under the same lock is what lets `cancel` reach a finished job the UI has not taken, which a channel could not. Nothing is lost by it: no result wakes the event loop today either (the four workers are polled from `drain_tab_events`), so the pool needs no notify hook.

The panic hook already prints a panic, so the pool logs nothing of its own; `ImageDecoder::tick` adds the path, as `guarded` does today. A job's `Drop` runs outside `catch_unwind`, so one that panicked would lose its thread's slot; no job here can panic there. `thread::spawn` stays (it panics on thread exhaustion on the UI thread, as `favicon_fetch` and `redirect_fetch` do today).

### Why a panic is `None` with the job handed back (Decide l1)

The pool catches a panic but can't word it: image's message names the file, redirect's carries the job. So `work` borrows `J` and `tick` returns the job beside `Option<R>`, and each wrapper turns `None` into its own failure where it already maps results. Nothing is built per request, and no job type needs `Clone`. Weighed and rejected: a prebuilt `failed` value per request (allocates on every request, and `FetchJob` needs `Clone`), a second `failed` closure at `new` (one more concept for what `tick` can do), and `run_child` borrowing its `Inbox` (loses the compile-time one-use guarantee, changes a public signature and five sites, and leaves a late frame in a kept `Inbox` until `tick`).

## The four wrappers

Each keeps its public type and its name in `Workers`, so `tick_fetchers` keeps its four loops.

| | `K` | `J` | `R` | on `None` | repeat request |
|---|---|---|---|---|---|
| `FaviconFetcher` | `String` origin | `String` origin | `Option<FaviconImage>` | `None` (`flatten`) | dropped |
| `RedirectFetcher` | `NodeId` (`job.node_id`) | `FetchJob` | `Option<WebBody>` | `FetchResult { job, body: None }` (`flatten`) | dropped |
| `ImageDecoder` | `NodeId` | `String` path | `Result<Decoded, String>` | `Err("the decoder crashed")`, named by `tick` | cancel, then run |
| `PdfRenderer` | `NodeId` | `(String, Cell<Option<Inbox>>)` | `Result<PdfRenderResult, String>` | `Err("the renderer crashed")` | cancel, then run |

`tick_fetchers` prefixes PDF errors with "Couldn't open PDF {name}: ", so the PDF failure reads "Couldn't open PDF d.pdf: the renderer crashed".

Redirect's key stays `job.node_id`, not the target: for `FetchTarget::Tab` that id is freshly allocated, so repeated events for one tab aren't deduped today, and keying on the tab would silently add that.

`tick` is `&self` on the first three, as `Pool::tick` is; only `PdfRenderer` needs `&mut self`.

**`FaviconFetcher`:** `work` closes over `cache_dir` and `fetch`. `tick` maps `(origin, _, result)` to `(origin, result.flatten())`. `in_flight` and `throttle` go. `request` becomes `&self`.

**`RedirectFetcher`:** `tick` maps `(_, job, body)` to `FetchResult { job, body: body.flatten() }`; `FetchResult` stays, as `tick_fetchers` reads it. `in_flight` and `throttle` go. `request` keeps its `bool`, which tests read, and becomes `&self`. Bodies don't pile up while `tick` is stopped: a finished job holds a path or a note's text, not the file.

**`ImageDecoder`:** `request` is `pool.cancel(&id); pool.request(id, path);`. Gains `cancel`. `tick` is the one place an error is named: `None` logs "image: decoder panicked on {path}" and becomes `Err("the decoder crashed")`, and every `Err` becomes "Couldn't open image {name}: {why}". `decode` keeps its `eprintln` and returns the bare reason, or the name would appear twice. `guarded`, `pending: Arc<AtomicUsize>` and the two workers' loop go; `pending` delegates to the pool.

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

An entry leaves `cancels` when its result is delivered or its job is cancelled, and every live job ends one of those two ways. A `Cancel` sent to a queued job's `Inbox` is harmless: `cancel` removes that job from the queue, so nothing reads it. `run_child` still returns `Err("cancelled")` for a killed child, but `pool.cancel` always runs first and `PdfRenderer` is the only user of `Cancel`, so that result is always a stale token. `State`, `Inner` and `is_cancelled` go; `#[cfg(test)] pending` delegates to the pool, as `ImageDecoder`'s does. `request`, `cancel` and `tick` become `&mut self`.

## One cancel for a node

Image decodes gain the cancel PDFs have, and every path that drops a node calls one helper rather than asking what kind it was:

```rust
impl Workers {
    /// Node `id` left the live graph: nothing queued or running for it may report.
    pub(crate) fn cancel_node(&mut self, id: NodeId) { .. }  // image_decoder, then pdf_renderer
}
```

- `unshow` (`node_store.rs`): replaces the `as_pdf` check.
- `teardown_running_session` (`session_files.rs`): replaces the PDF filter, over every id in `self.nodes`.
- `reload_fetched_node` (`fetched.rs`): at its `self.nodes.remove(&id)`, so a reload whose new content fails can't be overwritten by the old job's result on the placeholder.

Cancelling on the hide path is safe: every re-show runs `apply_visibility`, `materialize`, `insert_materialized`, then `insert_image_node_with_id` or its PDF twin, which requests a new job. `reopen_node` takes the same route, and the missing-file path inserts a placeholder with no job.

This closes the image half of defect 4, which the survey didn't list: a session replaced by one with the same ids could toast a stale decode error.

## Tests

The defect tests:

- `the_newest_request_for_a_node_decides_its_image` and `a_cancelled_render_says_nothing_to_a_card_that_took_its_id` are un-ignored and pass unchanged.
- `a_finished_render_leaves_a_later_render_of_its_id_cancellable` tests `State`, which goes. It is rewritten against `PdfRenderer`: request, request again, let the first stub finish, and assert `cancels` still holds the id.

New in `worker.rs`, with gated stubs and no fixed sleeps:

- at most `cap` jobs run at once, and queued jobs start without a `tick` (replaces `a_throttle_runs_at_most_its_cap_and_hands_on_in_order` and `at_most_four_favicon_fetches_run_at_once`);
- a live key's repeat is dropped, and the key can be requested again after `tick` returns it;
- `cancel` of a queued job never runs it; of a running job, drops its result; of a finished job not yet taken, drops its result (defect 5 is the first of these, followed by a `request`);
- a panic delivers `None` with its job, and the pool keeps working (defect 1; replaces `a_panicking_fetch_gives_its_slot_back`);
- threads exit when the queue empties, and a later `request` still runs.

Each wrapper keeps one test of its own `None` mapping, since the pool can't see its shape: `a_panicking_decode_is_an_error_for_that_image_only` (rewritten through `with_decode`, as `guarded` goes), a new PDF one reading "the renderer crashed" (through `with_render`), and `a_panicking_fetch_still_frees_its_node` (through `with_fetch`, checking the `FetchResult` carries its job).

Deleted with `State`, their cases now in the `cancel` tests above: `a_cancel_before_the_render_starts_skips_it`, `a_cancel_while_running_kills_instead_of_marking_and_a_finish_clears_it`, `a_new_request_clears_a_stale_cancel`.

Rewritten:

- `a_decode_comes_back_named_and_a_bad_file_comes_back_as_an_error`: its `starts_with` on the bad file becomes an `assert_eq` of the whole message, so a doubled name fails.
- `teardown_cancels_pending_pdf_renders`, renamed for what it now covers: the mark is gone, so it swaps in a gated `with_render` stub (a plain `request` on `App::default()` would launch the real PDF host) and a gated `with_decode` stub (made `pub(crate)`), requests one of each, tears down, and asserts both `pending`s are 0. Without the cancel, even a finished job stays live until `tick`, so the test can fail.
- The `favicon_fetch` and `redirect_fetch` tests that assign `fetcher.fetch` go through a `with_fetch` seam, the shape `with_decode` already has: `new` calls it, and it is private to the module. Every `let mut fetcher` in those two files loses its `mut`, since `request` and `tick` are `&self` and `-D warnings` rejects the leftover.

Stubs in `worker.rs` capture a `Receiver` as `Arc<Mutex<Receiver>>`, as the defect tests do: `work` must be `Sync`.

`finish_image_decodes` and `drain_fetches` keep working through `pending` and `tick`.

## Guard

`crates/gate/src/limits.rs` gains a check that `catch_unwind` appears in no production file of `crates/manifold/src` but `worker.rs`. `sources()` walks every crate and keeps inline test blocks, so the check filters on `manifold/src/` (the slash keeps `manifold-view-*` out), takes the first `measure(..).0` lines of each file, skips comment lines, and matches `catch_unwind(`. It fails on the likeliest form of the drift, a new worker with its own panic guard. No check can spot a hand-written queue in general; that half rests on the module doc of `worker.rs` naming itself as the place for keyed background work.

## End-state fit

| Change | Target | Replaces | Deletes |
|---|---|---|---|
| Queue, cap, dedupe, cancel, panic, delivery | `worker.rs` `Pool` | four hand-rolled loops | `net::Throttle` and its test, both `in_flight` sets, both `Mutex<Receiver>` queues, PDF `State`/`Inner`, `guarded`, `ImageDecoder`'s atomic, three `catch_unwind`s |
| Cancel on node removal | `Workers::cancel_node` | the two `as_pdf` checks | `PdfRenderer::is_cancelled` |
| Panic guard placement | `limits.rs` check | nothing | nothing |

Docs and leftovers:

- PARKED "Worker pool defect tests" goes, and ARC2 leaves "Shared-owner refactors".
- PARKED GATE-FETCHERS goes: it decided against unifying these until a fifth fetcher, and `image_decode` is that fifth. Its fourth, `suggestions`, stays out (a non-goal here), which the entry's replacement line says.
- README's module list names `worker`.
- Every doc comment naming `Throttle`, a shared queue, fixed workers or `tick` starting jobs is rewritten to point at `worker.rs`: the module docs of `net.rs`, `favicon_fetch.rs`, `redirect_fetch.rs`, `image_decode.rs`, `pdf/render.rs` and `event_loop/fetchers.rs`, and the item comments on `unshow` and `teardown_running_session`. A grep for `throttle` returns only history.
- Imports only the deleted code used go (`VecDeque` in `net.rs`, `HashSet` in `pdf/render.rs`, `mpsc` in the wrappers).

## Risks

- **An idle app now has no worker threads, and a burst pays for spawning them.** Up to `cap` spawns per burst, against a decode or a network fetch: not measurable next to the job.
- **A cancelled image decode still runs to its end and holds its slot.** Same as today's slot use, but a reload now has old and new decoding at once, on the two slots. Both only read the file.
- **A re-request goes to the back of the queue.** In a restore burst, a reload of the card in view waits behind the restored ones. Today it does too.
- **A re-request now kills the old render,** where before both ran. If that render was doing the first-run PDFium download, the new render starts the download again. Once per machine, and only on a reload during that first run.
- **`request`'s `&mut self` on `PdfRenderer`** needs the callers' borrows to be disjoint from `workers`; `fetched.rs:320` holds `pdf` from `self.nodes`, a different field.
