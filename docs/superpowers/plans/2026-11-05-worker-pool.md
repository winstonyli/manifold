---
status: draft
---

# One background-worker pool (ARC2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task, inline: the tasks are sequential and each consumes the last. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the four hand-rolled worker queues with one `worker::Pool`, closing the five defects the second ARC2 survey found.

**Architecture:** `Pool<K, J, R>` holds one list of entries, each with a result cell its thread fills. Threads spawn on demand up to a cap and exit when no job is queued. The four typed wrappers stay, each taking its work function in `new` and returning tuples from `tick`.

**Tech Stack:** Rust (`nightly-2026-09-23`), `std` only.

**Spec:** `docs/superpowers/specs/2026-11-05-worker-pool-design.md`. It is the authority; where this plan and the spec differ, the spec wins and the difference is a ruling.

**Prototype:** branch `proto-pool-final` holds the whole change in one commit range, and `model-harness.rs` at its root. Read it with `git show proto-pool-final:<path>`. Start each file from the prototype's version, then apply "Differences from the prototype" below. Never merge the branch.

## Global Constraints

- Work on `main`, one commit or more per task, nothing pushed, no version bump, no tag.
- `cargo fmt --all` before each commit; `cargo lint -j 8 -- -D warnings` must pass (the pre-commit hook checks both; never `--no-verify`).
- No fixed sleeps and no timed absence waits in tests. A cap is read from `threads`; silence is `wait_for_threads`, then `tick`.
- Caps and messages are the spec's (its Goals and wrapper paragraphs). Two new names, as in the prototype: `MAX_DECODES` in `image_decode.rs` and `MAX_RENDERS` in `pdf/render.rs`, the second replacing `WORKERS`.
- Any test or build run over about a minute goes through the wrapper (memory cap):
  `powershell -NoProfile -File "$USERPROFILE/.claude/runs/run.ps1" -Kind probe -Name <n> -Command "<cargo command>"`, from bash. The result is in `%LOCALAPPDATA%\claude-runs\manifold\<date>-<n>\`.
- Python, if any, only as a file: `python -P f.py < /dev/null`.

## Differences from the prototype

Every task applies those that touch its files:

- `Pool` gains `#[cfg(test)] wait_for_threads`; `ImageDecoder` and `PdfRenderer` pass it through under `cfg(test)`.
- Pool tests live in `worker/tests.rs` (`#[cfg(test)] mod tests;`), not inline.
- The prototype's timed absence waits go: `recv_timeout(100 ms)` expected to fail, and `thread::sleep`. Each becomes the rule in Global Constraints. `recv_timeout(PATIENCE)` for a signal that must arrive stays.
- New tests the prototype lacks: the model test, the order test, the Review Focus tests, `hiding_a_node_cancels_its_decode` and `reloading_a_node_cancels_its_decode`.
- The result alias and the `entries` comment follow the spec's text, not the prototype's.

## Review Focus

The five failure modes the spec implies and its test list doesn't exercise, each pinned by a test below:

1. A restore burst far past the cap: every job is delivered, none stranded (Task 1, `a_burst_past_the_cap_all_comes_back`).
2. A cancelled running job that panics: its thread lives on and takes the next job (Task 1, `a_cancelled_job_that_panics_keeps_its_thread`).
3. A repeat request after a decode finished but before `tick`: only the new result reaches the card (Task 1, `a_repeat_request_replaces_a_finished_decode`).
4. A modal dialog stopping `tick`: favicon jobs past the cap still run (Task 3, `favicons_past_the_cap_finish_without_a_tick`).
5. A pool dropped with jobs running, as tests and app exit do: the threads end without a panic (Task 1, `a_dropped_pool_lets_its_threads_finish`).

## End-state fit

| Change | Target | Replaces | Deletes | Task |
|---|---|---|---|---|
| Queue, cap, dedupe, cancel, panic, delivery | `crates/manifold/src/worker.rs` `Pool`, tests in `worker/tests.rs` | four hand-rolled loops | `ImageDecoder`'s atomic and `guarded`; both `Mutex<Receiver>` queues; PDF `State`/`Inner`; both `in_flight` sets; `net::Throttle` and its test; three `catch_unwind`s | 1-3 |
| Seams and result types | `new` parameters, tuples from `tick` | `with_decode`, `with_render`, the `fetch` fields | `FetchResult`, `Finished`, the `new`/`with_*` pairs | 1-3 |
| Cancel on node removal | `Workers::cancel_node` in `event_loop/fetchers.rs`, called from `unshow`, `teardown_running_session`, `reload_fetched_node` | the two `as_pdf` checks | `PdfRenderer::is_cancelled` | 2 |
| Panic guard placement | `crates/gate/src/limits.rs` check | nothing | nothing | 4 |
| Full-gate coverage | `"worker"` in `CORE_MODULES` (`crates/gate/src/tier.rs`) | nothing | nothing | 1 |
| Docs | module docs (per task), README, PARKED | GATE-FETCHERS text, "Worker pool defect tests", ARC2 in "Shared-owner refactors" | stale `Throttle` wording | 1-4 |

---

### Task 1: The pool, and `ImageDecoder` on it

Image comes first because its `request` alone calls both `Pool::cancel` and `Pool::request`, so nothing in `Pool` is dead code under `-D warnings`. `ImageDecoder::cancel` would be, so it waits for its caller in Task 2.

**Files:**
- Create: `crates/manifold/src/worker.rs`, `crates/manifold/src/worker/tests.rs`
- Modify: `crates/manifold/src/main.rs` (`mod worker;`), `crates/gate/src/tier.rs` (`"worker"` in `CORE_MODULES`), `crates/manifold/src/image_decode.rs`, `crates/manifold/src/event_loop/fetchers.rs` (`ImageDecoder::new(crate::image_decode::decode)`), spec front matter (`status: active`), this plan (`status: active`)

**Interfaces:**
- Produces, `worker.rs`, signatures exactly as the spec's "The pool": `Pool::new(cap, work)`, `request(&self, key, job) -> bool`, `cancel(&self, &key)`, `tick(&self) -> Vec<(K, J, Option<R>)>`, `#[cfg(test)] pending(&self) -> usize`, `#[cfg(test)] wait_for_threads(&self, n: usize)`.
- Produces, `image_decode.rs`: `ImageDecoder::new(decode: impl Fn(&str) -> Result<Decoded, String> + Send + Sync + 'static)`, `request(&self, id, path)`, `tick(&self) -> Vec<(NodeId, Result<Decoded, String>)>`, `pub(crate) fn decode`, and under `cfg(test)` `pending`, `wait_for_threads`. Both are test-only, as `finish_image_decodes`, `pending`'s one non-test-module user, is.

- [ ] **Step 1: Set `status: active` on the spec and this plan, then write `worker/tests.rs`**

Take the prototype's tests from `git show proto-pool-final:crates/manifold/src/worker.rs` with their helpers (`PATIENCE`, `collect`, `Gate`, `gated`), then:

- `at_most_cap_jobs_run_at_once_and_queued_jobs_start_without_a_tick`: after the requests and both `started` signals, assert `pool.shared.state.lock().unwrap().threads == 2` and the third entry's `queued.is_some()`; then the prototype's `release.send(())` and `started.recv_timeout(PATIENCE)` stay, with no `tick` before them, and after `drop(gate.release)` assert `peak == 2` and all five come back.
- `cap_holds_when_a_running_job_is_cancelled`: cap 1; wait for `started`; `cancel`; assert `threads == 1`; `request` a second job; assert `threads == 1` and its `queued.is_some()`; release; the second job comes back.
- `cancel_of_a_running_job_drops_its_result`: after `drop(gate.release)`, `wait_for_threads(0)`, then one `tick` returns exactly the new job. The sleep goes.
- `cancel_of_a_finished_job_not_yet_taken_drops_its_result`: `wait_for_threads(0)` replaces its polling loop.
- `threads_exit_when_the_queue_empties_and_a_later_request_still_runs`: `wait_for_threads(0)` replaces any wait.

New:

```rust
#[test]
fn jobs_start_and_return_in_request_order() {
    let (pool, gate, _) = gated(1);
    for n in 0..4 { pool.request(n, n); }
    assert_eq!(gate.started.recv_timeout(PATIENCE).unwrap(), 0);
    pool.cancel(&1); // Two entries follow it, so a `swap_remove` reorders them.
    drop(gate.release);
    assert_eq!(gate.started.recv_timeout(PATIENCE).unwrap(), 2);
    assert_eq!(gate.started.recv_timeout(PATIENCE).unwrap(), 3);
    pool.wait_for_threads(0);
    assert_eq!(pool.tick().into_iter().map(|(k, ..)| k).collect::<Vec<_>>(), [0, 2, 3]);
}

#[test]
fn a_burst_past_the_cap_all_comes_back() {
    let pool = Pool::new(2, |job: &u32| *job);
    for n in 0..500 { assert!(pool.request(n, n)); }
    pool.wait_for_threads(0);
    assert_eq!(pool.tick().len(), 500);
}

#[test]
fn a_cancelled_job_that_panics_keeps_its_thread() { /* cap 1, job 1 gated then panics, job 2 queued */
    // wait for job 1's start; cancel(&1); release; job 2 comes back with `Some`; `threads` reaches 0.
}

#[test]
fn a_dropped_pool_lets_its_threads_finish() {
    // A gated job signals `done` after its release. `let watch = Pool { shared: Arc::clone(&pool.shared) };`
    // Wait for `started`, drop the pool, release: `done.recv_timeout(PATIENCE)` is `Ok`, then
    // `watch.wait_for_threads(0)`, which a thread that died before leaving would fail.
}
```

The model test, `random_runs_match_a_plain_model`, comes from `git show proto-pool-final:model-harness.rs`. Its body is `for seed in 0..SEEDS { run_seed(seed).unwrap_or_else(|why| panic!("seed {seed}: {why}")); }`, with no panic hook. Edits to the harness:

- Keep `Rng`, `Gate`, `HANG`, `Job`, `Res`, `check_tick`, `wait_until`, `run_seed` and `is_race`, which becomes `seed % 4 == 3`.
- Drop `mod pool`, `#![allow]`, `LOCK_DEPTH`, `under_lock`, `DROP_UNDER_LOCK` with its `drops0` check, both `Drop` impls, `RACE_ONLY` and `main`.
- `Stats` goes, and `st` leaves `run_seed` and its `request` closure. `run_seed(seed: u64) -> Result<(), String>` owns a local `delivered: u64`, `check_tick` takes it as `&mut u64`, and the accounting check reads it (`delivered0` goes).
- `pool.pending_all()` becomes `pool.pending()`; `pool.threads()` becomes a read of `pool.shared.state`.

The exit race, `request_racing_a_thread_exit_is_never_stranded`, comes from the prototype unchanged (cap 1, 20,000 iterations, 2 s deadline per job).

- [ ] **Step 2: Add `mod worker;` and a `worker.rs` holding only `#[cfg(test)] mod tests;`, run the tests, see them fail**

Run: `cargo test -j 8 -p manifold --bins worker::`
Expected: compile error, `Pool` not found.

- [ ] **Step 3: Implement `worker.rs`**

Start from the prototype's production code, with the differences above. `wait_for_threads` polls with `yield_now`, as the spec's doc comment says. The module doc names this file as the place for keyed background work.

- [ ] **Step 4: Run the pool tests**

Run: `cargo test -j 8 -p manifold --bins worker::`
Expected: all pass. Then run `cargo test -j 8 -p manifold --bins random_runs_match_a_plain_model` alone, read `finished in` from its last line, and set the seed count so it takes about one second (start at 3,000; the survey's 0.3 ms a seed was an optimized build). Below 500 seeds a second is a ruling to record, with the seed count kept at 500.

- [ ] **Step 5: Un-ignore and rewrite the image tests in `image_decode.rs`**

- `the_newest_request_for_a_node_decides_its_image`: remove `#[ignore]`, `with_decode` becomes `new`, and the one-second wait becomes `decoder.wait_for_threads(0)` and a `tick`.
- `a_panicking_decode_is_an_error_for_that_image_only`: through `new`.
- `a_decode_comes_back_named_and_a_bad_file_comes_back_as_an_error`: `ImageDecoder::new(decode)`; the bad-file check is `assert_eq!(error, format!("Couldn't open image bad.png: {}", image::load_from_memory(b"not a picture").unwrap_err()))`.

New:

```rust
#[test]
fn a_repeat_request_replaces_a_finished_decode() {
    // Stub returns a size from the path. request(id, "old"), wait for it to finish (`wait_for_threads(0)`),
    // request(id, "new"), wait_for_threads(0): tick returns one result, the "new" one.
}
```

- [ ] **Step 6: Run them, see them fail**

Run: `cargo test -j 8 -p manifold --bins image_decode::`
Expected: compile errors on `new` taking an argument.

- [ ] **Step 7: Move `ImageDecoder` onto the pool**

Follow the spec's "`ImageDecoder`" paragraph and the prototype's file, leaving out `ImageDecoder::cancel` and `a_cancelled_decode_reports_nothing`, which land in Task 2. `decode` returns the bare reason. The module doc stops saying "Two workers share one queue" and points at `worker.rs`. `Finished`, `guarded`, the atomic and the worker loop go.

- [ ] **Step 8: Run the checks**

Run: `cargo test -j 8 -p manifold --bins -- image_decode:: worker:: fetched::`, then `cargo lint -j 8 -- -D warnings`.
Expected: all pass, no warnings.

- [ ] **Step 9: Commit**

```bash
git add crates docs/superpowers/specs/2026-11-05-worker-pool-design.md docs/superpowers/plans/2026-11-05-worker-pool.md
git commit -m "ARC2: worker::Pool, with ImageDecoder its first user"
```

### Task 2: `PdfRenderer` on the pool, and one cancel for a node

Together because `is_cancelled` goes with `State`, and the teardown test that reads it needs `cancel_node` to replace it.

**Files:**
- Modify: `crates/manifold/src/pdf/render.rs`, `crates/manifold/src/image_decode.rs`, `crates/manifold/src/event_loop/fetchers.rs`, `crates/manifold/src/node_store.rs`, `crates/manifold/src/fetched.rs`, `crates/manifold/src/session_files.rs`, `crates/manifold/src/session_sync/tests/files.rs`

**Interfaces:**
- Consumes: Task 1's `Pool`, and `ImageDecoder`'s `pending` and `wait_for_threads`.
- Produces: `PdfRenderer::new(render: impl Fn(&str, Inbox) -> Result<PdfRenderResult, String> + Send + Sync + 'static)`, `request(&mut self, id, path)`, `cancel(&mut self, id)`, `tick(&mut self) -> Vec<(NodeId, Result<PdfRenderResult, String>)>`, `pub(crate) fn render`, `ImageDecoder::cancel(&self, id)`, `Workers::cancel_node(&mut self, id: NodeId)`.

- [ ] **Step 1: Write the tests**

In `pdf/render.rs`:

- `a_finished_render_leaves_a_later_render_of_its_id_cancellable`: remove `#[ignore]`; stubs gated per path, each signalling its start. `request(id, "a")`, wait for a's start, `request(id, "b")`, wait for b's start, release a, `wait_for_threads(1)`, then:

```rust
assert!(renderer.tick().is_empty());
assert!(renderer.cancels.contains_key(&id));
renderer.cancel(id);
assert!(renderer.cancels.is_empty());
assert_eq!(renderer.pending(), 0);
```

- `a_cancelled_queued_render_is_not_queued_twice`: ids 1 and 2 hold both slots, both started. `request(3, "a")`, `cancel(3)`, `request(3, "b")`, release all, `wait_for_threads(0)`. The stub's log of paths holds "b" once and no "a".
- `a_panicking_render_is_an_error_for_that_node`: the `tick` result reads "the renderer crashed", and `renderer.cancels.is_empty()`.

In `image_decode.rs`, `a_cancelled_decode_reports_nothing`: gated stub, wait for its start, `cancel`, release, `wait_for_threads(0)`, `assert!(decoder.tick().is_empty())`.

Delete `a_cancel_before_the_render_starts_skips_it`, `a_cancel_while_running_kills_instead_of_marking_and_a_finish_clears_it` and `a_new_request_clears_a_stale_cancel`.

In `session_sync/tests/files.rs`:

- `a_cancelled_render_says_nothing_to_a_card_that_took_its_id`: remove `#[ignore]`, `with_render` becomes `new`, the one-second wait becomes `app.workers.pdf_renderer.wait_for_threads(0)` and a `tick`.
- `teardown_cancels_pending_pdf_renders` becomes `teardown_cancels_image_decodes_and_pdf_renders`: swap in gated stubs for both, `add` an image node and a PDF node as the old test does, request a job for each id, tear down, assert both `pending()` are 0.
- New `hiding_a_node_cancels_its_decode`: gated image stub on `App::default()`, insert an image node, assert `app.workers.image_decoder.pending() == 1`, `app.unshow(id)`, assert it is 0.
- New `reloading_a_node_cancels_its_decode`: gated image stub, insert an image node through `insert_content` with `ContentSource::Web` so it has a source url, then `reload_fetched_node` with `WebBody::File { kind: ContentKind::Json, path: <missing> }`. Assert `pending()` is 1 before the reload and 0 after.

- [ ] **Step 2: Run them, see them fail**

Run: `cargo test -j 8 -p manifold --bins -- image_decode:: pdf::render:: session_sync::`
Expected: compile errors on `new` taking an argument.

- [ ] **Step 3: Move `PdfRenderer` onto the pool**

Follow the spec's "`PdfRenderer`" paragraph, whose `request`, `cancel` and `tick` are exact. Work: `render(path, inbox.take().expect("a pool runs a job once"))`. `Workers::new` passes `crate::pdf::render::render`. Update the module doc.

- [ ] **Step 4: Add `Workers::cancel_node` and call it**

`ImageDecoder::cancel` is `self.pool.cancel(&id)`. `cancel_node` calls it, then the PDF renderer's. Call `cancel_node` from `unshow` (replacing the `as_pdf` check, unconditionally; rewrite its doc comment), from `teardown_running_session` (over every id in `self.nodes`, replacing the PDF filter; rewrite its doc comment) and from `reload_fetched_node` next to `self.nodes.remove(&id)`.

- [ ] **Step 5: Run the checks**

Run: `cargo test -j 8 -p manifold --bins -- image_decode:: pdf::render:: session_sync::`, then `cargo lint -j 8 -- -D warnings`.
Expected: all pass. For each new `cancel_node` test, remove the call it covers, see it fail, and restore it.

- [ ] **Step 6: Commit**

```bash
git add crates
git commit -m "ARC2: PdfRenderer on the pool; one cancel for a node"
```

### Task 3: Favicon and redirect on the pool

**Files:**
- Modify: `crates/manifold/src/favicon_fetch.rs`, `crates/manifold/src/redirect_fetch.rs`, `crates/manifold/src/net.rs`, `crates/manifold/src/event_loop/fetchers.rs`, `crates/manifold/src/main.rs` (`FetchResult` import), `crates/manifold/src/session_sync/tests.rs`, `crates/manifold/src/session_sync/tests/misc.rs`, `crates/manifold/src/session_sync/tests/remote.rs` (`results[0].job` and `.body`)

**Interfaces:**
- Consumes: Task 1's `Pool`.
- Produces: `FaviconFetcher::new(cache_dir: Option<PathBuf>, fetch: fn(&str) -> Option<FaviconImage>)`, `request(&self, origin)`, `tick(&self) -> Vec<(String, Option<FaviconImage>)>`; `RedirectFetcher::new(dir: Option<PathBuf>, fetch: fn(&Path, NodeId, &str, ContentKind) -> Option<WebBody>)`, `request(&self, job) -> bool`, `tick(&self) -> Vec<(FetchJob, Option<WebBody>)>`; `pub(crate) fn fetch_favicon`.

- [ ] **Step 1: Write the tests**

In `favicon_fetch.rs` and `redirect_fetch.rs`, every test passes its stub to `new`, every `let mut fetcher` loses `mut`, and tests read tuples. Delete `at_most_four_favicon_fetches_run_at_once`, `a_panicking_fetch_gives_its_slot_back` and `at_most_four_redirect_fetches_run_at_once`, whose cases the pool tests hold, and the helpers only the last used: `gated_fetch`, `RUNNING`, `PEAK`, `RELEASE`.

New, in `favicon_fetch.rs`:

```rust
#[test]
fn a_panicking_fetch_is_no_favicon() { /* stub panics */
    // Collect one result, then `assert!(matches!(done.as_slice(), [(origin, None)] if origin == "https://a.test"))`.
}

#[test]
fn favicons_past_the_cap_finish_without_a_tick() {
    let fetcher = FaviconFetcher::new(None, |_| None);
    for n in 0..MAX_FAVICON_FETCHES + 3 { fetcher.request(format!("https://{n}.test")); }
    fetcher.pool.wait_for_threads(0);
    assert_eq!(fetcher.tick().len(), MAX_FAVICON_FETCHES + 3);
}
```

`a_panicking_fetch_still_frees_its_node` in `redirect_fetch.rs` checks that the returned tuple's job has the requested `node_id` and a `None` body.

- [ ] **Step 2: Run them, see them fail**

Run: `cargo test -j 8 -p manifold --bins -- favicon_fetch:: redirect_fetch::`
Expected: compile errors on `new` taking a second argument.

- [ ] **Step 3: Move both onto the pool, and delete `Throttle`**

Follow the spec's two paragraphs and the prototype's files. `FaviconFetcher::new` keeps spawning `prune_cache`. `Workers::new` passes `crate::favicon_fetch::fetch_favicon` and `crate::redirect_fetch::fetch`. `tick_fetchers` reads `for (job, body) in ...`. `FetchResult` goes from `redirect_fetch.rs` and `main.rs`; the import in `session_sync/tests.rs` becomes `{FetchJob, FetchTarget}`, and `drain_fetches` returns `Vec<(FetchJob, Option<crate::WebBody>)>`. `net::Throttle`, its test and `VecDeque` go. Module docs in all three files point at `worker.rs`.

- [ ] **Step 4: Run the checks**

Run: `cargo test -j 8 -p manifold --bins -- favicon_fetch:: redirect_fetch:: net:: session_sync::`, then `cargo lint -j 8 -- -D warnings`.
Expected: all pass, no warnings.

- [ ] **Step 5: Commit**

```bash
git add crates
git commit -m "ARC2: favicon and redirect on the pool; Throttle goes"
```

### Task 4: Guard, docs and the closing checks

**Files:**
- Modify: `crates/gate/src/limits.rs`, `README.md`, `PARKED.md`, `docs/superpowers/specs/2026-11-05-worker-pool-design.md` and this plan (front matter)

**Interfaces:**
- Consumes: Tasks 1-3, with `worker.rs` the only production `catch_unwind` in `manifold/src/`.

- [ ] **Step 1: Add the guard**

`catch_unwind_lives_only_in_the_worker_pool` in `limits.rs`, from the prototype's file: filters on `manifold/src/`, reads the first `measure(&source).0` lines, skips comment lines, and asserts `users == ["manifold/src/worker.rs"]`.

- [ ] **Step 2: See it fail, then pass**

Add `std::panic::catch_unwind(|| ());` to a throwaway line above `net.rs`'s test module, since the guard reads production lines only.
Run: `cargo test -j 8 -p gate limits::`
Expected: FAIL naming `net.rs`. Remove the line, run again, expected PASS.

- [ ] **Step 3: Update the docs**

- `README.md` module list: "and the fetchers" becomes "`worker` and the fetchers".
- `PARKED.md`: delete "Worker pool defect tests"; take ARC2 and the pool out of "Shared-owner refactors"; replace GATE-FETCHERS with the spec's text; in "Pool scans" drop ", applies once the pool lands".
- Sweep: `git grep -n "Throttle\|throttle:\|with_decode\|with_render\|FetchResult\|\.is_cancelled(\|fn is_cancelled" -- crates/manifold/src` returns nothing; the same grep over `PARKED.md`, `README.md` and `CLAUDE.md` returns nothing; dated plans, specs and `docs/devlog.md` stay as history.

- [ ] **Step 4: Full checks**

Run through the wrapper: `cargo unit -j 8`.
Expected: 0 failed, and ignored down by the 3 defect tests from 20 to 17. Then `Get-Process python` shows none of ours.

- [ ] **Step 5: Commit**

```bash
git add crates README.md PARKED.md
git commit -m "ARC2: guard the panic catch, update docs"
```

- [ ] **Step 6: Gate and review**

Tier `full`, since `worker` is a core module. Build and launch per CLAUDE.md "Gate", detached through `run.ps1 -Kind gate`, arm a watcher, and report CPU load and Defender state. Then one final whole-branch review (Opus) from the commit before Task 1, with this plan's Review Focus. Fixes go in their own commits; a fix to production code reruns the gate.

- [ ] **Step 7: Close the plan and clean up**

Only after the gate and review pass: set `status: done` on the spec and this plan and commit, then remove the scratchpad worktree with `git worktree remove`, then `git branch -D proto-pool-base proto-pool-m1 proto-pool-m2 proto-pool-n1o1 proto-pool-final proto-pool-final-v1`.
