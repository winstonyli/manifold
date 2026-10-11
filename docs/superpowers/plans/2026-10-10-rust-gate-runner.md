# Rust gate runner: the headless flows as a Rust program with closures

> **For agentic workers:** execute task by task; each task ends with its proof. Steps use checkbox syntax. Do not start without the user's go-ahead (WORKING_STYLE).

**Goal:** replace `scripts/link-probe.sh`, `scripts/headless-smoke-test.sh` and (decision at Task 5) `scripts/two-instance-test.sh` with one Rust binary whose checks are inline closures over the exported state, so the gate has no python `eval`, no per-check process spawn and no MSYS pid/path juggling in these stages.

**Architecture:** a new workspace member `crates/gate` (bin, std + `serde`/`serde_json`/`image`). `gate link|smoke|two` launches `manifold --headless --profile-dir DIR` itself, writes command lines to its stdin, reads EXPORTSTATE/EXPORTSESSION/PNG files back, and prints the same `PASS:`/`FAIL:`/`TIMING` lines the scripts print. `scripts/gate.sh` stays the orchestrator (cpu sampler, Defender line, BelowNormal detached launch, `stage()` parsing) and calls `target/debug/gate <stage>` where it called `bash scripts/<x>.sh`.

**Tech stack:** Rust 2021, std `Command`/`CommandExt::creation_flags`, `std::net` for the two helper HTTP servers, `image` (already a manifold dependency) for the border PNG checks.

**Spec / evidence:** `.superpowers/rust-runner-survey.md` (protocol, machinery, sizes), `.superpowers/gate-checks-inventory.md` (the 78 eval'd expressions), `.superpowers/gate-time-survey.md`. Facts re-verified for this plan: `--headless`, `--profile-dir` and `--script` are real flags (`crates/manifold/src/cli.rs:14-24`); `manifold` has no `lib.rs`; `gate.sh stage()` counts lines containing `PASS`/`FAIL` (`gate.sh:52`) and takes the exit code as the verdict.

## Global constraints

- **Behaviour parity.** Every ported check keeps its label string verbatim, so the sorted `PASS:` lines of old and new runs diff empty (54 smoke, 30 link, 7 launch-forms is untouched). A port that cannot match a line says so in the task report; no silent drop.
- **A check that fails does not stop the run** (the scripts use `set -u` without `-e`); the program exits non-zero if any failed, and prints the same tail-of-log diagnostics on failure that the scripts print.
- **No stray processes.** Every instance spawned is killed on every exit path (normal, `FAIL`, panic, Ctrl-C): a Drop guard plus a Windows Job Object with kill-on-close for the runner's children. Engine hosts already die with `manifold.exe`'s own job.
- **Load rules (LONG_RUNS):** instances launch BelowNormal (`creation_flags(0x4000)`); the gate stays detached; CPU load and Defender state are reported with any timing.
- **Lints:** `crates/gate` joins the workspace lints; CLAUDE.md's clippy line, `.github/workflows/ci.yml` and `.githooks/pre-commit` gain `-p gate`. `cargo fmt --all` before each commit; no `--no-verify`.
- **Python that remains:** `export_state.py` (imported by chrome-shots, page-kind-probe, sleep-probe, scale-test, failed-spawn-probe), `probe_server.py` (hscroll, key, package, page-kind, sleep probes), `compare_sessions.py`/`session_asserts.py` (until Task 5). They stay until their last user is ported; this plan does not claim "no python in the repo".
- Work on a branch from `main`. Gate tier for the finished work: `full`.

## Design decisions

1. **Where it lives: `crates/gate` (Option B), not `crates/manifold/tests`.** Integration tests would be run by plain `cargo test` in CI (CI runs none of the gate scripts today), need `#[ignore]`, run in parallel, capture output, and `cargo test` does not build the WebView2 host exe. A separate bin keeps `gate.sh` in charge.
2. **State access.** First port: a thin `State(serde_json::Value)` with the accessors the closures need (`cards()`, `tabs()`, `groups()`, `card(id)`, `xy(id)`, `lt(id)`, `bbox()`, `card_is(c, TYPE, loaded)`), mirroring `export_state.py`/`smoke_eval.py` one for one. Typed structs shared with the producer (a `probe-types` crate) are a later, optional task; the exporter builds `json!` by hand today (`debug_control.rs:1437-1597`).
3. **Closures replace the 78 expressions one for one**, written next to the action that precedes them, e.g. `check("a headless screenshot shows no toast", |s| s.toast().is_none())`. Ids and coordinates become Rust variables; the `BEFORE`/`PREV` text round-trips disappear.
4. **Rounding.** Python `round()` is half-even and `"%.0f" % x` can print `-0`; the Rust port uses explicit helpers (`round_half_even`, a formatter that never emits `-0`) wherever a coordinate feeds a command or a comparison, with unit tests for the cases the scripts hit.
5. **Waits stay as they are in this plan** (14 `SLEEP 1`, the 5/6/8/10 s windows, `settle`'s two-equal-exports rule): this is a port, not a tuning pass. Tuning is a separate task after parity, with the pre-spawn gains measured first.
6. **Timing.** The runner emits `TIMING name secs epoch` per section exactly as `timing_mark` does, so `gate-timing.sh collect` is unchanged.

## Tasks

### Task 1: runner core and a one-instance hello
**Files:** create `crates/gate/{Cargo.toml,src/main.rs,src/instance.rs,src/check.rs,src/state.rs}`; modify workspace `Cargo.toml`, `CLAUDE.md` (clippy line), `ci.yml`, `.githooks/pre-commit`.
- [ ] `Instance::launch(exe, profile_dir, settings)` seeds `settings.json` (the `has_shown_first_launch_demo` seed, engine choice), spawns `manifold --headless --profile-dir DIR` BelowNormal with stdin piped, stdout/stderr to a log file (never an undrained pipe), registers it with the job object.
- [ ] `send(&str)` writes one line and flushes per line. `export_state(name) -> State` sends `EXPORTSTATE <win path>`, waits for the file (poll, deadline), parses. Same for `export_session` and `screenshot` (size-stable rule). `require_fresh_exe` ports `fresh-exe.sh` (exe and WebView2 host against cargo's `.d` files).
- [ ] `check(label, bool)` / `check_eq` print `PASS: label` / `FAIL: label: got [x], wanted [y]` in the scripts' format; a `Section` guard prints the `TIMING` line on drop. Exit code from the failure count.
- [ ] Unit tests: `State` accessors on a fixture, `round_half_even`, check formatting. A smoke `gate hello`: launch, WAITTABS, export, one check, QUIT, assert no child left.
- **Proof:** `cargo clippy -j 4 -p gate -- -D warnings`, `cargo test -j 4 -p gate`, `gate hello` under the repo path with a space (re-tests the "path with a space" claim at `link-probe.sh:41`; if it still fails, find out why before relying on it), `gate-timing.sh collect` parses its TIMING lines, a killed `gate` leaves no `manifold.exe`.

### Task 2: port link-probe (the smallest; 30 checks, 6 sections)
- [ ] `gate link`: the 5 launches, 3 drag loops and 30 checks, labels verbatim from `scripts/link-probe.sh`.
- **Proof:** sorted `PASS:` lines identical to the shell version over 3 runs of each; `pass=30` through `gate.sh`; sabotage run: flip one expected value in the Rust source and see exactly that line `FAIL`, then revert; record the wall-time of both, with CPU load.

### Task 3: HTTP helper and port the smoke test (53 check sites, 11 sections)
- [ ] `std::net` server for `GET /ac/?q=X` returning `[X,[X one,X two,X three]]` (what `probe_server.py` serves the smoke test), on an ephemeral port passed via `MANIFOLD_SUGGESTIONS_URL`; request log kept for the checks that read `page.log`.
- [ ] `gate smoke`: all sections; `settle`, `wait_state`, the conversion poll as functions with deadlines; border checks via `image` (they now always run: the old ones skipped without Pillow, so the `PASS` count may rise if Pillow was missing; record which).
- **Proof:** `pass=54 fail=0`, sorted `PASS:` lines diff empty against `gate-splits-full.log.smoke.log`, 11 TIMING sections, sabotage run, two runs in a row.

### Task 4: wire `gate.sh`, docs, delete what is now unused
- [ ] `stage smoke … target/debug/gate smoke`, `stage link … target/debug/gate link`; the build line in `gate.sh`/CLAUDE.md/README gains `-p gate`; README "scripts/gate.sh" and the scripts table updated.
- [ ] Delete `headless-smoke-test.sh`, `link-probe.sh`, `smoke_eval.py`, `eval_server.py`, `eval-client.sh`, `smoke-eval-test.sh` (and the `gate-tools-test.sh` line that calls it) and fix every reference (grep each name: `gate-tier.sh` lists, README, docs, plan files).
- **Proof:** one full gate through `gate.sh`: green, PASS counts equal, TIMING lines present; report CPU and Defender state.

### Task 5 (decision, not committed): two-instance-test
- It has no `PASS:` lines (only `scenario N: ok`, about 41 `FAIL` exits), four instances sharing a profile, `taskkill` of A, the `nonce_server.py` counters and the longest fixed windows (5 to 10 s). Port only after Tasks 2 to 4 land and show a real gain; decide then whether it should emit `PASS:` lines (a gate-format change). Decided 2026-10-10: ported; see 2026-10-10-rust-gate-two.md.

## Stress test (2 rounds so far)

Round 1, against the survey: (a) the app has **no response channel**; completion is observed only by files, so every ported wait is a poll with a deadline, and `WAITTABS`/`WAITIDLE`/`SLEEP`/`SCREENSHOT` block inside the app's stdin reader (60 s default, 30 s for screenshots), so the driver must never write faster than a blocking command allows and must not time out before the app does. (b) An undrained stdout/stderr pipe can block the app; logs go to a file. (c) Command order and per-line flush matter (the first `CLICK` depends on the previous command's frame); `send` flushes every line. (d) `gate.sh` counts the substring `PASS`/`FAIL` anywhere in the log (`gate.sh:52`), so diagnostics must not print those words. (e) A killed runner must not orphan `manifold.exe` (ten leaked profiles once): job object and Drop guard. (f) The scripts' MSYS workarounds (pid lookup, `tail -f`, `cygpath`) disappear, but Windows paths for commands and `%20` in file URLs remain.

Round 2, adversarial: (g) The plan said "no python"; five other scripts still import `export_state.py` and `probe_server.py`: the Global constraints now say so and Task 4 deletes only what loses its last user. (h) "Parity" could hide a changed assertion: labels are checked by diff, **semantics** by the sabotage run per script plus a side-by-side read of each closure against its original expression (a reviewer task, not a diff). (i) Python vs Rust arithmetic: half-even rounding, `-0`, float formatting in commands (`DOUBLECLICK x y`): decision 4 and unit tests. (j) `settle` compares string reprs of rounded lists; the Rust version compares the rounded numbers, which can differ for values that print the same: unit-tested against the python outputs on a recorded export. (k) The border check is currently skipped without Pillow; always running it can turn an environment skip into a failure: recorded, not hidden. (l) Windows only: `CommandExt::creation_flags` and the job object are `cfg(windows)`; the crate must still build and clippy on Linux CI (`cfg`-gate with a stub that bails out), or be excluded from CI's package list like the engine crates, decided in Task 1. (m) The eval helper from the other plan (`gate-eval-helper` branch) becomes throwaway for smoke and link if this lands; merge it only if its A/B shows value now, otherwise drop it.

Unverified: the speedup (the survey's per-spawn figure was inferred; the A/B on the helper branch is the evidence), whether the space-in-path problem still exists, and the true Rust line counts (survey estimates: link 200-250, smoke 700-900).

## Cost

Roughly: Task 1 ~400 lines plus tests, Task 2 ~250, Task 3 ~900 plus 60 for the server, Task 4 mostly deletion and edits. Several days of focused work; Task 2 is the point to stop and judge whether the gain is real before Task 3.
