# Wave 3a: engine hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close four verified engine robustness and security gaps, each behind a test that fails on today's code:
- E11: the engine installer writes every byte a server sends; it stops at the pinned size instead
- E12: every redirect fetch starts its own thread, each holding up to 100 MB; at most four run at once instead
- E7: a local engine's two named pipes accept any process that connects first; only the spawned child is accepted, and only the owner may open the pipes
- E6: a page-controlled URL is spliced into a shell string that passes through two shells in WSL; only hex reaches the command line instead

**Architecture:** Four small, independent mechanisms, no format or protocol change:
- E11: one `have + chunk > total` check inside the existing download loop (the cap is the pin's size, which the loop already has).
- E12: one `net::Throttle<J>` (a cap on running jobs plus an ordered wait queue), used by `RedirectFetcher` and, replacing its private copy of the same logic, by `FaviconFetcher`.
- E7: `accept_one` compares the connecting process id with the spawned child's (`GetNamedPipeClientProcessId` through interprocess's `peer_creds`), and `listen_local_pair` gives each pipe an owner-only DACL.
- E6: page-controlled text crosses the shell boundary only as hex (`transport::to_hex` / `from_hex`, generalizing the token's hex helpers), so no quoting rule of either shell matters.

**Tech Stack:** Rust (edition 2021, nightly-2026-09-23), interprocess 2.4.4, ureq 3, widestring 1.2 and windows-sys 0.61 (both already in `Cargo.lock`; new direct dependencies of `ipc-protocol` on Windows only).

**Source:** `.superpowers/survey/engines.md` sections E6, E7, E11, E12. There is no separate design spec: each item's decisions are in "Decisions resolved by reading the code" below. Every claim below was re-read against the code at `864f27e`; line numbers are quoted for orientation only, so find code by the quoted text. Paths are under `crates/` unless they start with `docs/` or `README.md`. Waves 0 to 2 land first and edit some of the same files (`process_manager.rs`, `net.rs`), so expect drift.

## Global Constraints

**Branch and commits**
- Work on branch `wave-3a-engine-hardening`, created from `main` after waves 0 to 2 have merged (`git switch -c wave-3a-engine-hardening main`), with no worktree.
- Commit once per task, with the exact message given in its last step.
- Merge into `main` only after the user's go-ahead (Task 7).
- Each implementer's commit message ends with that implementer's own `Co-Authored-By:` line, taken from its system reminder. The controller's is `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`.

**Formatting and lints**
- Run `cargo fmt --all` before each commit.
- `cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 --all-targets -- -D warnings` must pass.
- `.githooks/pre-commit` runs both and takes 2+ minutes, so commit in the background. Never use `--no-verify`.
- Don't silence a lint: fix it, or `#[allow(..., reason = "...")]` it (`allow_attributes_without_reason`).
- An item used only by a later task, or only by tests, is a dead-code warning under `-D warnings`. Each task adds only what it uses; a test-only accessor is `#[cfg(test)]`.
- CI runs `cargo test --locked` on Windows and on Linux (`-p manifold -p ipc-protocol`). Any new dependency must be in `Cargo.lock` in the same commit, and Windows-only code must be `cfg(windows)` so Linux still builds.

**Builds and the machine**
- Use `-j 8` on every cargo command. If cargo waits on the target-dir lock, wait; never kill another session's process.
- C: is nearly full: `df -h /c` before the first build.
- Before the gate and after the wave, check for leftovers with `Get-Process python, manifold, wsl -ErrorAction SilentlyContinue`.
- Tests that open named pipes use names containing `std::process::id()` so parallel test binaries never collide.

**Tests**
- No test may read or write the real `%APPDATA%\manifold`. These tests need none of it; any file goes through `crate::store::testutil::temp_dir(name)`.
- A test marked "red first" is run and seen to fail on the unmodified code, with the failure output pasted in the report, before the fix is written. A test that is expected to fail but passes is a finding: stop and report.
- Waiting in a test is on an observable condition with a deadline. The only fixed waits allowed are the ones that prove an absence (E12's "no fifth job starts"), and each says so.

**Platforms and what can be verified where**
- Windows (this machine): `ipc-protocol`, `manifold`, `manifold-engine-webview2` build, test and clippy here. All of E11, E12, E7 and the pure parts of E6 are tested here.
- Linux CI (`ci.yml`, `cargo test -p manifold -p ipc-protocol`): re-runs every non-`cfg(windows)` test. E7's PID tests and E6's hex tests are written to pass there too.
- `manifold-engine-webkit` (WSL only), CEF and Servo do not build on Windows and are not in CI. Only E6 touches an engine crate (`manifold-engine-webkit/src/main.rs`, a four-line change). The plan handles it by: putting all logic in `ipc-protocol` (tested on both OSes), keeping the engine edit to one call of that logic, and verifying the edit with `wsl.exe -d Ubuntu -- bash -c 'cd "/mnt/c/Users/Winston Li/Documents/GitHub/manifold" && CARGO_TARGET_DIR=target-linux cargo check -j 8 -p manifold-engine-webkit'` when WSL is available, otherwise by a second reader (the task says which). Task 6's report must say which of the two was done. CEF and Servo are untouched.

**Python**
- Never run `python` in any way that can read stdin: no `python -`, no heredoc python, no python call tacked on as a no-op. Write the script to a file and run `python -P f.py < /dev/null`. Prefer the Edit tool. None of these tasks needs python. Every implementer prompt repeats this rule. Subagents never dispatch subagents.

**Rust**
- Edition 2021, so no let-chains. Write `x.is_multiple_of(n)`, never `x % n == 0`, on unsigned integers (`clippy::manual_is_multiple_of`). In a function returning `Option`, use `?`, not `let Some(x) = .. else { return None }`.

**Gate**
- Engine changes need `scripts/gate.sh LOG full` after `cargo build -j 8 -p manifold`, detached at BelowNormal (CLAUDE.md "Gate"), once, in Task 7. Report CPU load and Defender state with the result.

## Decisions resolved by reading the code

1. **E11 is real, and smaller than the survey says.** `fetch` (`engine_install.rs`, the loop after `let (tx, rx) = std::sync::mpsc::sync_channel`) writes each chunk, adds its length to `have`, and compares `have != total` only after the loop. `total` is the pin's `size` (the only caller, `install`, passes `pin.size`), so no "no Content-Length" special case is needed: the cap is `total` whatever the headers say, and a chunked or header-less response is capped by the same line. Whatever the headers declare, the loop is the only place that sees the bytes arrive, so that is where the cap goes. Dropped from the survey: "check free space before starting". It needs a platform call (`GetDiskFreeSpaceExW`), and the cap already bounds disk use to the size the install needs anyway. Parked, not planned.
2. **On overflow the `.part` is deleted, not kept.** Every other `Network` failure keeps the `.part` so a retry resumes. A stream that exceeds the pinned size is wrong, not slow: a resume would send `Range: bytes=have-` to the same server. Also, `download_with_idle` treats a `.part` of exactly `total` bytes as complete, and `install` deletes it only on `BadHash`; the check runs before the write (so the file never holds more than `total` bytes) and the file is removed on overflow, so the next try starts clean.
3. **E12 is real.** `RedirectFetcher::request` does an unconditional `std::thread::spawn`, and `fetch` reads up to `max_bytes` (100 MB for image, pdf and json) into a `Vec`. `FaviconFetcher` has the cap logic (`MAX_FAVICON_FETCHES = 4`, `in_flight` plus `queue`) written out privately. Both have the shape "run at most N, queue the rest, start the next on a finish, dedupe by key", so the shared mechanism is `net::Throttle<J>` holding only the counting and the queue; each fetcher keeps its own `in_flight` key set, its own thread body and its own `tick`. Worst case memory drops from n x 100 MB to 4 x 100 MB. Dropped from the survey: "stream to a temp file above a few MB". `content::passes(kind, &bytes)` and the image and json decoders need the whole body in memory, so streaming would be a redesign of `save`; the cap of 4 is the bounded fix, and lowering the 100 MB limit is an open question for the user. A queued job waits behind up to four running ones, and a running one may take its whole body budget (about 13 minutes for 100 MB at the slow-link floor); this is the same trade the favicon fetcher already makes.
4. **`finish()` is called once per started job.** Both fetcher threads send exactly one result even on a panic (`catch_unwind`), so `tick` calls `Throttle::finish` once per result it drains. A result is never dropped without a `finish`, because `tick` is the only reader of the channel.
5. **E7: what interprocess 2.4.4 already does (read in `interprocess-2.4.4/src/os/windows/named_pipe/listener`):**
   - `PIPE_REJECT_REMOTE_CLIENTS` is on: `PipeListenerOptions.accept_remote` defaults to `false` and `pipe_mode()` adds the flag to every instance. The survey's "also set PIPE_REJECT_REMOTE_CLIENTS" is already true. It cannot be tested headlessly (it needs a second machine); it is pinned by `Cargo.lock` holding `interprocess 2.4.4` and by this reading. Task 5 re-reads it if the version changes.
   - The first-instance flag is on: `_create` passes `first = true`, so `CreateNamedPipe` fails with access denied if any process already owns the name. A pre-squatted name is therefore detected, not silently shared. Task 4 adds a test that pins it.
   - With no security descriptor, `create_security_attributes` passes a null descriptor, so Windows applies the default named-pipe DACL. Per Microsoft's documentation that grants full control to SYSTEM, Administrators and the creator owner and read access to Everyone and the anonymous account. This is what makes cross-user reading of the host-to-engine pipe (which carries `SetCookies`) possible. It is documentation, not measured here; Task 5's test reads back the DACL actually applied, so the claim is checked either way.
6. **E7's mechanisms, and what each defends against:**
   - Owner-only DACL, `D:P(A;;GA;;;OW)` (protected, one ACE: generic-all to the Owner Rights SID): defends against *another user* opening either pipe. Applied in `listen_local_pair` through `ListenerOptionsExt::security_descriptor`. Windows only.
   - Peer PID check: after `accept`, `peer_creds().pid()` (`GetNamedPipeClientProcessId` on Windows, `SO_PEERCRED` on Linux) must equal `Child::id()` of the process just spawned; otherwise the connection is dropped and the wait continues until the deadline. Defends against *another process of the same user* connecting first, which a DACL cannot (the owner is that user). Nothing is written to a connection before it passes (`send_version` comes after the accept).
   - Not planned: the random pipe-name suffix from the survey. `socket_id` is also the shared frame file's name (`frame_path(socket_id)`, `remove_frame_file`, the sweeps), so a random one changes file naming across `process_manager`, `event_loop` and the sweeps, and a same-user attacker can list `\\.\pipe\` anyway. Parked: revisit with E10 (`EngineEnv`). Also not planned: authenticating the *server* from the engine side (a fake listener on a pre-squatted fallback name). The first-instance flag makes a squatted primary name fail to listen; `spawn_tab` then retries once with the fallback id `{tab_id}-r{pid}-{n}`, which an attacker would have to have pre-squatted as well. That guessable fallback is the residual. Recorded in the README entry (Task 7).
   - Residual, stated plainly: the PID check trusts the PID not being reused while the connection is open (interprocess documents the same caveat); a flood of wrong-PID connections can delay but not extend past the deadline (Task 4 tests that), and could make the real engine's `connect` hit `ERROR_PIPE_BUSY` once, which the existing spawn path reports as a failed spawn the user retries.
7. **The spawned child is the process that connects.** `spawn_tab` does `Command::new(&host_exe)` and the engine mains connect from `main` (`connect_local_pair` in `manifold-engine-webview2`, `-cef`, `-servo`); none of them spawn a relauncher (`grep "Command::new\|current_exe"` over `manifold-engine-*/src` finds nothing). CEF's renderer and GPU subprocesses never open the control pipes. So `child.id()` is the right expected PID for all three local engines. WebKit does not use named pipes (TCP plus a token) and is unaffected by E7.
8. **E6 is real but its exploitability is unverified, and the plan does not depend on the answer.** `spawn_webkit_tab` builds a `bash -c` string with `shell_quote(initial_url)` and runs it as `wsl.exe -d Ubuntu -- bash -c <cmd>`; the comment at `reap_host` confirms the distro's default shell is fish, so `wsl.exe`'s re-joined command line may be parsed by fish before bash sees it. `shell_quote` only quotes for bash. Whether the outer layer re-quotes safely is undocumented `wsl.exe` behavior. The fix makes the question moot: the URL crosses as lowercase hex (`[0-9a-f]`), which both shells pass through unchanged. The survey's other options are not planned: `wsl.exe --exec` (not verifiable without WSL here, and it changes the process tree `reap_host` documents: the orphan analysis there would need redoing), and sending `Navigate` after the handshake (the engine would likely report `about:blank` as a navigation first, which the host may apply to the tab's URL; unverified, but a risk the hex route does not run).
9. **E6 also drops `ENGINE_TAB_ID` from the WebKit command.** The WebKit main reads it as `_tab_id` and never uses it; the tab id is session data (a synced session file can carry any id string) and not needed there. Removing it removes a second spliced value. The log file name `manifold-{tab_id}.log` (`per_tab_log_path`) also embeds the tab id and is not touched: node ids are validated store ids (`NodeId` parsing), so it is not a page-controlled path. Noted, not changed.
10. **E6 changes the WebKit host's env contract**: `ENGINE_INITIAL_URL` becomes `ENGINE_INITIAL_URL_HEX`, and `ENGINE_TAB_ID` is no longer passed. This is not the postcard protocol, so `PROTOCOL_VERSION` is not bumped. A stale WebKit binary still requires `ENGINE_TAB_ID`, so it panics at start (and would ignore the hex URL anyway): "rebuild WebKit in WSL after pulling" goes in the README entry, as the earlier token change did. No compatibility shim.
11. **`token_to_hex` is deleted, `to_hex` replaces it** (one caller, `process_manager`), and `token_from_hex` becomes a typed wrapper over `from_hex`. `from_hex` rejects what `u8::from_str_radix` would accept (a leading `+`), which also tightens the token parse. `engine_install::hex` is a third copy of bytes-to-hex and is replaced by `to_hex` in the same task (its callers are `verify` and tests).
12. **Order and file overlap.** Tasks 1, 2/3, 4/5, 6 are independent in behavior but share files: 3 builds on 2's `Throttle`; 4, 5 and 6 all edit `ipc-protocol/src/transport.rs`; 4 and 6 edit `process_manager.rs`. Run the tasks one at a time, in order.

## Survey items dropped, stale or parked

- **Not stale:** all four items still exist at `864f27e`. Only line references moved (E6 `process_manager.rs` 1357-1370 is now about 1352-1362, E7 `transport.rs` 65-91 is now `socket_name`, `listen_local_pair` and `accept_one`, E11 338-357 is the loop in `fetch`, E12 64-79 is `request`).
- **Already true, so dropped from E7:** `PIPE_REJECT_REMOTE_CLIENTS` (interprocess default) and the first-instance flag (Decision 5). The plan adds a test that pins the second and a DACL test; it adds no code for the first.
- **Parked, with why:**
  - E7 random pipe-name suffix: `socket_id` is also the frame file name (Decision 6). Revisit with E10.
  - E7 engine-side check that the *server* is manifold: only relevant after a pre-squat of both the primary and the fallback id (Decision 6).
  - E11 free-space check: needs `GetDiskFreeSpaceExW`; the cap bounds the disk use (Decision 1).
  - E12 streaming fetches to a temp file: `content::passes` and the decoders need the body in memory (Decision 3).
  - E6 `wsl.exe --exec`, computing the host IP inside the engine, and moving the token off the command line (the README already accepts the token: same user only): unnecessary once nothing page-controlled crosses the shells (Decision 8).

## Per-task summary

| # | Item | Kind | Model | Depends on | Proof |
|---|---|---|---|---|---|
| 1 | E11: stop the installer download at the pinned size | mechanical | haiku | none | red-first `download_aborts_past_the_pinned_size`; every existing `download_*` test still passes |
| 2 | E12: `net::Throttle`, `RedirectFetcher` runs at most 4 | judgment | sonnet | none | red-first `at_most_four_redirect_fetches_run_at_once`; `Throttle` unit test |
| 3 | E12: `FaviconFetcher` onto `Throttle`, delete its private copy | mechanical | haiku | 2 | `at_most_four_favicon_fetches_run_at_once` unchanged in intent, still passes |
| 4 | E7a: accept only the spawned child's PID | judgment | sonnet | none | red-first mismatch, flood and wiring tests; first-instance pin; Linux CI runs them too |
| 5 | E7b: owner-only DACL on both pipes | judgment | opus | 4 | applied-DACL read-back test (Windows); positive connect; manual cross-user check (cannot be headless) |
| 6 | E6: hex-encode WebKit spawn data | judgment | sonnet | 4 | red-first `page_controlled_text_never_reaches_the_wsl_command_line`; hex tests; engine edit checked in WSL or by reading |
| 7 | README, full gate, merge question | controller | controller | 1 to 6 | `cargo test`, clippy, `scripts/gate.sh LOG full`, CPU and Defender reported |

Mechanical tasks (1, 3) have complete code below; a reviewer checks every step landed and nothing else changed. Judgment tasks give the design and the tests and leave small call-site details to the implementer, who reports anything that differs.

---

### Task 1: E11, stop the installer download at the pinned size

**Model:** haiku. **Kind:** mechanical. **Proof:** the new test fails on the unmodified code (the file ends up 300 000 bytes and the error is "ended at"), passes after; the other `download_*` and `install_*` tests still pass.

**Files:**
- Modify: `manifold/src/engine_install.rs` (`fetch`, and its `tests` module)

**Interfaces:**
- Consumes: `download(url, part, total, progress, cancel)`, test helpers `serve`, `Mode::Honour`, `body`, `temp_dir` (all in the `tests` module).
- Produces: nothing new. `fetch` returns `Err(InstallError::Network("the server sent more than the {total} bytes expected"))` and removes `part` when a chunk would pass `total`.

**Blast radius:** `fetch` is called only by `download_with_idle`; `download` only by `install` (`engine_install.rs`, `download(pin.url, &part, pin.size, ..)`) and tests. No other module calls either. No format or protocol change.

- [ ] **Step 1: Add the test (red first).** In the `tests` module, after `download_fetches_everything`:

```rust
    #[test]
    fn download_aborts_past_the_pinned_size() {
        let b = body(300_000);
        let url = serve(b.clone(), Mode::Honour, None);
        let part = temp_dir("ei-cap").join("f.part");
        let total = 100_000u64; // the pin says 100 000 bytes; the server sends 300 000
        let mut seen = 0;
        let err = download(&url, &part, total, &mut |n| seen = n, &AtomicBool::new(false)).unwrap_err();
        assert!(matches!(&err, InstallError::Network(m) if m.contains("more than")), "{err:?}");
        assert!(seen <= total, "progress never passes the pin: {seen}");
        assert!(!part.exists(), "a stream that overran the pin is not kept for a resume");
    }
```

Run `cargo test -j 8 -p manifold --bins download_aborts_past_the_pinned_size`. Expected: FAIL (`err` is `Network("the download ended at 300000 of 100000 bytes")`). Paste the output.

- [ ] **Step 2: The fix.** In `fetch`, in the `Ok(Ok(chunk))` arm, before the `write_all`:

```rust
            Ok(Ok(chunk)) => {
                last_data = std::time::Instant::now();
                if have + chunk.len() as u64 > total {
                    // The pin says how big the file is. More than that is a wrong or hostile stream: stop
                    // before it fills the disk, and drop what arrived (a resume would ask the same server).
                    drop(file);
                    let _ = std::fs::remove_file(part);
                    return Err(InstallError::Network(format!("the server sent more than the {total} bytes expected")));
                }
                file.write_all(&chunk).map_err(io)?;
                have += chunk.len() as u64;
                progress(have);
            }
```

(`drop(file)` first: Windows cannot delete an open file. The helper thread ends when its next `tx.send` fails or the socket closes, the same as on cancel.)

- [ ] **Step 3: Update the doc comment of `download`.** Add one sentence after "A body that ends before `total` bytes is a `Network` error, never Ok.": "A body that would pass `total` is a `Network` error too, and deletes `part`."

- [ ] **Step 4: Run** `cargo test -j 8 -p manifold --bins engine_install`. Expected: PASS, including the new test and every existing `download_*` test.

- [ ] **Step 5: Commit** (`cargo fmt --all` first; in the background): `engine install: stop a download at the pinned size instead of at the end of the stream`.

---

### Task 2: E12, `net::Throttle` and the redirect fetcher's cap

**Model:** sonnet. **Kind:** judgment (the queue and the finish accounting are the part to get right). **Proof:** `Throttle`'s unit test; the new fetcher test fails on today's code (peak concurrency reaches 10) and passes after; the three existing `redirect_fetch` tests pass.

**Files:**
- Modify: `manifold/src/net.rs` (add `Throttle`, test)
- Modify: `manifold/src/redirect_fetch.rs`

**Interfaces:**
- Produces:
  - `crate::net::Throttle<J>` with `new(cap: usize)`, `admit(&mut self, job: J) -> Option<J>` (the job, if it may start now), `finish(&mut self) -> Option<J>` (a job finished: the next waiting one, to start now); and `#[cfg(test)]` accessors `running()` and `waiting()`.
  - `redirect_fetch::MAX_REDIRECT_FETCHES: usize = 4`.
- Consumes: nothing new.

**Blast radius:** `RedirectFetcher`'s public surface does not change (`new`, `request -> bool`, `tick`). Callers: `fetched.rs` (`refresh_node`), `event_loop.rs` (`RedirectableContent`), `main.rs` (`RedirectFetcher::new(profile::content_dir())`), and `tick` in `drain_tab_events`. A job now may sit queued before it starts; the tab-conversion target already re-checks that the tab still shows the URL when the result arrives, and a node refresh is idempotent, so a late start changes nothing for them. No format or protocol change.

- [ ] **Step 1: `Throttle` and its unit test in `net.rs`.** Add `use std::collections::VecDeque;` and, above `body_budget`:

```rust
/// At most `cap` background jobs at once; the rest wait, in order. The caller owns the threads and
/// the results: it starts whatever `admit` or `finish` hands back, and calls `finish` once per job
/// that was started, when that job's result is read.
pub(crate) struct Throttle<J> {
    cap: usize,
    running: usize,
    waiting: VecDeque<J>,
}

impl<J> Throttle<J> {
    pub(crate) fn new(cap: usize) -> Self {
        Self { cap, running: 0, waiting: VecDeque::new() }
    }

    /// `Some(job)` when it may start now (counted as running); `None` when it was queued.
    pub(crate) fn admit(&mut self, job: J) -> Option<J> {
        if self.running < self.cap {
            self.running += 1;
            Some(job)
        } else {
            self.waiting.push_back(job);
            None
        }
    }

    /// A running job finished. The next waiting job takes its slot and is returned to start; with
    /// nothing waiting the slot frees.
    pub(crate) fn finish(&mut self) -> Option<J> {
        let next = self.waiting.pop_front();
        if next.is_none() {
            self.running = self.running.saturating_sub(1);
        }
        next
    }

    #[cfg(test)]
    pub(crate) fn running(&self) -> usize {
        self.running
    }

    #[cfg(test)]
    pub(crate) fn waiting(&self) -> usize {
        self.waiting.len()
    }
}
```

In `net.rs`'s `tests` module:

```rust
    #[test]
    fn a_throttle_runs_at_most_its_cap_and_hands_on_in_order() {
        let mut t = Throttle::new(2);
        assert_eq!((t.admit(1), t.admit(2), t.admit(3), t.admit(4)), (Some(1), Some(2), None, None));
        assert_eq!((t.running(), t.waiting()), (2, 2));
        assert_eq!((t.finish(), t.finish()), (Some(3), Some(4)));
        assert_eq!((t.running(), t.waiting()), (2, 0), "a waiting job inherits the slot");
        assert_eq!((t.finish(), t.finish()), (None, None));
        assert_eq!((t.running(), t.waiting()), (0, 0));
        assert_eq!(t.admit(5), Some(5), "free again");
    }
```

Run `cargo test -j 8 -p manifold --bins a_throttle_runs` (PASS). `Throttle` is unused outside tests until Step 3, so do not run clippy until then.

- [ ] **Step 2: The fetcher test (red first).** In `redirect_fetch.rs`'s `tests` module add (the statics are used by this test alone):

```rust
    static RUNNING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    static PEAK: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    static RELEASE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    /// A stub fetch that counts how many run at once and holds until released.
    fn gated_fetch(_: &Path, _: &str, _: &str, _: ContentKind) -> Option<WebBody> {
        use std::sync::atomic::Ordering::SeqCst;
        let now = RUNNING.fetch_add(1, SeqCst) + 1;
        PEAK.fetch_max(now, SeqCst);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !RELEASE.load(SeqCst) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        RUNNING.fetch_sub(1, SeqCst);
        None
    }

    #[test]
    fn at_most_four_redirect_fetches_run_at_once() {
        use std::sync::atomic::Ordering::SeqCst;
        RUNNING.store(0, SeqCst);
        PEAK.store(0, SeqCst);
        RELEASE.store(false, SeqCst);
        let mut fetcher = RedirectFetcher::new(Some(crate::store::testutil::temp_dir("redirect_cap")));
        fetcher.fetch = gated_fetch;
        for n in 0..10 {
            let job = FetchJob {
                target: FetchTarget::Node(format!("7-{n}")),
                node_id: format!("7-{n}"),
                url: "https://example.invalid/x.pdf".into(),
                kind: ContentKind::Pdf,
            };
            assert!(fetcher.request(job));
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while RUNNING.load(SeqCst) < MAX_REDIRECT_FETCHES && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        // An absence cannot be waited for: give any extra thread 100 ms to show itself. Before the
        // cap all ten start within a millisecond or two.
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(PEAK.load(SeqCst), MAX_REDIRECT_FETCHES, "no fifth fetch started while four were held");
        RELEASE.store(true, SeqCst);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut done = 0;
        while done < 10 && std::time::Instant::now() < deadline {
            done += fetcher.tick().len();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(done, 10, "every queued job eventually ran and reported");
        assert_eq!(PEAK.load(SeqCst), MAX_REDIRECT_FETCHES);
    }
```

This does not compile yet (`MAX_REDIRECT_FETCHES`). To see it fail for the right reason, temporarily add `const MAX_REDIRECT_FETCHES: usize = 4;` only, run the test, and expect FAIL with `left: 10, right: 4` on the first `PEAK` assertion. Paste the output.

- [ ] **Step 3: The fix.** In `redirect_fetch.rs`: add

```rust
/// Fetches running at once; the rest wait in `throttle`. A pasted or restored session can name dozens
/// of large files, and each running fetch holds its whole body (up to 100 MB) in memory.
const MAX_REDIRECT_FETCHES: usize = 4;
```

Change the struct and `new`:

```rust
pub struct RedirectFetcher {
    dir: Option<PathBuf>,
    tx: mpsc::Sender<FetchResult>,
    rx: mpsc::Receiver<FetchResult>,
    /// Nodes with a fetch running or waiting: a second Refresh (or a Retry racing a
    /// restore-time refetch) is dropped instead of writing the same file.
    in_flight: HashSet<String>,
    /// Jobs beyond `MAX_REDIRECT_FETCHES` wait here.
    throttle: crate::net::Throttle<FetchJob>,
    /// `fetch`; tests swap in a stub.
    fetch: fn(&Path, &str, &str, ContentKind) -> Option<WebBody>,
}

impl RedirectFetcher {
    pub fn new(dir: Option<PathBuf>) -> Self {
        let (tx, rx) = mpsc::channel();
        let throttle = crate::net::Throttle::new(MAX_REDIRECT_FETCHES);
        Self { dir, tx, rx, in_flight: HashSet::new(), throttle, fetch }
    }

    /// Fetches `job.url` on a background thread, or queues it behind `MAX_REDIRECT_FETCHES` running
    /// ones. `false` (and nothing done) when `job.node_id` is already being fetched or waiting.
    pub fn request(&mut self, job: FetchJob) -> bool {
        if !self.in_flight.insert(job.node_id.clone()) {
            return false;
        }
        if let Some(job) = self.throttle.admit(job) {
            self.start(job);
        }
        true
    }

    fn start(&self, job: FetchJob) {
        let (tx, dir, fetch) = (self.tx.clone(), self.dir.clone(), self.fetch);
        std::thread::spawn(move || {
            // A panic still reports (as a failure), so `tick` frees the node and the slot;
            // the panic hook has already printed it.
            let body = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                dir.as_deref().and_then(|d| fetch(d, &job.node_id, &job.url, job.kind))
            }))
            .unwrap_or(None);
            let _ = tx.send(FetchResult { job, body });
        });
    }

    pub fn tick(&mut self) -> Vec<FetchResult> {
        let results: Vec<FetchResult> = self.rx.try_iter().collect();
        for r in &results {
            self.in_flight.remove(&r.job.node_id);
            if let Some(next) = self.throttle.finish() {
                self.start(next);
            }
        }
        results
    }
}
```

(Keep `tick`'s existing doc comment. Update the module doc's "a thread per job" to "a thread per running job, at most four".) Remove the temporary const from Step 2.

- [ ] **Step 4: Run** `cargo test -j 8 -p manifold --bins redirect_fetch` and `--bins net::`. Expected: PASS, including the three pre-existing fetcher tests (`a_second_request_for_a_node_in_flight_is_dropped`, `a_panicking_fetch_still_frees_its_node`, the save tests).
- [ ] **Step 5: Clippy** (`cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 --all-targets -- -D warnings`), `cargo fmt --all`.
- [ ] **Step 6: Commit:** `redirect fetch: at most four fetches at once, the rest wait (net::Throttle)`.

---

### Task 3: E12, `FaviconFetcher` onto `Throttle`

**Model:** haiku. **Kind:** mechanical. **Proof:** `at_most_four_favicon_fetches_run_at_once` and `a_panicking_fetch_gives_its_slot_back` pass with the asserts changed as given; `favicon_fetch.rs` no longer has its own queue.

**Files:**
- Modify: `manifold/src/favicon_fetch.rs`

**Interfaces:** consumes `crate::net::Throttle<String>`; produces nothing. `FaviconFetcher::{new, request, tick}` keep their signatures.

**Blast radius:** callers of `FaviconFetcher` are `card.rs` (`favicon_fetcher.request`), `event_loop.rs` (`request`, `tick`) and `main.rs` (`new`); none touches `in_flight` or `queue`. Behavior is unchanged: a request dedupes against running and waiting origins, at most four run, a waiting one starts when a result is drained. Only the tests read the removed `queue` field.

- [ ] **Step 1: Replace the private queue.** Delete `queue: VecDeque<String>` and its doc comment; add `throttle: crate::net::Throttle<String>`; make `in_flight` mean "running or waiting". Replace the constructor's field list, `request`, `start` and `tick`:

```rust
        Self { tx, rx, in_flight: HashSet::new(), throttle: crate::net::Throttle::new(MAX_FAVICON_FETCHES), cache_dir, fetch: fetch_favicon }
```

```rust
    pub fn request(&mut self, origin: String) {
        if !self.in_flight.insert(origin.clone()) {
            return;
        }
        if let Some(origin) = self.throttle.admit(origin) {
            self.start(origin);
        }
    }

    fn start(&self, origin: String) {
        let (tx, cache_dir, fetch) = (self.tx.clone(), self.cache_dir.clone(), self.fetch);
        std::thread::spawn(move || {
            // A panic still reports (as no favicon), so `tick` frees the
            // slot; the panic hook has already printed it.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match &cache_dir {
                Some(dir) => cached_or_fetched(dir, &origin, fetch),
                None => fetch(&origin),
            }))
            .unwrap_or(None);
            let _ = tx.send((origin, result));
        });
    }

    pub fn tick(&mut self) -> Vec<(String, Option<FaviconImage>)> {
        let mut results = Vec::new();
        while let Ok((origin, result)) = self.rx.try_recv() {
            self.in_flight.remove(&origin);
            if let Some(next) = self.throttle.finish() {
                self.start(next);
            }
            results.push((origin, result));
        }
        results
    }
```

Remove `VecDeque` from the imports if unused. Keep every existing doc comment, reworded where it says "queue" (the module's `request` doc: "starts at once while fewer than `MAX_FAVICON_FETCHES` run, and waits in the throttle otherwise").

- [ ] **Step 2: Update the test asserts.** In `at_most_four_favicon_fetches_run_at_once` replace `(fetcher.in_flight.len(), fetcher.queue.len())` with `(fetcher.throttle.running(), fetcher.throttle.waiting())` (still `(MAX_FAVICON_FETCHES, 2)`). In `a_panicking_fetch_gives_its_slot_back` the last assert stays `fetcher.in_flight.is_empty()` and add `assert_eq!(fetcher.throttle.running(), 0);`.
- [ ] **Step 3: Run** `cargo test -j 8 -p manifold --bins favicon_fetch`. Expected: PASS (the two tests above plus the origin and cache tests).
- [ ] **Step 4: Clippy and fmt** as in Task 2. **Commit:** `favicon fetch: use net::Throttle instead of a private copy of the same queue`.

---

### Task 4: E7a, accept only the spawned child

**Model:** sonnet. **Kind:** judgment. **Proof:** the mismatch, flood and wiring tests fail on today's code (they do not compile against the old signature, so red-first is shown with a temporary stub, see Step 1) and pass after; a fourth test pins the first-instance flag and passes before and after; all but the wiring test also run on Linux CI.

**Files:**
- Modify: `ipc-protocol/src/transport.rs` (`accept_one`, `accept_local_pair`, `accept_local_pair_while`, tests)
- Modify: `ipc-protocol/src/lib.rs` (the loopback test's call)
- Modify: `manifold/src/process_manager.rs` (`connect_host`, one new test)

**Interfaces:**
- Changes: `accept_local_pair(pair, peer_pid: u32, timeout)` and `accept_local_pair_while(pair, peer_pid: u32, timeout, waiting)`.
- Consumes: `interprocess`'s `StreamCommon::peer_creds() -> io::Result<PeerCreds>` (on a Windows server-side stream it calls `GetNamedPipeClientProcessId`) and `PeerCreds::pid() -> Option<u32>` (the prelude already imports the trait).

**Blast radius (grep done):** `accept_local_pair_while` is called only by `connect_host` (`process_manager.rs`); `accept_local_pair` only by `ipc-protocol/src/lib.rs`'s `local_transport_loopback_ping` and `transport.rs`'s own test. The engine crates call `connect_local_pair`, which is unchanged. No wire format change; `PROTOCOL_VERSION` stays.

- [ ] **Step 1: Write the tests (red first).** In `transport.rs`'s `tests` module:

```rust
    /// A process that is not the one just spawned must not become the engine, however early it dials.
    #[test]
    fn a_peer_that_is_not_the_expected_process_is_not_accepted() {
        let id = format!("test-peer-pid-{}", std::process::id());
        let pair = listen_local_pair(&id).unwrap();
        let intruder = {
            let id = id.clone();
            std::thread::spawn(move || connect_local_pair(&id).map(|_| ()))
        };
        let someone_else = std::process::id().wrapping_add(1);
        let err = accept_local_pair(&pair, someone_else, Duration::from_millis(300)).err().expect("rejected");
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        let _ = intruder.join();
    }

    /// Wrong peers that keep arriving must not hold the wait past its deadline (the sibling of
    /// `accept_remote_times_out_under_a_stream_of_bad_peers`).
    #[test]
    fn wrong_peers_cannot_extend_the_deadline() {
        let id = format!("test-peer-pid-flood-{}", std::process::id());
        let pair = listen_local_pair(&id).unwrap();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let intruder = {
            let (id, stop) = (id.clone(), stop.clone());
            std::thread::spawn(move || {
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let _ = connect_local_pair(&id);
                    std::thread::sleep(Duration::from_millis(5)); // pacing the flood, not a wait
                }
            })
        };
        let start = Instant::now();
        let result = accept_local_pair(&pair, std::process::id().wrapping_add(1), Duration::from_millis(300));
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        intruder.join().unwrap();
        assert_eq!(result.err().map(|e| e.kind()), Some(io::ErrorKind::TimedOut));
        assert!(start.elapsed() < Duration::from_secs(2), "took {:?}", start.elapsed());
    }

    /// Pins interprocess's first-instance flag: a name another process owns cannot be listened on, so a
    /// squatted pipe name fails loudly instead of being shared.
    #[test]
    fn a_pipe_name_already_in_use_cannot_be_listened_on() {
        let id = format!("test-first-instance-{}", std::process::id());
        let _first = listen_local_pair(&id).unwrap();
        assert!(listen_local_pair(&id).is_err());
    }
```

Add a stub to see red: temporarily give `accept_local_pair` a `peer_pid: u32` parameter that is ignored and run `cargo test -j 8 -p ipc-protocol a_peer_that_is_not wrong_peers`. Expected: FAIL (`a_peer_that_is_not_the_expected_process_is_not_accepted` gets `Ok`, not `TimedOut`; `wrong_peers_cannot_extend_the_deadline` gets `Ok`). The first-instance test passes already: it is a pin, not a fix. Paste the output.

- [ ] **Step 2: The fix in `accept_one`.** Add the parameter and the check; keep the doc comment and extend it with the PID rule:

```rust
fn accept_one(
    listener: &Listener,
    timeout: Duration,
    peer_pid: u32,
    waiting: &mut dyn FnMut() -> bool,
) -> io::Result<LocalStream> {
    listener.set_nonblocking(interprocess::local_socket::ListenerNonblockingMode::Accept)?;
    let deadline = Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok(stream) if is_peer(&stream, peer_pid) => return Ok(stream),
            // Dropped without a byte written or read; the wait goes on, still bounded by the deadline.
            Ok(_) => eprintln!("ignored a connection from a process other than the engine-host just spawned"),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e),
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("timed out after {timeout:?} waiting for engine-host to connect"),
            ));
        }
        if !waiting() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "stopped waiting for engine-host"));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Whether the process on the other end of `stream` is `pid`. The OS reports it (the client's process
/// id on a Windows named pipe, `SO_PEERCRED` on Linux); a peer whose id cannot be read is not trusted.
fn is_peer(stream: &LocalStream, pid: u32) -> bool {
    stream.peer_creds().ok().and_then(|creds| creds.pid()) == Some(pid)
}
```

Thread the parameter through `accept_local_pair` and `accept_local_pair_while` (both docs: "`peer_pid` is the process id of the engine-host just spawned (`Child::id()`): a connection from any other process is dropped"). Update the three existing callers: `lib.rs`'s `accept_local_pair(&listeners, std::process::id(), Duration::from_secs(5))` (the engine in that test is a thread of this process), and `transport.rs`'s `accept_local_pair_times_out_when_nobody_connects` (`accept_local_pair(&pair, std::process::id(), timeout)`).

What no headless test covers: "a wrong peer is dropped, then the right one is accepted". In a unit test every peer is this process, so the right and the wrong peer cannot differ, and a second client would also meet `ERROR_PIPE_BUSY` on a Windows pipe still holding the first one's stale connection. That sequence is the same loop iteration as the rejection test plus the unchanged `return Ok(stream)` arm, and the real spawns in the gate exercise the accept arm with the true child. Say so in the report; do not add a flaky test for it.

- [ ] **Step 3: Wire `connect_host`.** In `process_manager.rs`, before the closure: `let pid = child.id();` and pass `pid` as the new second argument of `transport::accept_local_pair_while(listeners, pid, timeout, &mut || ..)`. (Take the id first: the closure borrows `child` mutably.)
- [ ] **Step 4: The wiring test.** In `process_manager.rs`'s `tests`, beside `a_host_that_never_connects_is_reported_as_still_running_and_killed`:

```rust
    /// The pid check is wired to the spawned child: a process that dials the pipes first, but is not the
    /// child (here a thread of the test process), is not accepted.
    #[test]
    fn a_connection_from_a_process_that_is_not_the_child_is_refused() {
        let dir = crate::store::testutil::temp_dir("host_impostor");
        let log = dir.join("host.log");
        let id = format!("test-impostor-{}", std::process::id());
        let listeners = transport::listen_local_pair(&id).unwrap();
        let mut child = fake_host("ping -n 30 127.0.0.1", "sleep 30", &log);
        let impostor = {
            let id = id.clone();
            std::thread::spawn(move || transport::connect_local_pair(&id).map(|_| ()))
        };
        let err =
            connect_host(&listeners, &mut child, &log, Duration::from_millis(300)).err().expect("no host connected");
        assert!(err.to_string().starts_with("engine-host is still running but did not connect within 300ms"), "{err}");
        let _ = impostor.join();
        let _ = std::fs::remove_dir_all(dir);
    }
```

- [ ] **Step 5: Run** `cargo test -j 8 -p ipc-protocol` and `cargo test -j 8 -p manifold --bins process_manager`. Expected: PASS (the pre-existing `connect_host` tests never connect and are unaffected). Then clippy and fmt. Linux: CI runs the same tests (abstract sockets and `SO_PEERCRED`); if a Linux run cannot be done locally, say so in the report.
- [ ] **Step 6: Commit:** `local engine pipes: accept only the process just spawned (peer pid check)`.

---

### Task 5: E7b, owner-only DACL on both pipes

**Model:** opus. **Kind:** judgment: Win32 security semantics, with two claims this plan could not verify (see Step 1 and Step 4). **Proof:** a Windows-only test reads back the DACL applied to a live pipe and requires exactly the owner ACE and no Everyone, Anonymous or Users ACE; the existing loopback and spawn tests prove the owner can still connect. **Cannot be tested headlessly:** that a *different user* is denied. That needs a second Windows account; Step 5 is a manual check for the user, and the task's report must say whether it was done.

**Files:**
- Modify: `ipc-protocol/Cargo.toml`, `Cargo.lock`
- Modify: `ipc-protocol/src/transport.rs`

**Interfaces:**
- Produces: private `fn local_listener(name: Name<'static>) -> io::Result<Listener>` (replaces the two inline `ListenerOptions` chains in `listen_local_pair`) and, `#[cfg(windows)]`, private `fn owner_only() -> io::Result<SecurityDescriptor>`.
- Consumes: `interprocess::os::windows::{local_socket::ListenerOptionsExt, security_descriptor::SecurityDescriptor}`, `widestring::u16cstr!`.

**Blast radius:** `listen_local_pair` is called by `process_manager` (`listen_local_pair_with_retry`, so every local engine spawn), `lib.rs`'s loopback test and several `process_manager` tests; its signature is unchanged. On Linux the function is unchanged (the DACL code is `cfg(windows)`). The mechanisms in the Windows security model that apply, exactly:
- **DACL** (`D:P(A;;GA;;;OW)`): who may open the pipe. `D:P` is a protected DACL with one ACE, generic-all to the Owner Rights SID. Replaces the default (SYSTEM, Administrators, creator owner full; Everyone and Anonymous read).
- **`PIPE_REJECT_REMOTE_CLIENTS`**: already set by interprocess (Decision 5). Not changed.
- **`FILE_FLAG_FIRST_PIPE_INSTANCE`**: already set; pinned by Task 4's test. Not changed.
- Not used: a mandatory integrity label (the default medium level is what the engines run at) and impersonation.

- [ ] **Step 1: Spike before writing the test.** Confirm in a scratch test (not committed) that `SecurityDescriptor::deserialize(widestring::u16cstr!("D:P(A;;GA;;;OW)"))` succeeds and that a same-process `connect_local_pair` still works with it applied. Unverified assumption being tested: Owner Rights matches the connecting token both for a normal token and for the elevated Administrators token CI's runner uses (there the owner is the Administrators alias). If the elevated case fails in CI, replace `OW` with the current user's SID string (obtained with `OpenProcessToken` / `GetTokenInformation(TokenUser)` / `ConvertSidToStringSidW`) and say so in the report; do not widen the ACL.
- [ ] **Step 2: Dependencies.** In `ipc-protocol/Cargo.toml` add

```toml
[target.'cfg(windows)'.dependencies]
widestring = "1.2"

[target.'cfg(windows)'.dev-dependencies]
windows-sys = { version = "0.61", features = ["Win32_Foundation", "Win32_Security", "Win32_Security_Authorization"] }
```

Run `cargo check -j 8 -p ipc-protocol` and commit the resulting `Cargo.lock` change (CI uses `--locked`). `widestring 1.2.1` and `windows-sys 0.61.2` are already locked as interprocess's dependencies, so only the `ipc-protocol` entry's dependency list should change.

- [ ] **Step 3: The change.** In `transport.rs`:

```rust
/// The pipes' DACL: generic-all to the Owner Rights SID, nothing else (`D:P`, protected). The default a
/// named pipe gets also lets Everyone and Anonymous read, which would let another user read what the host
/// sends (the cookie jar seed). Another process of the *same* user is not stopped by this; the peer pid
/// check in `accept_one` is.
#[cfg(windows)]
fn owner_only() -> io::Result<interprocess::os::windows::security_descriptor::SecurityDescriptor> {
    interprocess::os::windows::security_descriptor::SecurityDescriptor::deserialize(widestring::u16cstr!(
        "D:P(A;;GA;;;OW)"
    ))
}

fn local_listener(name: Name<'static>) -> io::Result<Listener> {
    let options = ListenerOptions::new().name(name);
    #[cfg(windows)]
    let options = interprocess::os::windows::local_socket::ListenerOptionsExt::security_descriptor(options, owner_only()?);
    options.create_sync()
}

pub fn listen_local_pair(socket_id: &str) -> io::Result<LocalListenerPair> {
    Ok(LocalListenerPair {
        host_to_engine: local_listener(socket_name(socket_id, "h2e")?)?,
        engine_to_host: local_listener(socket_name(socket_id, "e2h")?)?,
    })
}
```

Add one line to `listen_local_pair`'s doc comment: "On Windows both pipes are owner-only (`owner_only`)."

- [ ] **Step 4: The test.** Windows only. It reads the DACL of the live pipe and pins the exact string it sees:

```rust
    #[cfg(windows)]
    #[test]
    fn the_pipes_dacl_grants_only_the_owner() {
        use std::os::windows::fs::OpenOptionsExt;
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Security::Authorization::{
            ConvertSecurityDescriptorToStringSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1, SE_KERNEL_OBJECT,
        };
        use windows_sys::Win32::Security::DACL_SECURITY_INFORMATION;
        const READ_CONTROL: u32 = 0x0002_0000;

        let id = format!("test-dacl-{}", std::process::id());
        let _pair = listen_local_pair(&id).unwrap();
        // READ_CONTROL alone is enough to read the DACL; the open connects to (and then leaves) the
        // listener's waiting instance.
        let pipe = std::fs::OpenOptions::new()
            .access_mode(READ_CONTROL)
            .open(format!(r"\\.\pipe\manifold-{id}-h2e"))
            .expect("the owner can open its own pipe");
        let (mut sd, mut dacl) = (std::ptr::null_mut(), std::ptr::null_mut());
        // SAFETY: `pipe` is a live handle; the out-pointers are valid; the descriptor is freed below.
        let status = unsafe {
            GetSecurityInfo(
                pipe.as_raw_handle() as _,
                SE_KERNEL_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut dacl,
                std::ptr::null_mut(),
                &mut sd,
            )
        };
        assert_eq!(status, 0, "GetSecurityInfo");
        let mut wide: *mut u16 = std::ptr::null_mut();
        // SAFETY: `sd` came from GetSecurityInfo; `wide` is freed with LocalFree below.
        let ok = unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                sd,
                SDDL_REVISION_1,
                DACL_SECURITY_INFORMATION,
                &mut wide,
                std::ptr::null_mut(),
            )
        };
        assert_ne!(ok, 0, "ConvertSecurityDescriptorToStringSecurityDescriptorW");
        // SAFETY: `wide` is a NUL-terminated UTF-16 string just allocated by the call above.
        let sddl = unsafe { widestring::U16CStr::from_ptr_str(wide) }.to_string_lossy();
        // SAFETY: both were allocated by the calls above and are not used again.
        unsafe {
            windows_sys::Win32::Foundation::LocalFree(wide.cast());
            windows_sys::Win32::Foundation::LocalFree(sd);
        }
        assert!(sddl.contains(";;;OW)"), "the owner has an ACE: {sddl}");
        for broad in [";;;WD)", ";;;AN)", ";;;BU)", ";;;AU)", ";;;SY)", ";;;BA)"] {
            assert!(!sddl.contains(broad), "{broad} must not be granted: {sddl}");
        }
    }
```

Implementer: run it once, record the exact SDDL printed (expected `D:P(A;;GA;;;OW)` or the equivalent the system returns), and tighten the assert to equality with that string if it is stable. (`sd` is a `PSECURITY_DESCRIPTOR`, a `*mut c_void`, which `LocalFree` takes as is in windows-sys 0.61; adjust the casts if the signature differs.) If `GetSecurityInfo` cannot be called on a client handle of a pipe, fall back to asserting only that `owner_only()` parses and that its serialized DACL contains no broad SID, and say in the report that the applied descriptor was not read back. The positive path (the owner still connects) is covered by `local_transport_loopback_ping` and the `process_manager` spawn tests, which now run with the DACL in place.

- [ ] **Step 5: Manual cross-user check (user; not headless).** Add an `#[ignore]` test `hold_pipes_for_a_cross_user_check` in `transport.rs` that calls `listen_local_pair("manual-xuser")`, prints `\\.\pipe\manifold-manual-xuser-h2e`, and waits up to 120 s inside `accept_local_pair(&pair, 0, 120 s)` (pid 0 never matches, so nothing is accepted). The user runs it (`cargo test -p ipc-protocol --release -- --ignored --nocapture hold_pipes`) and, from a second, non-admin Windows account, runs `[System.IO.Pipes.NamedPipeClientStream]::new('.','manifold-manual-xuser-h2e','In').Connect(2000)`; expected: `UnauthorizedAccessException`. The same probe from the owner's own account connects. Delete this ignored test after the check, or keep it only if the user wants it. The report states whether the user did the check.
- [ ] **Step 6: Run** `cargo test -j 8 -p ipc-protocol` and `cargo test -j 8 -p manifold --bins process_manager`; clippy; fmt.
- [ ] **Step 7: Commit:** `local engine pipes: owner-only DACL`.

---

### Task 6: E6, hex-encode what reaches the WSL command line

**Model:** sonnet. **Kind:** judgment (a boundary between two shells and an engine that cannot be built here). **Proof:** the new test fails on the extracted-but-unfixed command builder (it finds the hostile URL's text in the command), passes after; the hex helpers' tests pass on Windows and Linux; the engine edit is verified in WSL (`cargo check`) or by a named second reader.

**Files:**
- Modify: `ipc-protocol/src/transport.rs` (`to_hex`, `from_hex`; remove `token_to_hex`; `token_from_hex`)
- Modify: `manifold/src/process_manager.rs` (`spawn_webkit_tab`, new `webkit_shell_command`, its tests)
- Modify: `manifold/src/engine_install.rs` (delete its `hex`)
- Modify: `manifold-engine-webkit/src/main.rs` (read `ENGINE_INITIAL_URL_HEX`, stop requiring `ENGINE_TAB_ID`)

**Interfaces:**
- Produces: `ipc_protocol::transport::to_hex(&[u8]) -> String`, `from_hex(&str) -> Option<Vec<u8>>` (lowercase or uppercase digits, even length, ASCII hex digits only); `process_manager::webkit_shell_command(initial_url: &str, port: u16, token: &Token, binary_path: &str) -> std::io::Result<String>`.
- Removes: `token_to_hex`, `engine_install::hex`.
- Env contract change (WebKit host only): `ENGINE_INITIAL_URL_HEX` replaces `ENGINE_INITIAL_URL`; `ENGINE_TAB_ID` is gone. **Flag: not a postcard protocol change; no `PROTOCOL_VERSION` bump; the WebKit binary must be rebuilt in WSL together with `manifold`.**

**Blast radius (grep done):** `shell_quote` is called only by `spawn_webkit_tab` (three times); after this task only for `binary_path`. `token_to_hex` is called only in `spawn_webkit_tab` and `transport.rs`'s `token_tests`; `token_from_hex` in `manifold-engine-webkit` and `token_tests`. `engine_install::hex` is called in `verify` and in tests (`sha256`/`sha1` pins at the two `hex(&Sha256::digest(..))` sites). The Servo, CEF and WebView2 mains do not read any of the changed variables. `reap_host`'s comment about the fish/bash process tree stays true (the command is still `bash -c`).

- [ ] **Step 1: Hex helpers, their tests, and the caller of the removed `token_to_hex`** (`transport.rs`, `process_manager.rs`). Replace `token_to_hex`/`token_from_hex`:

```rust
/// Lowercase hex of `bytes`. The only form in which text that came from a page crosses a shell boundary.
pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The bytes `hex` spells, or `None` unless it is an even number of ASCII hex digits (`from_str_radix`
/// alone would also take a leading `+`).
pub fn from_hex(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()).collect()
}

pub fn token_from_hex(hex: &str) -> Option<Token> {
    from_hex(hex)?.try_into().ok()
}
```

In `token_tests`: `token_to_hex(&token)` becomes `to_hex(&token)`; add

```rust
    #[test]
    fn hex_round_trips_any_bytes_and_refuses_anything_else() {
        let text = "https://x/'$(touch${IFS}/tmp/pwn)'\";`id`\n\u{e9}";
        assert_eq!(from_hex(&to_hex(text.as_bytes())).unwrap(), text.as_bytes());
        assert!(to_hex(text.as_bytes()).bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        assert_eq!(from_hex("0A").unwrap(), [10]);
        for bad in ["0", "zz", "+f", "0x", "é0"] {
            assert_eq!(from_hex(bad), None, "{bad}");
        }
        assert_eq!(token_from_hex(&"+f".repeat(16)), None, "a sign is not a hex digit");
    }
```

In `process_manager.rs`'s `spawn_webkit_tab`, the one old caller becomes `transport::to_hex(&token)`. Run `cargo test -j 8 -p ipc-protocol hex` (PASS) and `cargo check -j 8 -p manifold`.

- [ ] **Step 2: Extract the command builder unfixed, with the test (red first).** In `process_manager.rs`, move the `format!` out of `spawn_webkit_tab` into

```rust
/// The `bash -c` string `spawn_webkit_tab` hands to `wsl.exe -d Ubuntu -- bash -c`. It crosses two
/// shells there (the distro's login shell parses what `wsl.exe` re-joins, then bash), so text from a
/// page must not appear in it as itself.
fn webkit_shell_command(
    initial_url: &str,
    port: u16,
    token: &transport::Token,
    binary_path: &str,
) -> std::io::Result<String> {
    Ok(format!(
        "ENGINE_INITIAL_URL={} ENGINE_TCP_PORT={} ENGINE_TOKEN={} \
         ENGINE_HOST_IP=$(ip route | awk '/^default/ {{print $3}}') xvfb-run -a {}",
        shell_quote(initial_url),
        port,
        transport::to_hex(token),
        shell_quote(binary_path),
    ))
}
```

Call it from `spawn_webkit_tab` (`let command = webkit_shell_command(initial_url, port, &token, &binary_path)?;`) and drop the `tab_id` argument; `tab_id` is still used for the log files. Add the test in `process_manager.rs`'s `tests`:

```rust
    /// Page-controlled text (a URL typed, pasted or synced) must reach `wsl.exe` only as hex: whatever
    /// the distro's login shell or bash do with quotes, `$`, backticks or newlines, they never see any.
    #[test]
    fn page_controlled_text_never_reaches_the_wsl_command_line() {
        let token = [7u8; transport::TOKEN_LEN];
        let build = |url: &str| webkit_shell_command(url, 4000, &token, "/mnt/c/repo/target-linux/debug/host").unwrap();
        let benign_url = "https://example.com/";
        let hostile_url = "https://x/'$(touch${IFS}/tmp/pwn)'\";`id`\n&|;<>\\ \u{e9}";
        let (benign, hostile) = (build(benign_url), build(hostile_url));
        assert!(!hostile.contains("pwn") && !hostile.contains('\n'), "{hostile}");
        // The two commands are the same text once each URL's hex is masked: nothing else depends on it.
        assert_eq!(
            benign.replace(&transport::to_hex(benign_url.as_bytes()), "H"),
            hostile.replace(&transport::to_hex(hostile_url.as_bytes()), "H"),
        );
        assert!(!benign.contains("ENGINE_TAB_ID"), "the tab id is not passed: {benign}");
    }

    #[test]
    fn a_url_too_long_for_a_command_line_is_refused_by_name() {
        let token = [7u8; transport::TOKEN_LEN];
        let err = webkit_shell_command(&"a".repeat(20_000), 4000, &token, "/x").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }
```

Run `cargo test -j 8 -p manifold --bins page_controlled_text`. Expected: FAIL (the hostile command contains `pwn`; also contains `ENGINE_TAB_ID` is already absent so only the first two asserts matter). Paste the output. The second test also fails (returns `Ok`).

- [ ] **Step 3: The fix.** In `webkit_shell_command` use hex and the guard:

```rust
    /// Windows' command line holds 32 767 characters; leave the rest of this one room.
    const MAX_URL_HEX: usize = 16_384;
    let url_hex = transport::to_hex(initial_url.as_bytes());
    if url_hex.len() > MAX_URL_HEX {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "the URL is too long to start a WebKit tab with"));
    }
    Ok(format!(
        "ENGINE_INITIAL_URL_HEX={url_hex} ENGINE_TCP_PORT={port} ENGINE_TOKEN={} \
         ENGINE_HOST_IP=$(ip route | awk '/^default/ {{print $3}}') xvfb-run -a {}",
        transport::to_hex(token),
        shell_quote(binary_path),
    ))
```

Update `shell_quote`'s doc comment ("for the one path `webkit_shell_command` splices: derived from the running exe's location, never from a page") and the comment above the token in `spawn_webkit_tab`. Delete `engine_install::hex`, replacing its callers with `ipc_protocol::transport::to_hex` (in `verify`: `to_hex(&sha256.finalize())`; in tests `to_hex(&Sha256::digest(..))`).

Run the two new tests (PASS), then `cargo test -j 8 -p manifold --bins engine_install process_manager`.

- [ ] **Step 4: The engine side** (`manifold-engine-webkit/src/main.rs`). Delete the `_tab_id` line and its comment block; replace the `initial_url` line with

```rust
    // Hex of the UTF-8 URL (process_manager.rs's webkit_shell_command): page text crosses the WSL shells
    // only as hex. A missing variable keeps the old default; a malformed one is a bug in the host.
    let initial_url = match std::env::var("ENGINE_INITIAL_URL_HEX") {
        Ok(hex) => String::from_utf8(transport::from_hex(&hex).expect("ENGINE_INITIAL_URL_HEX must be hex"))
            .expect("ENGINE_INITIAL_URL_HEX must spell UTF-8"),
        Err(_) => "https://example.com".to_string(),
    };
```

This crate does not build on Windows. Verify with, from PowerShell: `wsl.exe -d Ubuntu -- bash -c 'cd "/mnt/c/Users/Winston Li/Documents/GitHub/manifold" && CARGO_TARGET_DIR=target-linux cargo check -j 8 -p manifold-engine-webkit'` (expected duration: a few minutes cold; check `df -h /c` first). If WSL is not available, say so, and have a second agent read the diff against `transport::from_hex`'s signature. The report names which was done. Optionally, if WSL is available, record whether the unfixed command was ever exploitable: before Step 3, run the Step 2 command string with the survey's URL (`https://x/'$(touch${IFS}/tmp/pwn)'`) through `wsl.exe -d Ubuntu -- bash -c` with `xvfb-run -a <binary>` replaced by `env`, and look for `/tmp/pwn`. The result goes in `## Outcome`; it changes nothing about the fix.

- [ ] **Step 5: Clippy and fmt** on the three Windows crates. **Commit:** `webkit spawn: page-controlled text reaches the WSL command line only as hex`.

---

### Task 7: Docs, full gate, and the merge question

**Model:** controller. **Kind:** judgment. **Proof:** the whole suite and the full gate pass; the report carries CPU load and Defender state.

**Files:**
- Modify: `README.md` (the "IPC layer trusted its peers" entry and the engine-install and fetch entries; find them by those words)
- Modify: `.superpowers/survey/engines.md` (mark E6, E7, E11, E12 done, with the parked sub-items)
- Modify: this plan's `## Outcome`

- [ ] **Step 1: README.** One new "Fixed (2026-10-xx, wave 3a)" entry beside the 2026-09-30 IPC one, saying: the local engine pipes accept only the spawned child (PID) and are owner-only on Windows, with the manual cross-user check result; `PIPE_REJECT_REMOTE_CLIENTS` and the first-instance flag were already on (interprocess 2.4.4); the installer download stops at the pinned size and deletes its `.part` on overflow; redirect fetches run at most four at once (`net::Throttle`, shared with favicons); the WebKit command line carries the URL as hex, drops `ENGINE_TAB_ID`, and **the WebKit host must be rebuilt in WSL after pulling**. Under it, the parked items with their reasons: random pipe-name suffix (frame file naming; revisit with E10), engine-side server authentication, free-space check, streaming redirect fetches to disk, `wsl.exe --exec`. Update the earlier entry's "the token is visible in the command line" sentence only if it is now wrong (it is not).
- [ ] **Step 2: Build and gate.** `cargo build -j 8 -p manifold`, then `scripts/gate.sh LOG full` detached at BelowNormal (CLAUDE.md "Gate"). Before it: `Get-Process python, manifold, wsl` for leftovers; CPU `(Get-Counter '\Processor(_Total)\% Processor Time' -SampleInterval 2 -MaxSamples 2).CounterSamples[-1].CookedValue` under 20; record `(Get-MpComputerStatus).RealTimeProtectionEnabled`.
- [ ] **Step 3: Full tests and lints:** `cargo fmt --all --check`, the clippy line from Global Constraints, `cargo test -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2`. Confirm the Linux CI job's result after pushing, or run it under WSL (`CARGO_TARGET_DIR=$HOME/mf-target cargo test -p manifold -p ipc-protocol`) and report which.
- [ ] **Step 4: Ask the user** whether to merge `wave-3a-engine-hardening` into `main`, whether to push, and (if wanted) whether to cut a release. Do none of these unprompted.
- [ ] **Step 5: Commit the docs:** `docs: wave 3a engine hardening (README, survey)`.

---

## Outcome

(Left empty on purpose. The controller fills it in after Task 7: per task, the commit, the red-first output, the report's verification route for Task 6, whether Task 5's manual cross-user check was done, the gate's CPU and Defender readings, and any item dropped or parked while executing.)

## Review rounds

Three rounds of fixes. A fourth read (the front matter and decisions, Tasks 1 to 4 in full, and the edited parts of Tasks 5 and 6) found nothing material, so the plan stopped there; Tasks 5 to 7 were checked in rounds 2 and 3 against the interprocess and windows-sys sources but were not re-read end to end a final time. Each round was a read of the plan against the code and the interprocess 2.4.4 source, not against memory.

1. **Survey claims versus code (before drafting).** Caught that two E7 changes the survey asks for are already true: `PIPE_REJECT_REMOTE_CLIENTS` is interprocess's default and the first-instance flag is set on the first pipe instance (both read in `create_instance.rs`). Caught that E11 needs no Content-Length special case, because the loop already has the pinned `total`. Caught that E12's "stream to a temp file" cannot work without redesigning `save` (`content::passes` needs the bytes), and that E7's random pipe suffix would rename frame files (`socket_id` is the frame path). Caught that E6's `wsl.exe --exec` and send-`Navigate-later` options each carry an unverifiable risk here, and that hex-encoding removes the need to answer the open "is it exploitable" question.
2. **Code blocks and tests versus the sources.**
   - Removed a `the_expected_process_is_accepted_after_a_wrong_one_is_dropped` test: on Windows a second client meets `ERROR_PIPE_BUSY` on the stale instance, and on Linux the stale connection would be accepted, so it would be flaky; the gap is stated in Task 4 instead.
   - Added the missed third caller of `accept_local_pair` (`accept_local_pair_times_out_when_nobody_connects`).
   - Corrected `peer_creds` to `StreamCommon`, not `Stream`.
   - Reordered Task 6 so `token_to_hex`'s removal and its caller change in one step instead of leaving a dangling reference.
   - Replaced two placeholders ("existing body, unchanged", "free with LocalFree here") with real code.
   - Removed an unverified "15 existing tests" count.
   - Marked three claims as unverified rather than asserted: the elevated-token behavior of the Owner Rights ACE (with a fallback to the user's SID), `GetSecurityInfo` on a client pipe handle (with a fallback to checking the SDDL constant), and the `about:blank` side effect of a late `Navigate`.
3. **Contradictions and overclaims.** Fixed "a squatted pipe name fails loudly": `spawn_tab` falls back to a predictable `{tab_id}-r{pid}-{n}` id, so that is the residual, now stated (Decision 6 and the parked list). Fixed the stale-WebKit-binary description (it panics on the missing `ENGINE_TAB_ID`; it does not fall back to example.com). Dropped an assertion about ureq's `Content-Length` handling that was not read. Reworded the E11 overflow rule to say what the code does (check before the write, delete on overflow).
4. **Fourth read: no material findings.** Remaining known soft spots, all flagged in-task rather than hidden: Task 5's two unverified Win32 behaviors, the 100 ms absence wait in Task 2's test, and Task 6's engine edit that only WSL or a reader can check.
