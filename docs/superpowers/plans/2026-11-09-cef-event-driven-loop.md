---
status: done
---

# Event-driven CEF host loop: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** the CEF host stops polling. Its loop blocks until a control message, a pump request from CEF or its audio-poll deadline, so a still page turns about 10 times a second instead of 116, and nothing in the CEF host sleeps.

**Architecture:** the WebView2 shape (plan `2026-11-09-event-driven-host-loop.md`, done) applied to CEF. One `Arc<WakeEvent>` is made before `ViewApp`, and three things share it:
- the reader's wake closure;
- CEF's `on_schedule_message_pump_work`, which only calls `set()`;
- one `idle(deadline)` closure, `wake.wait(deadline)` then `do_message_loop_work()`, which `await_cookies` and the end of the loop both call.

`WakeEvent` gains a non-Windows implementation in the same file, so the CEF host has no platform branch (decision c1).

**Tech Stack:** Rust; the `cef` crate 152.1.0 (CEF 152.0.6); `windows-sys` 0.59 in the protocol crate; std `Mutex` and `Condvar` elsewhere; `gate hostrate` and the CEF gate stages.

**Spec:** `PARKED.md`, the entries "Host loops poll on a sleep; WebView2 waits since 2026-11-09" (its *Shape*) and "Hosts other than WebView2 still poll" (its CEF trigger), plus the spike below and decisions b and c. No separate spec file exists.

## Evidence this plan rests on

Survey: workflow `wf_37caadc4-02c`, 2026-11-09, read-only. Four readers covered the loop map, the CEF pump API, the Windows queue, and the tests and gate; an Opus critic then spot-checked them and proposed the shape with the fewest concepts. The spike was a throwaway edit of `app.rs`, `main.rs` and `wake.rs` at `bef4f41a`, reverted with `git checkout bef4f41a -- <files>`, and the tree was checked to match.

- **CEF signals only immediate work.** Across every spike run, CEF called `on_schedule_message_pump_work` 7,266 times, and `delay_ms` was always 0.
  - hostrate on an animating and a still page: 2,956 calls.
  - 27 host logs from the CEF stages below: 4,310 calls.
  - None was positive or negative, and most came from CEF's own threads.
  - So CEF 152 never asks for a delayed pump. A browser-process task with a delay runs on the next turn, whatever ends that turn's wait.
  - cefclient's reference pump (`tests/shared/browser/main_message_loop_external_pump.cc`, read 2026-11-09) covers this with `kMaxTimerDelay = 1000 / 30`. That timer is re-armed after every pump that leaves no timer pending, so cefclient turns at least 30 times a second forever.
- **Dispatched messages:** none on CEF's behalf. `dispatch_queued` dispatched one message in each hostrate phase (`Chrome_WidgetWin_0`, 0x31f).
  - So under `external_message_pump` CEF posts no `WM_USER+1` to its own message window on this thread.
  - There is no starvation risk from an uncapped drain, and `WakeEvent`'s dispatch costs nothing for CEF.
- **hostrate with the spike shape** (two scratch runs, `VIEWS=cef`, about 30 s each, not recorded; Task 3 repeats them recorded). Baseline is run `2026-11-09-hostrate-cef-baseline`.

  | | Baseline | Spike |
  |---|---|---|
  | Animating page | 57.0 fps, 116 polls/s | 59.9-60.1 fps, 175-178 polls/s, host CPU 6.9-9.2% |
  | Still page | 116 polls/s | 10-11 polls/s |
  | Answer median | 11 ms | 7-11 ms |

  On the still page, every gap over 200 ms ends at the 1 s audio deadline, and the bursts are the latency trials.
- **Falsified:** the same spike with the no-op reader wake restored fails `gate hostrate`'s latency check, with a median of 903 ms against the 400 ms bound. So the check can catch a missing wake on CEF.
- **CEF stages on the spike** (run `2026-11-09-cef-pump-spike-stages`, exit 0), all passing:
  - `download` (saved, refused, broken and stalled);
  - `wheel`, `key`, `page-kind`, `sleep`, `page-text`, `kill-check`;
  - `jar` (`RUNS="login fresh cef webview2"`).

  `gate all full` with `WITH_ENGINES=cef` runs only `download`, `wheel`, `kill-check` and `jar` on CEF (`crates/gate/src/all.rs:222-260`), so Task 3 repeats the rest as their own recorded batch.
- **The order is wait, then pump.** The pump runs at the end of `idle`, so the next turn's drain and cell reads see what that pump's callbacks did, as today's pump-then-drain does. "Pump, then wait" would leave a callback's result unread until the next wake.
- **CEF runs on Linux,** but nothing checks it there.
  - Its subprocesses were checked under WSL2 on 2026-10-02 (`main.rs:104-110`).
  - No check builds the CEF host on Linux: `.cargo/config.toml:14` `unit-linux` is `manifold`, `manifold-hosts` and `manifold-view-protocol`, and CI runs that alias (`.github/workflows/ci.yml:24`).
  - `wake` is `#[cfg(windows)]` today (`lib.rs:14-15`).

## Global Constraints

- `cargo fmt --all` and `cargo lint -j 8 -- -D warnings` pass before every commit (pre-commit hook; never `--no-verify`).
- `cargo lint` and `cargo unit` leave out `manifold-view-cef`, so every CEF change also runs `cargo clippy -j 8 -p manifold-view-cef -- -D warnings` and `cargo test -j 8 -p manifold-view-cef`.
- No `#[allow]` without `reason = "..."`.
- `crates/gate/src/limits.rs` limits hold: production files at most 1,200 lines, inline `mod tests` under 250 lines, and `HOST_SLEEPS` exact.
- No compatibility shims: callers change with the signature, in the same commit.
- Runs over about a minute go through `~/.claude/runs/run.ps1` with a record (launched from bash). After a CEF build, run `target/debug/gate.exe sync-runtime-deps` (CEF runs from `target/debug/cef-runtime/`). No tree edits while a gate or stage batch runs.
- WSL builds run from `/mnt/c/Users/Winston Li/Documents/GitHub/manifold` with `CARGO_TARGET_DIR=$HOME/mf-target`, kept off `/mnt/c` as README.md:21 advises (`target-linux` on `/mnt/c` is the WebKit host's). From git-bash, set `MSYS_NO_PATHCONV=1`.
- Python never reads stdin (`python -P f.py < /dev/null`).

## Decisions (b1 and c1 chosen 2026-11-09)

- **b1:** the callback ignores `delay_ms` and adds no fallback timer. The alternatives:
  - b2, a due-time cell, would add a `Mutex<Option<Instant>>` and a `min` into `idle`'s deadline in Task 2.
  - b3, cefclient's 33 ms cap, would also raise `STILL_POLL_CEILING`.
- **c1:** a portable `WakeEvent`. The alternatives:
  - c2, a CEF-local wait, replaces Task 1 with a `cfg(not(windows))` wait inside the CEF crate.
  - c3, keeping a Linux sleep, deletes Task 1. CEF then keeps a `cfg(not(windows))` 8 ms sleep, its `HOST_SLEEPS` entry stays at 1, and a parked entry is needed.

## End-state fit

- **`crates/manifold-view-protocol/src/wake.rs`** stays the one file that owns `WakeEvent`. It holds:
  - the module doc, reduced to the contract (set, wait, backstop);
  - `WAIT_BACKSTOP` and one shared `fn wait_for(deadline: Instant) -> Duration`, the clamp both platforms use;
  - `#[cfg(windows)] mod imp`, today's event, timer, `Owned` and `dispatch_queued`, with the Windows reasoning from the old module doc;
  - `#[cfg(not(windows))] mod imp`, a `Mutex<bool>` and `Condvar`;
  - `pub use imp::WakeEvent`;
  - the shared `#[cfg(test)] mod tests`, which imports only std, and after it a `#[cfg(all(test, windows))] mod windows_tests` with the four message and timer tests.

  It replaces nothing on Linux, where no host waits yet. The file grows from 306 to about 350 lines. `limits.rs` measures the test block from `#[cfg(test)] mod tests {` to the end of the file (`limits.rs:73-80`), so `windows_tests` must come after it. Both modules together count toward the 250 (about 200), and the `thread::sleep` calls in the Windows tests stay out of the `HOST_SLEEPS` scan. `lib.rs` drops `#[cfg(windows)]` from `pub mod wake`. The `windows-sys` comment in the protocol `Cargo.toml:19-21` keeps pointing at `wake.rs`, which stays right.
- **`crates/manifold-view-cef/src/main.rs`:**
  - Replaces: the per-turn loop-top `do_message_loop_work()` (one call stays, before `loop`), the loop's `sleep(8 ms)`, the startup pump-and-sleep closure given to `await_cookies`, and the no-op `|| {}` given to `spawn_host_reader`, together with its "No wake yet" comment.
  - Adds: one `Arc<WakeEvent>` made before `ViewApp::new`, and one `idle` closure.
- **`crates/manifold-view-cef/src/app.rs`:** `ViewApp` and `ViewBrowserProcessHandler` carry an `Arc<WakeEvent>`. `impl BrowserProcessHandler` gains `on_schedule_message_pump_work`, replacing the default no-op that dropped CEF's requests.
  - Subprocesses make the wake too. It costs one event and one timer handle each, and nothing calls it there, because CEF asks for `browser_process_handler` only in the browser process. That cost is accepted in place of an `Option`.
- **`crates/gate/src/limits.rs`:** deletes the `("manifold-view-cef/src/main.rs", 2)` row of `HOST_SLEEPS`.
- **Docs and comments:**
  - `PARKED.md` "Hosts other than WebView2 still poll" is renamed "Servo and WebKitGTK still poll", and the comments citing it (`manifold-view-servo/src/main.rs:163-164`, `manifold-view-webkitgtk/src/main.rs:82`) are updated.
  - The host-loop entry's title, *Shape* (CEF's `delay_ms`, PARKED.md:202) and *Deletes* (`cef :502`, :203) say what this plan did.
  - A new parked entry for CEF's unsignaled delayed work (decision b's trigger).
  - A `docs/devlog.md` entry with the spike and the recorded runs.
- **Deletes nothing else.** `AUDIO_PLAYING_POLL_INTERVAL`, `SHUTDOWN_GRACE`, `shutdown_due`, `jar_cookies`' timeouts and `StallWatch` stay. They are checked once per turn, and the 1 s backstop bounds them, as in the WebView2 plan's "Considered and not done".

## Considered and not done

- **An `Option<Arc<WakeEvent>>` made only in the browser process.** It saves two kernel handles per subprocess, at the cost of an `Option` on two constructors, a conditional and an `expect`.
- **A `static OnceLock<WakeEvent>` in the CEF crate.** It removes the constructor parameters, but it is a global, and WebView2 passes its `Arc` the same way this plan does.
- **`wake/windows.rs` and `wake/condvar.rs` as separate files.** The file is far below the limits, and two `mod imp` blocks keep one owner.
- **Explicit deadlines for the download stall, the cookie pull and the shutdown grace.** They are checked once per turn as today, and the 1 s backstop bounds how late each can be. The stall is 60 s, the pull gives up at 2 s or 8 s, and the grace is 3 s. Neither `StallWatch` nor `JarPull` has a next-expiry accessor to add. The audio poll is the one fed as the deadline: at the backstop alone its period would wander between 1 and 2 s.
- **`wake.set()` in each CEF handler** (`on_paint`, the nav handlers, process messages). They run inside `do_message_loop_work`, and the next turn reads their cells straight after the pump, so a wake from them would only end a wait that hasn't started.
- **Dropping the first turn's pump.** Today the loop pumps before every drain, the first one included, and the drain uses `browser.main_frame()` (`main.rs:238-240` drops a `Navigate` when there is none). The spike passed without that first pump, but no stage sends a `Navigate` as the first message. So Task 2 keeps one `do_message_loop_work()` before `loop`, which matches today's first turn exactly.
- **Dropping `ViewBrowserProcessHandler` and letting the builder hold the `Arc`.** It would save one struct, but `ViewRenderProcessHandler` keeps the same wrapper pattern beside it.
- **A cap on `dispatch_queued`.** CEF posts nothing to this thread (Evidence), so there is no queue to drain.
- **The `delay_ms` cell and the 33 ms cap:** decision b.

## Review Focus

1. **Shutdown when manifold died.** The reader's synthesized `Shutdown` wakes the wait (`transport.rs:60-68`), and `close_browser` then drives CEF's own pump requests until `on_before_close`. A loop that never turns again here is the orphan `SHUTDOWN_GRACE` exists for. The check is `kill-check-cef` in the gate's `full` mode.
2. **Startup before the browser exists.** `await_cookies` calls `idle` once per empty `try_recv`, and CEF's startup requests end those waits early. A wait that swallows the `SetCookies` wake would lose the seed. The check is `jar-cef`, the `login` and `fresh` runs.
3. **A hidden CEF tab.** `was_hidden(true)` stops paints, so the loop turns only for the audio deadline and CEF's own requests, and `sink.show()` resumes. **No check here can fail on it.**
   - No gate stage sends `SetVisible(false)`, and hostrate covers visible pages only. `gate sleep` sleeps and wakes tabs, which is a different path.
   - hostrate cannot count a hidden host's turns: its poll count comes from `FrameSink::take`, which `main.rs:481-488` skips while hidden, and `PHASES` (`hostrate.rs:42`) is animate and still only.
   - Task 3 Step 5 records the gap in PARKED "A hidden WebView2 tab still wakes per captured frame", whose fix would add a hidden phase. Until then, this is a reviewer item, read against `main.rs:282-291`.
4. **The Linux condvar wait.**
   - A `set()` before `wait()` must end that wait (the flag, not a missed notify).
   - Spurious wakeups must loop until the flag or the deadline.
   - Its tests and clippy run in WSL (Task 1), and CI's `unit-linux` runs the tests again.
5. **A browser-process delayed task.** It runs on the next turn (decision b), at most about 1 s late on an idle page. No stage shows one mattering (Evidence), and the parked entry names the trigger.

---

### Task 1: `WakeEvent` on every platform

**Files:**
- Modify: `crates/manifold-view-protocol/src/wake.rs`.
- Modify: `crates/manifold-view-protocol/src/lib.rs:14-15`, dropping `#[cfg(windows)]`.

**Interfaces:**
- Produces, unchanged on Windows: `pub struct WakeEvent` (`Send + Sync`), `pub fn new() -> io::Result<Self>`, `pub fn set(&self)`, `pub fn wait(&self, deadline: Instant)`.
  - On Windows, `wait` still dispatches this thread's messages after waking.
  - On other platforms it only waits: until a `set` (one kept from before the wait counts), or until `deadline` clamped to `WAIT_BACKSTOP` from now.

- [ ] **Step 0:** set this plan's front matter to `status: active` as its own commit, "plan: event-driven CEF loop active".
- [ ] **Step 1: Restructure on Windows, no behaviour change.**
  - Move today's Windows items into `#[cfg(windows)] mod imp`, add `pub use imp::WakeEvent;` and extract the clamp (`wake.rs:88-92`) into `fn wait_for(deadline: Instant) -> Duration` beside `WAIT_BACKSTOP`. The child module reaches both through `super::`, since they are private.
  - Move the Windows reasoning in the module doc (MWMO_INPUTAVAILABLE, the waitable timer, WGC, the DispatcherQueue) to `imp`'s doc. The top doc keeps the contract and adds one line: other platforms have no window messages, so their wait is a flag and a condvar.
  - Split the tests by what they import. `mod tests` (std only) keeps `a_wake_from_another_thread_ends_the_wait`, `many_wakes_before_the_wait_end_one_wait`, `a_later_deadline_after_a_wake_is_kept`, `a_past_deadline_returns_at_once` and `a_far_deadline_waits_only_the_backstop`.
  - `#[cfg(all(test, windows))] mod windows_tests`, placed after `mod tests` (see End-state fit), takes `a_posted_message_ends_the_wait_and_is_dispatched`, `a_message_queued_before_the_wait_ends_it`, `the_deadline_is_not_rounded_to_the_system_tick` and `a_stale_timer_does_not_end_the_next_wait`, with the `windows_sys` imports and `queued()`.
- [ ] **Step 2: Run** `cargo test -j 8 -p manifold-view-protocol wake::`. Expected: the same 9 tests pass as before Step 1 (5 in `wake::tests`, 4 in `wake::windows_tests`).
- [ ] **Step 3: Write the condvar `imp`:** `#[cfg(not(windows))] mod imp`, `WakeEvent { woken: Mutex<bool>, cv: Condvar }`.
  - `set` sets the flag and calls `notify_one`.
  - `wait` loops `wait_timeout` on what `wait_for` leaves until the flag is set or that time is spent, then clears the flag.
  - `new` returns `Ok`.
  - Drop `#[cfg(windows)]` in `lib.rs`.
- [ ] **Step 4: Linux check in WSL.** The first run is a cold build of the protocol crate, a minute or two, more if rustup installs the pinned nightly first (README.md:79). Run:
  ```bash
  MSYS_NO_PATHCONV=1 wsl.exe -d Ubuntu -- bash -lc 'cd "/mnt/c/Users/Winston Li/Documents/GitHub/manifold" && export CARGO_TARGET_DIR=$HOME/mf-target && cargo test -p manifold-view-protocol wake:: && cargo clippy -p manifold-view-protocol --all-targets -- -D warnings'
  ```
  Expected: 5 tests pass in `wake::tests`, and clippy is clean. Try the quoting once with `pwd` in place of the cargo commands before relying on it.
  - To show the tests can fail, once wait only on the condvar and ignore the flag (a scratch edit). `many_wakes_before_the_wait_end_one_wait` must then fail. Revert the edit and check `git diff` shows only the plan's changes.
- [ ] **Step 5: Run on Windows** `cargo unit` and `cargo lint -j 8 -- -D warnings`. Expected: pass. `cargo unit-linux` is CI's, and its cold `-p manifold` build stays there.
- [ ] **Step 6: Commit** "protocol wake: a condvar WakeEvent where there are no window messages".

### Task 2: CEF waits instead of polling

**Files:**
- Modify: `crates/manifold-view-cef/src/app.rs:1, 9-16, 58-62, 82-97`.
- Modify: `crates/manifold-view-cef/src/main.rs`: the imports (`:14-31`), `:84` (`ViewApp::new`), `:154-155` (reader), `:189-192` (startup idle), `:226` (the `shutting_down` comment), `:232` (loop-top pump), `:506` (sleep).
- Modify: `crates/gate/src/limits.rs:205`.
- Modify: `PARKED.md` (the stopgap's entry, Step 5).

**Interfaces:**
- Consumes: Task 1's `WakeEvent`; `HostEnv::await_cookies(&self, rx, idle: impl FnMut(Instant))`; `spawn_host_reader(from, wake: impl Fn() + Send + 'static)`.
- Produces: `ViewApp::new(wake: Arc<WakeEvent>)`; `ViewBrowserProcessHandler::new(wake: Arc<WakeEvent>)`.

- [ ] **Step 1: Write the failing check.** Delete the CEF row from `HOST_SLEEPS` and run `cargo test -j 8 -p gate limits`. Expected: FAIL, naming `manifold-view-cef/src/main.rs` with 2 sleeps found against 0 listed.
- [ ] **Step 2: Wire the wake.**
  - Both files import `std::sync::Arc` and `manifold_view_protocol::wake::WakeEvent`.
  - In `main.rs`, before `ViewApp::new` (`:84`), add `let wake = Arc::new(WakeEvent::new().expect("wake event"));` (as WebView2's `main.rs:155` does), and pass `wake.clone()`.
  - `ViewApp { wake }` hands a clone to `ViewBrowserProcessHandler::new` in `browser_process_handler`.
  - The method is `fn on_schedule_message_pump_work(&self, _delay_ms: i64) { self.handler.wake.set() }`. Its comment says three things:
    - CEF calls it from any thread;
    - on Windows, CEF 152 only asked for 0 (spike, 2026-11-09), and Linux was not measured;
    - a positive delay would be pumped at once, which CEF allows, and its delayed task then runs on a later turn (PARKED "CEF does not signal delayed pump work", Step 5).
- [ ] **Step 3: Wait.**
  - Reader: `spawn_host_reader(host_to_view, { let wake = wake.clone(); move || wake.set() })`. Delete the "No wake yet" comment.
  - Right after the reader (`:155`), before its first use in `await_cookies` (`:189`), add `let idle = |deadline| { wake.wait(deadline); do_message_loop_work(); };` with a two-line comment: the pump comes after the wait, so the next turn's drain and reads see what its callbacks did. It captures only `&wake`, so it is `Copy`; pass it by value to `await_cookies` and call it again at the loop end.
  - Move the loop-top `do_message_loop_work()` to just before `loop` (Considered and not done: the first turn's pump), and replace the final sleep with `idle(last_audio_poll + AUDIO_PLAYING_POLL_INTERVAL)`.
  - The `shutting_down` comment (`:226`, "the loop keeps pumping (`do_message_loop_work` below)") now says it pumps in `idle`.
- [ ] **Step 4: Sweep for leftovers.** `grep -n 'poll\|8 ms\|sleep\|No wake' crates/manifold-view-cef/src/main.rs`. Fix each hit that describes the old loop. The audio-poll hits (`AUDIO_PLAYING_POLL_INTERVAL`, `last_audio_poll`) are a real deadline and stay.
- [ ] **Step 5: Park the stopgap in the same commit.** Add a `PARKED.md` entry "CEF does not signal delayed pump work":
  - *What:* on Windows every `on_schedule_message_pump_work` call asked for 0, so a browser-process task with a delay runs on the next turn, at most about 1 s late on an idle page. Linux was not measured.
  - *Why parked:* by decision b1, and no stage shows it.
  - *Trigger:* a CEF pin bump, or a CEF check on Linux (count positive `delay_ms` again, as in this plan's Evidence); or a browser-side delay seen to lag.
  - *Evidence:* this plan, run `2026-11-09-cef-pump-spike-stages`.
  - *Status:* parked.
- [ ] **Step 6: Run** `cargo test -j 8 -p gate limits`, `cargo clippy -j 8 -p manifold-view-cef -- -D warnings`, `cargo test -j 8 -p manifold-view-cef` and `cargo lint -j 8 -- -D warnings`. Expected: all pass.
- [ ] **Step 7: Commit** "cef host: wait on a WakeEvent that CEF's pump requests and the reader set, instead of polling".

### Task 3: Gate and record

**Files:**
- Modify: `PARKED.md`, `docs/devlog.md`, `crates/manifold-view-servo/src/main.rs:163-164`, `crates/manifold-view-webkitgtk/src/main.rs:82`, and this plan's `status`.
- `docs/runs.md` gets its rows from the wrapper.

Every run below goes through `powershell -NoProfile -File "$USERPROFILE/.claude/runs/run.ps1"` from bash. Each detached run gets a watcher that exits when its `run.json` `end` is set.

- [ ] **Step 1: Build.** Use `-Kind probe -Name cef-wait-build -Command 'cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p manifold-view-cef -p gate && target\debug\gate.exe sync-runtime-deps'`. Expected: exit 0. It takes a few minutes after the protocol change, which every host links.
- [ ] **Step 2: Recorded hostrate.** Use `-Kind probe -Name hostrate-cef-wait -Command 'set "VIEWS=cef"& target\debug\gate.exe hostrate'`. Expected: PASS on its four checks:
  - animating at 35 fps or more;
  - still frames at most 2;
  - still polls between 0.5 and 20 per second;
  - an answer median under 400 ms.

  The figures to compare with the spike (about 10 still polls a second, a median near 10 ms) are read from the output, not checked. They go in the row's headline.
- [ ] **Step 3: Recorded CEF stages the gate does not run.** Use `-Kind probe -Name cef-wait-stages -Detach -Command 'set "VIEWS=cef"& target\debug\gate.exe key && target\debug\gate.exe page-kind && target\debug\gate.exe sleep && target\debug\gate.exe page-text'`, with a watcher. It takes about 2-3 minutes: the spike's batch of these plus four more stages took about 2 min. Expected: each prints `<stage>: ok`, exit 0.
- [ ] **Step 4: Gate.** Use `-Kind gate -Name gate-cef-wait -Detach -Command 'set "WITH_ENGINES=cef"& target\debug\gate.exe all "%RUN_DIR%\gate-log.txt" full'`. It takes about 30 minutes: the last `full` gate (`2026-11-09-host-loop-gate`) took 27 without CEF. Expected:
  - `GATE done` with no failed stage, including `download-cef`, `wheel-cef`, `kill-check-cef` and `jar-cef`;
  - CPU load and Defender state reported with the result;
  - the row's headline set from the log's last lines.
- [ ] **Step 5: Record.**
  - `PARKED.md` host-loop entry:
    - its title becomes "WebView2 and CEF wait since 2026-11-09";
    - its *Shape* sentence on CEF's `delay_ms` says `delay_ms` is not folded in (decision b1, this plan);
    - its *Deletes* line (PARKED.md:203) marks cef `:502` done, and drops `wait_for_first_message`, which no longer exists;
    - its *Evidence* (`manifold-view-cef/src/main.rs:135,185,502`) and *Risks* (4) say what this plan found: CEF's thread queue is waited on, and nothing is dispatched for CEF;
    - "Still unverified" drops CEF's maximum pump delay ("recalled, not read": cefclient's value was read, and CEF 152 asked only for 0) and keeps GLib wakeup stickiness;
    - its *Status* says WebView2 and CEF are converted (both plans linked), and points "the other hosts" at "Servo and WebKitGTK still poll".
    - its *Risks* sentences "Windows gets a single `cfg(windows)` `WakeEvent`", "Servo uses `park_timeout`" and "The protocol crate gets nothing on Linux" (PARKED.md:205) say that `WakeEvent` is portable since this plan (decision c1); the GLib reason stays, for WebKitGTK.
  - "Hosts other than WebView2 still poll" becomes "Servo and WebKitGTK still poll" (PARKED.md:207):
    - its *What* drops CEF, CEF's startup `idle` and the CEF baseline run;
    - its *Trigger* drops CEF, and Servo's (its `EventLoopWaker`, today a `NoopWaker`) adds that it can wait on the portable `WakeEvent`;
    - its *Evidence* drops the CEF lines;
    - the Servo and WebKitGTK comments citing the old title use the new one. `docs/devlog.md:604` is a dated record and keeps the old title.
  - "A hidden WebView2 tab still wakes per captured frame" (PARKED.md:206) gains one line: hostrate has no hidden phase, so a hidden CEF host's turn rate (`was_hidden(true)`, CEF's own pump requests and the 1 s audio deadline) is unmeasured too, and that entry's fix should add a hidden phase covering both hosts.
  - `docs/devlog.md`: one entry with the spike, the falsified latency check and the three recorded runs.
  - The plan's `status: done`.
- [ ] **Step 6: Docs check.** Grep `PARKED.md`, `README.md`, `docs/` (not `docs/runs.md` or done plans) and `crates/` for:
  - `still poll`, `No wake yet`, `delay_ms`, `on_schedule_message_pump_work`, `:502`, `main.rs:135,185`, `wait_for_first_message`, `recalled, not read`, `next entry` and `sleep(8`;
  - `cfg(windows)` near `WakeEvent`, and `nothing on Linux`;
  - `CEF` near `poll`.

  Fix the live text; leave done plans and run rows as written.
- [ ] **Step 7: Commit** "cef host loop: gate and stages recorded; parked entries updated".
