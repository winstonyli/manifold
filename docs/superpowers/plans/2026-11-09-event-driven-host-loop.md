---
status: active
---

# Event-driven host loop, WebView2 first: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** the WebView2 host stops polling. It blocks until a control message, a window message (including a captured frame) or a real deadline arrives, and it sends every changed frame instead of at most one per 33 ms. A probe reports each host's frame rate, polls and idle CPU, and a guard fails on any new `thread::sleep(` in a host.

**Architecture:** decision u1 (2026-11-09). `spawn_host_reader` takes a plain wake closure and calls it after every message it forwards. Each host has one `idle(deadline)` closure that pumps its engine and waits; `await_cookies` and the main loop both call it. On Windows the wait is one `WakeEvent` in `manifold-view-protocol`: an auto-reset event plus a high-resolution waitable timer, waited on with `MsgWaitForMultipleObjectsEx`, which dispatches the thread's window messages after waking. CEF, Servo and WebKitGTK only take the new signature in this plan and keep their sleeps; each converts in its own later plan.

**Tech Stack:** Rust; `windows-sys` 0.59 in the protocol crate, `windows` 0.62 in the WebView2 host; the gate's probe harness (`crates/gate/src/probes.rs`).

**Spec:** `PARKED.md`, "Native feel (2026-11-08)", the entry "Host loops poll on a sleep, and WebView2 sends at most one frame per 33 ms" (its *Shape*, *Deletes*, *Cannot cover* and *Risks* sub-bullets), plus decision u1 and the timer probe below. No separate spec file exists.

## Evidence this plan rests on

- **Timer probe (v2), run `2026-11-09-timer-quantization`:** 40 waits per row, medians in ms, CPU 60% from other sessions. A wait's millisecond timeout rounds up to the 15.6 ms system tick; a high-resolution waitable timer in the handle list does not. A wake from another thread costs about 0.05 ms.

  | Requested | std sleep | `MsgWait` timeout | `MsgWait` + hi-res timer | after `timeBeginPeriod(1)` |
  |---|---|---|---|---|
  | 1 | 1.54 | 15.54 | 1.60 | 1.54 |
  | 4 | 4.51 | 15.62 | 4.42 | 4.36 |
  | 8 | 8.25 | 15.48 | 8.28 | 8.54 |
  | 16 | 16.27 | 30.80 | 16.44 | 16.48 |

  Wake latency: `SetEvent` median 0.05 ms (max 0.31); `PostThreadMessage` 0.06 ms (max 1.11). So `WakeEvent` uses the timer handle, not the timeout argument, and not `timeBeginPeriod`, which changes the whole process's timer rate. The probe source stays in the session scratchpad; the numbers above are its record.
- **t1 (2026-11-09):** WGC sends no frames on a static page, so dropping `FRAME_INTERVAL` costs nothing there.
- **WGC frames arrive as window messages.** The frame pool is `Direct3D11CaptureFramePool::Create` (not `CreateFreeThreaded`, `capture.rs:50-56`) on the thread's `DispatcherQueue` (`CreateDispatcherQueueController` with `DQTYPE_THREAD_CURRENT`, `main.rs:~123-141`), so `FrameArrived` (`capture.rs:62-79`) runs only while the loop's thread dispatches messages. The same holds for the WebView2 event handlers that fill the `pending_*` cells.

## Global Constraints

- `cargo fmt --all` and `cargo lint -j 8 -- -D warnings` pass before every commit (pre-commit hook; never `--no-verify`).
- No `#[allow]` without `reason = "..."`.
- A host's startup variable is named once, as a constant in `manifold_view_protocol::host_env`; other files, the gate included, use the constant, never the literal (gate `host_variables_are_named_only_in_host_env`).
- `crates/gate/src/limits.rs` file and test-block limits hold; a file over the limit is split, not its ceiling raised.
- No test reads or writes the real `%APPDATA%\manifold`.
- Runs over about a minute go through `~/.claude/runs/run.ps1` with a record.
- No compatibility shims: callers change with the signature, in the same commit.
- Python never reads stdin: `python -P f.py < /dev/null`.
- Commits end with the session's attribution line. Nothing is pushed.
- Before each commit, check `git diff --stat` and stage by path or hunk (`git add -p`): `PARKED.md` and `docs/runs.md` may hold other sessions' edits, and `run.ps1` writes `docs/runs.md` rows during the work. The plan file itself is committed before execution starts.

## End-state fit

| Change | Target location | Replaces | Deletes |
|---|---|---|---|
| Frame log | `manifold-view-protocol/src/frame_sink.rs` `FrameSink::take`, one `[frame]` line per call when `host_env::ENV_FRAME_STATS` is set; every host sends through `take`, so every host is counted | no host frame or poll count anywhere | nothing |
| Rate probe | `crates/gate/src/probes.rs` `hostrate`, a by-hand stage in `gate/src/main.rs` | no repo measurement of host frame rate or host CPU | nothing |
| Process helpers | `crates/gate/src/procs.rs`, moved out of `kill_check.rs` as their own commit | private helpers in `kill_check.rs` | nothing |
| Sleep guard | `crates/gate/src/limits.rs` `host_loops_do_not_sleep` | nothing | nothing |
| `WakeEvent` | new `manifold-view-protocol/src/wake.rs` (`cfg(windows)`), beside `job.rs` | WebView2's `window::pump_messages` and its 8 ms sleep | `window::pump_messages`; the `main.rs:530` sleep |
| Wake closure on the reader | `manifold-view-protocol/src/transport.rs` `spawn_host_reader` | the reader that only forwards | nothing |
| `idle(deadline)` in the startup wait | `host_env.rs` `await_cookies`, which takes in the loop of `transport::wait_for_first_message` | `pump: impl FnMut()` plus an 8 ms sleep | `wait_for_first_message` and its sleep at `transport.rs:298` |
| Uncapped frames | WebView2 `main.rs` loop | `FRAME_INTERVAL`, `last_frame_sent` | both |
| Tests out of `transport.rs` | `manifold-view-protocol/src/transport/tests.rs` | the inline block at its `TEST_ALLOWED` ceiling (274) | its `TEST_ALLOWED` entry |

Stopgaps this plan creates, each parked in the commit that makes it (Task 4): CEF, Servo and WebKitGTK pass a no-op wake closure and keep their loop sleeps; CEF's startup `idle` keeps an 8 ms sleep.

## Considered and not done

- **`WakeEvent` without a timer or deadline** (round-1 fewer-concepts reviewer). With the stall check left as is, WebView2's only deadline is the 1 s device check, equal to the backstop, so `wait()` with a fixed 1 s timeout would do here. Not done: u1 fixed `idle(deadline)` as the one shape for every host, and CEF's pump delay and Servo's `park_timeout` need sub-tick deadlines in their plans. The timer costs one handle and two tests now and saves a signature change later.
- **The stall check as a deadline** (`StallWatch::next_expiry`, a `StallCheck` return value). Dropped: the loop turns at least once a second (device check and backstop), so a stall on a 60 s timeout, or the gate's 3 s one, is seen at most 1 s late. Nothing needs better.
- **`hostrate` in `gate all`.** Not done: its animating floor depends on machine load and the gate runs hosts at BelowNormal, so it would be flaky in every tier; the cheap permanent guard is the sleep test. A frame throttle without a sleep (a `FRAME_INTERVAL` comeback) is the one regression only the probe catches, so it is run by hand at Task 5 and whenever a host's loop changes.
- **A hidden-tab phase.** Not done: hidden is the separate v3 item, and the loop already skips the frame block when `visible` is false.
- **A per-turn counter in each host's loop** (round 1 had WebView2 alone print `[loop]`). Replaced by the count in `FrameSink::take`, which all four hosts already call (`webview2/main.rs:514`, `cef/main.rs:480`, `servo/main.rs:366`, `webkitgtk/main.rs:654`), so no host gets its own counter. WebKitGTK calls it only behind its own `FRAME_INTERVAL` (`main.rs:653`), so its poll count is not a turn count until its plan. Cannot cover: a turn on which a host does not call `take` (WebView2 skips it while hidden), which no check here needs.
- **A calibrated animating floor** (a scratch build with `FRAME_INTERVAL` at zero before Task 5). Dropped: the floor only has to sit above the 30 frames/s the cap allows for the baseline to be red, and Task 5's green run measures the real uncapped rate.
- **Process helpers left in `kill_check.rs` as `pub(crate)`** (round-4 fewer-concepts reviewer). Not done: `snapshot`, `creation_time` and `cpu_time` are process queries with two users, and a file named for the kill check is the wrong home for them; the move is one mechanical commit.
- **No backstop or clamp** (round-5 fewer-concepts reviewer: no caller here passes a deadline over 1 s). Not done: u1 fixed the ~1 s backstop as part of the shape, and the CEF and Servo plans will pass engine deadlines of any length; it costs one constant, one test and one mutation.
- **The thread's message queue as the only wake source** (round 6: `set()` posts `WM_NULL` to the host thread, dropping the event handle). Not done: a thread message has no window, so a nested or foreign pump (`wait_for_async_operation` in `set_cookies`) dispatches it to nothing, and a `Shutdown` wake landing there would be lost until the backstop; the event keeps it.
- **Kept: an `await_cookies_within` seam.** `COOKIE_SEED_WAIT` is 5 s, and the deadline-stop test would otherwise take that long.
- **A separate "the host logs [frame] lines" check.** Dropped in round 5: the print is one wrapper shared by both phases, so a missing print fails the same view's animating floor.
- **A still-page CPU ceiling.** Not done: the poll ceiling already fails a loop that spins or polls on a clock, and CPU varies with load where a poll count does not.
- **The key-latency A/B.** Left as a follow-up after this plan: the goal is no polling and every frame sent, which `hostrate` shows; keylat needs two release builds, a timed lease and the foreground.

## Review Focus

1. **A message queued while the loop body runs must not be lost.** `wait` never dispatches before blocking; `MWMO_INPUTAVAILABLE` returns at once for anything queued, even a message a nested pump has already peeked at, and the dispatch after waking runs it before the next turn. Pinned by Task 3's `a_message_queued_before_the_wait_ends_it`. One case stays open by assumption, with no automated check (a missing `set` costs at most 1 s and passes the gate): a call inside a turn that itself dispatches (an STA COM call) fills its cell after the loop looked, and the next wait sleeps until the device check, up to 1 s. One such call is known: the loop's `SetCookies` arm, whose `set_cookies` (`main.rs:~655-680`) waits at `:663` in `wait_for_async_operation`, which pumps messages through `webview2_com`; Task 5 sets the event after it. Any other found later gets the same one line.
2. **A timer left signaled by an earlier wait must not end the next one early.** Pinned by Task 3's `a_stale_timer_does_not_end_the_next_wait`.
3. **An orphaned host must still wake, and a wake must never come before its message.** When manifold dies, the reader sends a `Shutdown` it made up itself; without a wake the host would never see it, and a wake sent before the message would find an empty inbox and sleep up to the backstop. The reader has one send-and-wake site that every message, made-up `Shutdown`s included, goes through. Pinned by Task 4's `every_wake_finds_its_message` (connection drop), `a_wrong_version_from_the_host_becomes_shutdown` (version skew) and `a_host_gone_before_its_version_wakes` (version read failure), and for WebView2's hookup of the closure by `hostrate`'s control-latency check (Task 1 Step 4), which fails if the host passes a no-op wake. A loop that never reaches `WakeEvent::wait` because it still polls answers quickly and passes it; that case belongs to the poll ceiling and `host_loops_do_not_sleep`. The startup `idle` handed to `await_cookies` is not covered by it (the trials run long after startup); a `|_| {}` there would spin without pumping until the cookies came, and is unowned.
4. **A burst of messages after one wake.** The auto-reset event collapses many wakes into one return, so the loop must drain the inbox until it is empty before the next wait. The drain holds by construction (the `while let ... rx.try_recv()` at `webview2/main.rs:284`); Task 3's `many_wakes_before_the_wait_end_one_wait` pins only the collapse. The drain itself is unowned: an `if let` there passes every test and `hostrate` (one control message per trial), and would leave a burst's tail waiting up to the 1 s backstop.
5. **Frames uncapped on an animating page reach the app at the compositor rate.** The app's one-frame slot and the sink's damage check should absorb them. Task 5's probe checks the host's frames per second; no check owns the app's own CPU under that load, and Task 6's full gate only shows the app still passes.

---

### Task 1: Frame log and the rate probe (baseline)

Before Step 1, set this plan's front matter to `status: active` as its own commit: `plan: event-driven host loop active`.

**Files:**
- Create: `crates/gate/src/procs.rs` (moved from `kill_check.rs`)
- Modify: `crates/gate/src/kill_check.rs`, `crates/gate/src/main.rs` (`mod procs;`, the stage), `crates/gate/src/probes.rs` (`hostrate`, one `Site` route)
- Modify: `crates/manifold-view-protocol/src/host_env.rs` (`ENV_FRAME_STATS`, `frame_stats()`), `crates/manifold-view-protocol/src/frame_sink.rs` (the `[frame]` line)
- Modify: `README.md` (a `gate hostrate` paragraph after `gate bad-url`'s, `README.md:~216`: the phases, `VIEWS`, the four checks, by hand and not in `gate all`)

**Interfaces:**
- Produces: `host_env::ENV_FRAME_STATS: &str = "VIEW_FRAME_STATS"`; `pub fn host_env::frame_stats() -> bool`, true when the variable is set to anything, read once (`OnceLock`).
- Produces: with `frame_stats()` on, every `FrameSink::take` call prints `[frame] 1` when it returns `Some` and `[frame] 0` when it returns `None` (`eprintln!`, unbuffered, into the host log). `take`'s current body becomes a private `take_inner`, and `take` wraps it, so the line covers every return, the `?` exits at `self.store.as_ref()?` and `self.changed.take()?` included. A `[frame]` line counts a poll: CEF and Servo call `take` on every visible turn (`cef/main.rs:477-484`, `servo/main.rs:365-369`), WebView2 today only once 33 ms have passed since the last captured frame (`main.rs:500`) and after Task 5 on every visible turn. So polls equal turns for CEF, Servo and WebView2's still phase; the animating phase's polls are printed but not compared across the change.
- Produces: `procs::Proc { pub(crate) pid, ppid, created, name }`, `procs::snapshot()`, `procs::creation_time(pid)`, and `procs::cpu_time(pid: u32) -> Option<Duration>` (kernel plus user). `creation_time` and `cpu_time` share one private `process_times(pid)` around `GetProcessTimes`.

- [ ] **Step 1: Move the process helpers.** Move `Proc`, `snapshot` and `creation_time` from `kill_check.rs` to `procs.rs`, with the `windows_sys` imports they use; make the struct and its fields `pub(crate)`. `kill_check.rs` (including `terminate`, `suspend` and its tests' `p()` constructor) uses `crate::procs`. Add `#[cfg(windows)] mod procs;` to `main.rs`, beside `#[cfg(windows)] mod kill_check;` (`probes.rs` already uses the Windows-only `instance` unconditionally, and `unit-linux` leaves the gate out, so `hostrate` needs no `cfg` of its own). Add `cpu_time` and the shared `process_times`, with one test in `procs.rs`, `cpu_time_of_this_process_rises`: `cpu_time(std::process::id())` is `Some`, and after a busy loop that spins for 200 ms of wall time it has risen by at least 15 ms (one process-time tick), so a constant, zero or kernel-only stub fails. Run `cargo test -p gate kill_check` and `cargo test -p gate procs`; expect both to pass. Commit: `gate: process helpers to procs.rs, with cpu_time, for a second user`.
- [ ] **Step 2: Implement** `ENV_FRAME_STATS`, `frame_stats()` and the wrapper. The label comes from a private `frame_label(&Option<ViewToHost>) -> &'static str` (the type `take_inner` returns), tested by `frame_label_tells_a_frame_from_a_poll` (`Some` gives `"1"`, `None` gives `"0"`): after Task 5 nothing else tells the two apart, since a still page polls about once a second either way. The line's presence is checked by `hostrate`'s animating floor, which a missing line fails at 0 frames/s. Run `cargo test -p manifold-view-protocol` and `cargo test -p gate limits`; expect both to pass. The new test grows `frame_sink.rs`'s inline block past its `TEST_ALLOWED` entry (273, `limits.rs:27`), so first move the whole block to `frame_sink/tests.rs` (leaving `#[cfg(test)] mod tests;`) and delete that entry, the same mechanical move as Task 4 Step 1's for `transport.rs`.
- [ ] **Step 3: Add `/animate` to `Site::start`'s route match** (`probes.rs:83`, its `match path` at `:100`; beside the `/text` arm at `:138`), and name it in `Site`'s doc comment and the module header: a page whose pixels change on every frame, so `FrameSink`'s damage check drops none: a full-viewport element with a hard-edged `conic-gradient` background (a few colored sectors) rotated by a CSS `transform` animation (`animation: spin 1s infinite linear`), larger than the viewport so its edges stay out of view. A uniform box would change no pixels and send nothing. The still page is the existing `/text` (`TEXT`, `probes.rs:60-63`: plain text, no input, caret or animation).
- [ ] **Step 4: Write `hostrate`,** a hand-written per-view loop shaped like `bad_url` (`probes.rs:484-545`), not `for_each_view` (which takes no environment). For each view in `views()` and each phase in `[("animate", "/animate"), ("still", "/text")]`, one launch:
  - `Section::start(&format!("hostrate-{view}-{phase}"))`, `guarded`, a per-case `dir` and `dir/tmp`; seed one tab on the phase's URL (`tab_node`).
  - `Instance::launch_env(exe, &profile, &dir, "log", &[("TEMP", ..), ("TMP", ..), (host_env::ENV_FRAME_STATS, "1")])`.
  - Bring the tab up as `wheel` does (`probes.rs:353-358`): `settle`, `step(&["GRAPHDOWN", "GRAPHOPEN"], .., || site.hits(path) > 0)`, `export_until(.., State::tab_view)`, `WAITIDLE`. Without `GRAPHOPEN` the imported tab sleeps in Graph view and nothing is measured.
  - Find the host: the instance's child process whose name starts with `manifold-view-`, created after the instance (`procs::snapshot`; the first three conditions of `kill_check.rs`'s `host_with_engine`, without its engine-subprocess requirement, so not that function itself). Find its log with `host_logs(&tmp)`; the case fails unless there is exactly one (a stray second log would make the count ambiguous; a respawned host reuses the tab's log, `process_manager.rs:706-715`, and is caught by the window's host-exit check instead).
  - Window: record the log's byte length, the host's `cpu_time` and `Instant::now()`; wait `HOSTRATE_WINDOW` (a private `const` in `probes.rs`, 10 s; not an environment variable) on a deadline loop that also fails fast if the host exits; record the three again. Read the log bytes between the two lengths, cut at the last newline so only complete lines count, and count the `[frame] ` lines (polls) and the `[frame] 1` lines (frames).
  - Still phase only, after the window: the control-latency check. Five times, alternating `/tall` and `/text`: send `ADDRESSBAR {site.url(path)}` and `WAITIDLE` (as `sleep.rs:200-202` does: the typed text needs two frames to land, and a URL typed over a load still pending is lost), then `inst.barrier()` so the app has finished both; record `before = site.hits(path)`; then wait (on a deadline loop, 5 s) for a new complete `[frame] ` line in the host log that arrives at least 800 ms after the line before it, which marks a device-check turn after a quiet stretch (a line from a turn some window message woke, such as a late load event, would leave a no-wake host less than 1 s to its next check); if 3 s pass with lines arriving more often than that (a polling host, as in the baseline), take the next new line, and at once send `ENTER` and start the clock; wait on a deadline loop (5 s) until `site.hits(path) > before`, and stop the clock. Then `WAITIDLE` again before the next trial, so the previous load is done. Print the five times. Sending right after a turn is the worst case for a host with no wake: it sees the message only at its next device check, about 1 s later, so every trial reads about 1 s minus the app's round trip; a host that wakes on its inbox answers in tens of milliseconds. The log-growth read reuses the window step's reader (the line is `eprintln!`, so it reaches the log unbuffered).
  - `quit(&mut inst, &label)`, then `register_log` each host log, as `bad_url` does, so a failing case prints the log tail.
  - Print `hostrate {view} {phase}: {frames/s:.1} frames/s, {polls/s:.0} polls/s, host cpu {pct:.2}% of a core`. The CPU covers the `manifold-view-*` process only, which is where the loop runs; the engine's own processes (`msedgewebview2`, CEF's renderer and GPU processes) do the page's work and are left out, for every view alike.

  Checks: `hostrate {view}: an animating page sends at least {ANIMATE_FLOOR} frames/s` and `hostrate {view}: a still page sends at most 2 frames/s`, with `const ANIMATE_FLOOR: f64 = 35.0`: above the 30 frames/s `FRAME_INTERVAL` allows, so the baseline must fail. A third check, `hostrate {view}: a still page polls between 0.5 and {STILL_POLL_CEILING} times/s`, with `const STILL_POLL_CEILING: f64 = 20.0` (today about 125, the 8 ms sleep; the target is about 1, the device check): it is the check that the loop stopped polling, and it fails a loop that spins or waits on a clock shorter than 50 ms however it is spelled, which the sleep guard cannot see. The 0.5 floor proves the `[frame] 0` line still prints (every host polls at least once a second today; a host whose own plan makes a still page poll less lowers it there). A fourth, `hostrate {view}: the host answers a control message within 400 ms`, fails when the median of the five times above is 400 ms or more (the median, so one slow navigation on a loaded machine does not fail a correct host; the red case reads about 1 s in every trial); it passes on today's 8 ms sleep too, so it is a guard on Task 5's wake hookup, not part of the red test. The animating floor also proves the print exists: a host that logs no `[frame]` line sends 0 frames/s there, and the failure message says "no [frame] lines" when the window holds none. CPU is printed, not checked. Register `Stage::work("hostrate", crate::probes::hostrate)` in `main.rs` beside `bad-url`, and do not add it to `all.rs`.
- [ ] **Step 5: Baseline.** Build with `cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate` (the gate refuses a stale pdfium host, `fresh.rs`). Read the run's recorded CPU load before its frame rates: the gate runs hosts at BelowNormal, and a loaded machine lowers them. From bash, run `powershell -NoProfile -File "$USERPROFILE/.claude/runs/run.ps1" -Kind probe -Name hostrate-baseline -Command 'target\debug\gate.exe hostrate'` (the wrapper runs `-Command` under `cmd /c`, so backslashes; `views()` defaults to `webview2`). Expected: the animating check FAILS at about 25-30 frames/s (PARKED measured 25.8-27.6 under the cap), and the poll ceiling FAILS at about 125 polls/s; the still frame check and the control-latency check pass. Set the row's headline by hand from the `hostrate ...` lines in `out.log`. These two failures are the red test for Task 5.
- [ ] **Step 6: The other hosts' idle numbers.** Step 2 changed `frame_sink.rs`, which every host links, so each host is rebuilt and its runtime re-synced first, or it prints no `[frame]` lines (or `fresh.rs` refuses a stale CEF runtime). CEF (needs the CEF download; else say "CEF not run"): `-Name hostrate-cef-baseline -Command 'set "VIEWS=cef"& cargo build -j 8 -p manifold-view-cef && target\debug\gate.exe sync-runtime-deps && target\debug\gate.exe hostrate'` (the `set` goes first: cmd reads `a && b & c` as `(a && b) & c`, so a later `&` would run the probe after a failed build). Servo, only if `target-servo\debug\manifold-view-servo.exe` exists (README "Building Servo on its own"; a warm rebuild is minutes, a cold one about 33; else say "Servo not run"): `-Name hostrate-servo-baseline -Command 'set "VIEWS=servo"& set "CARGO_TARGET_DIR=target-servo"& cargo build -j 8 -p manifold-view-servo && set "CARGO_TARGET_DIR=" && target\debug\gate.exe sync-runtime-deps && target\debug\gate.exe hostrate'`. A run that may pass the tool's 10-minute foreground limit (a cold Servo or CEF build) takes `-Detach` and a watcher on its `run.json` (RUNS.md); give the others an explicit timeout. Check `peak_commit_mb` and raise `-MaxMemGB` only if a build hit the cap. Record still-page polls/s and CPU in the headline. Their animating and poll checks are expected to fail and are printed for the record; a failure there is a finding for their own plans, not this one. WebKitGTK runs only under WSL's gate and is left out; say so.
- [ ] **Step 7:** Run `cargo unit -j 8` and `cargo lint -j 8 -- -D warnings`. Commit: `gate hostrate: frame rate, polls and host CPU per phase from FrameSink's [frame] lines, run by hand; WebView2 baseline fails the animating floor`.

### Task 2: Guard against sleeps in hosts

**Files:**
- Modify: `crates/gate/src/limits.rs`

**Interfaces:**
- Produces: `const HOST_SLEEPS: &[(&str, usize)]` and the test `host_loops_do_not_sleep`.

- [ ] **Step 1: Write the test.** For each production source whose path starts with `manifold-view-` (every host crate, `manifold-view-pdfium` and any later one included), except `manifold-view-protocol/` where only `src/host_env.rs` is scanned, count `thread::sleep(` in `source.lines().take(measure(&source).0)`, skipping lines that start with `//` after trimming, as `catch_unwind_lives_only_in_the_worker_pool` does (`sources()` drops test files but not inline test blocks). Fail, naming the file and both counts, when a count differs from its `HOST_SLEEPS` entry (0 when absent). An entry whose file is gone also fails, so the list only shrinks. Starting list:

```rust
const HOST_SLEEPS: &[(&str, usize)] = &[
    ("manifold-view-cef/src/main.rs", 1),        // loop poll, until the CEF plan
    ("manifold-view-servo/src/main.rs", 1),      // loop poll, until the Servo plan
    ("manifold-view-webkitgtk/src/main.rs", 1),  // loop poll, until a paint signal is found
    ("manifold-view-webview2/src/main.rs", 2),   // loop poll (Task 5 removes it); profile-busy retry at startup
    ("manifold-view-webview2/src/window.rs", 1), // taskbar retry on its own thread, not the loop
];
```

  `host_env.rs` is listed by absence at 0: Task 4 moves the startup wait's loop there. `transport.rs` stays out: its two connect-retry sleeps (`:130`, `:266`) are not a host loop. The doc comment says: a host's loop waits on its engine and its inbox (`WakeEvent`, or the engine's own wait), never on a clock; a count rises only with a parked stopgap beside it, and otherwise only falls; the scan matches the literal `thread::sleep(` (every sleep in the host crates is spelled that way today), so an imported bare `sleep(` would slip past it, and other clock waits (`recv_timeout`, `park_timeout`, a short `MsgWait` timeout) shorter than 50 ms are caught only when `hostrate` is run by hand (its still-page poll ceiling; Task 5, and whenever a host's loop changes) (none is in a host crate today), as is a swap that keeps a file's count (a startup sleep removed and a loop sleep added).
- [ ] **Step 2:** Run `cargo test -p gate limits`; expect a pass. Add a `std::thread::sleep(Duration::ZERO);` to `manifold-view-webview2/src/capture.rs` as a scratch edit, check the test fails naming that file, then `git checkout -- crates/manifold-view-webview2/src/capture.rs`. Likewise lower one `HOST_SLEEPS` entry by 1, and rename one listed path, checking each fails; undo both by hand, not with `git checkout` (the new test in `limits.rs` is uncommitted until Step 3), and check `git diff crates/gate/src/limits.rs` shows only Step 1's change.
- [ ] **Step 3:** Commit: `limits: host loops do not sleep; the six current sleeps are listed, and a count rises only with a parked stopgap`.

### Task 3: `WakeEvent`

**Files:**
- Create: `crates/manifold-view-protocol/src/wake.rs` (tests inline; well under 250 lines)
- Modify: `crates/manifold-view-protocol/src/lib.rs` (`#[cfg(windows)] pub mod wake;`), `crates/manifold-view-protocol/Cargo.toml` (add the `windows-sys` feature `Win32_UI_WindowsAndMessaging`, and for the test's window `Win32_Graphics_Gdi` (`WNDCLASSW`, `RegisterClassW`) and `Win32_System_LibraryLoader` (`GetModuleHandleW`); `Win32_Foundation`, `Win32_Security` and `Win32_System_Threading` are already on)

**Interfaces:**
- Produces, all `cfg(windows)`:
  - `pub struct WakeEvent`, owning an auto-reset event (`CreateEventW`, not manual reset, initially unsignaled) and a high-resolution synchronization timer (`CreateWaitableTimerExW` with flags exactly `CREATE_WAITABLE_TIMER_HIGH_RESOLUTION`, no `CREATE_WAITABLE_TIMER_MANUAL_RESET`, and `TIMER_ALL_ACCESS`). Each handle sits in a small owned type that closes it on drop. `WakeEvent` is `Send + Sync` (`unsafe impl` with a `// SAFETY:` line: kernel event and timer handles may be used from any thread); hosts share it as `Arc<WakeEvent>`.
  - `WakeEvent::new() -> std::io::Result<WakeEvent>`. A failure is `io::Error::last_os_error()`; the host `expect`s it like its other startup objects.
  - `WakeEvent::set(&self)`: `SetEvent`. Any thread.
  - `WakeEvent::wait(&self, deadline: Instant)`, called only on the thread whose window messages it dispatches (the doc says so):
    1. Clamp `deadline` to `now + WAIT_BACKSTOP`. If it has passed, go to step 4.
    2. Arm the timer for the time left, rounded up to 100 ns (`SetWaitableTimer` with a negative due time, no period). This also resets a signal left from an earlier arm; if `a_stale_timer_does_not_end_the_next_wait` shows it does not, call `CancelWaitableTimer` first (same feature). A zero return panics with `io::Error::last_os_error()`, like a failed startup object: a host that cannot time its waits is broken.
    3. `MsgWaitForMultipleObjectsEx(2, [event, timer], INFINITE, QS_ALLINPUT, MWMO_INPUTAVAILABLE)`. `WAIT_FAILED` panics the same way, rather than returning at once and letting the loop spin. The other returns (the event, the timer, input) all go to step 4.
    4. Dispatch every queued message (`PeekMessageW` with `PM_REMOVE`, `TranslateMessage`, `DispatchMessageW`), then return.
  - `const WAIT_BACKSTOP: Duration = Duration::from_secs(1)`, private; `wait` and the module's tests use it.
- The module doc says why each part is there:
  - No dispatch before waiting: a message the caller's last turn did not see would be handled with nobody looking at its result, and the wait would then block on an empty queue. `MWMO_INPUTAVAILABLE` returns at once for anything already queued, and step 4 runs it.
  - The dispatch after waking runs the engine's callbacks (WGC `FrameArrived`, WebView2's events) before the caller looks at their results. It is also what WebView2's message routing and the `DispatcherQueue` need (the point of the doc on WebView2's old message pump; the doc does not spell its old name, so Task 5's grep stays clean).
  - The timer is there because the timeout argument rounds up to the 15.6 ms tick (run `2026-11-09-timer-quantization`).
  - The backstop turns a forgotten deadline into at most a second of latency.

- [ ] **Step 1: Write the failing tests** in `wake.rs`. Timing bounds allow for the timer and `Instant` running on different clocks:

```rust
#[test]
fn a_wake_from_another_thread_ends_the_wait() {
    let ev = Arc::new(WakeEvent::new().unwrap());
    let e = ev.clone();
    let t = std::thread::spawn(move || { std::thread::sleep(Duration::from_millis(20)); e.set(); });
    let start = Instant::now();
    ev.wait(start + Duration::from_secs(5));
    assert!(start.elapsed() < Duration::from_millis(500));
    t.join().unwrap();
}

#[test]
fn many_wakes_before_the_wait_end_one_wait() {
    let ev = WakeEvent::new().unwrap();
    for _ in 0..100 { ev.set(); }
    let start = Instant::now();
    ev.wait(start + Duration::from_secs(5)); // the wakes are kept
    assert!(start.elapsed() < Duration::from_millis(500));
    let start = Instant::now();
    ev.wait(start + Duration::from_millis(50));
    assert!(start.elapsed() >= Duration::from_millis(40));
}

#[test]
fn the_deadline_is_not_rounded_to_the_system_tick() {
    let ev = WakeEvent::new().unwrap();
    // best of 5, so load cannot fail it; the tick would give at least ~15 ms every time. A regression guard only
    // while no process has raised the system timer rate (timeBeginPeriod); hostrate is the end-to-end check.
    let best = (0..5).map(|_| { let s = Instant::now(); ev.wait(s + Duration::from_millis(4)); s.elapsed() }).min().unwrap();
    assert!(best >= Duration::from_micros(3500) && best < Duration::from_millis(10), "{best:?}");
}

#[test]
fn a_stale_timer_does_not_end_the_next_wait() {
    let ev = WakeEvent::new().unwrap();
    ev.set();
    ev.wait(Instant::now() + Duration::from_millis(5)); // ends on the event; the timer stays armed
    std::thread::sleep(Duration::from_millis(20));       // and fires with nobody waiting
    let start = Instant::now();
    ev.wait(start + Duration::from_millis(50));
    assert!(start.elapsed() >= Duration::from_millis(40), "{:?}", start.elapsed());
}

#[test]
fn a_past_deadline_returns_at_once() {
    let ev = WakeEvent::new().unwrap();
    let start = Instant::now();
    ev.wait(start - Duration::from_millis(1)); // a no-panic guard (deadline in the past) more than a timing one
    assert!(start.elapsed() < Duration::from_millis(500));
}

#[test]
fn a_far_deadline_waits_only_the_backstop() {
    let ev = WakeEvent::new().unwrap();
    let start = Instant::now();
    ev.wait(start + Duration::from_secs(60));
    let took = start.elapsed();
    assert!(took >= WAIT_BACKSTOP - Duration::from_millis(5) && took < WAIT_BACKSTOP + Duration::from_millis(500), "{took:?}");
}

#[test]
fn a_posted_message_ends_the_wait_and_is_dispatched() {
    // the test thread creates a message-only window (CreateWindowExW with HWND_MESSAGE, a registered class whose
    // wndproc sets a static AtomicBool on WM_USER and otherwise calls DefWindowProcW); another thread posts WM_USER
    // to that hwnd with PostMessageW after 20 ms (HWND is not Send: pass it as isize). wait(now + 5 s) returns
    // within 500 ms, the flag is set, and PeekMessageW(PM_NOREMOVE) finds nothing. A thread message (null hwnd)
    // would not do: DispatchMessageW calls no window procedure for it, so a drain without dispatch would pass.
}

#[test]
fn a_message_queued_before_the_wait_ends_it() {
    // one PeekMessageW(PM_NOREMOVE) creates this thread's queue; the thread posts WM_USER to itself
    // (PostThreadMessageW with GetCurrentThreadId, asserting it returned nonzero), then peeks it with
    // PeekMessageW(WM_USER, WM_USER, PM_NOREMOVE) so it counts as seen, as a nested pump inside a turn would.
    // wait(now + 5 s) returns within 500 ms (without MWMO_INPUTAVAILABLE it would wait the 1 s backstop),
    // and the queue is empty afterwards.
}
```

- [ ] **Step 2:** Run `cargo test -p manifold-view-protocol wake`; expect a compile failure (no module).
- [ ] **Step 3: Implement** `wake.rs` to the interface above.
- [ ] **Step 4:** Run `cargo test -p manifold-view-protocol wake` three times; expect all eight to pass each time (the backstop test takes about 1 s). Note the CPU load.
- [ ] **Step 5:** Run `cargo lint -j 8 -- -D warnings`. Commit: `protocol: WakeEvent, a wait on an event, the thread's window messages and a high-resolution deadline (the timeout argument rounds up to the 15.6 ms tick)`.
- [ ] **Step 6: Each test catches its feature.** In scratch edits, one at a time: drop the timer from the handle list (pass the time left as the timeout), drop `MWMO_INPUTAVAILABLE`, remove only `DispatchMessageW` (keep the `PM_REMOVE` drain), re-arm the timer only when the new deadline is earlier than the last one armed, drop the clamp, make the event manual-reset, dispatch the queue before the `MsgWait`. After each, run the module's tests and check the matching test fails (in order: `the_deadline_is_not_rounded_to_the_system_tick`, `a_message_queued_before_the_wait_ends_it`, `a_posted_message_ends_the_wait_and_is_dispatched`, `a_stale_timer_does_not_end_the_next_wait` (the tick test may fail too), `a_far_deadline_waits_only_the_backstop` (which waits its full 60 s before failing), `many_wakes_before_the_wait_end_one_wait`, `a_message_queued_before_the_wait_ends_it` again). The first mutation is caught only while the system timer runs at 15.6 ms: if the tick test still passes, check the timer resolution (Sysinternals ClockRes, or `NtQueryTimerResolution` from a scratch program) and record that some process had raised it, rather than adding a test. Revert with `git checkout -- crates/manifold-view-protocol/src/wake.rs` after each (committed in Step 5), and end with `git status` clean. A removal no test catches is a missing test: add it.

### Task 4: The wake closure and the deadline-taking startup wait

**Files:**
- Create: `crates/manifold-view-protocol/src/transport/tests.rs` (Step 1, its own commit)
- Modify: `crates/manifold-view-protocol/src/transport.rs` (`spawn_host_reader`; delete `wait_for_first_message`), `crates/manifold-view-protocol/src/host_env.rs` (`await_cookies` and its tests)
- Modify: `crates/manifold-view-webview2/src/main.rs:159,245`, `crates/manifold-view-cef/src/main.rs:154,188`, `crates/manifold-view-servo/src/main.rs:164`, `crates/manifold-view-webkitgtk/src/main.rs:82`
- Modify: `crates/gate/src/limits.rs` (`TEST_ALLOWED` loses `transport.rs`; `HOST_SLEEPS` CEF `main.rs` 1 to 2)
- Modify: `PARKED.md` (the stopgap entry)

**Interfaces:**
- Consumes: `WakeEvent::{new, set, wait}` (Task 3).
- Produces: `pub fn spawn_host_reader<S: Read + Write + Send + 'static>(from_host: MessageChannel<S>, wake: impl Fn() + Send + 'static) -> mpsc::Receiver<HostToView>`. The reader has one `tx.send` followed by `wake()`, whatever the send returned: the handshake's outcome becomes an optional first message (version skew and a failed version read both give `Some(Shutdown)` and end the reader after it is sent; a good version gives `None`), sent through the same site as every later message and the connection-drop `Shutdown`.
- Produces: `HostEnv::await_cookies(&self, rx: &mpsc::Receiver<HostToView>, idle: impl FnMut(Instant)) -> StartupMessages`, a thin call to a private `await_cookies_within` that takes the timeout (`host_env.rs` imports `Instant` beside `Duration`). Its loop, moved in from `wait_for_first_message`: `try_recv`; on `Empty`, stop with nothing if the deadline (`COOKIE_SEED_WAIT` from the call) has passed, else `idle(deadline)`; on `Disconnected`, stop with nothing. It sleeps nowhere.

- [ ] **Step 1: Move `transport.rs`'s tests** (the `#[cfg(test)] mod tests` block at `:306` and `token_tests` at `:534`) into one file `transport/tests.rs`: drop the `mod tests { }` wrapper, merge `token_tests`'s tests in (no names collide), keep one `use super::*;`, and leave `#[cfg(test)] mod tests;` behind; and delete `transport.rs`'s `TEST_ALLOWED` entry. Run `cargo test -p manifold-view-protocol transport` and `cargo test -p gate limits`; expect the same tests to pass. Commit: `protocol: transport tests to their own file, off the test-block ceiling`.
- [ ] **Step 2: Write the failing tests.** In `transport/tests.rs`, built on a real local pair as the existing reader tests are:

```rust
/// A wake closure that, at each wake, takes one message from the inbox and records what it found: a wake that
/// comes before its message, a second wake for one message, or a wake while the slot is still empty records `None`.
fn recording_wake() -> (Arc<Mutex<Option<mpsc::Receiver<HostToView>>>>, Arc<Mutex<Vec<Option<HostToView>>>>, impl Fn() + Send + 'static);

fn wait_for(seen: &Mutex<Vec<Option<HostToView>>>, n: usize) -> Vec<Option<HostToView>> {
    // polls with a deadline; returns what was recorded
}

#[test]
fn every_wake_finds_its_message() {
    // order: create the pair, spawn_host_reader, install the receiver in the slot, and only then let the host
    // end write the version and 3 messages and close, so the reader adds its own Shutdown
    wait_for(&seen, 4);
    // then wait until the slot's receiver reports Disconnected (the reader has exited, so no later wake can
    // come), and only then read the records: a fifth, stray wake would show here
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 4);
    assert!(seen[..3].iter().all(Option::is_some), "{seen:?}");
    assert!(matches!(seen[3], Some(HostToView::Shutdown)), "{seen:?}");
}
```

  `HostToView` may not be `PartialEq`; compare by `matches!`, and record a `bool` plus the last message if `Debug` is missing. The receiver slot exists because `spawn_host_reader` returns the receiver after taking the closure.

  The new test brings `use std::sync::{Arc, Mutex};`. `a_wrong_version_from_the_host_becomes_shutdown` uses `recording_wake` too, in the same order (the receiver is in the slot before the host end writes its wrong version); it waits for one record, then until the slot's receiver reports `Disconnected` (the reader thread has exited, so no later wake can come), and asserts exactly `[Some(Shutdown)]`. `every_wake_finds_its_message` asserts its count after the same `Disconnected` wait. A third, `a_host_gone_before_its_version_wakes` (the host end closes without writing anything), does the same and asserts exactly `[Some(Shutdown)]`, so all three made-up `Shutdown`s are pinned; the other reader tests (`a_dropped_host_becomes_shutdown`, `the_view_inbox_is_bounded_and_loses_nothing`) pass `|| {}`. The two `wait_for_first_message` tests (`the_first_message_wait_pumps_until_one_arrives` and its empty-channel sibling) are deleted from `transport/tests.rs` and become `await_cookies` tests in `host_env.rs` (an env with `await_cookies: true`):
  - `await_cookies_idles_until_the_cookies_arrive`: `idle` records its argument and sends `SetCookies("[]")` on its third call; the result has those cookies, `idle` ran exactly 3 times, all three deadlines are equal, and the first is at least `COOKIE_SEED_WAIT` minus 100 ms after the call began.
  - `await_cookies_stops_at_its_deadline`: through a private `await_cookies_within(rx, timeout, idle)` that `await_cookies` calls with `COOKIE_SEED_WAIT`, a 50 ms timeout and an `idle` that sleeps 1 ms and panics after 2,000 calls (so a missing deadline check fails and stops instead of spinning for the rest of the suite), run on a spawned thread that sends its result back: the test receives an empty result within 1 s (`recv_timeout`), with the sender still alive, so a missing deadline check fails instead of hanging the suite.

  The existing `await_cookies` tests (`host_env.rs:202,205,219`) pass `|_| {}`.
- [ ] **Step 3:** Run `cargo test -p manifold-view-protocol`; expect a compile failure (the extra arguments).
- [ ] **Step 4: Implement** both signatures, delete `wait_for_first_message`, and update every caller:
  - WebView2: create `let wake = Arc::new(WakeEvent::new().expect("wake event"));` before `connect_local_pair`; pass `{ let w = wake.clone(); move || w.set() }` to `spawn_host_reader` and `|d| wake.wait(d)` to `await_cookies`; add `use std::sync::Arc;` (the file imports none today). Its loop is unchanged until Task 5 (it still pumps and sleeps), so nothing spins between the two commits.
  - CEF: `|| {}` and `|_| { do_message_loop_work(); std::thread::sleep(Duration::from_millis(8)) }`, which is today's startup behavior except that the pump now runs only while the inbox is empty (the old wait pumped before each `try_recv`; the main loop pumps on its first turn); `HOST_SLEEPS` for CEF `main.rs` becomes 2.
  - Servo and WebKitGTK: `|| {}` (neither calls `await_cookies`).
  - Doc comments: `await_cookies`'s (`host_env.rs:~82-84`, "calling `pump` between polls") describes `idle(deadline)`; `spawn_host_reader`'s (`transport.rs:~40-49`) describes the wake closure and the one send-and-wake site.
- [ ] **Step 5:** Run `cargo test -p manifold-view-protocol`, `cargo test -p manifold-view-webview2` and `cargo test -p gate limits`; expect all to pass. Check the hosts the default build leaves out:
  - CEF: `cargo check -j 8 -p manifold-view-cef` (needs the CEF download; if missing, say so and skip).
  - Servo, through the wrapper from bash (a cold check can take tens of minutes, so add `-Detach` and a watcher on `run.json` as in Task 1 Step 6): `powershell -NoProfile -File "$USERPROFILE/.claude/runs/run.ps1" -Kind probe -Name servo-check -Command 'set "CARGO_TARGET_DIR=target-servo"& cargo check -j 8 -p manifold-view-servo'`; check `peak_commit_mb` before raising `-MaxMemGB`.
  - WebKitGTK, from git-bash: `MSYS_NO_PATHCONV=1 wsl.exe -d Ubuntu -- bash -c 'cd "/mnt/c/Users/Winston Li/Documents/GitHub/manifold" && CARGO_TARGET_DIR=target-linux cargo check -p manifold-view-webkitgtk'`.

  Report each that could not run.
- [ ] **Step 6: Park the stopgap** in the same commit: a `PARKED.md` entry under "Native feel". *What:* CEF, Servo and WebKitGTK pass a no-op wake and keep their loop sleeps; CEF's startup idle keeps an 8 ms sleep. *Why parked:* each host's wait is its own plan. *Trigger:* that plan (CEF: `on_schedule_message_pump_work`; Servo: its `EventLoopWaker`; WebKitGTK: the Linux paint-signal spike). *Evidence:* the call sites above, with the date. *Status:* parked. Link it from the host-loop entry.
- [ ] **Step 7:** Commit: `protocol: the reader wakes its host after every message, and await_cookies hands idle its deadline; WebView2 waits on WakeEvent at startup, other hosts pass a no-op (parked)`.
- [ ] **Step 8: Each test catches its feature.** After Step 7's commit, as scratch edits reverted with `git checkout -- <file>` after each (the work is committed, so the checkout restores it): move `wake()` before the send (expect `every_wake_finds_its_message` to fail); send the version-failure `Shutdown` without the wake (expect `a_host_gone_before_its_version_wakes` to fail); drop `await_cookies_within`'s deadline check (expect `await_cookies_stops_at_its_deadline` to fail, not hang). End with `git status` showing only files that were dirty before Task 4.

### Task 5: WebView2 waits instead of polling, and sends every frame

**Files:**
- Modify: `crates/manifold-view-webview2/src/main.rs` (loop end; delete `FRAME_INTERVAL`, `last_frame_sent` and the sleep)
- Modify: `crates/manifold-view-webview2/src/window.rs` (delete `pump_messages` and the imports only it used)
- Modify: `crates/gate/src/limits.rs` (`HOST_SLEEPS` WebView2 `main.rs` 2 to 1)

**Interfaces:**
- Consumes: the `wake: Arc<WakeEvent>` created in Task 4; the `[frame]` line (Task 1).

- [ ] **Step 1: Convert the loop.**
  - Delete the leading `window::pump_messages()`. From turn 2 on, the previous turn's wait has dispatched everything queued. On turn 1 messages queued during startup may still wait (`await_cookies` returns without idling when there is nothing to await or a message is already there); the first wait returns at once for them (`MWMO_INPUTAVAILABLE`) and dispatches them, so the cost is one extra turn.
  - Drop the `last_frame_sent.elapsed() >= FRAME_INTERVAL` condition, so the frame block runs whenever `visible`. Replace the comment above the old `last_frame_sent` with one line: the damage check and the app's one-frame slot absorb unchanged frames, and a still page sends none (t1, 2026-11-09).
  - Keep the device check on its 1 s clock and the stall check once per turn, as today.
  - End the turn with `wake.wait(last_device_check + DEVICE_CHECK_INTERVAL)` in place of the sleep.
  - Move `pump_messages`'s doc point to the comment where `main` creates the `WakeEvent`, then delete `pump_messages`. The `CoInitializeEx` comment near `main.rs:116` that names `window::pump_messages` names `WakeEvent::wait`.
  - Comments: the module header (`main.rs:1-3`, "fixed-tick poll loop") says the loop waits on `WakeEvent`; WebKitGTK's `main.rs:19`, which calls its constant "as WebView2's own `FRAME_INTERVAL`", drops the comparison (its constant stays until its plan).
  - Make the loop's `HostToView::SetCookies` arm (`main.rs:337`, one expression today) a block, `{ seed_cookies(&webview, in_private, &json); wake.set(); }`: `set_cookies` (`main.rs:~655-680`) waits at `:663` in `wait_for_async_operation`, a nested message loop whose handlers fill cells after the turn read them, so the next wait must return at once rather than sleep up to the device check.
  - Set `HOST_SLEEPS` for WebView2 `main.rs` to 1.
- [ ] **Step 2: Unit checks.** Run `cargo test -p manifold-view-webview2` and `cargo test -p gate limits`; expect both to pass. `grep -rn "FRAME_INTERVAL\|last_frame_sent" crates/manifold-view-webview2`, `grep -rn pump_messages crates/` and `grep -rn PeekMessageW crates/manifold-view-webview2/src`; expect no matches (the dispatch loop lives only in `WakeEvent`).
- [ ] **Step 3: The red test turns green.** Build and run as in Task 1 Step 5, with `-Name hostrate-wait`. Expected: every check passes: animate at least `ANIMATE_FLOOR` frames/s; still at most 2 frames/s and at most `STILL_POLL_CEILING` polls/s (about 1, the device check, instead of about 125); host CPU is recorded, not checked: the still phase's should fall, and the animating phase's should rise with the frame rate. If animate fails, find the cause before changing anything (systematic-debugging): compare polls with frames, and check whether `FrameArrived` fires. If the animating rate lands under 45 frames/s, report it rather than lowering the floor: the change may not have bought the frames it is for.
- [ ] **Step 4:** Run `cargo unit -j 8` and `cargo lint -j 8 -- -D warnings`. Commit: `webview2: the loop waits on WakeEvent until a message, a window message or the device check, and sends every frame (FRAME_INTERVAL, pump_messages and the 8 ms sleep are gone)`.
- [ ] **Step 5: The latency check catches a missing wake.** After Step 4's commit, as a scratch edit, pass `|| {}` instead of the `wake.set()` closure to WebView2's `spawn_host_reader`, rebuild, and run `hostrate` again (`-Name hostrate-nowake`). Expect `the host answers a control message within 400 ms` to FAIL, with a median near 1 s. Revert with `git checkout -- crates/manifold-view-webview2/src/main.rs`, rebuild, and end with `git status` clean. If it does not fail, find what else wakes a still host (count polls in that run) before relying on the check. Record both runs' five times in the devlog entry.

### Task 6: Gate and record

**Files:**
- Modify: `PARKED.md` (the host-loop entry), `docs/devlog.md` (one entry with the run ids), `docs/runs.md` (rows written by the wrapper), this plan's front matter

- [ ] **Step 1: Gate.** From bash, one wrapped run that builds and gates (an engine change, so `full`): `powershell -NoProfile -File "$USERPROFILE/.claude/runs/run.ps1" -Kind gate -Name host-loop-gate -Detach -Command 'cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate && target\debug\gate.exe all "%RUN_DIR%\gate-log.txt" full'`, with a watcher armed on its `run.json`; set the row's headline from the log's last lines. Report CPU load and Defender state with the result. Make no tree edits while it runs.
- [ ] **Step 2: Record.**
  - The devlog entry, the one home for the numbers: the `hostrate-baseline`, `hostrate-wait`, `hostrate-nowake` (its five latency times beside `hostrate-wait`'s) and other hosts' runs (frames/s, still-page polls/s and CPU) and the gate run id.
  - The PARKED host-loop entry: WebView2 converted, linking the devlog entry; the other hosts point at Task 4's stopgap entry. Its *Deletes* line that says the stall check "becomes a deadline while a download is tracked" changes to: the stall check stays per turn, bounded by the device check and the backstop (this plan's "Considered and not done").
  - This plan's `status: done` (it became `active` when Task 1 started).
- [ ] **Step 3:** Commit: `host loop: WebView2 converted; hostrate and gate results recorded`.

## Self-review notes

- **Coverage of the PARKED entry's *Deletes* for WebView2:** the `main.rs:530` sleep, `FRAME_INTERVAL` and its last-sent time, `pump_messages`, and the startup wait's sleep (with `wait_for_first_message`) are covered. The per-turn stall check stays, by decision above. `NoopWaker` and the other hosts' sleeps belong to their plans.
- **The user's asks:** frame counter (Task 1's `[frame]` lines from `FrameSink`, every host), idle CPU per host (Task 1 Steps 5-6 for every built host, Task 5 Step 3 for WebView2 after), sleep guard (Task 2).
- **Order:** `WakeEvent` (Task 3) lands before the signature change (Task 4), so WebView2's startup wait never spins.
- **Interfaces:** `idle: impl FnMut(Instant)` and `wake: impl Fn() + Send + 'static` are spelled the same in Tasks 4 and 5; `WakeEvent::{new, set, wait}` the same in Tasks 3 to 5.
