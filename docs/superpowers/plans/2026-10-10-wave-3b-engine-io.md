---
status: done
---

# Wave 3b: engine I/O off the UI thread Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The UI thread never waits on an engine, and nothing between an engine and the app grows without bound:
- E1: every `EngineTab::send` today is a blocking pipe or socket write on the UI thread. An engine that stops reading freezes the whole app (input, redraw, headless commands), and so does closing its tab, since `Drop` sends `Shutdown` the same way. Instead, a send becomes a non-blocking enqueue, one writer thread per tab writes, and a tab whose writer is stuck shows as "not responding" (card badge plus toast). The user then picks Restart Engine or End Engine. Nothing is killed automatically.
- E3: every engine message, frames included, goes into an unbounded channel that the UI thread empties completely each tick. Instead:
  - frames go to a one-frame slot per tab;
  - every other message goes through a bounded channel;
  - one drain applies at most K messages per tab;
  - a reader wakes the event loop when a message arrives, instead of waiting for the next tick.

**Architecture:** One new module, `process_manager/engine_io.rs`, holds both directions:
- **Sends.**
  - `SendQueue` is a bounded FIFO. While the writer is behind, a new message merges into the newest queued one through one table, `coalesce(prev, next)`: the latest pointer move wins, the latest resize wins, and wheel deltas add up. Everything else stays strict FIFO.
  - `Writer` owns the queue and one named thread per tab that writes it out. A refused push, or a single write blocked for `UNRESPONSIVE_AFTER`, marks the tab unresponsive.
  - `shutdown` queues `Shutdown` last, and `Drop` waits for it at most `SHUTDOWN_FLUSH` before the existing kill.
- **Reads.**
  - The reader thread puts `FrameReady` into a `FrameSlot` (latest wins) and every other message into a `sync_channel(EVENTS_CAP)`. When that channel is full, the reader blocks. No message is dropped: the engine backs up on its own pipe instead.
  - The reader calls a `Wake` after each non-frame message. The `Wake` posts `UserEvent::EngineMessage`, coalesced by an `AtomicBool`, and that event drains without counting as user activity.
- **Engine side, inside `ipc-protocol` only.** `MessageChannel::send` writes each message as one buffer in one write. The engine's own reader (`spawn_host_reader`) gets the same kind of bound (`ENGINE_INBOX`), so an engine whose main loop is stuck backs up the app's writer (Task 7).

**Tech Stack:** Rust (edition 2021, nightly-2026-09-23), `std::sync::{mpsc, Mutex, Condvar}`, postcard 1.1.3 (`to_extend`, unconditional in `ser/mod.rs`), interprocess 2.4.4 (unchanged), winit 0.30.13 `EventLoopProxy`. No new dependencies.

**Source:**
- `.superpowers/survey/engines.md` sections E1 (lines 13-28) and E3 (52-65), plus the wake half of E9 (165-175). E2 (30-50) is parked by the user (below).
- User decisions given with the task:
  - E1 and E3 ship as one wave.
  - When a tab's queue stays full, the tab is marked unresponsive and the user chooses kill or restart; nothing is auto-killed.
  - Coalescing is decided by one table.
  - E2 and engine-host-side sends are parked.
- Every claim below was re-read against the code at `9fce9d8` (`main`). Line numbers are for orientation only, so find code by the quoted text.
- Paths are under `crates/` unless they start with `docs/`, `scripts/` or `README.md`.
- Wave 3a's held Tasks 4 and 5 (E7) edit `process_manager.rs` `connect_host` and `transport.rs` `accept_*`, which are different hunks from this wave. If they land first, the one test here that calls `transport::accept_local_pair` (Task 2 Step 6) needs their new `peer_pid` argument (`std::process::id()`).

## Global Constraints

**Branch and commits**
- Work on branch `wave-3b-engine-io`, created from `main` (`git switch -c wave-3b-engine-io main`), with no worktree.
- Commit once per task, with the exact message given in its last step. Each implementer ends its message with its own `Co-Authored-By:` line, taken from its system reminder.
- Merge into `main` only after the user's go-ahead (Task 9).

**Formatting and lints**
- Run `cargo fmt --all` before each commit.
- `cargo clippy -j 4 -p manifold -p ipc-protocol -p manifold-engine-webview2 --all-targets -- -D warnings` must pass.
- `.githooks/pre-commit` runs both checks and takes 2+ minutes, so commit in the background. Never use `--no-verify`.
- Don't silence a lint: fix it, or `#[allow(..., reason = "...")]` it.
- An item used only by a later task, or only by tests, is a dead-code error under `-D warnings`. So each task adds only what it uses, and a test-only helper is `#[cfg(test)]` and used by a test in the same task. The tasks below are cut along that rule; an implementer who finds an unused item reports it rather than adding an `allow`.

**Builds and the machine**
- Use `-j 4` on every cargo command (the task brief; it overrides the `-j 8` in CLAUDE.md).
- If cargo waits on the target-dir lock, wait. Never kill another session's process.
- C: is nearly full (37 GB free on 2026-10-10): run `df -h /c` before the first build.
- Before the gate and after the wave, check for leftovers with `Get-Process python, manifold, manifold-engine-webview2 -ErrorAction SilentlyContinue`.
- Each task's last step runs, in order:
  1. `cargo fmt --all`
  2. the clippy line above
  3. `cargo test -j 4 -p manifold -p ipc-protocol`
  4. the commit

**Tests**
- No test may read or write the real `%APPDATA%\manifold`. Any file goes through `crate::store::testutil::temp_dir(name)`. Named pipes use names containing `std::process::id()`.
- A test marked "red first" is run on the unmodified code and seen to fail before the fix is written, with the failure output pasted in the report. A test expected to fail that passes is a finding: stop and report.
- Waiting in a test is on an observable condition with a deadline (`eventually`, `recv_timeout`). The only fixed waits are the ones that prove an absence ("nothing more got through for 300 ms"), and each says so.
- `manifold` is a binary crate. Run one test with `cargo test -j 4 -p manifold --bins <name>`; for `ipc-protocol` use `--lib`.

**Platforms and what can be verified where**
- Windows (this machine): `ipc-protocol`, `manifold` and `manifold-engine-webview2` build, test and clippy here. All of E1 and E3 on the app side is tested here, and the WebView2 host is exercised end to end by Task 8.
- Linux CI (`cargo test -p manifold -p ipc-protocol`) re-runs every test. None is `cfg(windows)`: the stuck-pipe test uses interprocess local sockets (Unix domain sockets on Linux), and its 64 KiB messages fill either OS's buffers.
- `manifold-engine-cef`, `-servo` and `-webkit` do not build on Windows and are not in CI, and no engine crate's source changes.
  - They see two `ipc-protocol` changes: the single-write `send` (Task 1, same signature) and the bounded inbox (Task 7, same `mpsc::Receiver<HostToEngine>` type, same `try_recv` polling).
  - Their compile is therefore unaffected. Their behaviour under the bound is unverified: Task 7 says what to watch for, and the README entry says it.

**Python**
- Never run `python` in any way that can read stdin: no `python -`, no heredoc python, no python call tacked on as a no-op. Write the script to a file and run `python -P f.py < /dev/null`, or `python -P -c '...' args </dev/null` as the probes do. Prefer the Edit tool. Every implementer prompt repeats this rule. Subagents never dispatch subagents.

**Rust**
- Edition 2021, so no let-chains.
- Write `x.is_multiple_of(n)`, never `x % n == 0`, on unsigned integers.
- In a function returning `Option`, use `?`, not `let Some(x) = .. else { return None }`.
- Never format a `HostToEngine::SetCookies` or an `EngineToHost::Cookies` with `{:?}`: they carry cookies. Tests use `matches!` and say "is not Cookies" instead.
- Lock a `Mutex` with `.lock().unwrap_or_else(PoisonError::into_inner)`, as the rest of the crate does.

**Gate**
- `process_manager.rs`, `process_manager/*` and `crates/ipc-protocol/*` are on `scripts/gate-tier.sh`'s `full` list, so the wave needs `scripts/gate.sh LOG full`.
- Run it once, in Task 9, after `cargo build -j 4 -p manifold -p manifold-engine-webview2` (`-p manifold` alone leaves the WebView2 host old), launched detached at BelowNormal: `cmd //c start "" //b //belownormal bash scripts/gate.sh LOG full`.
- Report CPU load and Defender state with the result.

## Decisions resolved by reading the code

1. **Where the host blocks today, exactly.**
   - **Sends.** `ToEngine::send` (`process_manager.rs`, `impl ToEngine`) calls `MessageChannel::send`, which does two `write_all`s and a `flush` (`ipc-protocol/src/channel.rs` lines 27-35, not 32-41 as the survey says). It runs on the UI thread from about 28 `.send(&HostToEngine::…)` sites and 9 `.set_visible(` sites (`grep -rn` over `crates/manifold/src`, tests excluded).
   - **Buffers.** interprocess 2.4.4 creates every local pipe with `input_buffer_size_hint`/`output_buffer_size_hint` of 512 bytes (`os/windows/named_pipe/listener/options.rs:89-90`, used by `local_socket/listener.rs:35`). So a host-to-engine write blocks after about 512 bytes that the engine has not read. WebKit's TCP socket blocks after about 2 MiB (measured).
   - **When the engine stops reading.** Every engine reads its pipe on a dedicated thread (`ipc_protocol::transport::spawn_host_reader`, used by `manifold-engine-cef/src/main.rs:178`, `-servo` `:174`, `-webkit` `:70`, `-webview2` `:156`) into an unbounded `mpsc::channel`. So today a host freezes the app only when that reader thread stops:
     - the whole process is suspended (a debugger, a paused VM or WSL);
     - or the reader is wedged.
     A stuck engine main loop (a native modal, a deadlock) instead grows the engine's memory one message at a time, and the app never learns of it. Task 7 bounds that inbox so the stuck main loop also surfaces as "not responding". It is a separate task because its effect on CEF, Servo and WebKit is unverified (Open question 1).
2. **Why a writer thread rather than nonblocking writes.**
   - A nonblocking socket is shared by its duplicates: `set_nonblocking` on WebKit's write clone made the reader clone return `WouldBlock` (measured), and interprocess has no nonblocking write on a pipe stream.
   - A thread per tab is the one mechanism that works for both transports. It needs no transport change and keeps `MessageChannel` as it is.
3. **What unblocks a stuck writer (measured in a throwaway probe crate, std + interprocess 2.4.4, not in the repo).**
   - **Named pipe.** A write blocked on a full pipe returned `BrokenPipe` (os error 109) 226 µs after the engine's end closed. A killed engine therefore frees its writer at once, and so does `Drop`'s existing `child.kill()`. Pipe writes never partly completed in the probe (0 bytes written, then the error).
   - **TCP (WebKit).** `shutdown(Both)` on a `try_clone` of the socket did not unblock a blocked `send` within 3 s, and neither did closing the duplicate (2 s). Only the peer closing did.
   - So a WebKit tab force-killed while its writer is stuck leaves that thread, its socket and up to `SEND_QUEUE_CAP` queued messages behind until the Linux-side host dies. This is accepted: today the same case freezes the whole app forever. Parked fix: a separate host-to-engine connection (below).
4. **Unresponsive, defined.** A tab is unresponsive while either of these holds:
   - a push was refused because its queue was full and the write in flight has been blocked for `REFUSED_BLOCKED_AFTER` (50 ms). A healthy engine can fall behind a burst of more than 256 unmergeable messages (a long headless `TYPE`) for a moment, and a refusal alone would flag it (final review M3);
   - or one write has been blocked for `UNRESPONSIVE_AFTER` (2 s), whatever the queue holds.

   Either flag clears on the next completed write. The first condition is the user's rule ("queue full sets the flag"). The second is added because coalescing would otherwise hide a stuck tab: pointer moves over a stuck card merge into one queued item, so a hovering user never fills the queue. A writer thread that panicked, or failed to start, also reads as unresponsive (`broken`), because nothing more reaches its engine. A writer that ended on a write error does not: that engine died, and its reader reports `Crashed`, which restarts it as today.
5. **Messages refused while full are lost**, by design of a bounded queue. With `SEND_QUEUE_CAP = 256` plus coalescing, that takes 256 unmerged messages queued behind a stuck write.
   - Dropped `GetCookies` and `QueryPageState` are already covered by existing timeouts (the cookie hold expiry, `expire_pending_sleeps`, `QUIT_PULL_WAIT`).
   - A dropped `Navigate` or keystroke is not replayed. The toast tells the user to restart the tab. The README entry says so.
6. **Ordering.**
   - The queue is strict FIFO. A merge touches only the newest unwritten message, so nothing moves past another message.
   - `SetCookies` is pushed by `spawn_tab` before the `EngineTab` is returned, so it is always the first message after the version and still precedes any `Navigate` (the seed-wait order from E1).
   - `Shutdown` is pushed last (`push_last`, ignoring the cap) and never merges. After `shutdown`, `send` drops everything, so nothing can follow it.
7. **One coalescing table, `coalesce(prev, next)`.**
   - **Subsumes:** runs of `Input(MouseMove)` with equal modifiers (latest wins); runs of `Resize` (latest wins, which covers a window drag); runs of `Input(Scroll)` with equal modifiers (deltas add, position from the latest).
   - **Cannot cover, and why:**
     - Interleaved kinds, such as move, click, move: merging across the click would reorder input.
     - `DownloadProgress`: it runs engine to app; the drain's existing same-path collapse stays.
     - Frames: they get a slot (below), not a queue.
     - `MouseMove` across a modifier change: the page may react to the modifier.
8. **Frames.**
   - The host reads only `width`, `height` and `pixels` of a `FrameReady`. `frame_id` and `damage` are ignored: `drain_engine_ipc` destructures `{ width, height, pixels, .. }` and `handle_engine_message` has `FrameReady { .. } => {}`.
   - The reader therefore stores `Frame { width, height, pixels }` in a per-tab `Arc<Mutex<Option<Frame>>>`, latest wins. This replaces the drain's per-tick `HashMap` dedupe of frames on the channel with a structural one-per-tab bound.
   - WebKit's inline pixels (`pixels: Some(..)`, `manifold-engine-webkit/src/main.rs:556`) are held one frame per tab instead of one per message.
   - Frames do not wake the loop. They are drawn at the next redraw anyway, an idle loop already redraws every `IDLE_FRAME`, and WebView2 captures continuously, so waking on frames would turn every idle tab into a 60 Hz wake source.
9. **The bounded events channel never drops.** The reader blocks on `SyncSender::send` when the channel is full. That cannot deadlock, because after Task 2 the UI thread never waits on an engine: the UI drains, the reader resumes, and the engine meanwhile waits on its pipe.
   - The survey's "never drop Crashed, NavigationStateChanged, Cookies (it says CookiesChanged; the variant is `EngineToHost::Cookies`)" is therefore moot: no non-frame message is ever dropped, `ContextMenuLink` included.
   - When the tab is dropped, the blocked `send` returns `Err` (the receiver is gone) and the thread ends.
10. **The wake.**
    - `UserEvent::EngineMessage` is posted at most once until it is handled. The `AtomicBool` `FramePacing::engine_wake` is swapped to true by the reader and cleared by the handler before it drains.
    - `user_event` calls `mark_active()` for every event today (`event_loop.rs`, first line of `fn user_event`). It must not for this one, or the idle pacing (`IDLE_AFTER`, `is_idle`) would never engage while any engine talks. The drain itself still marks the loop active when it applies a message, exactly as now.
    - The proxy goes into a `Mutex` inside the `Wake`. winit's Windows `EventLoopProxy` is declared `Send` only (`platform_impl/windows/event_loop.rs:820`); it is `Sync` there today only through windows-sys 0.52's `HWND = isize`, and the Linux CI builds a different implementation.
    - The test path (`App::event_proxy == None`, which `spawn_tab_async` already returns early on) never builds a `Wake`. Fake tabs have no reader.
    - No redraw request is needed. winit runs `about_to_wait` after the event, and it recomputes the pacing: a drain that applied a message has called `mark_active`, so `about_to_wait` takes its active branch (`ControlFlow::Poll` plus `request_redraw`); a minimized window drains in `minimized_tick` as today. `drain_tab_events` already runs outside a redraw there, so calling it from `user_event` is not a new kind of call site.
11. **The per-drain cap.** `ENGINE_MESSAGES_PER_DRAIN = 64` per tab per drain. A backlog left over has already marked the loop active (`mark_active` on any applied message), so the next tick comes at the active rate and continues it. `final_cookie_pull` reads `events_rx` directly and is unaffected: frames no longer pass through it, and its own deadline bounds it.
12. **Closing.**
    - **`Drop` for a local engine.** Today it sends `Shutdown`, then kills the host at once. Now:
      1. It reads `stuck = is_unresponsive(now)`.
      2. It calls `shutdown()`.
      3. Unless stuck, it waits `wait_finished(SHUTDOWN_FLUSH)` for the queue and `Shutdown` to go out. The deadline runs from the first `shutdown`.
      4. It kills, removes the frame file and removes the cache dir, all inline as now. Inline still matters, because a respawn reuses `socket_id` (comment in `Drop`).
    - **The kill frees a stuck writer** within about 0.2 ms (Decision 3). The writer's pipe handle closes just after. A same-id respawn's `listen_local_pair_with_retry` already retries for about 0.45 s (10 tries, 50 ms apart) before falling back to `fresh_fallback_socket_id`, so a late close is covered.
    - **App exit.** `exiting` calls `begin_engine_shutdown()` (every live tab's `shutdown()`) right after `final_cookie_pull`. The later `drop(app)` then waits one `SHUTDOWN_FLUSH` for all tabs together, not one per tab. Exit time does not grow with stuck tabs because `begin_engine_shutdown` has set every tab's deadline (`closed_at + SHUTDOWN_FLUSH`) before the first `Drop`, so the waits overlap; the "stuck: skip the wait" rule matters for a single tab closed on its own (final review M1: `stuck` is always false inside `drop(app)`).
    - **WebKit.** The reaper thread is unchanged: up to 3 s for a natural exit, then a kill. Today a stuck WebKit engine would block the UI thread in `Drop`'s `send(Shutdown)`; now it does not.
    - **A spawn still in flight at exit** (a `TabSpawned` event not yet handled) holds an `EngineTab` that `begin_engine_shutdown` cannot reach. It is dropped with the event loop and pays its own `SHUTDOWN_FLUSH` at most, and only if its writer is still busy; a just-connected host has only `SetCookies` queued. `TabPayload` is the only other holder of an `EngineTab` (`grep -rn "EngineTab" crates/manifold/src`).
13. **Restart and End reuse the crash path.**
    - The `Crashed` arm's spawn-building (the mirror's engine, a swap's fallback, `TabSpawn::respawn`) becomes `App::restart_spawn(id)`, and its teardown (remove the node, clear `dragging`/`hovered`, `respawn_tab`) becomes `App::restart_with(id, spawn)`. "Restart Engine" is those two plus a toast, and does not count toward `after_crash`'s limit.
    - "End Engine" is `leave_crashed_card`, renamed `leave_failed_card`, whose `cannot_start: bool` becomes a three-way `LeftClosed` enum: `KeepsCrashing`, `CannotStart` and the new `Ended`. That is one mechanism with three toasts, rather than a second copy of the card teardown. A failed card already restarts on click.
14. **Threads and memory with many tabs.**
    - Each live tab now has two threads (reader and writer), up from one.
    - Bounded memory per tab is at most `SEND_QUEUE_CAP` messages to send plus `EVENTS_CAP` received plus one frame. The only large messages in practice are `SetCookies` and `Cookies` (a jar's JSON); before this wave they were unbounded in count.
    - The writer is spawned with a 256 KiB stack, since it only serializes and writes. 200 tabs is 400 threads, well within Windows limits.
    - If spawning the writer fails, the tab reads as unresponsive (`broken`) instead of panicking on the spawn thread, which would lose the `TabSpawned` event.
15. **Headless mode.** `main.rs` sets `event_proxy` in headless runs too (before `run_app`), and headless runs `ControlFlow::Poll`, so the wake is harmless there and the drain cap applies the same way. Task 8 runs the whole path headless.
16. **The engine-side inbox (Task 7).** `spawn_host_reader` becomes `sync_channel(ENGINE_INBOX = 256)`.
    - The type returned is still `mpsc::Receiver<HostToEngine>`, and every engine polls it with `try_recv` (`wait_for_first_message`, `transport.rs:276-293`, and each main loop), which behaves the same. No engine source changes.
    - On host EOF the reader's final `Shutdown` may wait for space, which only matters to an engine whose main loop is stuck anyway.

## Survey items dropped, stale or parked

- **Stale details corrected:** `channel.rs` send is lines 27-35; the never-drop variant is `Cookies`, not `CookiesChanged`; `spawn_host_reader` is the engine-side reader (E3 lists it in the host blast radius: it is the engine's half, Task 7).
- **Parked, with why** (recorded in the README and the survey by Task 9):
  - **E2, frame-path rework.** The user parked it. This wave only bounds the frame hand-off (one slot per tab); the copy and upload path is unchanged.
  - **Engine-host-side sends.** An engine's main thread writes to the app directly and could block if the app's reader stalled. The user parked it. After this wave the app's reader only blocks on a full events channel, which the UI empties every tick, so the exposure is short.
  - **Engine-side blocking waits** (E9's engine part, the 8 ms `wait_for_first_message` poll and the engines' `try_recv` loops). These are engine source changes, which this wave keeps out.
  - **A separate host-to-engine TCP connection for WebKit**, so a stuck writer can be freed by closing its own socket (Decision 3). This changes the WebKit handshake and is not verifiable here.
  - **A Restart affordance in Tab view.** The badge and the menu live on the Graph card; the toast says "Right-click its card". This is Open question 3.

## Per-task summary

| # | Item | Kind | Model | Depends on | Proof |
|---|---|---|---|---|---|
| 1 | One write per message (`MessageChannel::send`) | mechanical | haiku | none | red-first `a_message_is_one_write`; all `ipc-protocol` tests |
| 2 | `SendQueue`, `coalesce`, `Writer`; `EngineTab` sends and closes through it | judgment | sonnet | 1 | red-first `sends_to_an_engine_that_stopped_reading_never_block`; queue and writer unit tests; stuck-pipe test; quit test |
| 3 | Reader: frame slot, bounded events channel, one generic reader | judgment | sonnet | 2 | red-first `frames_never_queue_on_the_events_channel` and `an_app_that_stops_draining_stops_the_reader_and_loses_nothing` |
| 4 | Wake (`UserEvent::EngineMessage`) and the per-drain cap | judgment | sonnet | 3 | red-first `one_drain_applies_at_most_its_cap_per_tab`; wake coalescing, not-activity and reader-wake tests |
| 5 | Unresponsive: toast, Restart/End, `RESTARTENGINE`/`ENDENGINE`, `EXPORTSTATE` | judgment | sonnet | 4 | toast-once, restart, end, export and parse tests |
| 6 | Menu rows and card badge | mechanical | haiku | 5 | menu test; badge checked by Task 8's screenshot |
| 7 | Engine-side inbox bound (`ENGINE_INBOX`) | mechanical | haiku | 1 | red-first `the_engine_inbox_is_bounded_and_loses_nothing` |
| 8 | `scripts/unresponsive-probe.sh` and the manual run | judgment | sonnet | 6, 7 | the probe's PASS lines, a screenshot looked at |
| 9 | README, survey, Outcome, full gate, merge question | controller | controller | 1 to 8 | `cargo test`, clippy, `scripts/gate.sh LOG full`, CPU and Defender reported |

- **Mechanical tasks (1, 6, 7)** have complete code below; a reviewer checks every step landed and nothing else changed.
- **Judgment tasks** give the design, the code and the tests, and leave small call-site details (imports, an argument threaded through) to the implementer, who reports anything that differs.
- **Reviewers.** Per-task reviewers are sonnet; the final whole-branch review is opus.

---

### Task 1: One write per message

**Model:** haiku. **Kind:** mechanical. **Proof:** the new test fails on today's code (2 writes) and passes after; every existing `channel` and `transport` test passes.

**Files:**
- Modify: `ipc-protocol/src/channel.rs` (`send`, `tests`)

**Interfaces:**
- Produces: nothing new. `MessageChannel::send` keeps its signature and error kinds.
- Consumes: `postcard::to_extend`.

**Blast radius:** every sender in every process: the app (`process_manager.rs`) and all four engines through `ipc-protocol`. The bytes on the wire are identical (a 4-byte little-endian length, then the postcard body). Only the number of `write` calls changes, so a reader sees no difference. The engines are not rebuilt or run by this task; their compile is unaffected (same signature).

- [ ] **Step 1: The test (red first).** In `channel.rs`'s `tests` module, after `a_message_round_trips`:

```rust
    /// Counts `write` calls: one per message, so a writer thread blocked on a full pipe never leaves a length prefix
    /// written without its body (survey E1).
    struct CountWrites {
        writes: usize,
        bytes: Vec<u8>,
    }

    impl Write for CountWrites {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.writes += 1;
            self.bytes.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Read for CountWrites {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Ok(0)
        }
    }

    #[test]
    fn a_message_is_one_write() {
        let mut out = MessageChannel::new(CountWrites { writes: 0, bytes: Vec::new() });
        out.send(&"hello".to_string()).unwrap();
        let sink = out.into_inner();
        assert_eq!(sink.writes, 1, "the length prefix and the body go out in one write");
        assert_eq!(MessageChannel::new(Cursor::new(sink.bytes)).recv::<String>().unwrap(), "hello");
    }
```

(`Read` and `Write` reach the module through `use super::*`; the file imports `std::io::{self, Read, Write}` at the top. If it does not, add `use std::io::{Read, Write};` to the tests module.)

Run `cargo test -j 4 -p ipc-protocol --lib a_message_is_one_write`. Expected: FAIL, `left: 2, right: 1`. Paste the output.

- [ ] **Step 2: The fix.** Replace the body of `send`:

```rust
    pub fn send<T: Serialize>(&mut self, msg: &T) -> io::Result<()> {
        // One buffer, one write: the 4-byte length is reserved up front and filled in after the body.
        let mut frame =
            postcard::to_extend(msg, vec![0u8; 4]).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let len = frame.len() - 4;
        if len > MAX_MESSAGE_BYTES {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "message larger than MAX_MESSAGE_BYTES"));
        }
        frame[..4].copy_from_slice(&(len as u32).to_le_bytes());
        self.stream.write_all(&frame)?;
        self.stream.flush()
    }
```

- [ ] **Step 3: Run** `cargo test -j 4 -p ipc-protocol`. Expected: PASS, including `a_message_round_trips`, `a_version_round_trips_ahead_of_messages`, `a_huge_length_prefix_is_refused_before_allocating` and the `transport` tests.

- [ ] **Step 4: Checks and commit.** The four commands from Global Constraints, then commit: `ipc-protocol: write each message in one write (survey E1)`.

---

### Task 2: `SendQueue`, `coalesce` and `Writer`; `EngineTab` sends and closes through them

**Model:** sonnet. **Kind:** judgment: the lock discipline, the `Finished` guard and `Drop`'s order are the parts to get right. **Proof:**
- The `EngineTab` test fails on today's code: a thread doing 4096 sends of 64 KiB to a fake engine that never reads is still blocked after 5 s. It passes after.
- Six queue tests, five writer tests and the stuck-pipe test pass.
- The quit test sees `Shutdown` reach every fake engine before the app is dropped.

**Files:**
- Create: `manifold/src/process_manager/engine_io.rs`
- Modify:
  - `manifold/src/process_manager.rs`: `mod engine_io;`, delete `ToEngine`; `EngineTab` fields, `send`, `set_visible`, `begin_shutdown`, `fake`, `Drop`; `spawn_tab`'s tail; `spawn_webkit_tab`'s tail
  - `manifold/src/process_manager/tests.rs`
  - `manifold/src/tabs.rs`: `begin_engine_shutdown`, beside `final_cookie_pull`
  - `manifold/src/event_loop.rs`: `exiting`
  - `manifold/src/session_sync/tests.rs`: one test

**Interfaces:**
- Produces:
  - `pub(crate) mod engine_io` holding:
    - constants: `SEND_QUEUE_CAP: usize = 256`, `UNRESPONSIVE_AFTER: Duration = 2 s`, `SHUTDOWN_FLUSH: Duration = 200 ms`
    - `fn coalesce(&HostToEngine, &HostToEngine) -> Option<HostToEngine>`
    - `struct SendQueue` with `push(msg) -> bool` (false when full), `push_last(msg)` and `pop()`
    - `#[derive(Clone)] struct Writer` with `spawn(MessageChannel<S>, name) -> Writer`, `send(HostToEngine)`, `is_unresponsive(Instant) -> bool`, `shutdown()`, `wait_finished(Duration) -> bool`, and `#[cfg(test)] wedge()`
  - On `EngineTab`: `send(&self, &HostToEngine)` (was `&mut self`), `set_visible(&self, bool)` (was `&mut self`), `pub(crate) fn begin_shutdown(&self)`.
  - On `App`: `pub(crate) fn begin_engine_shutdown(&self)`.
- Consumes: `MessageChannel::send` (Task 1).

**Blast radius:**
- `EngineTab::send` and `set_visible` keep their names and argument types; only the receiver loosens to `&self`. The 37 call sites compile unchanged. Some reach the tab through `as_tab_mut()` only to send; that still compiles, so leave them (no clippy lint in the default groups flags it).
- `EngineTab::fake` keeps its signature. It is used by `bench.rs`, `card_kind.rs`, `debug_control.rs` tests, `session_sync/tests.rs` (`succeed_spawn`, `live_tab` and two more) and `process_manager/tests.rs`.
- Fake sends now go through a writer thread. The peer's 200 ms read timeout (`sent(peer)` reads until it) absorbs the thread hand-off, which takes microseconds. If a `sent()` assertion ever flakes, raise that timeout to 1 s rather than adding waits.
- No protocol change.

- [ ] **Step 1: The `EngineTab` test (red first).** Append to `process_manager/tests.rs`:

```rust
/// An engine that stops reading must never block the app: 4096 sends of 64 KiB (far more than a pipe or a loopback
/// socket buffers) return at once (survey E1).
#[test]
fn sends_to_an_engine_that_stopped_reading_never_block() {
    let (mut tab, _peer) = EngineTab::fake(EngineKind::Servo);
    let (done_tx, done_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let big = HostToEngine::SetCookies("x".repeat(64 * 1024));
        for _ in 0..4096 {
            tab.send(&big);
        }
        let _ = done_tx.send(tab);
    });
    let tab = done_rx.recv_timeout(Duration::from_secs(5)).expect("the sends returned although the engine read none");
    drop(tab);
}
```

Run `cargo test -j 4 -p manifold --bins sends_to_an_engine_that_stopped_reading_never_block`. Expected: FAIL after 5 s with "the sends returned although the engine read none" (the sending thread stays blocked in `write_all`; the test binary still exits). Paste the output.

- [ ] **Step 2: Create `process_manager/engine_io.rs`.**

```rust
//! The app's side of an engine's control channel, off the UI thread (survey E1).
//!
//! `EngineTab::send` only queues (`SendQueue`); one writer thread per tab (`Writer`) writes the queue out, so the UI
//! thread never waits on an engine. While the writer is behind, a new message merges into the newest queued one where
//! only the result matters (`coalesce`). A tab whose writer is stuck reads as unresponsive (`Writer::is_unresponsive`)
//! and the user restarts or ends it; nothing here kills an engine.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use ipc_protocol::{HostToEngine, InputEvent, MessageChannel};

/// Messages a tab's send queue holds before it refuses more (the tab is then unresponsive).
pub(crate) const SEND_QUEUE_CAP: usize = 256;
/// How long one write may stay blocked before the tab reads as unresponsive.
pub(crate) const UNRESPONSIVE_AFTER: Duration = Duration::from_secs(2);
/// How long closing a tab waits for its queue and `Shutdown` to go out, counted from the first `Writer::shutdown`.
pub(crate) const SHUTDOWN_FLUSH: Duration = Duration::from_millis(200);
/// The writer only serializes and writes.
const WRITER_STACK: usize = 256 * 1024;

/// `next` folded into `prev` when the engine needs only the result: the latest pointer position or size, or the sum of
/// a run of wheel steps. `None` keeps both. Only the queue's newest unwritten message is ever `prev`, so a merge never
/// moves a message past another one. A modifier change ends a run (the page may react to it).
pub(crate) fn coalesce(prev: &HostToEngine, next: &HostToEngine) -> Option<HostToEngine> {
    use HostToEngine::{Input, Resize};
    use InputEvent::{MouseMove, Scroll};
    match (prev, next) {
        (Input(MouseMove { modifiers: a, .. }), Input(MouseMove { modifiers: b, .. })) if a == b => Some(next.clone()),
        (Resize { .. }, Resize { .. }) => Some(next.clone()),
        (
            Input(Scroll { delta_x: dx, delta_y: dy, modifiers: a, .. }),
            Input(Scroll { x, y, delta_x, delta_y, modifiers: b }),
        ) if a == b => {
            Some(Input(Scroll { x: *x, y: *y, delta_x: dx + delta_x, delta_y: dy + delta_y, modifiers: *b }))
        }
        _ => None,
    }
}

/// A bounded FIFO of messages not yet written.
pub(crate) struct SendQueue {
    items: VecDeque<HostToEngine>,
    cap: usize,
}

impl Default for SendQueue {
    fn default() -> Self {
        Self { items: VecDeque::new(), cap: SEND_QUEUE_CAP }
    }
}

impl SendQueue {
    /// Queues `msg`, or merges it into the newest queued message (`coalesce`, allowed even when full). False when
    /// the queue is full and `msg` was dropped.
    pub(crate) fn push(&mut self, msg: HostToEngine) -> bool {
        if let Some(last) = self.items.back_mut() {
            if let Some(merged) = coalesce(last, &msg) {
                *last = merged;
                return true;
            }
        }
        if self.items.len() >= self.cap {
            return false;
        }
        self.items.push_back(msg);
        true
    }

    /// Queues `msg` last whatever the cap: `Shutdown`, which nothing follows.
    pub(crate) fn push_last(&mut self, msg: HostToEngine) {
        self.items.push_back(msg);
    }

    pub(crate) fn pop(&mut self) -> Option<HostToEngine> {
        self.items.pop_front()
    }
}

#[derive(Default)]
struct State {
    queue: SendQueue,
    /// `shutdown` was called: no more messages are taken.
    closed_at: Option<Instant>,
    /// The thread has ended: it wrote `Shutdown`, a write failed, or it panicked.
    finished: bool,
    /// Nothing will reach the engine although it may be alive: the thread panicked or never started.
    broken: bool,
    /// When the write under way started.
    writing_since: Option<Instant>,
    /// A push was refused since the last completed write.
    refused: bool,
}

impl State {
    fn unresponsive(&self, now: Instant) -> bool {
        self.broken
            || (self.closed_at.is_none()
                && (self.refused || self.writing_since.is_some_and(|t| now.duration_since(t) >= UNRESPONSIVE_AFTER)))
    }
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    /// Signalled on every change: a push (the thread waits for work) and a finish (`wait_finished`).
    changed: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// One tab's send queue and the thread that writes it out.
#[derive(Clone)]
pub(crate) struct Writer(Arc<Shared>);

impl Writer {
    /// Starts the thread writing to `channel`. If it cannot start, the writer is `broken` from the outset (the tab
    /// reads as unresponsive) rather than panicking on the spawn thread.
    pub(crate) fn spawn<S: Read + Write + Send + 'static>(channel: MessageChannel<S>, name: &str) -> Writer {
        let writer = Writer(Arc::default());
        let shared = Arc::clone(&writer.0);
        let started = std::thread::Builder::new()
            .name(name.to_owned())
            .stack_size(WRITER_STACK)
            .spawn(move || write_queued(channel, &shared));
        if let Err(e) = started {
            eprintln!("{name}: could not start: {e}");
            let mut state = writer.0.lock();
            state.finished = true;
            state.broken = true;
        }
        writer
    }

    /// Queues `msg`; never blocks. Dropped once closed or finished, and when the queue is full (the tab then reads as
    /// unresponsive).
    pub(crate) fn send(&self, msg: HostToEngine) {
        let mut state = self.0.lock();
        if state.finished || state.closed_at.is_some() {
            return;
        }
        if !state.queue.push(msg) {
            state.refused = true;
        }
        drop(state);
        self.0.changed.notify_all();
    }

    /// A refused push, or one write blocked for `UNRESPONSIVE_AFTER`, since the last completed write; or a broken
    /// writer. Never while closing (`shutdown`).
    pub(crate) fn is_unresponsive(&self, now: Instant) -> bool {
        self.0.lock().unresponsive(now)
    }

    /// Queues `Shutdown` after everything queued and takes nothing more. Repeat calls do nothing.
    pub(crate) fn shutdown(&self) {
        let mut state = self.0.lock();
        if state.closed_at.is_some() {
            return;
        }
        state.closed_at = Some(Instant::now());
        state.queue.push_last(HostToEngine::Shutdown);
        drop(state);
        self.0.changed.notify_all();
    }

    /// Waits until the thread has ended, at most until `flush` after the first `shutdown` (or after now, if none).
    /// True when it ended.
    pub(crate) fn wait_finished(&self, flush: Duration) -> bool {
        let mut state = self.0.lock();
        let deadline = state.closed_at.unwrap_or_else(Instant::now) + flush;
        while !state.finished {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            state = self.0.changed.wait_timeout(state, deadline - now).unwrap_or_else(PoisonError::into_inner).0;
        }
        true
    }

    /// Fills the queue of a writer whose engine end is never read, until it reads as unresponsive.
    #[cfg(test)]
    pub(crate) fn wedge(&self) {
        let big = HostToEngine::SetCookies("x".repeat(64 * 1024));
        for _ in 0..4096 {
            if self.is_unresponsive(Instant::now()) {
                return;
            }
            self.send(big.clone());
        }
        panic!("4096 sends of 64 KiB never filled the queue: is the engine end being read?");
    }
}

/// Marks the writer finished when its thread ends, a panic included (then also `broken`).
struct Finished<'a>(&'a Shared);

impl Drop for Finished<'_> {
    fn drop(&mut self) {
        let mut state = self.0.lock();
        state.finished = true;
        state.writing_since = None;
        if std::thread::panicking() {
            state.broken = true;
        }
        drop(state);
        self.0.changed.notify_all();
    }
}

/// The writer thread: writes the queue in order, never holding the lock while writing. Ends after `Shutdown` or on
/// the first failed write (the engine is gone; its reader reports the crash).
fn write_queued<S: Read + Write>(mut channel: MessageChannel<S>, shared: &Shared) {
    let _finished = Finished(shared);
    loop {
        let msg = {
            let mut state = shared.lock();
            loop {
                if let Some(msg) = state.queue.pop() {
                    state.writing_since = Some(Instant::now());
                    break msg;
                }
                state = shared.changed.wait(state).unwrap_or_else(PoisonError::into_inner);
            }
        };
        let last = matches!(msg, HostToEngine::Shutdown);
        let written = channel.send(&msg).is_ok();
        {
            let mut state = shared.lock();
            state.writing_since = None;
            state.refused = false;
        }
        if !written || last {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use std::sync::mpsc;

    fn url(n: usize) -> HostToEngine {
        HostToEngine::Navigate { url: n.to_string(), referrer: None }
    }

    fn urls(msgs: &[HostToEngine]) -> Vec<usize> {
        msgs.iter()
            .filter_map(|m| match m {
                HostToEngine::Navigate { url, .. } => url.parse().ok(),
                _ => None,
            })
            .collect()
    }

    fn mouse(x: f64, modifiers: u8) -> HostToEngine {
        HostToEngine::Input(InputEvent::MouseMove { x, y: 0.0, modifiers })
    }

    fn scroll(delta_y: f64) -> HostToEngine {
        HostToEngine::Input(InputEvent::Scroll { x: 1.0, y: 2.0, delta_x: 0.0, delta_y, modifiers: 0 })
    }

    fn resize(width: u32) -> HostToEngine {
        HostToEngine::Resize { width, height: 10, dpi_scale: 1.0 }
    }

    fn drain(queue: &mut SendQueue) -> Vec<HostToEngine> {
        std::iter::from_fn(|| queue.pop()).collect()
    }

    #[test]
    fn messages_leave_in_the_order_they_were_sent() {
        let mut queue = SendQueue::default();
        for n in 0..5 {
            assert!(queue.push(url(n)));
        }
        assert_eq!(urls(&drain(&mut queue)), [0, 1, 2, 3, 4]);
    }

    #[test]
    fn a_run_of_mouse_moves_or_resizes_keeps_only_the_latest() {
        let mut queue = SendQueue::default();
        for x in 0..10u8 {
            queue.push(mouse(f64::from(x), 0));
        }
        for width in 0..10 {
            queue.push(resize(width));
        }
        let out = drain(&mut queue);
        assert_eq!(out.len(), 2);
        assert!(matches!(out[0], HostToEngine::Input(InputEvent::MouseMove { x, .. }) if x == 9.0));
        assert!(matches!(out[1], HostToEngine::Resize { width: 9, .. }));
    }

    #[test]
    fn a_run_of_scrolls_adds_up() {
        let mut queue = SendQueue::default();
        for _ in 0..4 {
            queue.push(scroll(-1.5));
        }
        let out = drain(&mut queue);
        assert_eq!(out.len(), 1);
        assert!(matches!(
            out[0],
            HostToEngine::Input(InputEvent::Scroll { x, y, delta_y, .. }) if delta_y == -6.0 && x == 1.0 && y == 2.0
        ));
    }

    #[test]
    fn nothing_merges_across_another_message_or_a_modifier_change() {
        let mut queue = SendQueue::default();
        for msg in [mouse(1.0, 0), url(0), mouse(2.0, 0), mouse(3.0, 1), HostToEngine::Shutdown, resize(1), resize(2)] {
            queue.push(msg);
        }
        // Only the two resizes at the end merge.
        assert_eq!(drain(&mut queue).len(), 6);
    }

    #[test]
    fn a_full_queue_refuses_new_messages_but_still_merges_into_its_last() {
        let mut queue = SendQueue { items: VecDeque::new(), cap: 2 };
        assert!(queue.push(url(0)) && queue.push(mouse(1.0, 0)));
        assert!(!queue.push(url(1)), "full");
        assert!(queue.push(mouse(2.0, 0)), "a move still merges into the newest");
        let out = drain(&mut queue);
        assert_eq!(out.len(), 2);
        assert!(matches!(out[1], HostToEngine::Input(InputEvent::MouseMove { x, .. }) if x == 2.0));
    }

    #[test]
    fn shutdown_goes_last_even_when_full() {
        let mut queue = SendQueue { items: VecDeque::new(), cap: 1 };
        assert!(queue.push(resize(1)));
        queue.push_last(HostToEngine::Shutdown);
        assert!(matches!(drain(&mut queue).as_slice(), [HostToEngine::Resize { .. }, HostToEngine::Shutdown]));
    }

    /// An engine end whose every write waits for a permit: no permits, a stuck engine; the sender dropped, a dead one.
    struct Gate {
        permits: mpsc::Receiver<()>,
        written: Arc<Mutex<Vec<u8>>>,
        panic_on_write: bool,
    }

    impl Write for Gate {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            assert!(!self.panic_on_write, "a writer bug");
            self.permits.recv().map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
            self.written.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Read for Gate {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Ok(0)
        }
    }

    fn gated(panic_on_write: bool) -> (Writer, mpsc::Sender<()>, Arc<Mutex<Vec<u8>>>) {
        let (permit, permits) = mpsc::channel();
        let written = Arc::new(Mutex::new(Vec::new()));
        let gate = Gate { permits, written: Arc::clone(&written), panic_on_write };
        (Writer::spawn(MessageChannel::new(gate), "test writer"), permit, written)
    }

    /// Every message in `bytes`, in order.
    fn decode(bytes: Vec<u8>) -> Vec<HostToEngine> {
        let mut channel = MessageChannel::new(io::Cursor::new(bytes));
        std::iter::from_fn(|| channel.recv().ok()).collect()
    }

    /// Polls `done` until it holds or `within` passes.
    fn eventually(within: Duration, done: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + within;
        while Instant::now() < deadline {
            if done() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        done()
    }

    #[test]
    fn sends_never_block_and_a_write_stuck_for_unresponsive_after_is_reported() {
        let (writer, _permit, _) = gated(false);
        let start = Instant::now();
        for n in 0..10 {
            writer.send(url(n));
        }
        assert!(start.elapsed() < Duration::from_secs(1), "ten sends to a stuck engine returned at once");
        assert!(
            eventually(Duration::from_secs(5), || writer.is_unresponsive(Instant::now() + UNRESPONSIVE_AFTER)),
            "the first write is under way and stuck"
        );
        assert!(!writer.is_unresponsive(Instant::now()), "not before UNRESPONSIVE_AFTER: the queue is not full");
    }

    #[test]
    fn a_full_queue_is_unresponsive_at_once_and_recovers_in_order() {
        let (writer, permit, written) = gated(false);
        let sent = SEND_QUEUE_CAP + 10;
        for n in 0..sent {
            writer.send(url(n));
        }
        assert!(writer.is_unresponsive(Instant::now()), "a refused message marks the tab at once");
        for _ in 0..sent + 2 {
            permit.send(()).unwrap();
        }
        assert!(eventually(Duration::from_secs(5), || !writer.is_unresponsive(Instant::now())), "a write clears it");
        writer.send(url(sent));
        writer.shutdown();
        assert!(writer.wait_finished(Duration::from_secs(5)));
        let out = decode(std::mem::take(&mut *written.lock().unwrap()));
        let got = urls(&out);
        assert!(got.windows(2).all(|w| w[0] < w[1]), "order kept: {got:?}");
        assert!(got.len() < sent, "some were refused while full");
        assert_eq!(got.last(), Some(&sent), "a message sent after the recovery arrives");
        assert!(matches!(out.last(), Some(HostToEngine::Shutdown)));
    }

    #[test]
    fn closing_waits_at_most_its_deadline() {
        let (writer, permit, _) = gated(false);
        writer.send(url(0));
        writer.shutdown();
        let start = Instant::now();
        assert!(!writer.wait_finished(Duration::from_millis(100)), "the engine is stuck");
        assert!(start.elapsed() < Duration::from_secs(2));
        writer.send(url(1)); // after shutdown: dropped, and never blocks
        drop(permit);
        assert!(writer.wait_finished(Duration::from_secs(5)), "the write failed, so the thread ended");
    }

    #[test]
    fn a_dead_engine_ends_the_writer_and_is_not_unresponsive() {
        let (writer, permit, _) = gated(false);
        writer.send(url(0));
        drop(permit); // the engine died: the write fails
        // No `shutdown`: `wait_finished` then waits `flush` from now.
        assert!(writer.wait_finished(Duration::from_secs(5)), "the write failed, so the thread ended");
        assert!(
            !writer.is_unresponsive(Instant::now() + UNRESPONSIVE_AFTER),
            "a dead engine is a crash for its reader to report, not a stuck one"
        );
        writer.send(url(1)); // a finished writer drops it
    }

    #[test]
    fn a_writer_that_panicked_is_unresponsive() {
        let (writer, _permit, _) = gated(true);
        writer.send(url(0));
        assert!(eventually(Duration::from_secs(5), || writer.is_unresponsive(Instant::now())));
    }
}
```

Notes for the implementer:
- `x == 9.0` style float comparisons are exact here, since the values are small integers. `clippy::float_cmp` is in the pedantic group, which this workspace does not enable. If clippy flags them anyway, compare `x.to_bits()` with `9.0f64.to_bits()` instead.
- `a_writer_that_panicked_is_unresponsive` prints a panic message from the writer thread to stderr. That is expected; the test passes.
- The test-only `Writer::wedge` is used by Step 6's test in this same task.

- [ ] **Step 3: `EngineTab` on the writer.** In `process_manager.rs`:

1. Next to `mod tests;` at the bottom, add `pub(crate) mod engine_io;`.
2. Delete `enum ToEngine` and `impl ToEngine`, including their doc comment, which is still accurate. Move its one fact ("Local: shared memory for frames; Remote: TCP loopback, frames inline in `FrameReady`") onto the `writer` field's doc below.
3. In `pub struct EngineTab`, replace `to_engine: ToEngine,` with:

```rust
    /// Sends to the engine-host off the UI thread (`engine_io::Writer`), over its local pipe or, for the WSL2-hosted
    /// WebKit, its TCP connection (frames then travel inline in `FrameReady`).
    writer: engine_io::Writer,
```

4. Replace `send` and `set_visible`, and add `begin_shutdown`:

```rust
    /// Queues `msg` for the engine and returns at once. A run of pointer moves, resizes or wheel steps the engine has
    /// not read yet merges into one (`engine_io::coalesce`). Dropped when the engine has stopped reading and its queue
    /// is full (the tab then reads as unresponsive), and after `Shutdown`.
    pub fn send(&self, msg: &HostToEngine) {
        self.writer.send(msg.clone());
    }

    /// M5: at most one tab is ever visible (actively capturing/sending
    /// frames) at a time — see `HostToEngine::SetVisible`'s doc comment.
    pub fn set_visible(&self, visible: bool) {
        self.send(&HostToEngine::SetVisible(visible));
    }

    /// Queues `Shutdown` now. An app exit calls this on every tab before dropping them, so `Drop` then waits one
    /// `engine_io::SHUTDOWN_FLUSH` for all of them together.
    pub(crate) fn begin_shutdown(&self) {
        self.writer.shutdown();
    }
```

5. In `fake`, replace `let to_engine = ToEngine::Remote(RemoteTransport::new(host));` and the `to_engine,` field with `writer: engine_io::Writer::spawn(RemoteTransport::new(host), "engine writer (fake)"),`.
6. Replace `impl Drop for EngineTab` with:

```rust
impl Drop for EngineTab {
    fn drop(&mut self) {
        // Read before `shutdown`, after which a writer never reads as unresponsive.
        let stuck = self.writer.is_unresponsive(std::time::Instant::now());
        self.writer.shutdown();
        let Some(child) = self.child.take() else { return };
        let (cache_dir, slot) = (self.cache_dir.take(), self.profile_slot.take());
        if self.kind == EngineKind::Webkit {
            // Off the UI thread: a WebKit host gets up to 3 s to exit on its own,
            // and dropping a session of N of them used to freeze the window for 3N s.
            // It has no frame file (frames arrive over TCP) and no cache dir.
            // A writer stuck on its socket stays blocked until the Linux side dies: closing the
            // socket on this side does not unblock a send on Windows (measured, wave 3b).
            let reaper = std::thread::spawn(move || reap_host(EngineKind::Webkit, child));
            CACHE_REMOVALS.lock().unwrap_or_else(|e| e.into_inner()).push(reaper);
            return;
        }
        // The queue and `Shutdown` get until `SHUTDOWN_FLUSH` after the first `shutdown` to go out.
        // A stuck engine is not waited for: the kill below breaks its pipe, which ends the blocked
        // write too (measured: about 0.2 ms after the engine's end closes).
        if !stuck {
            self.writer.wait_finished(engine_io::SHUTDOWN_FLUSH);
        }
        // A local host is killed at once, so this stays inline. It must: a respawn
        // of the same tab reuses `socket_id`, and Windows deletes a file even while
        // another process maps it (measured), so a delete that ran late on another
        // thread could take the new host's frame file.
        reap_host(self.kind, child);
        // The host has exited, so its frame file is no longer mapped by it.
        ipc_protocol::remove_frame_file(&self.socket_id);
        // The profile slot is freed after the folder's deletion is tried.
        if let Some(dir) = cache_dir {
            remove_cache_dir(dir, slot);
        }
    }
}
```

7. In `spawn_tab`'s tail, replace the `let mut tab = EngineTab { … to_engine: ToEngine::Local(to_engine), … };` block with:

```rust
    let writer = engine_io::Writer::spawn(to_engine, &format!("engine writer {socket_id}"));
    let tab = EngineTab { kind, socket_id, writer, events_rx, child: Some(child), cache_dir, profile_slot };
```

   The `SetCookies` send that follows stays as it is: it is the first message queued, so it is the first written.
8. In `spawn_webkit_tab`'s tail, replace `to_engine: ToEngine::Remote(to_engine),` with `writer: engine_io::Writer::spawn(to_engine, &format!("engine writer {tab_id}")),`. Update the `socket_id` field comment's "see ToEngine::Remote/RemoteTransport above" to "see `RemoteTransport`".
9. Run `grep -n "ToEngine\b\|to_engine" crates/manifold/src/process_manager.rs`. Expected: only the local variables `to_engine` (from `connect_host` and the WebKit `MessageChannel::new(stream)`) remain.

- [ ] **Step 4: The `EngineTab` test, on the new API.** Change `let (mut tab, _peer)` to `let (tab, _peer)`. Replace the `drop(tab);` line with:

```rust
    assert!(tab.writer.is_unresponsive(std::time::Instant::now()), "its queue filled, so it reads as unresponsive");
    let start = std::time::Instant::now();
    drop(tab);
    assert!(start.elapsed() < Duration::from_secs(1), "closing a stuck tab does not wait on it");
```

(`tab.writer` is private, but `tests` is a child module of `process_manager`, so it can read it.)

Run `cargo test -j 4 -p manifold --bins sends_to_an_engine_that_stopped_reading_never_block`. Expected: PASS.

- [ ] **Step 5: The quit path.**
- In `tabs.rs`, after `final_cookie_pull`, add:

```rust
    /// Starts every live tab's engine shutdown at once (`EngineTab::begin_shutdown`), so dropping the app waits one
    /// `SHUTDOWN_FLUSH` for all of them rather than one per tab, and a stuck engine adds nothing.
    pub(crate) fn begin_engine_shutdown(&self) {
        for node in self.nodes.values() {
            if let Some(tab) = node.as_tab() {
                tab.tab.begin_shutdown();
            }
        }
    }
```

- In `event_loop.rs` `fn exiting`, after `self.final_cookie_pull();`, add `self.begin_engine_shutdown();`.
- In `session_sync/tests.rs`, after `fn sent`, add:

```rust
/// Quitting starts every engine's shutdown before the app is dropped: each fake host is sent `Shutdown` while the
/// app is still alive.
#[test]
fn quitting_sends_every_engine_shutdown_before_the_drop() {
    let mut app = app_on(&temp_dir("quit-shutdown").join("session.mnf"), 1);
    let (_, mut a) = live_tab(&mut app, EngineKind::Servo, "https://a/");
    let (_, mut b) = live_tab(&mut app, EngineKind::Servo, "https://b/");
    app.begin_engine_shutdown();
    for peer in [&mut a, &mut b] {
        assert!(matches!(sent(peer).last(), Some(HostToEngine::Shutdown)));
    }
    drop(app);
}
```

Run `cargo test -j 4 -p manifold --bins quitting_sends_every_engine_shutdown_before_the_drop`. Expected: PASS. Then check it is not vacuous: comment out the `begin_engine_shutdown` call, rerun, see it fail, and restore the call.

- [ ] **Step 6: A writer stuck on a real local pipe.** Append to `process_manager/tests.rs`:

```rust
/// A writer stuck on a local pipe its engine stopped reading ends when the engine's end closes, as killing the engine
/// closes it (`Drop` relies on this; measured on Windows at about 0.2 ms).
#[test]
fn a_writer_stuck_on_a_local_pipe_ends_when_the_engine_end_closes() {
    let id = format!("test-stuck-writer-{}", std::process::id());
    let pair = transport::listen_local_pair(&id).unwrap();
    let engine = std::thread::spawn(move || transport::connect_local_pair(&id).unwrap());
    let (to_engine, _from_engine) = transport::accept_local_pair(&pair, Duration::from_secs(5)).unwrap();
    let engine_end = engine.join().unwrap();
    let writer = engine_io::Writer::spawn(to_engine, "test writer");
    writer.wedge();
    writer.shutdown();
    assert!(!writer.wait_finished(Duration::from_millis(100)), "stuck while the engine end is open");
    drop(engine_end);
    assert!(writer.wait_finished(Duration::from_secs(5)), "the blocked write failed once the engine end closed");
}
```

If wave 3a's E7 has landed, `accept_local_pair` takes the peer pid: pass `std::process::id()`. Run it. Expected: PASS.

- [ ] **Step 7: Checks and commit.** The four commands from Global Constraints, then commit: `engines: send through a per-tab writer thread; a stuck engine no longer freezes the app (survey E1)`.

---

### Task 3: Reader: frame slot, bounded events channel, one generic reader

**Model:** sonnet. **Kind:** judgment. **Proof:**
- On today's code, a burst of 100 frames ahead of a `Cookies` makes `FrameReady` the first event on the channel, and an app that reads nothing lets an engine write all 1000 of its 64 KiB messages. Both tests fail there.
- After the change, `Cookies` comes first, the slot holds the last frame, the engine stalls before 1000, and all 1000 arrive in order.

**Files:**
- Modify: `manifold/src/process_manager/engine_io.rs`: `EVENTS_CAP`, `Frame`, `FrameSlot`
- Modify: `manifold/src/process_manager.rs`:
  - `spawn_reader_thread`; delete `RemoteOrLocalReceiver`
  - the `frame` field and `take_frame`
  - `spawn_tab`, `spawn_webkit_tab` and `fake`
- Modify: `manifold/src/event_loop.rs`: `drain_engine_ipc`'s frame arm
- Modify: `manifold/src/process_manager/tests.rs`

**Interfaces:**
- Produces:
  - `engine_io::EVENTS_CAP: usize = 256`
  - `engine_io::Frame { width: u32, height: u32, pixels: Option<Vec<u8>> }`
  - `engine_io::FrameSlot = Arc<Mutex<Option<Frame>>>`
  - `EngineTab::take_frame(&self) -> Option<Frame>`
  - `spawn_reader_thread<S: Read + Write + Send + 'static>(MessageChannel<S>, FrameSlot) -> mpsc::Receiver<EngineToHost>`
- Consumes: Task 2's `EngineTab`.

**Blast radius:**
- `events_rx` keeps its type (`mpsc::Receiver<EngineToHost>`; `sync_channel` returns the same type). So `final_cookie_pull` and `drain_engine_ipc` compile unchanged, apart from the frame arm.
- No `FrameReady` reaches `events_rx` any more. The drain's `FrameReady` arm moves to `take_frame`, and `handle_engine_message`'s `FrameReady { .. } => {}` arm stays because the match must be exhaustive.
- No protocol change.

- [ ] **Step 1: The tests (red first).** In `process_manager/tests.rs`:
- Replace `first_event_after_version` with a shared helper plus the old function on top of it.
- Add the two tests below.
- At the top, add `use std::sync::atomic::{AtomicUsize, Ordering};` and `use std::sync::Arc;` if missing. (`process_manager.rs` imports `std::sync::atomic::{AtomicU64, Ordering}` and `std::sync::mpsc`, and `use super::*` brings those in.)

```rust
/// The host's reader of one engine connection; `feed` writes the engine's side of it, its version first.
fn reader_fed(feed: impl FnOnce(std::net::TcpStream) + Send + 'static) -> mpsc::Receiver<EngineToHost> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || feed(std::net::TcpStream::connect(("127.0.0.1", port)).unwrap()));
    let (stream, _) = listener.accept().unwrap();
    spawn_reader_thread(RemoteOrLocalReceiver::Remote(ipc_protocol::MessageChannel::new(stream)))
}

/// The host's reader of one engine connection, fed `version` and then a
/// well-formed message; returns the first event it delivers.
fn first_event_after_version(version: u32) -> EngineToHost {
    use std::io::Write as _;
    let rx = reader_fed(move |mut stream| {
        stream.write_all(&version.to_le_bytes()).unwrap();
        // Best effort: on a mismatch the reader may already have hung up.
        let _ = ipc_protocol::MessageChannel::new(stream).send(&EngineToHost::Cookies("[]".into()));
    });
    rx.recv_timeout(Duration::from_secs(5)).unwrap()
}

fn frame(n: u32) -> EngineToHost {
    EngineToHost::FrameReady { frame_id: u64::from(n), width: n, height: 1, damage: None, pixels: None }
}

/// Frames never wait in the events channel (survey E3): a burst of them ahead of a message leaves the message first.
#[test]
fn frames_never_queue_on_the_events_channel() {
    let rx = reader_fed(|stream| {
        let mut engine = ipc_protocol::MessageChannel::new(stream);
        engine.send_version().unwrap();
        for n in 0..100 {
            engine.send(&frame(n)).unwrap();
        }
        engine.send(&EngineToHost::Cookies("[]".into())).unwrap();
    });
    assert!(matches!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), EngineToHost::Cookies(_)), "a frame came first");
}

/// `counter` once it has not moved for `quiet`, or at `deadline`. The `quiet` sleep proves an absence: nothing more
/// got through.
fn still_at(counter: &AtomicUsize, quiet: Duration, deadline: Duration) -> usize {
    let end = std::time::Instant::now() + deadline;
    let mut last = counter.load(Ordering::SeqCst);
    while std::time::Instant::now() < end {
        std::thread::sleep(quiet);
        let now = counter.load(Ordering::SeqCst);
        if now == last {
            return now;
        }
        last = now;
    }
    last
}

/// The events channel is bounded (survey E3): an app that stops draining it stops the reader, which stops the engine
/// on its pipe, and no message is lost.
#[test]
fn an_app_that_stops_draining_stops_the_reader_and_loses_nothing() {
    const N: usize = 1000;
    let written = Arc::new(AtomicUsize::new(0));
    let progress = Arc::clone(&written);
    let rx = reader_fed(move |stream| {
        let mut engine = ipc_protocol::MessageChannel::new(stream);
        engine.send_version().unwrap();
        for i in 0..N {
            if engine.send(&EngineToHost::Cookies(format!("{i:0>65536}"))).is_err() {
                return;
            }
            progress.store(i + 1, Ordering::SeqCst);
        }
    });
    let stalled_at = still_at(&written, Duration::from_millis(300), Duration::from_secs(20));
    assert!(stalled_at < N, "the engine wrote all {N} while the app read none: the channel is unbounded");
    for i in 0..N {
        match rx.recv_timeout(Duration::from_secs(10)).unwrap() {
            EngineToHost::Cookies(s) => assert_eq!(s.trim_start_matches('0').parse::<usize>().unwrap_or(0), i),
            _ => panic!("message {i} is not Cookies"),
        }
    }
}
```

Run `cargo test -j 4 -p manifold --bins -- frames_never_queue_on_the_events_channel an_app_that_stops_draining_stops_the_reader_and_loses_nothing a_stale_engine_host_is_a_protocol_mismatch_crash`. Expected:
- the first test FAILS with "a frame came first";
- the second FAILS with "the channel is unbounded";
- the third PASSES (the helper refactor kept it working).

Paste the output.

- [ ] **Step 2: `engine_io` types.** Add, below `SHUTDOWN_FLUSH`:

```rust
/// Engine messages, frames aside, waiting for the app (`process_manager::spawn_reader_thread`). Full, the reader
/// waits and the engine backs up on its own pipe; no message is dropped (survey E3).
pub(crate) const EVENTS_CAP: usize = 256;

/// The newest frame an engine reported. Only the newest is ever drawn, so a slow app holds one per tab, never a
/// queue of them (survey E3). The host never reads `FrameReady`'s `frame_id` or `damage`.
pub(crate) struct Frame {
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// Inline pixels (WebKit over TCP); `None` for a local engine, whose pixels are in its shared frame file.
    pub(crate) pixels: Option<Vec<u8>>,
}

/// One tab's newest frame, written by its reader thread and taken by the drain.
pub(crate) type FrameSlot = Arc<Mutex<Option<Frame>>>;
```

- [ ] **Step 3: The reader.** In `process_manager.rs`, replace `spawn_reader_thread`'s signature and body, keeping its doc comment and adding one paragraph to it, and delete `enum RemoteOrLocalReceiver`:

```rust
/// Frames go to `frame` (newest wins); everything else goes through a channel of `engine_io::EVENTS_CAP`, whose
/// `send` waits while the app is that far behind (survey E3).
fn spawn_reader_thread<S: std::io::Read + std::io::Write + Send + 'static>(
    mut from_engine: ipc_protocol::MessageChannel<S>,
    frame: engine_io::FrameSlot,
) -> mpsc::Receiver<EngineToHost> {
    let (tx, rx) = mpsc::sync_channel(engine_io::EVENTS_CAP);
    std::thread::spawn(move || {
        let lost = || EngineToHost::Crashed { reason: "engine-host connection lost".to_string() };
        match from_engine.recv_version().map(ipc_protocol::check_version) {
            Ok(Ok(())) => {}
            Ok(Err(mismatch)) => {
                eprintln!("{mismatch}");
                let _ = tx.send(EngineToHost::Crashed { reason: mismatch });
                return;
            }
            Err(_) => {
                let _ = tx.send(lost());
                return;
            }
        }
        while let Ok(msg) = from_engine.recv::<EngineToHost>() {
            match msg {
                EngineToHost::FrameReady { width, height, pixels, .. } => {
                    *frame.lock().unwrap_or_else(std::sync::PoisonError::into_inner) =
                        Some(engine_io::Frame { width, height, pixels });
                }
                // Fails once the tab is gone (its receiver dropped).
                msg => {
                    if tx.send(msg).is_err() {
                        return;
                    }
                }
            }
        }
        let _ = tx.send(lost());
    });
    rx
}
```

`EngineTab` gets a field after `events_rx`:

```rust
    /// The newest frame its reader saw (`engine_io::FrameSlot`), taken by `take_frame`.
    frame: engine_io::FrameSlot,
```

and, after `begin_shutdown`:

```rust
    /// The newest frame since the last call, if any.
    pub(crate) fn take_frame(&self) -> Option<engine_io::Frame> {
        self.frame.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take()
    }
```

Callers:
- In `spawn_tab`, write `let frame = engine_io::FrameSlot::default();` and `let events_rx = spawn_reader_thread(from_engine, Arc::clone(&frame));`, and add `frame` to the struct literal.
- In `spawn_webkit_tab`, do the same with `spawn_reader_thread(from_engine, Arc::clone(&frame))`.
- In `fake`, add `frame: engine_io::FrameSlot::default(),`.
- Add `use std::sync::Arc;` to the imports.

- [ ] **Step 4: The drain takes frames from the slot.** In `event_loop.rs` `drain_engine_ipc`:
- Delete the `EngineToHost::FrameReady { width, height, pixels, .. } => { frame_updates.insert(…); }` arm.
- After the `while let Ok(msg) = tab.tab.events_rx.try_recv() { … }` loop, still inside the `for tab_id` loop, add:

```rust
            if let Some(frame) = tab.tab.take_frame() {
                frame_updates.insert(
                    tab_id.clone(),
                    FrameUpdate {
                        tab_id: tab_id.clone(),
                        socket_id: tab.tab.socket_id.clone(),
                        kind: tab.tab.kind,
                        width: frame.width,
                        height: frame.height,
                        pixels: frame.pixels,
                    },
                );
            }
```

Keep the long comment above `let mut frame_updates`, and append: "Since wave 3b the reader keeps one frame per tab itself (`EngineTab::take_frame`), so the burst described above cannot build up in the first place."

- [ ] **Step 5: The tests on the new API.** In `reader_fed`:
- return `(mpsc::Receiver<EngineToHost>, engine_io::FrameSlot)`;
- build `let frame = engine_io::FrameSlot::default();`;
- end with `(spawn_reader_thread(ipc_protocol::MessageChannel::new(stream), Arc::clone(&frame)), frame)`.

Update the callers:
- `first_event_after_version`: `let (rx, _) = reader_fed(…)`.
- `an_app_that_stops_draining…`: `let (rx, _) = …`.
- `frames_never_queue…`: `let (rx, frame) = …`, and append:

```rust
    let newest = frame.lock().unwrap().take().expect("the frames went to the slot");
    assert_eq!(newest.width, 99, "the newest frame wins");
```

Run the same three tests. Expected: all PASS.

- [ ] **Step 6: Checks and commit.** The four commands from Global Constraints, then commit: `engines: frames go to a one-frame slot, other messages through a bounded channel (survey E3)`.

---

### Task 4: Wake the loop on engine messages; cap one drain per tab

**Model:** sonnet. **Kind:** judgment. **Proof:**
- On today's code one drain applies all 74 queued messages; after, it applies 64 and leaves 10.
- A burst of wakes posts one event until it is handled.
- An engine wake is not activity.
- The reader wakes for messages but not for frames.

**Files:**
- Modify: `manifold/src/process_manager/engine_io.rs`: `Wake`
- Modify: `manifold/src/process_manager.rs`: `spawn_reader_thread`, `spawn_tab`, `spawn_webkit_tab`, `fake_fed`/`fake`
- Modify: `manifold/src/event_loop.rs`:
  - `UserEvent::EngineMessage`; `FramePacing::engine_wake`
  - `is_activity`, `engine_wake`, `coalesced_wake`, `on_engine_wake`, `ENGINE_MESSAGES_PER_DRAIN`
  - `user_event`, `drain_engine_ipc`, and tests
- Modify: `manifold/src/tabs.rs`: `spawn_tab_async`
- Modify: `manifold/src/process_manager/tests.rs`: `reader_fed`

**Interfaces:**
- Produces:
  - `engine_io::Wake = Arc<dyn Fn() + Send + Sync>`
  - `process_manager::spawn_tab(.., jar_json, wake: engine_io::Wake)`
  - `#[cfg(test)] EngineTab::fake_fed(kind) -> (EngineTab, Peer, mpsc::SyncSender<EngineToHost>)`
  - `UserEvent::EngineMessage`
  - `event_loop::engine_wake(Arc<AtomicBool>, EventLoopProxy<UserEvent>) -> Wake`
  - `event_loop::ENGINE_MESSAGES_PER_DRAIN: usize = 64`
- Consumes: Task 3's reader.

**Blast radius:**
- `spawn_tab` has one caller, `tabs.rs` `spawn_tab_async` (`grep -rn "process_manager::spawn_tab(" crates/manifold/src`).
- `UserEvent` is matched only in `event_loop.rs` `user_event`.
- `FramePacing` is built with `..Default::default()` in the two idle tests and with `FramePacing::default()` in `main.rs`, so a new field with a `Default` breaks neither.

- [ ] **Step 1: The cap test (red first).**
- In `process_manager.rs`, replace `fake` with `fake_fed` plus a `fake` that wraps it:

```rust
    /// A live tab for unit tests: no process, its control channel a loopback TCP connection. The returned peer
    /// reads what the app sends it (`HostToEngine`), timing out after 200 ms when nothing more comes; the sender
    /// feeds the tab's events as its reader would.
    #[cfg(test)]
    pub(crate) fn fake_fed(
        kind: EngineKind,
    ) -> (EngineTab, ipc_protocol::MessageChannel<std::net::TcpStream>, mpsc::SyncSender<EngineToHost>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind loopback");
        let host = std::net::TcpStream::connect(listener.local_addr().expect("local addr")).expect("connect");
        let (peer, _) = listener.accept().expect("accept");
        peer.set_read_timeout(Some(Duration::from_millis(200))).expect("read timeout");
        let (feed, events_rx) = mpsc::sync_channel(engine_io::EVENTS_CAP);
        let tab = EngineTab {
            kind,
            socket_id: String::new(),
            writer: engine_io::Writer::spawn(RemoteTransport::new(host), "engine writer (fake)"),
            events_rx,
            frame: engine_io::FrameSlot::default(),
            child: None,
            cache_dir: None,
            profile_slot: None,
        };
        (tab, ipc_protocol::MessageChannel::new(peer), feed)
    }

    /// `fake_fed` without the feed: its events channel is closed and empty.
    #[cfg(test)]
    pub(crate) fn fake(kind: EngineKind) -> (EngineTab, ipc_protocol::MessageChannel<std::net::TcpStream>) {
        let (tab, peer, _) = Self::fake_fed(kind);
        (tab, peer)
    }
```

- In `event_loop.rs`, next to `IDLE_FRAME`'s neighbours, add:

```rust
/// Engine messages one drain applies per tab; the rest wait for the next tick (survey E3). An applied message marks
/// the loop active, so that tick comes at the active rate.
pub(crate) const ENGINE_MESSAGES_PER_DRAIN: usize = 64;
```

- In `idle_tests`, add:

```rust
    #[test]
    fn one_drain_applies_at_most_its_cap_per_tab() {
        use crate::node::TabPayload;
        use crate::process_manager::{EngineKind, EngineTab};
        let mut app = App::default();
        let (tab, _peer, feed) = EngineTab::fake_fed(EngineKind::Servo);
        let payload = TabPayload {
            tab,
            url: "https://a/".into(),
            title: String::new(),
            can_go_back: false,
            can_go_forward: false,
            loading: false,
            audio_playing: false,
            muted: false,
            zoom_factor: 1.0,
            swap_target: None,
        };
        app.test_node("t", crate::store::model::Kind::Tab, Runtime::Tab(payload));
        for i in 0..ENGINE_MESSAGES_PER_DRAIN + 10 {
            feed.send(EngineToHost::AudioPlayingChanged { playing: i.is_multiple_of(2) }).unwrap();
        }
        app.drain_engine_ipc();
        let left = app.nodes.get("t").and_then(|n| n.as_tab()).map_or(0, |t| t.tab.events_rx.try_iter().count());
        assert_eq!(left, 10, "the rest waits for the next drain");
    }
```

(Import `Runtime` and `EngineToHost` in `idle_tests` if `use super::*` does not bring them; `event_loop.rs` uses both.)

Run `cargo test -j 4 -p manifold --bins one_drain_applies_at_most_its_cap_per_tab`. Expected: FAIL, `left: 0, right: 10`. Paste the output. A dead-code warning for the constant is expected until Step 3.

- [ ] **Step 2: The wake type and the reader.**
- In `engine_io.rs`:

```rust
/// Called by a reader after each message it hands over (frames aside): wakes the event loop to apply it now rather
/// than at its next tick (`event_loop::engine_wake`).
pub(crate) type Wake = Arc<dyn Fn() + Send + Sync>;
```

- `spawn_reader_thread` takes a third parameter, `wake: engine_io::Wake`. Each `tx.send(..)` of a non-frame message, including the two `Crashed` sends and the mismatch, is followed by `wake();`, whatever the send returned. Frames do not wake.
- `spawn_tab` gains a last parameter `wake: engine_io::Wake`. It passes it on to `spawn_reader_thread(from_engine, Arc::clone(&frame), wake)`, and to `spawn_webkit_tab(tab_id, initial_url, wake)`, which gains the same parameter.
- Update `spawn_tab`'s doc comment: "`wake` is called by the tab's reader thread after each engine message (`event_loop::engine_wake`)."
- In `tests.rs`, make `reader_fed` return a third value, the wake count:

```rust
    let wakes = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&wakes);
    let wake: engine_io::Wake = Arc::new(move || {
        count.fetch_add(1, Ordering::SeqCst);
    });
    (spawn_reader_thread(ipc_protocol::MessageChannel::new(stream), Arc::clone(&frame), wake), frame, wakes)
```

  Adjust the three callers to `let (rx, ..)` or `let (rx, frame, wakes)`. Append to `frames_never_queue_on_the_events_channel`:

```rust
    // The engine end closed after the Cookies: the reader reports it, waking once more.
    assert!(matches!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), EngineToHost::Crashed { .. }));
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while wakes.load(Ordering::SeqCst) < 2 && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(wakes.load(Ordering::SeqCst), 2, "one wake per message (Cookies, Crashed), none for the 100 frames");
```

  This is exact, not racy: the reader handles the frames before it sends `Cookies`, so any wake for a frame would already be counted when the count reaches 2.

- [ ] **Step 3: The event loop.** In `event_loop.rs`:

1. Add the `UserEvent` variant:

```rust
    /// An engine reader handed over a message (`engine_wake`): apply it now. Posted at most once until handled.
    EngineMessage,
```

2. In `FramePacing`, add a field, and `engine_wake: std::sync::Arc::default(),` in its `Default`:

```rust
    /// Set by a reader when it posts `UserEvent::EngineMessage`, cleared when the loop handles it, so a burst of
    /// messages posts one event (`engine_wake`).
    pub(crate) engine_wake: std::sync::Arc<std::sync::atomic::AtomicBool>,
```

3. Module-level functions:

```rust
/// Whether `event` counts as activity that ends idle pacing (`App::mark_active`). An engine wake does not: the drain
/// it runs marks the loop active only when it applies a message, as a tick's drain does.
fn is_activity(event: &UserEvent) -> bool {
    !matches!(event, UserEvent::EngineMessage)
}

/// The `Wake` a tab's reader calls (`process_manager::spawn_tab`): posts `UserEvent::EngineMessage` unless one is
/// already pending (`FramePacing::engine_wake`).
pub(crate) fn engine_wake(
    pending: std::sync::Arc<std::sync::atomic::AtomicBool>,
    proxy: EventLoopProxy<UserEvent>,
) -> crate::process_manager::engine_io::Wake {
    // In a Mutex: winit declares its proxy `Send`, not `Sync`.
    let proxy = std::sync::Mutex::new(proxy);
    coalesced_wake(pending, move || {
        if let Ok(proxy) = proxy.lock() {
            let _ = proxy.send_event(UserEvent::EngineMessage);
        }
    })
}

/// `post`, unless `pending` says a post is already waiting to be handled.
fn coalesced_wake(
    pending: std::sync::Arc<std::sync::atomic::AtomicBool>,
    post: impl Fn() + Send + Sync + 'static,
) -> crate::process_manager::engine_io::Wake {
    std::sync::Arc::new(move || {
        if !pending.swap(true, std::sync::atomic::Ordering::AcqRel) {
            post();
        }
    })
}
```

   (Import `EventLoopProxy` from `winit::event_loop` if the file does not already.)

4. In `fn user_event`, replace the first line `self.mark_active();` with `if is_activity(&event) { self.mark_active(); }`, and add the arm `UserEvent::EngineMessage => self.on_engine_wake(),`.
5. In `impl App`, next to `mark_active`:

```rust
    /// `UserEvent::EngineMessage`: clears the pending flag first (a message arriving during the drain posts again),
    /// then applies what arrived (`drain_tab_events`).
    fn on_engine_wake(&mut self) {
        self.pacing.engine_wake.store(false, std::sync::atomic::Ordering::Release);
        self.drain_tab_events();
    }
```

6. In `drain_engine_ipc`, replace `while let Ok(msg) = tab.tab.events_rx.try_recv() {` with `for msg in tab.tab.events_rx.try_iter().take(ENGINE_MESSAGES_PER_DRAIN) {`. The `continue` in the `DownloadProgress` arm still continues this loop. Update the function's doc comment: "Takes up to `ENGINE_MESSAGES_PER_DRAIN` messages off each tab's `events_rx` and that tab's newest frame, …".
7. Tests in `idle_tests`:

```rust
    #[test]
    fn a_burst_of_engine_messages_posts_one_wake_until_it_is_handled() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::Arc;
        let pending = Arc::new(AtomicBool::new(false));
        let posted = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&posted);
        let wake = coalesced_wake(Arc::clone(&pending), move || {
            count.fetch_add(1, Ordering::SeqCst);
        });
        for _ in 0..5 {
            wake();
        }
        assert_eq!(posted.load(Ordering::SeqCst), 1);
        pending.store(false, Ordering::Release); // what `on_engine_wake` does first
        wake();
        assert_eq!(posted.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn an_engine_wake_is_not_activity_but_a_command_is() {
        assert!(!is_activity(&UserEvent::EngineMessage));
        assert!(is_activity(&UserEvent::Debug(DebugCommand::Help)));
    }
```

- [ ] **Step 4: The spawn passes the wake.** In `tabs.rs` `spawn_tab_async`, after `let proxy = …expect(…);`:

```rust
        let wake = crate::event_loop::engine_wake(std::sync::Arc::clone(&self.pacing.engine_wake), proxy.clone());
```

and pass `wake` as the last argument of `process_manager::spawn_tab(…, &seed, wake)`.

- [ ] **Step 5: Run** `cargo test -j 4 -p manifold --bins -- one_drain_applies_at_most_its_cap_per_tab a_burst_of_engine_messages an_engine_wake_is_not_activity frames_never_queue`. Expected: PASS.

- [ ] **Step 6: Checks and commit.** The four commands from Global Constraints, then commit: `event loop: engine readers wake the loop; one drain applies at most 64 messages per tab (survey E3, E9)`.

---

### Task 5: Unresponsive tabs: toast, Restart and End, headless commands, `EXPORTSTATE`

**Model:** sonnet. **Kind:** judgment: this task refactors the `Crashed` arm without changing what it does. **Proof:**
- A wedged fake tab is toasted once, not once per drain.
- `restart_engine` respawns under the same id and `end_engine` leaves a failed card, each with its toast.
- `EXPORTSTATE` reads `unresponsive` false, then true after a wedge.
- The two new commands parse and are in `HELP`.
- Every existing crash test (`after_crash`, the `leave_crashed_card` callers) passes.

**Files:**
- Modify: `manifold/src/process_manager/engine_io.rs`: the `reported` flag, `take_newly_unresponsive`, one test
- Modify: `manifold/src/process_manager.rs`: `EngineTab::is_unresponsive`, `take_newly_unresponsive`, `#[cfg(test)] wedge`
- Modify: `manifold/src/event_loop.rs`: the toast in `drain_engine_ipc`; the `Crashed` arm onto `restart_spawn`/`restart_with`
- Modify: `manifold/src/tabs.rs`: `LeftClosed`, `leave_crashed_card` renamed `leave_failed_card`, `restart_spawn`, `restart_with`, `restart_engine`, `end_engine`
- Modify: `manifold/src/debug_control.rs`: `RestartEngine`, `EndEngine`, help, samples, handler, the `export_state` field, the export test
- Modify: `manifold/src/session_sync/tests.rs`: the three `leave_crashed_card` calls, three tests
- Modify: `scripts/export_state.py` (docstring: the new field)

**Interfaces:**
- Produces:
  - `Writer::take_newly_unresponsive(&self, Instant) -> bool`
  - On `EngineTab`: `is_unresponsive(&self, Instant) -> bool` and `take_newly_unresponsive(&self, Instant) -> bool`
  - `#[cfg(test)] EngineTab::wedge(&self)`
  - `pub(crate) enum LeftClosed { KeepsCrashing, CannotStart, Ended }`
  - On `App`: `leave_failed_card(&mut self, &str, &str, LeftClosed)`, `restart_spawn(&self, &str) -> Option<TabSpawn>`, `restart_with(&mut self, &str, TabSpawn)`, `restart_engine(&mut self, &str) -> bool`, `end_engine(&mut self, &str) -> bool`
  - Debug commands `RESTARTENGINE id` and `ENDENGINE id`
  - The `EXPORTSTATE` card field `unresponsive`
- Consumes: Tasks 2 to 4.

**Blast radius:**
- `leave_crashed_card` has five callers: `event_loop.rs` (2) and `session_sync/tests.rs` (3). Check with `grep -rn "leave_crashed_card" crates/manifold/src`, and also check comments naming it.
- The `Crashed` arm's behaviour is unchanged: same spawn, same `after_crash`, same toast text, same teardown.
- `EXPORTSTATE` gains a field. Probes read cards by key, so an added key breaks none (`scripts/export_state.py` reads `type`, `loaded`, `json_card`).

- [ ] **Step 1: `take_newly_unresponsive` in the writer.**
- In `State`, add:

```rust
    /// `take_newly_unresponsive` has reported the current stall.
    reported: bool,
```

- In `write_queued`'s post-write block, add `state.reported = false;` next to `state.refused = false;`.
- In `impl Writer`:

```rust
    /// True once per stall: the first call that finds the tab unresponsive since its last completed write (the toast).
    pub(crate) fn take_newly_unresponsive(&self, now: Instant) -> bool {
        let mut state = self.0.lock();
        let newly = state.unresponsive(now) && !state.reported;
        if newly {
            state.reported = true;
        }
        newly
    }
```

- Test in `engine_io::tests`:

```rust
    #[test]
    fn a_stall_is_reported_once_until_a_write_completes() {
        let (writer, permit, _) = gated(false);
        for n in 0..SEND_QUEUE_CAP + 2 {
            writer.send(url(n));
        }
        assert!(writer.take_newly_unresponsive(Instant::now()));
        assert!(!writer.take_newly_unresponsive(Instant::now()), "once per stall");
        permit.send(()).unwrap();
        assert!(eventually(Duration::from_secs(5), || !writer.is_unresponsive(Instant::now())));
        for n in 0..SEND_QUEUE_CAP + 2 {
            writer.send(url(n)); // the writer is stuck on its next write again, and the queue refills
        }
        assert!(writer.take_newly_unresponsive(Instant::now()), "a new stall is reported again");
    }
```

  The second burst refills the queue: after one permit the writer completes one write, then blocks on the next pop, and the queue had at most `SEND_QUEUE_CAP` left. `SEND_QUEUE_CAP + 2` more pushes therefore overflow it again.

- [ ] **Step 2: `EngineTab` accessors.** After `take_frame`:

```rust
    /// The engine has stopped reading: a send was refused, or one write has been stuck for
    /// `engine_io::UNRESPONSIVE_AFTER` (the card badge and the menu's Restart and End rows).
    pub(crate) fn is_unresponsive(&self, now: std::time::Instant) -> bool {
        self.writer.is_unresponsive(now)
    }

    /// `is_unresponsive`, true once per stall (the toast).
    pub(crate) fn take_newly_unresponsive(&self, now: std::time::Instant) -> bool {
        self.writer.take_newly_unresponsive(now)
    }

    /// Makes a fake tab unresponsive: fills its queue (its peer must never be read).
    #[cfg(test)]
    pub(crate) fn wedge(&self) {
        self.writer.wedge();
    }
```

- [ ] **Step 3: The toast.** In `drain_engine_ipc`:
- before the `for tab_id in &self.node_order` loop: `let now = std::time::Instant::now(); let mut stuck: Vec<String> = Vec::new();`;
- inside the loop, after the frame block: `if tab.tab.take_newly_unresponsive(now) { stuck.push(tab.url.clone()); }`;
- after the loop:

```rust
        for url in stuck {
            self.queue_toast(format!(
                "Not responding: {}. Right-click its card to restart or end it.",
                Self::toast_url(&url)
            ));
        }
```

- [ ] **Step 4: Restart and End, and the `Crashed` arm on them.** In `tabs.rs`:
- Rename `leave_crashed_card` to `leave_failed_card`, and replace its `cannot_start: bool` parameter with `why: LeftClosed`.
- Update its doc: "`why` picks the toast: `CannotStart` says why the host could not start".
- Replace its final `if cannot_start { … } else { … }` with:

```rust
        match why {
            LeftClosed::CannotStart => {
                let message = format!("Couldn't start {} for {}: {reason}", engine_label(kind), Self::toast_url(&url));
                self.queue_failure_toast(kind, &url, group.as_deref(), reason, message);
            }
            LeftClosed::KeepsCrashing => {
                self.queue_toast(format!("Tab keeps crashing, left closed: {}", Self::toast_url(&url)))
            }
            LeftClosed::Ended => self.queue_toast(format!("Ended (not responding): {}", Self::toast_url(&url))),
        }
```

Then add above it:

```rust
/// Why `leave_failed_card` left a failed card, which picks its toast.
pub(crate) enum LeftClosed {
    /// `AfterCrash::GiveUp`: it crashed too often.
    KeepsCrashing,
    /// `AfterCrash::CannotStart`: the host could not start; the toast says why.
    CannotStart,
    /// "End Engine" on a tab that stopped responding.
    Ended,
}
```

and, after `leave_failed_card`:

```rust
    /// The spawn that restarts live tab `id` on the engine the mirror holds for it: a swap in flight (`swap_target`,
    /// or one waiting for its cookies) has already saved its target, so the tab restarts on it, the running engine
    /// its fallback. `None` when `id` is no live tab.
    pub(crate) fn restart_spawn(&self, id: &str) -> Option<TabSpawn> {
        let want = self.mirror.engine(id);
        let node = self.nodes.get(id)?;
        let tab = node.as_tab()?;
        let fallback_kind = (tab.tab.kind != want).then_some(tab.tab.kind);
        let spawn = TabSpawn::respawn(id, node, self.mirror.carried(id), want, tab.url.clone())?;
        Some(TabSpawn { fallback_kind, ..spawn })
    }

    /// Replaces live tab `id`'s engine with one started from `spawn` (`restart_spawn`). Dropping the old `EngineTab`
    /// ends its host.
    pub(crate) fn restart_with(&mut self, id: &str, spawn: TabSpawn) {
        self.nodes.remove(id);
        // Same stale-state guard `destroy_tab`/`finish_swap` apply — see `finish_swap`'s own comment on this.
        if self.interaction.dragging.as_deref() == Some(id) {
            self.interaction.dragging = None;
        }
        if self.interaction.hovered.as_deref() == Some(id) {
            self.interaction.hovered = None;
        }
        self.respawn_tab(spawn);
    }

    /// "Restart Engine": a new host for live tab `id` at its current URL, as a crash restart does, without counting
    /// toward the crash limit. False when `id` is no live tab.
    pub(crate) fn restart_engine(&mut self, id: &str) -> bool {
        let Some(spawn) = self.restart_spawn(id) else { return false };
        self.queue_toast(format!("Restarted: {}", Self::toast_url(&spawn.url)));
        self.restart_with(id, spawn);
        true
    }

    /// "End Engine": ends live tab `id`'s host and leaves a failed card, which a click starts again. False when `id`
    /// is no live tab.
    pub(crate) fn end_engine(&mut self, id: &str) -> bool {
        if self.nodes.get(id).and_then(|n| n.as_tab()).is_none() {
            return false;
        }
        self.leave_failed_card(id, "It stopped responding and was ended.", LeftClosed::Ended);
        true
    }
```

In `event_loop.rs`'s `EngineToHost::Crashed { reason }` arm:
- Replace the `let want = …; let Some(spawn) = self.nodes.get(tab_id).and_then(|n| { … }) else { return false; };` block with `let Some(spawn) = self.restart_spawn(tab_id) else { return false };`. Move its comment ("Restarted on the mirror's engine: …") to `restart_spawn`, where it now lives.
- The two `self.leave_crashed_card(tab_id, …, false/true)` calls become `self.leave_failed_card(tab_id, &reason, LeftClosed::KeepsCrashing)` and `self.leave_failed_card(tab_id, why, LeftClosed::CannotStart)`.
- Replace everything after the toast (`self.nodes.remove(tab_id);` through `self.respawn_tab(spawn);`) with `self.restart_with(tab_id, spawn);`.
- `let url = &spawn.url;` and the `eprintln!` and toast that use it stay before that call.
- Import `crate::tabs::LeftClosed`.

In `session_sync/tests.rs`:
- the calls at about lines 4882 and 4883 become `leave_failed_card(&a, "no engine", LeftClosed::CannotStart)` and the same for `&b`;
- the call at about 4918 becomes `leave_failed_card(&live, "keeps crashing", LeftClosed::KeepsCrashing)`;
- import `crate::tabs::LeftClosed`.

Then run `grep -rn "leave_crashed_card" crates/manifold/src`. Expected: nothing, comments included.

- [ ] **Step 5: Headless commands and `EXPORTSTATE`.** In `debug_control.rs`:
- **Variants**, after `SleepNode(String)`:

```rust
    /// `RESTARTENGINE id` -- the "Restart Engine" row of live tab `id`'s menu (`App::restart_engine`).
    RestartEngine(String),
    /// `ENDENGINE id` -- the "End Engine" row of live tab `id`'s menu (`App::end_engine`).
    EndEngine(String),
```

- **`COMMAND_HELP`**, right after the `"SLEEPNODE id"` entry:

```rust
    (
        "RESTARTENGINE id",
        "Start a new engine host for live tab `id` at its URL, same as its \"Restart Engine\" row \
         (offered while it is not responding)",
    ),
    ("ENDENGINE id", "End live tab `id`'s engine host and leave a failed card, same as its \"End Engine\" row"),
```

- **`parse_line`**, right after `"SLEEPNODE" => …`:

```rust
        "RESTARTENGINE" => non_empty(DebugCommand::RestartEngine),
        "ENDENGINE" => non_empty(DebugCommand::EndEngine),
```

- **The `SAMPLES` list** of the help test, after `"SLEEPNODE t1",`: `"RESTARTENGINE t1",` and `"ENDENGINE t1",`.
- **The handler**, next to `DebugCommand::SleepNode(id) => …`:

```rust
            DebugCommand::RestartEngine(id) => {
                if !self.restart_engine(&id) {
                    eprintln!("RESTARTENGINE: no live tab {id}; ignored");
                }
            }
            DebugCommand::EndEngine(id) => {
                if !self.end_engine(&id) {
                    eprintln!("ENDENGINE: no live tab {id}; ignored");
                }
            }
```

- **`export_state`**: before `let cards: Vec<serde_json::Value> = self`, add `let now = std::time::Instant::now();`. In the stored-node card's `json!`, after `"pinned"`, add:

```rust
                    // A live tab whose engine stopped reading (`EngineTab::is_unresponsive`).
                    "unresponsive": node.as_tab().is_some_and(|t| t.tab.is_unresponsive(now)),
```

  The JSON-level card's `json!` gains `"unresponsive": false,`.
- **The test** `export_state_names_cards_by_type_loaded_and_json_card`: append at its end:

```rust
        assert_eq!(cards(&app)["live"]["unresponsive"], serde_json::json!(false));
        app.nodes.get("live").and_then(|n| n.as_tab()).unwrap().tab.wedge();
        assert_eq!(cards(&app)["live"]["unresponsive"], serde_json::json!(true), "its queue filled");
```

  (Its `_peer` binding keeps the fake engine's end open and unread, which `wedge` needs.)
- **`scripts/export_state.py`**: in the module docstring's field list, add a line: `  unresponsive  true for a live tab whose engine stopped reading its pipe (wave 3b)`.

- [ ] **Step 6: App-level tests.** In `session_sync/tests.rs`, after `a_crashed_tab_beside_a_failed_card_for_its_intent_leaves_one_card`:

```rust
/// A tab whose engine stops reading is toasted once per stall, not once per drain.
#[test]
fn a_tab_that_stops_responding_is_toasted_once() {
    let mut app = app_on(&temp_dir("not-responding").join("session.mnf"), 1);
    let (id, _peer) = live_tab(&mut app, EngineKind::Servo, "https://a/");
    app.nodes.get(&id).and_then(|n| n.as_tab()).unwrap().tab.wedge();
    let toasts = |app: &App| app.toasts.save_toast_queue.iter().filter(|t| t.starts_with("Not responding: ")).count();
    app.drain_tab_events();
    assert_eq!(toasts(&app), 1);
    app.drain_tab_events();
    assert_eq!(toasts(&app), 1, "once per stall");
}

/// "Restart Engine" starts a new host for the tab under the same id, and says so.
#[test]
fn restart_engine_respawns_the_tab_under_its_id() {
    let mut app = app_on(&temp_dir("restart-engine").join("session.mnf"), 1);
    let live = succeed_spawn(&mut app, "https://a/");
    assert!(app.restart_engine(&live));
    assert!(app.test_spawns.iter().any(|(s, _)| s.id == live), "a respawn under the same id");
    assert!(app.toasts.save_toast_queue.iter().any(|t| t == "Restarted: https://a/"));
    assert!(!app.restart_engine("no-such-tab"));
}

/// "End Engine" leaves a failed card for the tab, and says so.
#[test]
fn end_engine_leaves_a_failed_card() {
    let mut app = app_on(&temp_dir("end-engine").join("session.mnf"), 1);
    let live = succeed_spawn(&mut app, "https://a/");
    assert!(app.end_engine(&live));
    assert_eq!(failed_cards(&app), std::slice::from_ref(&live));
    assert!(app.toasts.save_toast_queue.iter().any(|t| t == "Ended (not responding): https://a/"));
    assert!(!app.end_engine(&live), "a failed card is no live tab");
}
```

Run `cargo test -j 4 -p manifold --bins -- a_tab_that_stops_responding restart_engine_respawns end_engine_leaves export_state_names_cards help_and_parser_agree crash`. Expected: PASS.

- [ ] **Step 7: Checks and commit.** The four commands from Global Constraints, then commit: `tabs: a tab whose engine stops reading is toasted; Restart and End Engine, RESTARTENGINE/ENDENGINE, EXPORTSTATE unresponsive (survey E1)`.

---

### Task 6: Menu rows and the card badge

**Model:** haiku. **Kind:** mechanical. **Proof:** the new menu test; the existing pinned-menu tests pass unchanged (`unresponsive` is false in `view_of`); Task 8's screenshot shows the badge.

**Files:**
- Modify: `manifold/src/node_menu.rs` (`NodeAction`, `hover_text`, `NodeMenuView`, `node_menu_config`, `node_menu_view`, `apply_node_action`)
- Modify: `manifold/src/node_menu/tests.rs` (`view_of`, one test)
- Modify: `manifold/src/card.rs` (`paint_tab_body`)

**Interfaces:**
- Produces: `NodeAction::RestartEngine` and `NodeAction::EndEngine`.
- Consumes: Task 5's `restart_engine`, `end_engine` and `EngineTab::is_unresponsive`.

**Blast radius:**
- `NodeAction` is matched exhaustively in `apply_node_action`, and possibly elsewhere: run `grep -rn "match .*action\|NodeAction::" crates/manifold/src` and add the two arms wherever the compiler asks.
- The card change only adds an overlay and a tooltip line.

- [ ] **Step 1: The test.** In `node_menu/tests.rs`, add `unresponsive: false,` to `view_of`'s struct literal, and append:

```rust
/// A live tab whose engine stopped responding leads its menu with Restart Engine and End Engine; a responsive one
/// shows neither.
#[test]
fn an_unresponsive_tab_menu_leads_with_restart_and_end() {
    let labels = |view: &NodeMenuView| -> Vec<&'static str> {
        node_menu_config(view, false)
            .items
            .iter()
            .filter_map(|item| match item {
                NodeMenuItem::Button(label, _) => Some(*label),
                _ => None,
            })
            .collect()
    };
    let stuck = NodeMenuView { unresponsive: true, ..view_of(loaded_tab()) };
    assert_eq!(labels(&stuck)[..2], ["Restart Engine", "End Engine"]);
    assert!(!labels(&view_of(loaded_tab())).contains(&"Restart Engine"));
}
```

- [ ] **Step 2: The menu.** In `node_menu.rs`:
1. `NodeAction`, after `SleepGroup`:

```rust
    /// A live tab that stopped responding gets a new engine host (`App::restart_engine`).
    RestartEngine,
    /// A live tab that stopped responding has its engine host ended, leaving a failed card (`App::end_engine`).
    EndEngine,
```

2. `hover_text`: add the arms `NodeAction::RestartEngine => Some("Start a new engine for this tab at its current page"),` and `NodeAction::EndEngine => Some("Close this tab's engine; click the card to start it again"),`.
3. `NodeMenuView`, after `pinned`:

```rust
    /// Live tab only: its engine stopped reading (`EngineTab::is_unresponsive`), so the menu leads with Restart
    /// Engine and End Engine.
    unresponsive: bool,
```

4. `node_menu_view`, after `pinned: …,`: `unresponsive: node.as_tab().is_some_and(|t| t.tab.is_unresponsive(std::time::Instant::now())),`.
5. `node_menu_config`'s `TYPE_TAB if view.kind.loaded()` arm becomes:

```rust
        TYPE_TAB if view.kind.loaded() => {
            let mut items = Vec::new();
            if view.unresponsive {
                items.extend([Button("Restart Engine", A::RestartEngine), Button("End Engine", A::EndEngine), Separator]);
            }
            items.extend([
                Button(if view.pinned { "Unpin" } else { "Pin" }, A::TogglePin),
                Button("Duplicate", A::Duplicate),
                Button("Sleep", A::Sleep),
                Button("Copy URL", A::CopyUrl),
            ]);
            items.push(Button("Fit Subtree", A::FitSubtree));
            items.extend(view.jars.iter().flat_map(|j| jar_items(j, true, true)));
            finish("tab_context_menu", 150.0, items, delete("Close"))
        }
```

6. `apply_node_action`, after the `Sleep` arm:

```rust
            NodeAction::RestartEngine => {
                self.restart_engine(id);
            }
            NodeAction::EndEngine => {
                self.end_engine(id);
            }
```

- [ ] **Step 3: The badge.** In `card.rs` `paint_tab_body`:
- Extend the `let Some((title, url, engine_kind)) = …map(|t| (t.title.clone(), t.url.clone(), t.tab.kind))` tuple with a fourth element, `t.tab.is_unresponsive(std::time::Instant::now())`, bound as `unresponsive`.
- Just before the final `body`, after the `if !self.graph_lod_simplified { … }` block, add:

```rust
        // At every level of detail: a stuck engine is the one thing on this card that needs the user.
        if unresponsive {
            let middle = tab_thumb_rect(rect, inv_zoom).center();
            push_centered_text(deferred_overlays, middle, egui_phosphor::light::WARNING_CIRCLE, 24.0, DANGER_COLOR);
            body.tooltip = chain_tooltip(
                Some("Not responding. Right-click for Restart Engine or End Engine.".to_owned()),
                body.tooltip.take(),
            );
        }
```

- [ ] **Step 4: Run** `cargo test -j 4 -p manifold --bins node_menu`. Expected: PASS, including `node_menu_config_matches_each_kinds_menu` unchanged.

- [ ] **Step 5: Checks and commit.** The four commands from Global Constraints, then commit: `cards: a not-responding badge, and Restart Engine and End Engine rows on a stuck tab's menu`.

---

### Task 7: The engine-side inbox bound

**Model:** haiku. **Kind:** mechanical. **Proof:** the new test fails on today's code (the app writes all 1000 messages while the engine takes none) and passes after; all `ipc-protocol` tests pass.

**Files:**
- Modify: `ipc-protocol/src/transport.rs` (`ENGINE_INBOX`, `spawn_host_reader`, `tests`)

**Interfaces:**
- Produces: `pub const ENGINE_INBOX: usize = 256` in `ipc_protocol::transport`.
- Consumes: nothing new.

**Blast radius:**
- All four engines call `spawn_host_reader` (`cef/src/main.rs:178`, `servo:174`, `webkit:70`, `webview2:156`). The return type is unchanged, so none needs a source change.
- WebView2 is built and exercised here (Task 8).
- CEF, Servo and WebKit are **not built or run** by this wave. What to watch for there:
  - An engine whose main loop legitimately stops draining its inbox for a while would back up the app's writer and show "Not responding". Examples: a long synchronous init after `connect_local_pair`, or a nested native modal loop.
  - It needs 256 queued messages, then a full pipe, then 2 s with a write blocked. Pointer moves flow freely until the inbox fills.
  - The badge clears by itself when the engine catches up, and nothing is killed.
  - Open question 1 asks whether to ship this task at all.

- [ ] **Step 1: The test (red first).** Append to `transport.rs`'s `tests` module:

```rust
    /// The engine's inbox is bounded (wave 3b): an engine whose main loop takes nothing stops its reader, which backs
    /// the app up on the pipe, and no message is lost.
    #[test]
    fn the_engine_inbox_is_bounded_and_loses_nothing() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        const N: usize = 1000;
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let host = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (engine, _) = listener.accept().unwrap();
        let written = Arc::new(AtomicUsize::new(0));
        let progress = Arc::clone(&written);
        std::thread::spawn(move || {
            let mut out = MessageChannel::new(host);
            out.send_version().unwrap();
            for i in 0..N {
                if out.send(&HostToEngine::SetCookies(format!("{i:0>65536}"))).is_err() {
                    return;
                }
                progress.store(i + 1, Ordering::SeqCst);
            }
        });
        let rx = spawn_host_reader(MessageChannel::new(engine));
        // Proves an absence: the count stops moving once the inbox and the socket buffers are full.
        let mut last = usize::MAX;
        let end = Instant::now() + Duration::from_secs(20);
        while Instant::now() < end {
            std::thread::sleep(Duration::from_millis(300));
            let now = written.load(Ordering::SeqCst);
            if now == last {
                break;
            }
            last = now;
        }
        assert!(last < N, "the app wrote all {N} while the engine took none: the inbox is unbounded");
        for i in 0..N {
            match rx.recv_timeout(Duration::from_secs(10)).unwrap() {
                HostToEngine::SetCookies(s) => assert_eq!(s.trim_start_matches('0').parse::<usize>().unwrap_or(0), i),
                _ => panic!("message {i} is not SetCookies"),
            }
        }
    }
```

(Import `HostToEngine`, `Instant` and `Duration` into the tests module if `use super::*` does not bring them; `transport.rs` imports `Duration` and `Instant` at the top, and `HostToEngine` through `crate::HostToEngine`.)

Run `cargo test -j 4 -p ipc-protocol --lib the_engine_inbox_is_bounded_and_loses_nothing`. Expected: FAIL with "the inbox is unbounded". Paste the output.

- [ ] **Step 2: The bound.** Above `spawn_host_reader`:

```rust
/// `HostToEngine` messages an engine has not taken yet (`spawn_host_reader`). Full, the reader stops reading, so an
/// engine whose main loop is stuck backs up the app's writer for it, and the app shows the tab as not responding;
/// unbounded, the engine's memory grew instead and the app never knew (wave 3b).
pub const ENGINE_INBOX: usize = 256;
```

In `spawn_host_reader`, replace `let (tx, rx) = mpsc::channel();` with `let (tx, rx) = mpsc::sync_channel(ENGINE_INBOX);`. Add one sentence to its doc comment: "The channel holds `ENGINE_INBOX` messages; past that the reader waits for the engine to take one."

- [ ] **Step 3: Run** `cargo test -j 4 -p ipc-protocol`. Expected: PASS.

- [ ] **Step 4: Checks and commit.** The four commands from Global Constraints, plus `cargo build -j 4 -p manifold-engine-webview2` (the one engine that builds here), then commit: `ipc-protocol: bound an engine's inbox, so a stuck engine main loop shows as not responding`.

---

### Task 8: The not-responding probe and the manual run

**Model:** sonnet. **Kind:** judgment: it runs the app and looks at a screenshot. **Proof:**
- `scripts/unresponsive-probe.sh` prints PASS for each check:
  1. the tab is live;
  2. a suspended engine plus typing marks the card `unresponsive` with a "Not responding" toast;
  3. the app keeps answering commands throughout;
  4. `RESTARTENGINE` brings the tab back live and responsive, and the suspended host is gone;
  5. `QUIT` with a second suspended engine ends the app within 10 s.
- The badge screenshot is opened and looked at.

**Files:**
- Create: `scripts/unresponsive-probe.sh`

**Interfaces:**
- Consumes:
  - `scripts/lib.sh`: `seed_settings`, `launch_headless`, `send`, `wait_file`, `check`
  - `scripts/winpid.sh`: `kill_tree`, and `launched_winpid` as set by `launch_headless`
  - `scripts/fresh-exe.sh`
  - `scripts/export_state.py`
- Debug commands used: `WAITTABS`, `SLEEP`, `EXPORTSTATE`, `GRAPHDOWN`, `GRAPHOPEN`, `TYPE`, `ESCAPE`, `SCREENSHOT`, `RESTARTENGINE`, `QUIT`.

**Blast radius:** a new script. Nothing calls it. It is listed in the README beside `sleep-probe.sh` (Task 9). It is not added to `scripts/gate.sh`, because it needs a WebView2 host and suspends processes.

- [ ] **Step 1: Write the script.** It is modeled on `scripts/sleep-probe.sh`:

```bash
#!/usr/bin/env bash
# Usage: scripts/unresponsive-probe.sh
#
# Checks "not responding" on WebView2 (wave 3b, survey E1). A seeded tab is opened, its engine host is suspended
# (NtSuspendProcess on the instance's own child, found by parent pid), and 1000 characters are typed into the page:
# the app's writer for it blocks and its queue fills. EXPORTSTATE must then read the card `unresponsive` with a
# "Not responding" toast, while the app keeps answering commands. `RESTARTENGINE id` must bring the tab back live and
# responsive and end the suspended host. Last, the new host is suspended too and `QUIT` must end the app within 10 s:
# closing a stuck tab never waits on it.
# One headless instance on a fresh isolated APPDATA. Build first:
# cargo build -j 4 -p manifold -p manifold-engine-webview2.
set -u
repo_root="$(cd "$(dirname "$0")/.." && pwd)"
exe="$repo_root/target/debug/manifold.exe"
# shellcheck source=fresh-exe.sh
. "$repo_root/scripts/fresh-exe.sh"
# shellcheck source=winpid.sh
. "$repo_root/scripts/winpid.sh"
# shellcheck source=lib.sh
. "$repo_root/scripts/lib.sh"
require_fresh_exe "$exe" || exit 1
export PYTHONPATH="$(cygpath -w "$repo_root/scripts")"
# Not under the repo: a path with a space (this one has "Winston Li") stops the app reading its commands.
out="$(mktemp -d)"
inst=""
cleanup() {
    [ -n "$inst" ] && kill_tree "$inst"
    [ -n "${KEEP:-}" ] || rm -rf "$out"
}
trap cleanup EXIT

# Suspends the WebView2 engine hosts whose parent is Windows pid $1 and prints their pids.
cat >"$out/suspend.ps1" <<'PS'
param([int]$Parent)
Add-Type -Namespace P -Name N -MemberDefinition '[DllImport("ntdll.dll")] public static extern int NtSuspendProcess(IntPtr h);'
Get-CimInstance Win32_Process -Filter "ParentProcessId=$Parent AND Name='manifold-engine-webview2.exe'" | ForEach-Object {
    $null = [P.N]::NtSuspendProcess((Get-Process -Id $_.ProcessId).Handle)
    $_.ProcessId
}
PS
suspend_engine() {
    powershell -NoProfile -ExecutionPolicy Bypass -File "$(cygpath -w "$out/suspend.ps1")" -Parent "$1" </dev/null | tr -d '\r'
}
# card FILE ID FIELD: that card's field in an exported state ("" if absent).
card() {
    python -P -c 'import json,sys; c = [x for x in json.load(open(sys.argv[1], encoding="utf-8"))["cards"] if x["id"] == sys.argv[2]]; print(str(c[0].get(sys.argv[3], "")).lower() if c else "")' "$(cygpath -w "$1")" "$2" "$3" </dev/null
}
# toast FILE: the exported toast ("" if none).
toast() {
    python -P -c 'import json,sys; print(json.load(open(sys.argv[1], encoding="utf-8"))["toast"] or "")' "$(cygpath -w "$1")" </dev/null
}
st() { cygpath -w "$out/$1.json"; }

status=0
profile="$out/appdata"
seed_settings "$profile" webview2
echo '{"nodes": [{"id": "t1", "type": "application/vnd.manifold.tab", "url": "about:blank", "engine": "webview2", "pos": [0.0, 0.0]}]}' >"$profile/seed.json"
"$exe" --import-json "$(cygpath -w "$profile/seed.json")" --out "$(cygpath -w "$profile/manifold/session.mnf")" >/dev/null
cmd="$profile/cmd"
: >"$cmd"
launch_headless "$exe" "$profile" "$cmd" "$profile/log"
inst=$!
w="$launched_winpid"
[ -n "$w" ] || { echo "FAIL: no Windows pid for the instance"; exit 1; }

send WAITTABS "SLEEP 4" "EXPORTSTATE $(st live)"
wait_file "$out/live.json" || exit 1
# The import may give the node a new id (sleep-probe.sh reads it the same way): the one card's id.
id="$(python -P -c 'import json,sys; print(json.load(open(sys.argv[1], encoding="utf-8"))["cards"][0]["id"])' "$(cygpath -w "$out/live.json")" </dev/null)"
check "the tab is live" "true" "$(card "$out/live.json" "$id" loaded)"

# Into Tab view, so typing reaches the page; then suspend its engine.
send GRAPHDOWN GRAPHOPEN "SLEEP 4" "EXPORTSTATE $(st open)"
wait_file "$out/open.json" || exit 1
first="$(suspend_engine "$w")"
[ -n "$first" ] || { echo "FAIL: no engine host to suspend"; exit 1; }
# Back in Graph view before exporting (the cards are the graph's), and within the toast's 4 s.
send "TYPE $(printf 'a%.0s' $(seq 1000))" "SLEEP 1" ESCAPE "SLEEP 1" "EXPORTSTATE $(st stuck)" "SCREENSHOT $(cygpath -w "$out/stuck.png")"
wait_file "$out/stuck.json" || exit 1
check "the card reads unresponsive" "true" "$(card "$out/stuck.json" "$id" unresponsive)"
case "$(toast "$out/stuck.json")" in
"Not responding: "*) echo "PASS: the toast says not responding" ;;
*) echo "FAIL: toast: got [$(toast "$out/stuck.json")]"; status=1 ;;
esac

send "RESTARTENGINE $id" WAITTABS "SLEEP 4" "EXPORTSTATE $(st restarted)"
wait_file "$out/restarted.json" || exit 1
check "restarted: live" "true" "$(card "$out/restarted.json" "$id" loaded)"
check "restarted: responsive" "false" "$(card "$out/restarted.json" "$id" unresponsive)"
gone="$(powershell -NoProfile -Command "if (Get-Process -Id $first -ErrorAction SilentlyContinue) { 'alive' } else { 'gone' }" </dev/null | tr -d '\r')"
check "the suspended host was ended" "gone" "$gone"

# A second stuck engine, then quit: exit must not wait on it.
send GRAPHDOWN GRAPHOPEN "SLEEP 4" "EXPORTSTATE $(st open2)"
wait_file "$out/open2.json" || exit 1
[ -n "$(suspend_engine "$w")" ] || { echo "FAIL: no second engine host to suspend"; exit 1; }
send "TYPE $(printf 'b%.0s' $(seq 1000))" "SLEEP 1" ESCAPE "SLEEP 1" "EXPORTSTATE $(st stuck2)"
wait_file "$out/stuck2.json" || exit 1
check "the second host is stuck too" "true" "$(card "$out/stuck2.json" "$id" unresponsive)"
send QUIT
quit_at=$(date +%s)
for _ in $(seq 60); do kill -0 "$inst" 2>/dev/null || break; sleep 0.25; done
if kill -0 "$inst" 2>/dev/null; then
    echo "FAIL: still running 15 s after QUIT with a stuck tab"
    status=1
else
    check "QUIT with a stuck tab ends within 10 s" "yes" "$([ $(($(date +%s) - quit_at)) -le 10 ] && echo yes || echo no)"
    inst=""
fi
echo "screenshot: $out/stuck.png (KEEP=1 keeps it)"
exit $status
```

  Reading the script:
  - The `seq`/`printf` line builds 1000 `a`s. `TYPE` sends one `Input(Char)` per character (`debug_control.rs`, `DebugCommand::TypeIntoPage`), about 7 bytes each on the wire, so 1000 overflow a 512-byte pipe plus the 256-slot queue with room to spare; characters never merge.
  - Every `EXPORTSTATE` that reads cards runs in Graph view, after `ESCAPE`, and within `SAVE_TOAST_DURATION` (4 s) of the stall, since `"toast"` is the toast showing or the next queued. The `sleep 0.25` polls in the QUIT wait are a wait on an observable condition (the process exiting) with a 15 s deadline.
  - `SLEEP n` lines are app-side pacing commands, as in every probe.
  - In `cards[].unresponsive`, Python prints `True` and `False`; `card` lowercases them.

- [ ] **Step 2: Build and run.**
  1. Run `cargo build -j 4 -p manifold -p manifold-engine-webview2`.
  2. Check for leftovers: `Get-Process manifold, manifold-engine-webview2 -ErrorAction SilentlyContinue`. Note any that are not yours, and never stop them.
  3. Run `KEEP=1 bash scripts/unresponsive-probe.sh`. Expected: every line PASS, and exit 0.
  4. Open `stuck.png` with the Read tool and confirm the warning badge is on the tab's card. Say what you saw.
  5. Afterwards, check that no `manifold-engine-webview2` from this run is left (its pids were printed by `suspend_engine`; look them up). A suspended one left behind would be a bug in `Drop`: report it.
- If a check fails:
  - **Not unresponsive.** Read `$out/appdata/log`. Make sure `suspend_engine` printed a pid. 1000 characters at about 7 bytes each overflow a 512-byte pipe plus 256 queue slots; if they did not, say what you observed rather than raising the count blindly.
  - **QUIT slow.** Time each phase from the log. `final_cookie_pull` waits `QUIT_PULL_WAIT` (1 s) for the stuck tab's cookies, and that is expected.

- [ ] **Step 3: Commit:** `scripts: unresponsive-probe.sh checks not responding, restart and quit with a stuck WebView2 engine`. Paste the PASS lines into the report.

---

### Task 9: Docs, full gate, and the merge question

**Model:** controller. **Kind:** judgment. **Proof:** the whole suite and the full gate pass; the report carries CPU load and Defender state.

**Files:**
- Modify: `README.md`: a "Fixed (2026-10-xx, wave 3b)" entry beside the wave 3a one in "Known limitations"; the `unresponsive-probe.sh` paragraph beside `sleep-probe.sh`'s in "Headless mode"
- Modify: `.superpowers/survey/engines.md`: mark E1 and E3 done; mark the wake half of E9 done and its engine half parked; record the parked items
- Modify: this plan's `## Outcome`

- [ ] **Step 1: README entry.** One paragraph saying:
  - A send is queued and written by a per-tab writer thread. An engine that stops reading no longer freezes the app, and neither does closing its tab or quitting.
  - Pointer moves and resizes merge (latest wins) and wheel steps add up while an engine is behind; nothing else merges or reorders.
  - A tab whose queue fills, or whose write is stuck for 2 s, shows a warning badge and a "Not responding" toast, and its menu offers Restart Engine and End Engine (headless `RESTARTENGINE` and `ENDENGINE`). Nothing is killed automatically. Messages refused while the queue was full are lost; a restart starts from the tab's URL.
  - Frames go to one slot per tab and other engine messages through a bounded channel; one drain applies at most 64 per tab; readers wake the loop.
  - The engine's own inbox is bounded (if Task 7 shipped). CEF, Servo and WebKit were not built or run with it. **Rebuild every engine host after pulling.** The wire format is unchanged and `PROTOCOL_VERSION` is not bumped, but a host built before still has an unbounded inbox.
  - A WebKit tab force-closed while its writer is stuck leaves that thread until the Linux host exits (measured: closing the Windows-side socket does not unblock it).
  - **Parked, with reasons:** E2 frame path (user decision); engine-host-side sends (user decision); the engine-side blocking waits of E9 (engine source changes); a separate host-to-engine connection for WebKit; a Restart affordance in Tab view.
- [ ] **Step 2: README headless paragraph.** "`scripts/unresponsive-probe.sh` suspends a WebView2 engine host and types into its page: the card must read `unresponsive` with a "Not responding" toast, `RESTARTENGINE` must bring it back, and `QUIT` with a second suspended host must end within 10 s. Not part of the gate (it suspends processes)."
- [ ] **Step 3: Survey.** In `.superpowers/survey/engines.md`:
  - E1 and E3: done, with the commits.
  - E9: the wake is done; the engine-side waits are parked.
  - E2: parked by the user.
  - Correct E3's `CookiesChanged` to `Cookies`.
- [ ] **Step 4: Build and gate.**
  1. `cargo build -j 4 -p manifold -p manifold-engine-webview2`.
  2. Before the gate:
     - check for leftovers with `Get-Process python, manifold, manifold-engine-webview2`;
     - check CPU with `(Get-Counter '\Processor(_Total)\% Processor Time' -SampleInterval 2 -MaxSamples 2).CounterSamples[-1].CookedValue`, under 20;
     - record `(Get-MpComputerStatus).RealTimeProtectionEnabled`.
  3. `cmd //c start "" //b //belownormal bash scripts/gate.sh LOG full`. Expect about 11 minutes; arm a notification on `GATE done` in `LOG` rather than polling.
- [ ] **Step 5: Full tests and lints.**
  - Run `cargo fmt --all --check`, the clippy line from Global Constraints, and `cargo test -j 4 -p manifold -p ipc-protocol -p manifold-engine-webview2`.
  - Confirm the Linux CI result after pushing, or run it under WSL (`CARGO_TARGET_DIR=$HOME/mf-target cargo test -p manifold -p ipc-protocol`), and report which.
- [ ] **Step 6: Outcome.** Fill the table below.
- [ ] **Step 7: Ask the user** whether to merge `wave-3b-engine-io` into `main`, whether to push, and whether to cut a release. Do none of these unprompted.
- [ ] **Step 8: Commit the docs:** `docs: wave 3b engine I/O (README, survey, plan outcome)`.

---

## Outcome

Run 2026-10-10 on branch `wave-3b-engine-io`, by subagent-driven development (per-task Sonnet reviews, final Opus review). Red first: the test failed on the old code before the change.

| Task | Commit | Red first |
|---|---|---|
| 1 One write per message | `d21e7d3` | `a_message_is_one_write` |
| 2 Writer thread, `EngineTab` send and close | `5967c23`, `ed8e6fe` | `sends_to_an_engine_that_stopped_reading_never_block` failed after 5.02 s |
| 3 Frame slot, bounded events channel | `b517a09` | both new tests ("a frame came first", "channel is unbounded") |
| 4 Wake and the per-drain cap | `1647c1a`, `aa3961c` | the cap test (left 0, right 10) |
| 5 Toast, Restart and End, headless commands | `68d9c5c` | engine_io test (compile error); the rest by absence of the new APIs |
| 6 Menu rows and badge | `ed31b77` | by absence of the new variants |
| 7 Engine-side inbox bound | `ed9d7a8` | "the inbox is unbounded" |
| 8 Probe and manual run | `eefa526` | n/a (8 of 8 checks PASS; badge screenshot seen) |
| 9 Docs and gate | `71ea375` (final-review fixes), docs commit | n/a (`gate.sh full` all steps exit 0 at 11:54, CPU 16% at start, Defender real-time off; `cargo test` 1108 + 47 + 9 pass) |

## Review rounds

Three rounds, each re-reading the code at `9fce9d8` against the plan's code and claims.

**Round 1: the plan's code against the source.**
- `a_dead_engine…` was folded into the closing test after `shutdown`, where "not unresponsive" holds trivially, because a closed writer never reads as unresponsive. It is now its own test, with no `shutdown`.
- The probe assumed the card id `t1`. `sleep-probe.sh` reads the id from `EXPORTSTATE` because the import may remap it, so the probe now does the same.
- The probe exported state in Tab view, and more than 4 s (`SAVE_TOAST_DURATION`) after the stall, so it could read no cards and no toast. It now runs `ESCAPE` first and exports within about 2 s.
- 400 typed characters overflowed the pipe plus the queue by a thin margin, so the probe now types 1000. The second (QUIT) phase did not check that the tab was stuck, and now it does.
- The help test's name was guessed. It is `help_and_parser_agree`.
- Checked and found correct:
  - `TabPayload`'s fields and `test_node`
  - `NodeMenuItem::Button(&str, NodeAction)` and the `view_of` literal; the only other `NodeMenuView` literal, at `node_menu/tests.rs:432`, uses `..view_of`
  - `push_centered_text`, `chain_tooltip`, `tab_thumb_rect`, `DANGER_COLOR` and `WARNING_CIRCLE`, already used together at `card.rs:1375`
  - `failed_cards`, `succeed_spawn` and `test_spawns` tuples
  - `toast_url`
  - `UserEvent::Debug(DebugCommand::Help)`
  - `spawn_tab`'s one caller
  - `MAX_MESSAGE_BYTES` and the `channel.rs` imports
  - `LocalTransport` and `RemoteTransport` as `MessageChannel` aliases

**Round 2: the brief's list of hazards.**
- **Queue full during the seed wait.** `SetCookies` is the first message queued, and nothing else can be sent before `TabSpawned` hands the tab over.
- **Resize race.** Only consecutive resizes merge; the latest size and DPI win; frames carry their own size.
- **`Shutdown` versus merged items.** `push_last` never merges, and `send` drops everything after it.
- **Writer panic or EOF.** Handled by the `Finished` guard; there is a test for each.
- **Restart and respawn.** A stuck writer is freed by the kill, and `listen_local_pair_with_retry` covers a late pipe close.
- **WebKit.** Its stuck writer can leak (Decision 3).
- **Headless.** Decision 15.
- **App exit.** Decision 12.
- **Many tabs.** Decision 14.
- Three findings from this round:
  - A wake applies messages outside a redraw. Checked that `about_to_wait` recomputes the pacing after the event, so no redraw request is needed (Decision 10).
  - A spawn still in flight at exit is beyond `begin_engine_shutdown`'s reach. Its cost is bounded and recorded (Decision 12).
  - Open question 1 now names the slow-start route to a false positive concretely.

**Round 3: a full pass.** No material findings. It covered:
- every interface named in a task's Produces against its uses in later tasks;
- dead code at each commit under `-D warnings`: each task's new items are used in that task;
- that each red-first test fails for the stated reason on the code before its task;
- that `wedge` needs an unread peer at every call site;
- the step order inside Tasks 2 to 5.

## Self-review

- **Spec coverage.** Every point of the brief maps to a task:
  - one writer thread per tab with a bounded FIFO: Task 2
  - non-blocking `send`: Task 2
  - merging, with one `coalesce` table: Decision 7, Task 2
  - a full queue sets unresponsive, plus the stuck-write rule: Decision 4, Task 2
  - `Shutdown` with a deadline, then kill: Decision 12, Task 2
  - one write per message: Task 1
  - frame slot and bounded channel, with a blocking reader: Task 3
  - at most K per drain, and the proxy wake: Task 4
  - badge, toast, and the user's choice of Restart or End: Tasks 5 and 6
  - E2 and engine-side sends parked: "Survey items", Task 9
  - manual check of the badge, headless: Task 8
  - full gate: Task 9
- **Placeholders.** None. Each step has its code or an exact edit, its command and its expected result. Task 9's README and survey prose is described, not quoted, because it records outcomes known only after the run.
- **Type consistency.** `Writer::send(HostToEngine)` takes ownership and `EngineTab::send(&HostToEngine)` clones; `Wake` is `Arc<dyn Fn() + Send + Sync>` in every signature; `FrameSlot` and `Frame` are named the same in Tasks 3 and 4; `LeftClosed` has three variants at all six uses (the definition and five call sites).
- **Not verified here, said where it applies.** CEF, Servo and WebKit under Tasks 1 and 7 (Global Constraints, Task 7, README entry). The constants (Open question 4).

## Open questions

1. **Ship Task 7 (the engine-side inbox bound)?**
   - Without it, the app no longer freezes, but "Not responding" appears only when a whole engine process is suspended or its reader thread wedges. A deadlocked engine main loop would still just grow the engine's memory.
   - With it, a stuck main loop is caught too. The risk is false "Not responding" toasts from an engine that pauses draining legitimately. The likeliest case is a slow start: the engine's reader runs from connect, but its main loop takes only the seed (`wait_for_first_message`) until init ends, so pointer moves over a newly opened tab can fill the 256-message inbox in 2-4 s at 60-120 Hz, and 2 s more with a write blocked raises the badge. A nested modal loop is the other case. CEF, Servo and WebKit cannot be checked here.
   - Recommendation: ship it. The badge clears itself and nothing is killed.
2. **Should a stall count only after the engine has answered once?** That would be a flag set by the reader on its first message, which would remove any startup false positive at the cost of one more cross-thread signal. Not planned, and only worth it if the probe or use shows startup toasts.
3. **A Restart affordance in Tab view.** The badge and the menu rows are on the Graph card, so a user in Tab view only sees the toast. Parked until wanted.
4. **`SEND_QUEUE_CAP = 256`, `UNRESPONSIVE_AFTER = 2 s`, `ENGINE_MESSAGES_PER_DRAIN = 64`** were chosen by reasoning, not measured. Keep them unless the probe or use says otherwise.

**Resolved by the user (2026-10-10, "as recommended"):** 1 ship Task 7; 2 no first-message gate; 3 Tab-view Restart parked; 4 constants kept, lost-when-full accepted; 5 WebKit stuck-writer leak accepted and parked (separate send connection).

## Final review (Opus, whole branch)

No Critical or Important findings. Fixed: M2 (a refused wake post left the coalescing flag set and muted later wakes) and M4 (the quit cookie pull waited its full second on tabs that had stopped reading; quit with a stuck tab went from about 4 s to about 2 s), plus M1's wrong claim in Decision 12, a test comment that overstated what `fake_fed` covers, and a stale `reap_host` comment. **Since fixed (M3):** a healthy engine can be flagged (and drop input) when more than 256 unmergeable messages queue before the writer catches up, e.g. a long headless `TYPE`; a refusal now counts as unresponsive only once the write in flight has been blocked for 50 ms (`REFUSED_BLOCKED_AFTER`). **Also fixed (minor):** M5 (closing several tabs at once, i.e. a group, Close All, Close Selected, Close Others and replacing the session, starts all their shutdowns first, `App::begin_tabs_shutdown`, so the flush waits share one deadline), M6 (`wedge` pushes until a refusal happens while a write is blocked), M8 (poison-tolerant test locks), M9 (`Writer::send` takes `&HostToEngine` and clones only after the closed check).
