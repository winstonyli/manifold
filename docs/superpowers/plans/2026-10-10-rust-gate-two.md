---
status: done
---

# Rust gate runner, part two: `gate two` replaces two-instance-test.sh

> **For agentic workers:** execute task by task; each task ends with its proof. Steps use checkbox syntax. Do not start without the user's go-ahead (WORKING_STYLE). Task 1 can end the plan (its stop condition); report to the user before Task 2 either way.

**Goal:** port `scripts/two-instance-test.sh` and its three python helpers (`compare_sessions.py`, `session_asserts.py`, `nonce_server.py`) into `crates/gate` as `gate two`, the same way `gate link` and `gate smoke` replaced their scripts (plan `2026-10-10-rust-gate-runner.md`, Tasks 1 to 4). The goal is speed with no loss of coverage: no python process per assert, no MSYS fork per poll, and a run that keeps every assertion.

**Architecture:** three modules that work without the app, then the flow.
- `http.rs` is a generic `std::net` server split out of `suggest.rs`.
- `nonce.rs` is the nonce site, in process: its counters are read directly, not through files.
- `session.rs` holds the JSON asserts.
- `two/` is the flow: `mod.rs` holds the driver and its helpers; `groups.rs` holds scenarios 1, 2 and 8 to 12; `web.rs` holds 3 to 5; `restart.rs` holds 6 and 7.
- `scripts/gate.sh` keeps orchestrating and calls `target/debug/gate two` where it called `bash scripts/two-instance-test.sh`.

**Tech stack:** Rust 2021, std only plus the crate's existing `serde_json`. No new dependency.

**Evidence read for this plan (re-read 2026-10-10):**
- the scripts: `scripts/two-instance-test.sh` (659 lines), `session_asserts.py`, `compare_sessions.py`, `nonce_server.py`, `gate.sh`, `gate-tier.sh`, `gate-timing.sh`, `gate-tools-test.sh`, `timing.sh`;
- the crate: `crates/gate/src/*.rs`;
- the survey and the earlier plan's reports: `.superpowers/two-instance-survey.md`, `.superpowers/rust-runner-task{1,2,3}-report.md`;
- the last full gate log: `%TEMP%\gate-rust-full.log`, 2026-10-10 20:38, CPU 16.7 % at the start, Defender RT False.

## Global constraints

- **Behaviour parity.**
  - Every assertion of the script is kept, with its comparison unchanged: exact where the script is exact (`'[321.0, -123.0]'`), and the same 1e-3 position tolerance in `field_agree`.
  - The script prints no `PASS:` lines today. `gate two` prints one per assertion, with the 66 labels listed under "PASS labels". None carries run-varying data, so the sorted `PASS:` lines of any two green runs diff empty.
  - The informational lines are kept: per-scenario headers, `scenario N: ok (...)` with the race winner, the `right after the moves` lines, `A's new note id`, and the closing `two-instance test: ok`.
- **The stop rule** is decided under "Stop rule" below: preconditions stop at once, and observations finish their scenario. A failed scenario ends the run, because later scenarios build on its document, instances and counters.
- **No stray processes.** Every instance runs through `Instance` (BelowNormal `creation_flags(0x4000)`, kill-on-close job, kill on Drop). The nonce site is a thread in `gate`, so it dies with it. Each proof run checks that no `manifold.exe` with a pid printed in its log is left.
- **No fixed sleeps added.**
  - Every wait is a poll on an observable condition with a deadline.
  - The script's negative windows are kept as they are and listed with why under "Negative windows".
  - The 200 ms pause between poll rounds is the script's own (`sleep 0.2`). It is kept so the app is polled at the same cadence.
- **No compatibility shims.** Task 8 deletes the four replaced files and `scripts/timing.sh` (its last user goes), and fixes every reference.
- **Lints and format (CLAUDE.md):** `cargo fmt --all` before each commit. `cargo fmt --all --check` and `cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 -p gate --all-targets -- -D warnings` must pass. Never `--no-verify`. A lint is fixed, or `#[allow(..., reason = "...")]`ed with its reason.
- **Tests:** `cargo test -j 8 -p gate`, and the same with `--features serde_json/preserve_order`. `crates/manifold` enables that feature, and Cargo unifies it into `gate` when both are built in one invocation (checked: `cargo test -j 4 -p gate --features serde_json/preserve_order --no-run` compiles). The session comparisons must not depend on it.
- **Build before any run:** `cargo build -j 8 -p manifold -p manifold-engine-webview2 -p gate`. `manifold_exe()` refuses a stale exe or host.
- **Load rules (LONG_RUNS):**
  - Before any build or run, check for orphans with `Get-Process manifold,python,gate,cargo,rustc -EA 0`.
  - One cargo at a time.
  - Every app run is launched detached at BelowNormal: `cmd //c start "" //b //belownormal ...`.
  - Every timing is reported with the CPU load (gate.sh's `cpu()` one-liner or `scripts/cpu-sampler.ps1`) and Defender state (`(Get-MpComputerStatus).RealTimeProtectionEnabled`). Waits use Monitor or an until-loop with a deadline, never `sleep N`.
- **Python, where a step still runs it:** `python -P file.py ... < /dev/null`, never `python -` or a heredoc. This rule is repeated in every implementer brief.
- **Briefs:** each task goes to a Sonnet implementer, with the line "change nothing beyond this; report every deviation". The controller diffs the result against the task's file list and reruns the proof commands itself.
- Work on `main` (CLAUDE.md "Branches"). `gate.sh` keeps calling the shell script until Task 8, so a half-ported `gate two` never runs in the gate. The gate tier for the finished work is `full` (`crates/gate/*` and `scripts/gate*.sh` are on gate-tier's full list).
- Scratch files go under `.superpowers/two/` (gitignored).
- **Running a `gate` stage by hand** (Tasks 2 to 6). Write this wrapper once in Task 2 as `.superpowers/two/run.sh`, launch it detached, and wait on its `.done` file. Never pass `/c`-style arguments through MSYS: it rewrites them as paths.

```bash
#!/usr/bin/env bash
# Usage: cmd //c start "" //b //belownormal bash .superpowers/two/run.sh NAME STAGE
#   runs target/debug/gate STAGE into .superpowers/two/NAME.log, then writes NAME.done ("exit=N").
cd "$(dirname "$0")/../.." || exit 1
name=$1
shift
rm -f ".superpowers/two/$name.done"
target/debug/gate "$@" >".superpowers/two/$name.log" 2>&1
echo "exit=$?" >".superpowers/two/$name.done"
```

  Wait with an until-loop over `[ -f .superpowers/two/NAME.done ]` with a deadline (10 min for a whole `gate two` or `gate smoke`), never a fixed sleep. Then read the `.done` exit code, `grep -c PASS` and `grep -c FAIL`, and the log's TIMING lines.

## Survey corrections (verified against the script)

`.superpowers/two-instance-survey.md` is mostly right. These points are wrong or missing there; the plan follows the script.

1. **The script uses `set -euo pipefail`.** It stops at the first failure anywhere. The earlier plan's line "the scripts use set -u, not -e" was about link-probe and the smoke test, not this script. So parity for a failure means stopping, which is why this plan defines a stop rule rather than "continue everywhere".
2. **The race sends are already microseconds apart in bash.** Both sends are builtin `echo`s to coproc pipes (`sa ...; sb ...`), so Rust does not make them meaningfully "faster". What changes is the time before and after them: no `cp` fork before scenario 9 or 10's sends, and quicker polls after. See "Risks", R1.
3. **The final position check is exact**, not a tolerance: `[ "$(shown pos s12m "$n12")" = '[321.0, -123.0]' ]` compares the JSON text. The Rust check `field_of(..) == Some(json!([321.0, -123.0]))` keeps the same strictness. A float written as an integer (`321`) fails both. The 1e-3 tolerance exists only inside `field_agree`, which is unchanged.
4. **Comparing sessions should not be "sort by id, then `Value ==`".** `compare_sessions.py` compares the sorted multiset of `json.dumps(n, sort_keys=True)`. The port compares a sorted multiset of canonical strings (keys sorted at every depth). That keeps duplicates, `-0.0`, and `1` vs `1.0` distinct exactly as python does, and does not depend on `preserve_order`.
5. **Scenario 3 has a `WAITIDLE` between `ADDRESSBAR` and `ENTER`**, as every typed navigation in the script does. The survey listed it without one.
6. **`ENGINE` affects scenarios 3, 4, 5 and 7** (the script header). README:397 says "3, 4 and 6", which is stale. Task 8 fixes it.
7. **Scenario 5's poll loop has no `FAIL` of its own.** It waits up to its limit, then the count check after the 8 s window decides. The port keeps that: no label for the loop.
8. **`wait_file "$profile/manifold/session.mnf.prev"` in scenario 2 is an assertion.** It fails the run if A never compacts. It becomes a precondition label. Under `--profile-dir` the file is `<profile>/session.mnf.prev`: the flag names the manifold folder itself (`crates/gate/src/instance.rs` `seed_settings` doc, `crates/manifold/src/profile.rs` `dir`).
9. README:397 numbers the scenarios 1 to 6 in prose ("(5) A adds a note and is killed"), but the script numbers them 1 to 12. Task 8 rewrites the paragraph with the script's numbers.
10. **Also found: `scripts/gate-timing.sh` loses the CPU join** when its CPULOG is a Windows path. `awk -v cpulog="$4"` treats backslashes as escapes. Every `cpu_start`/`cpu_mean` in `%TEMP%\gate-rust-full.log` is `-`, although the sampler file has data. Reading the path through `ENVIRON` fixes it (checked by hand on that log). Task 1 fixes it, because this plan's before/after numbers need the CPU column.

## Measurements

Task 1 fills this section in. Until then the expected saving is an estimate.

**Baseline.** In the 20:38 full gate the three two-N runs took 133, 132 and 134 s. Per section in two-1, in seconds:

| setup | s1 | s2 | s8 | s9 | s10 | s11 | s12 | s3 | s4 | s5 | s6 | s7 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 3.5 | 1.7 | 1.9 | 5.2 | 9.1 | 11.0 | 11.7 | 19.2 | 19.7 | 22.3 | 11.9 | 4.1 | 11.6 |

**Kept fixed windows.** These total 36 s per run and stay in both versions: s3 6 + 10 s, s4 5 + 5 s, s5 8 s, and s7's in-app `SLEEP 2`.

**Estimate.** Python asserts run once per poll round and per check, about 60 to 80 per run. MSYS forks (`rm`, `seq`, `cygpath`, `cat`, `grep`, `tr`, `sleep`) run a few hundred times. At normal fork cost that is roughly 25 to 50 s per run, so 133 s would become about 85 to 110 s. A `full` gate (3 runs) would save about 75 to 150 s.

The survey estimated 20 to 30 s. Smoke's 4x gain (251 to 63 s) is not a guide here, because 36 s of this script is fixed windows.

**Fork cost varies by session.** In the session that wrote this plan, one MSYS `$(...)` cost 2.2 to 2.8 s; a native `cmd /c exit` cost 23 ms. The 132 to 134 s gate runs imply far cheaper forks then. Task 1 therefore calibrates the fork cost before trusting its split.

**Task 1 results (2026-10-10, taken under load).** The calibration clause triggered: `fork_ms_before` 375 and `fork_ms_after` 342, both above the 200 ms limit. The user reviewed this and overrode the stop: the numbers below are load-inflated (shared machine, CPU 27 to 40 %, Defender RT False), so read S_lo as an upper bound on the saving, not an estimate of it. No re-measure was done.

- Traced wall: 292 s and 250 s; untraced wall: 239 s (all exit 0). The 20:38 gate ran the same script in about 133 s.
- Python: 22.7 to 26.9 s over 34 spawns. MSYS: 58.4 to 64.6 s over 429 spawns. Fixed windows: 36.3 to 37.0 s (5 sleeps). `wait`: 1.8 to 4.5 s.
- S_lo (python + msys): run 1 = 91.5 s, run 2 = 81.1 s; median 86.3 s (463 processes). It is above the 20 s stop line, but forks cost 340 to 375 ms here and `bash -x` adds its own cost.
- Untraced CPU per section (cpu.log): mean 26.7 to 40.1 %.

Run 1, seconds/spawn count per kind:

```
section                    python         msys         poll        fixed         wait        other
pre                      0.0/0        3.3/10       0.0/0        0.0/0        0.0/0        2.9/429 
setup                    0.0/0        1.4/9        0.0/0        0.0/0        0.0/0        4.0/39  
s1-import                0.3/1        1.5/9        0.0/0        0.0/0        0.0/0        2.0/43  
s2-compaction            0.5/1        0.9/11       0.0/0        0.0/0        0.0/0        3.7/452 
s8-move-into-group       1.6/3        2.1/27       0.0/0        0.0/0        0.0/0        9.6/117 
s9-two-moves             3.1/6        5.6/54       0.0/0        0.0/0        0.0/0       17.5/205 
s10-cycle                6.4/6        9.5/58       0.0/0        0.0/0        0.0/0       22.4/187 
s11-rename               4.6/5        5.6/54       0.0/0        0.0/0        0.0/0       23.4/224 
s12-note-text            7.9/7        8.1/83       0.0/0        0.0/0        0.0/0       35.6/338 
s3-nonce-site            0.0/0        3.9/13       0.8/1       17.6/2        0.0/0        3.5/63  
s4-click-vs-script       0.0/0        8.5/38       0.0/0       11.1/2        0.0/0        8.7/108 
s5-json-convert          1.7/3        1.6/18       0.0/0        8.3/1        0.0/0        4.3/69  
s6-kill                  0.7/2        1.7/10       0.0/0        0.0/0        1.9/2        4.5/51  
s7-restart               0.0/0        9.0/30       0.0/0        0.0/0        2.6/2       13.8/93  
end                      0.0/0        1.9/5        0.0/0        0.0/0        0.0/0        1.7/40  
TOTAL                   26.9/34      64.6/429      0.8/1       37.0/5        4.5/4      157.7/2458
S_lo = python + msys = 91.5 s (463 processes)
```

Run 2:

```
section                    python         msys         poll        fixed         wait        other
pre                      0.0/0        3.2/10       0.0/0        0.0/0        0.0/0        4.1/429 
setup                    0.0/0        2.5/9        0.0/0        0.0/0        0.0/0        5.5/39  
s1-import                0.6/1        1.3/9        0.0/0        0.0/0        0.0/0        1.9/43  
s2-compaction            0.3/1        1.0/11       0.0/0        0.0/0        0.0/0        3.2/452 
s8-move-into-group       2.1/3        2.6/27       0.0/0        0.0/0        0.0/0       10.4/117 
s9-two-moves             3.7/6        6.5/54       0.0/0        0.0/0        0.0/0       16.5/204 
s10-cycle                3.7/6        8.2/58       0.0/0        0.0/0        0.0/0       15.1/187 
s11-rename               3.9/5        4.7/54       0.0/0        0.0/0        0.0/0       18.2/224 
s12-note-text            6.3/7        9.1/83       0.0/0        0.0/0        0.0/0       28.8/337 
s3-nonce-site            0.0/0        2.4/13       0.4/1       17.5/2        0.0/0        3.0/63  
s4-click-vs-script       0.0/0        6.1/38       0.0/0       10.5/2        0.0/0        5.4/108 
s5-json-convert          1.5/3        1.2/18       0.0/0        8.2/1        0.0/0        3.6/69  
s6-kill                  0.6/2        1.0/10       0.0/0        0.0/0        0.6/2        4.1/51  
s7-restart               0.0/0        6.8/30       0.0/0        0.0/0        1.3/2        9.4/93  
end                      0.0/0        1.7/5        0.0/0        0.0/0        0.0/0        1.4/40  
TOTAL                   22.7/34      58.4/429      0.4/1       36.3/5        1.8/4      130.5/2456
S_lo = python + msys = 81.1 s (463 processes)
```

**Task 7 results (2026-10-10 23:25 to 23:49, `.superpowers/two/ab.sh`, alternating shell/rust x3, BelowNormal, taken under load).** Machine CPU was 73 to 79 % at start and 74 % at end, from another session's work (a python process from a different repo, not ours). Defender real-time protection was False at start and at end. Forks were slower than in Task 1: `fork_ms_before` 640 and `fork_ms_after` 255, both over the 200 ms limit, so the shell figures are load-inflated and the saving is an upper bound (reported, not re-measured). The Rust run does not fork, so its figures are less affected.

All six runs exit 0. The three Rust runs each have `pass=66 fail=0`; the sorted `PASS:` lines of the three runs are identical to each other and to `.superpowers/two/labels.txt` (prefix `PASS: ` stripped); each Rust log has the 13 TIMING names. The shell runs show `pass=0 fail=0` as expected.

Wall (s): shell 419, 377, 337 (median 377); Rust 76, 69, 59 (median 69). Total saving 308 s per run against Task 1's `S_lo` of 86.3 s (median of its two traced runs); Rust is faster by far more than `S_lo - 5`. The saving exceeds `S_lo` because `S_lo` counts only python and MSYS spawns, not the `$(...)`/`sleep` fork overhead that lands in `other`, and because forks cost more in this run (640 ms vs 340 to 375 ms).

Per-section medians in seconds (CPU is the machine-wide mean from `cpu.log` during the section, shell / Rust, so it includes the other session's load). The Rust `setup` is 0.0 s because the instance launch happens outside its TIMING span; the sections sum to 355.0 (shell) and 63.9 (Rust), and the remaining 22 s (shell) and 5 s (Rust) of the wall time lie outside the 13 spans.

| section | shell s | Rust s | delta | mean CPU % (shell / Rust) |
|---|---|---|---|---|
| setup | 8.1 | 0.0 | -8.1 | 86 / - |
| s1-import | 6.1 | 2.8 | -3.3 | 87 / 77 |
| s2-compaction | 6.6 | 0.5 | -6.1 | 81 / 66 |
| s8-move-into-group | 12.3 | 0.9 | -11.5 | 67 / - |
| s9-two-moves | 32.7 | 1.4 | -31.3 | 81 / 100 |
| s10-cycle | 47.0 | 1.1 | -45.9 | 76 / 71 |
| s11-rename | 51.2 | 1.3 | -49.9 | 70 / 79 |
| s12-note-text | 51.0 | 1.9 | -49.1 | 76 / 95 |
| s3-nonce-site | 26.0 | 17.7 | -8.4 | 79 / 71 |
| s4-click-vs-script | 42.3 | 11.6 | -30.7 | 70 / 85 |
| s5-json-convert | 23.3 | 8.7 | -14.6 | 73 / 81 |
| s6-kill | 10.5 | 3.4 | -7.1 | 72 / 85 |
| s7-restart | 38.0 | 12.5 | -25.5 | 72 / 90 |
| sum | 355.0 | 63.9 | -291.2 | |

(A `-` CPU mean means the sampler had no sample inside a very short Rust section in any of the three runs. In other Rust sections, runs with no sample were left out of the mean, so a Rust mean may rest on one or two samples; the 100 for s9 is such a case.)

Every section gained; the smallest gains are the ones dominated by the kept fixed windows (s3, s5, s6, s7).

Race winners (ok-lines), shell then Rust, runs 1 to 3:

| scenario | shell | Rust |
|---|---|---|
| s9 (group) | A, B, A | B, B, A |
| s10 (G in, H in) | H wins, H wins, H wins | H wins, H wins, G wins |
| s11 (named) | Both A, Both B, Both A | Both A, Both A, Both B |
| s12 (holds) | Both typed A x3 | Both typed B x3 |

Three runs per side is too few to call a shift: s12 went to A in all three shell runs and to B in all three Rust runs, which is the one pattern that could be a real change in who wins (the Rust driver may send B's typing later or earlier than the shell). Every outcome passed its scenario's own assertions, so this is a distribution note, not a failure.

**ENGINE=cef (run once each, after the A/B).** `ENGINE=cef target/debug/gate two`: exit 0, 66 PASS lines, s7 10.2 s. `ENGINE=cef bash scripts/two-instance-test.sh`: exit 0, s7 31.9 s. Logs: `.superpowers/two/ab/cef-rust.log` and `cef-shell.log` (git-ignored).

## Stop rule

Each assertion is either a **P** (precondition) or an **O** (observation).

- **P (precondition):** a later step means nothing without it. Examples: the instances agree before a value is compared, a node exists before it is moved, and the positive side of a negative window. A failed P prints its `FAIL:` line and stops the run at once (`Err(Halt::Failed)`).
- **O (observation):** the property under test. A failed O prints its `FAIL:` line, and the scenario goes on to its end, so one run shows every broken property of that scenario. Then the run stops.
- **A failed scenario stops the run** (no `scenario N: ok`, no later scenario). The reasons:
  - every later scenario adds to the same shared document;
  - s4 to s7 read the nonce counters that s3 starts;
  - s6 and s7 need the instances' state.

  Running past a failure would turn one fault into a cascade of misleading `FAIL`s. This matches the script's `set -e` at scenario granularity.
- **Every negative window's positive precondition is a P** (s3's ">= 4 requests", s4's "dests >= 2" and "spa_clicks >= 1", s7's "D reopens /dest"). Otherwise a run where nothing happened would pass the window vacuously.
- **Runner errors** stop the run with `ERROR: two stopped early: <e>` and count as a failure: an export file that never appears (60 s, as the script's `wait_file`), an instance that won't quit (60 s), or an I/O error. That line has no `FAIL` in it (gate.sh counts that word); this is the smoke test's precedent.

## PASS labels (66)

Each label is printed as `PASS: <label>` or `FAIL: <label>`. **P** stops the run; **O** continues to the end of the scenario. "agree on the shared nodes" is `wait_agree`, and "agree on the <field>" is `wait_field`.

| # | Label | Kind | Script source |
|---|---|---|---|
| 1 | scenario 1: A and B agree on the shared nodes | P | `wait_agree "scenario 1"` |
| 2 | scenario 1: the document has at least 1 note | O | `has_kinds text/markdown:1` |
| 3 | scenario 1: the document has at least 2 tabs | O | `has_kinds ...tab:2` |
| 4 | scenario 2: A's save after the import compacts the store (session.mnf.prev appears) | P | `wait_file .../session.mnf.prev` |
| 5 | scenario 2: A and B agree on the shared nodes | P | `wait_agree "scenario 2"` |
| 6 | scenario 2: the document has at least 201 notes | O | `has_kinds text/markdown:201` |
| 7 | scenario 8: A's NEWGROUP adds exactly one node | P | `new_node` (`new_id`) |
| 8 | scenario 8: A's NEWNOTE adds exactly one node | P | `new_node` |
| 9 | scenario 8: A and B agree on the shared nodes | P | `wait_agree` |
| 10 | scenario 8: the document puts the note in the group | O | `group_of = $g8` |
| 11 | scenario 8: A, B and the document agree on the groups | P | `wait_mirror s8` |
| 12 | scenario 9: A's first NEWGROUP adds exactly one node | P | `new_node` |
| 13 | scenario 9: A's second NEWGROUP adds exactly one node | P | `new_node` |
| 14 | scenario 9: A's NEWNOTE adds exactly one node | P | `new_node` |
| 15 | scenario 9: A, B and the document agree on the groups before the moves | P | `wait_mirror s9-setup` |
| 16 | scenario 9: A and B agree on the shared nodes after both moves | P | `wait_agree` |
| 17 | scenario 9: A, B and the document agree on the groups after both moves | P | `wait_mirror s9` |
| 18 | scenario 9: the note ends in A's or B's group | O | `got9` check |
| 19 | scenario 9: no node is lost | O | `kept_ids` |
| 20 | scenario 9: no group cycle | O | `no_cycle` |
| 21 | scenario 10: A's first NEWGROUP adds exactly one node | P | `new_node` |
| 22 | scenario 10: A's second NEWGROUP adds exactly one node | P | `new_node` |
| 23 | scenario 10: A, B and the document agree on the groups before the moves | P | `wait_mirror s10-setup` |
| 24 | scenario 10: A and B agree on the shared nodes after both moves | P | `wait_agree` |
| 25 | scenario 10: A, B and the document agree on the groups after both moves | P | `wait_mirror s10` |
| 26 | scenario 10: no node is lost | O | `kept_ids` |
| 27 | scenario 10: no group cycle | O | `no_cycle` |
| 28 | scenario 11: A's NEWGROUP adds exactly one node | P | `new_node` |
| 29 | scenario 11: A, B and the document agree on the new group's name | P | `wait_field name s11-setup` |
| 30 | scenario 11: A and B agree on the shared nodes after A's rename | P | `wait_agree` |
| 31 | scenario 11: A, B and the document agree on the name after A's rename | P | `wait_field name s11` |
| 32 | scenario 11: the group has A's name | O | `shown = "Renamed by A"` |
| 33 | scenario 11: A and B agree on the shared nodes after both renames | P | `wait_agree` |
| 34 | scenario 11: A, B and the document agree on the name after both renames | P | `wait_field name s11c` |
| 35 | scenario 11: the group ends with A's or B's name | O | `got11` check |
| 36 | scenario 12: A's NEWNOTE adds exactly one node | P | `new_node` |
| 37 | scenario 12: A, B and the document agree on the new note's text | P | `wait_field text s12-setup` |
| 38 | scenario 12: A and B agree on the shared nodes after A's edit | P | `wait_agree` |
| 39 | scenario 12: A, B and the document agree on the text after A's edit | P | `wait_field text s12` |
| 40 | scenario 12: the note holds A's text | O | `shown = "Typed by A"` |
| 41 | scenario 12: A and B agree on the shared nodes after both edits | P | `wait_agree` |
| 42 | scenario 12: A, B and the document agree on the text after both edits | P | `wait_field text s12c` |
| 43 | scenario 12: the note ends with A's or B's text | O | `got12` check |
| 44 | scenario 12: A, B and the document still agree on the text after B saves | P | `wait_field text s12c2` |
| 45 | scenario 12: B's save leaves the winning text in place | O | `shown s12c2 = got12` |
| 46 | scenario 12: A and B agree on the shared nodes after A's move | P | `wait_agree` |
| 47 | scenario 12: A, B and the document agree on the pos after A's move | P | `wait_field pos s12m` |
| 48 | scenario 12: the note is where A dropped it | O | `= '[321.0, -123.0]'` |
| 49 | scenario 3: A's url reaches B's document | P | b3 grep loop |
| 50 | scenario 3: B follows A's navigation (at least 4 requests) | P | `hits >= 4` |
| 51 | scenario 3: the instances stop navigating each other (no new request in 10 s) | O | `hits = settled` |
| 52 | scenario 4: the server sees A's /link load | P | `wait_served /link` |
| 53 | scenario 4: B follows A's link click | O | `field 1 > before` |
| 54 | scenario 4: A's and B's /js pages each navigate by script | P | `field 3 >= 2` |
| 55 | scenario 4: B does not follow A's script navigation (5 s window) | O | `field 2 = 0` |
| 56 | scenario 4: the server sees A's /spa load | P | `wait_served /spa` |
| 57 | scenario 4: B follows A's pushState from a click | P | `field 4 >= 1` |
| 58 | scenario 4: B does not follow A's pushState from a timer (5 s window) | O | `field 5 = 0` |
| 59 | scenario 5: A and B agree on the shared nodes after the conversion | P | `wait_agree "scenario 5"` |
| 60 | scenario 5: the document gains exactly one Json node | O | `json_after = before + 1` |
| 61 | scenario 6: A's NEWNOTE adds exactly one node | P | `new_id` |
| 62 | scenario 6: a fresh instance on the profile has A's note | O | `has_id` |
| 63 | scenario 7: C requests the typed /js url | P | `served /js > js_first` |
| 64 | scenario 7: C's /js page navigates to /dest | P | `field 3 > dests` |
| 65 | scenario 7: D reopens C's /dest url | P | `field 2 > dest_repeats` |
| 66 | scenario 7: D does not load the typed /js again | O | `js_loads = js_before` |

Two of the script's checks are deliberately not ported: "couldn't find A's/B's/C's/D's manifold.exe Windows PID". They checked an MSYS pid lookup that no longer exists, because `Child::id()` is the Windows pid. Instead, the runner prints `A: pid N` and so on, for the stray-process check.

Every label is a `&'static str` in the code, written exactly as above. Task 7 checks the 66 sorted lines against this table.

## Negative windows (kept, with why)

| Where | Window | Why it exists |
|---|---|---|
| s3 | 6 s, then read `settled` | Gets past `HOST_LOAD_GRACE` (5 s, `crates/manifold/src/session_sync.rs:34`). Within it, a host-started load is still treated as local, so a bounce could only start after it. |
| s3 | 10 s, then `hits == settled` | A bounce is a round trip every 1 to 2 s, so 10 s would see several. |
| s4 | 5 s after `dests >= 2`, then `dest_repeats == 0` | The page's script navigates 500 ms after load. 5 s lets a wrongly shared `/dest?n=` reach B and be requested. |
| s4 | 5 s after `spa_clicks >= 1`, then `spa_timers == 0` | The timer's `pushState` fires 1 s after the click. 5 s lets a wrongly shared one reach B. |
| s5 | 8 s after A's conversion | Time for B to follow the url, load the file and convert it too, if the bug is there. |
| s7 | `SLEEP 2` sent to C before `QUIT` | The sleep happens in the app, not the runner. C sees its tab settle on `/dest` before quitting saves it. The runner reads `js_before` and `dest_repeats` right after sending it, as the script does. |

Every window starts once its positive condition has been seen. Polls run every 200 ms in both versions (the script's `sleep 0.2`). Rust spends less time per poll round (no forks), so a window can start a few tens of milliseconds closer to the event than before. That does not shorten the time the window covers after the event, and is accepted.

## Risks (from the survey, resolved)

- **R1: race scenarios s9, s10, s11c and s12c under faster sends.**
  - The sends themselves are as close as before (correction 2).
  - What changes: there is no `cp` between `wait_field ... setup` and the sends, and the later polls run sooner.
  - Every assertion already accepts either winner, and `s12c2` already guards against a transient agreement.
  - So a new failure under the port is evidence of an app bug that the slower script hid. It is never loosened. It is debugged with superpowers:systematic-debugging, and the user is told.
  - Task 7 records the winner of each race in 3 shell and 3 Rust runs, from the `scenario N: ok (...)` lines, so a shift in the distribution is visible.
- **R2: one `--profile-dir` shared by two instances, against the script's shared `APPDATA`.**
  - The two are equivalent. `profile::dir()` returns `--profile-dir` in place of `%APPDATA%\manifold`, and every path into the profile starts there (CLAUDE.md "Tests and the user's profile"; `profile::tests` enforces it).
  - The store's lock is per write (`session.mnf.lock`), and sharing is what the script already does.
  - The WebView2 user data folder is `<host exe>.WebView2` and does not depend on the profile.
  - Engine exes are found beside `manifold.exe` (`availability.rs` `engine_binary_path`).
  - One difference, harmless: the script's coprocs wrote stdout into a pipe nobody drained. `Instance` writes it to `out/<who>.log`, which also gives the failure tails.
- **R3: scenario 7's retry.** Kept: 2 attempts, each `WAITIDLE`, `ADDRESSBAR`, `WAITIDLE`, `ENTER`, then up to 10 s for the server to see `/js`. The script's 50 x 0.2 s gives the same 10 s.
- **R4: serde_json `preserve_order` when comparing sessions.**
  - `session::canonical` sorts object keys at every depth itself.
  - The unit tests build the same node with keys in two orders, and run under both feature sets (Global constraints).
  - `Value ==` is used only for scalars and arrays: the field values and the final position.
- **R5: the 240 s server lifetime is gone.** The site lives exactly as long as the run. The script needed the lifetime only so that nothing had to kill python.
- **R6: positive-wait deadlines.**
  - The script counts iterations (150 or 300 rounds of work plus 0.2 s). The port uses wall-clock deadlines instead: 90 s for agreement and counter waits, 150 s for s3's ">= 4 requests" (the script's 600 rounds), and 10 s per s7 attempt.
  - A deadline matters only when the check is failing anyway. 90 s is longer than a script round count took at normal fork cost (about 150 x 0.4 s).
  - Decision 6 asks the user to confirm.
- **R7: one tick file per instance.**
  - The script's `tick` writes both instances' EXPORTSTATE to one `tick.json`. In wait_agree and wait_field, A's and B's ticks are sent back to back, so two processes can rename over the same file at once.
  - The port uses `a-tick.json` and `b-tick.json`. The tick's only purpose, a drawn frame, is unchanged.
  - This is the one deliberate behaviour deviation (Decision 7).

## File structure

| File | Change | Responsibility |
|---|---|---|
| `crates/gate/src/http.rs` | create | The generic HTTP/1.0 server: `Handler`, `Server::start/port`, `response`. |
| `crates/gate/src/suggest.rs` | modify | Keeps `respond`, `query_param`, `percent_decode`, `url_query` and its `Server` API (`start`, `url`, `requests`), built on `http::Server`. |
| `crates/gate/src/nonce.rs` | create | `NonceState::respond`, `Counters`, `Site` (was `nonce_server.py`). |
| `crates/gate/src/session.rs` | create | `canonical`, `same_nodes`, `count_kind`, `new_id`, `has_id`, `group_of`, `Field`, `field_agree`, `field_of`, `no_cycle`, `lost_ids` (was `compare_sessions.py` and `session_asserts.py`). |
| `crates/gate/src/check.rs` | modify | Gains `Marks` (moved from smoke.rs) and loses the `timing.sh` wording. |
| `crates/gate/src/instance.rs` | modify | Gains `pub fn remove_dir_with_retries` (moved from smoke.rs `remove_with_retries`). |
| `crates/gate/src/smoke.rs` | modify | Uses the moved `Marks` and `remove_dir_with_retries`. |
| `crates/gate/src/two/mod.rs` | create | `two()` entry, `Two`, `Who`, `Halt`, `Step`, `require`, `observe`, the wait helpers. |
| `crates/gate/src/two/groups.rs` | create | Scenarios 1, 2, 8, 9, 10, 11, 12. |
| `crates/gate/src/two/web.rs` | create | Scenarios 3, 4, 5. |
| `crates/gate/src/two/restart.rs` | create | Scenarios 6, 7. |
| `crates/gate/src/main.rs` | modify | `mod http; mod nonce; mod session; #[cfg(windows)] mod two;`, the `"two"` arm, usage string, module doc. |
| `crates/gate/Cargo.toml` | modify (Task 8) | Description names the two-instance test. |
| `scripts/gate-timing.sh`, `scripts/gate-tools-test.sh` | modify (Task 1, Task 8) | The ENVIRON fix and its check (Task 1); drop the `timing.sh` check (Task 8). |
| `scripts/gate.sh`, `scripts/gate-tier.sh`, `README.md`, `CHANGELOG.md`, `CLAUDE.md` | modify (Task 8) | References. |
| `scripts/two-instance-test.sh`, `compare_sessions.py`, `session_asserts.py`, `nonce_server.py`, `timing.sh` | delete (Task 8) | Replaced. |
| `docs/superpowers/plans/2026-10-10-rust-gate-runner.md` | modify (Task 8) | Task 5 points here. |

`http.rs`, `nonce.rs` and `session.rs` are not Windows-specific and are declared without `cfg`, like `suggest.rs`. `two/` is `#[cfg(windows)]`, like `smoke`.

## Tasks

### Task 1: measure the python and MSYS share of the current script, and fix gate-timing's CPU join

The point of this task is to know the real saving before any port code is written.

**Files:**
- create (scratch, gitignored): `.superpowers/two/measure.sh`, `.superpowers/two/sum.awk`
- modify: `scripts/gate-timing.sh`, `scripts/gate-tools-test.sh`

- [ ] **Preconditions.**
  - `Get-Process manifold,python,gate,cargo,rustc -EA 0` shows nothing of ours (another session's processes are theirs: note them, never stop them).
  - `df -h /c` has at least 5 GB free.
  - Record CPU with gate.sh's `cpu()` one-liner, and `(Get-MpComputerStatus).RealTimeProtectionEnabled`. If CPU is above about 30 %, wait or report it.
  - Build with `cargo build -j 8 -p manifold -p manifold-engine-webview2 -p gate`.
- [ ] **Write `.superpowers/two/measure.sh`.** It runs, sequentially:
  1. the fork calibration;
  2. two traced runs;
  3. one untraced run;
  4. the calibration again.

  The CPU sampler runs throughout.

```bash
#!/usr/bin/env bash
# Task 1 measurement for docs/superpowers/plans/2026-10-10-rust-gate-two.md. Launch detached:
#   cmd //c start "" //b //belownormal bash .superpowers/two/measure.sh
# Writes .superpowers/two/{measure.out,trace-1.err,trace-2.err,trace-N.out,plain.out,cpu.log} and measure.done last.
cd "$(dirname "$0")/../.." || exit 1
d=.superpowers/two
rm -f "$d/measure.done" "$d/cpu.log" "$d/cpu-stop"
powershell -NoProfile -File "$(cygpath -aw scripts/cpu-sampler.ps1)" -Out "$(cygpath -aw "$d/cpu.log")" \
    -Stop "$(cygpath -aw "$d/cpu-stop")" </dev/null >/dev/null 2>&1 &
cal() { # mean milliseconds per $(cat /dev/null), the fork every poll in the script pays
    local t0 t1 x
    t0=${EPOCHREALTIME/[.,]/}
    for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do x=$(cat /dev/null); done
    t1=${EPOCHREALTIME/[.,]/}
    echo "fork_ms_$1 $(((t1 - t0) / 20000))${x:-}" >>"$d/measure.out"
}
: >"$d/measure.out"
echo "start $(date +%s)" >>"$d/measure.out"
cal before
for n in 1 2; do
    t0=${EPOCHREALTIME/[.,]/}
    PS4='+ ${EPOCHREALTIME} ' bash -x scripts/two-instance-test.sh >"$d/trace-$n.out" 2>"$d/trace-$n.err"
    rc=$?
    echo "traced_$n exit=$rc secs=$(((${EPOCHREALTIME/[.,]/} - t0) / 1000000))" >>"$d/measure.out"
done
t0=${EPOCHREALTIME/[.,]/}
bash scripts/two-instance-test.sh >"$d/plain.out" 2>&1
rc=$?
echo "plain exit=$rc secs=$(((${EPOCHREALTIME/[.,]/} - t0) / 1000000))" >>"$d/measure.out"
cal after
touch "$d/cpu-stop"
echo "end $(date +%s)" >>"$d/measure.out"
echo done >"$d/measure.done"
```

- [ ] **Write `.superpowers/two/sum.awk`.** It splits a trace by `timing_mark` section and charges the time between consecutive trace lines to the kind of the earlier command.
  - `python` is an assert. `msys` is a forked MSYS tool. `poll` is `sleep` under 1 s, `fixed` is `sleep` of 1 s or more, and `wait` is `wait PID`. Everything else (builtins, function calls, the `[ -s ]` file tests, app time) is `other`.
  - In a pipeline after python or an MSYS tool, a later member traces while the first still runs, so `tr`, `cut`, `head` and `awk` there are counted but not timed.
  - The coproc's own `manifold.exe` line is skipped.

```awk
# Usage: awk -f .superpowers/two/sum.awk .superpowers/two/trace-N.err
# Input: a `PS4='+ ${EPOCHREALTIME} ' bash -x scripts/two-instance-test.sh` trace (lines "+ 1791693471.696452 cmd ...",
# one more + per subshell level).
function kind(c, a4) {
    if (c == "python") return ($0 ~ /nonce_server\.py/) ? "other" : "python"
    if (c == "sleep") return (a4 + 0 >= 1) ? "fixed" : "poll"
    if (c == "wait") return "wait"
    if (c ~ /^(cygpath|rm|tr|grep|cut|cat|ps|seq|cp|head|awk|taskkill|sed|mkdir|date|wc)$/) return "msys"
    return "other"
}
BEGIN { split("python msys poll fixed wait other", K, " "); sec = "pre"; order[++m] = sec; seen[sec] = 1 }
$1 ~ /^\++$/ && $2 ~ /^[0-9]+\.[0-9]+$/ {
    if ($0 ~ /manifold\.exe'? --headless/) next
    t = $2 + 0; c = $3; k = kind(c, $4)
    if ((ok == "python" || ok == "msys") && c ~ /^(tr|cut|head|awk)$/) { n[osec SUBSEP "msys"]++; next }
    if (ok != "") s[osec SUBSEP ok] += t - ot
    if (c == "timing_mark") { sec = ($4 == "" ? "end" : $4); if (!(sec in seen)) { seen[sec] = 1; order[++m] = sec } }
    n[sec SUBSEP k]++; ok = k; ot = t; osec = sec
}
END {
    printf "%-20s", "section"; for (j = 1; j <= 6; j++) printf " %12s", K[j]; printf "\n"
    for (i = 1; i <= m; i++) {
        x = order[i]; printf "%-20s", x
        for (j = 1; j <= 6; j++) {
            k = K[j]; printf " %7.1f/%-4d", s[x SUBSEP k], n[x SUBSEP k]
            T[k] += s[x SUBSEP k]; N[k] += n[x SUBSEP k]
        }
        printf "\n"
    }
    printf "%-20s", "TOTAL"; for (j = 1; j <= 6; j++) printf " %7.1f/%-4d", T[K[j]], N[K[j]]; printf "\n"
    printf "S_lo = python + msys = %.1f s (%d processes)\n", T["python"] + T["msys"], N["python"] + N["msys"]
}
```

- [ ] **Launch and wait.**
  - Launch `cmd //c start "" //b //belownormal bash .superpowers/two/measure.sh`.
  - Wait for `.superpowers/two/measure.done` with a Monitor or until-loop, deadline 15 min. Expect about 7 min: 3 runs of about 135 to 170 s plus the calibration.
  - Afterwards check `Get-Process manifold,python -EA 0` for leftovers.
- [ ] **Calibration gate.** If `fork_ms_before` or `fork_ms_after` in `measure.out` is above 200, the environment is not the one the gate normally runs in: a normal MSYS fork costs tens of milliseconds, and this session measured 2,200 to 2,800. Stop and report both numbers to the user; the split would overstate the saving.
- [ ] **Summarize.**
  - Run `awk -f .superpowers/two/sum.awk .superpowers/two/trace-N.err` for N = 1, 2.
  - Check that `traced_N exit=0` and `plain exit=0`. A red run is reported, not summarized.
  - Join the CPU with `bash scripts/gate-timing.sh collect plain .superpowers/two/plain.out .superpowers/two/cpu.log`. `plain.out` has the TIMING lines. The relative path has no backslash, so this works before the fix below.
- [ ] **Stop condition.**
  - `S_lo` is the median over the two traced runs. It is a lower bound on what the port removes: it does not count the forks of `$(...)` subshells or the per-`sleep` fork overhead, both of which land in `other` or `poll`.
  - **If `S_lo < 20 s`, stop:** report the table to the user and do not start Task 2.
  - Otherwise, fill in the Measurements section (both tables, `S_lo`, fork_ms, the traced and untraced wall times, CPU mean and Defender) and continue.
- [ ] **Fix `scripts/gate-timing.sh`'s CPU join.** Replace the `collect)` awk invocation's first two lines with these:

```bash
    grep '^TIMING ' "$3" | CPULOG="${4:-/dev/null}" awk -v tag="$2" '
        BEGIN { cpulog = ENVIRON["CPULOG"]; while ((getline l < cpulog) > 0) { split(l, f, " "); n++; ts[n] = f[1]; cp[n] = f[2] } close(cpulog) }
```

  Add this line to the `CPULOG is ...` header comment: `It is read through ENVIRON, not awk -v, so a Windows path keeps its backslashes.`

- [ ] **Add a Windows-path check** to `scripts/gate-tools-test.sh`, after the "collect joins each step with the cpu samples" check:

```bash
check "collect reads a Windows-path cpu log" "TIMING g/setup 12.345 100 cpu_start=5.0 cpu_mean=15.0" \
    "$(bash "$repo_root/scripts/gate-timing.sh" collect g "$t/run.log" "$(cygpath -w "$t/cpu.log")" | head -1)"
```

- [ ] **Proof for the fix.**
  - `bash scripts/gate-tools-test.sh` exits 0 with the new check.
  - The check fails without the fix:
    1. `git show HEAD:scripts/gate-timing.sh > .superpowers/two/old-gate-timing.sh`;
    2. run the new check's command line by hand with `.superpowers/two/old-gate-timing.sh` in place of `scripts/gate-timing.sh`;
    3. see `cpu_start=- cpu_mean=-`.
  - `bash scripts/gate-timing.sh collect two-1 "$(cygpath -w "$TEMP")\\gate-rust-full.log.two-1.log" "$(cygpath -w "$TEMP")\\gate-rust-full.log.cpu"` prints numbers, not `-`. If that log is gone, skip this and say so.
  - `git diff --stat` shows exactly the two script files.
- [ ] **Commit** the two script files only: `gate-timing: read the cpu log path through ENVIRON (a Windows path lost its backslashes); gate-tools-test checks it`.
- [ ] **Commit the Measurements section** of this plan in its own commit, whether or not the stop condition fired.

### Task 2: the parts that need no app (http, nonce site, session asserts) with unit tests

**Files:** create `crates/gate/src/http.rs`, `crates/gate/src/nonce.rs`, `crates/gate/src/session.rs`; modify `crates/gate/src/suggest.rs`, `crates/gate/src/main.rs` (the `mod` lines only).

- [ ] **`http.rs`.** Move `READ_TIMEOUT`, `handle` and the accept loop out of `suggest.rs`, so the handler is a parameter.

```rust
//! A minimal HTTP/1.0 server on 127.0.0.1 with an ephemeral port, for the gate's local sites: the smoke test's
//! suggestions endpoint (suggest.rs) and the two-instance test's nonce site (nonce.rs). Each connection gets a thread;
//! the handler turns the request line into the whole response, which is written before the connection closes.
//! Dropping the server stops its accept loop.

use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// A connection that sends nothing is dropped after this long (python's server would wait forever in a thread).
const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// The request line (`GET /r?n=1 HTTP/1.1`) to the complete response bytes.
pub type Handler = dyn Fn(&str) -> Vec<u8> + Send + Sync;

/// `HTTP/1.0 <status>` with Content-Type, Content-Length, the `extra` headers and `Connection: close`, then `body`.
pub fn response(status: &str, extra: &[(&str, &str)], ctype: &str, body: &[u8]) -> Vec<u8> {
    let mut head = format!("HTTP/1.0 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\n", body.len());
    for (k, v) in extra {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("Connection: close\r\n\r\n");
    let mut out = head.into_bytes();
    out.extend_from_slice(body);
    out
}

fn handle(mut stream: TcpStream, handler: &Handler) -> io::Result<()> {
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    let mut head = Vec::new();
    let mut buf = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") && head.len() < 16 * 1024 {
        let n = stream.read(&mut buf)?;
        if n == 0 {
            break;
        }
        head.extend_from_slice(&buf[..n]);
    }
    let text = String::from_utf8_lossy(&head);
    let line = text.lines().next().unwrap_or("");
    stream.write_all(&handler(line))?;
    stream.shutdown(Shutdown::Both)
}

/// The running server. Dropping it stops the accept loop.
pub struct Server {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    pub fn start(handler: Arc<Handler>) -> io::Result<Server> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let addr = listener.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let thread = thread::spawn(move || {
            for stream in listener.incoming() {
                if stop2.load(Ordering::SeqCst) {
                    break;
                }
                if let Ok(stream) = stream {
                    let handler = handler.clone();
                    thread::spawn(move || {
                        let _ = handle(stream, &*handler);
                    });
                }
            }
        });
        Ok(Server { addr, stop, thread: Some(thread) })
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the accept loop so it sees the flag.
        let _ = TcpStream::connect(self.addr);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
```

  If clippy flags `format_push_string`, use `use std::fmt::Write as _;` with `let _ = write!(head, "{k}: {v}\r\n");`, and rename the io import to `io::Write as _` where it is needed.

- [ ] **`suggest.rs`.**
  - Keep the module doc (append: "The server itself is `http::Server`."), `percent_decode`, `hex`, `query_param`, `url_query` and the tests.
  - `respond` builds its response with `http::response(status, &[], ctype, body.as_bytes())`.
  - `Server` becomes:

```rust
/// The running endpoint. Dropping it stops the server.
pub struct Server {
    http: http::Server,
    log: Arc<Mutex<Vec<String>>>,
}

impl Server {
    /// Listens on `127.0.0.1` with an ephemeral port and answers each connection on a thread of its own.
    pub fn start() -> io::Result<Server> {
        let log = Arc::new(Mutex::new(Vec::new()));
        let log2 = log.clone();
        let http = http::Server::start(Arc::new(move |line: &str| {
            if let Ok(mut l) = log2.lock() {
                l.push(line.split(' ').nth(1).unwrap_or("").to_string());
            }
            respond(line)
        }))?;
        Ok(Server { http, log })
    }

    /// The value for `MANIFOLD_SUGGESTIONS_URL`.
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/ac/", self.http.port())
    }

    /// The request targets seen so far (path and query), oldest first.
    pub fn requests(&self) -> Vec<String> {
        self.log.lock().map(|l| l.clone()).unwrap_or_default()
    }
}
```

  The existing tests (`suggestions_have_duckduckgos_list_shape`, `serves_over_a_socket_and_logs_the_request`) pass unchanged. Two things the move leaves behind, which clippy `-D warnings` rejects:
  - `Read`, `Write` and `TcpStream` are then used only by the tests, so their imports move into `mod tests`;
  - `READ_TIMEOUT`'s doc comment goes with it to `http.rs`, leaving no doc line orphaned.

  Items moved into `check.rs` and `instance.rs` (Task 3) go above their `#[cfg(test)] mod tests`, because clippy's `items_after_test_module` fires otherwise.

- [ ] **`nonce.rs`.** It has `nonce_server.py`'s exact semantics, which these items list.
  - Every GET is logged, as python's `do_GET` logs before routing: `/r`, `/link`, `/js`, `/spa`, `/spa/c`, `/spa/t`, `/dest`, and the 404s (favicon).
  - `n` is `parse_qs(query).get("n", [None])[0]`, so a blank `n=` counts as none (`query_param(q, "n", false)`).
  - `/dest` adds `n` to the set, including `None`.
  - A non-GET request gets 501 and is not logged (BaseHTTPRequestHandler has no `do_POST`).

```rust
//! The two-instance test's local site (was scripts/nonce_server.py), served in process so the test reads its counters
//! directly instead of from files. `/r` redirects to a fresh `/r?n=N` on every load and serves each issued `n` once,
//! like a login nonce consumed on use: another instance loading a url one instance shared asks for a served `n` again
//! (`r_repeats`). `/link` is one window-sized link to `/r`; `/js` navigates by script, 500 ms after load, to a fresh
//! `/dest?n=` (`dests`, `dest_repeats`); `/spa` is a single-page app whose click pushes `/spa/c?n=` and whose timer
//! pushes `/spa/t?n=` a second later. Neither push asks the server for anything; only a load of a shared url does
//! (`spa_clicks`, `spa_timers`).

use std::collections::HashSet;
use std::io;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use crate::http::{self, response};
use crate::suggest::{query_param, url_query};

const LINK: &str = "<!doctype html><title>link</title>\n<a href=\"/r\" style=\"position:fixed;inset:0;display:block\">go</a>";
const JS: &str = "<!doctype html><title>js</title>\n<script>setTimeout(function () { location.href = \"/dest?n=\" + Math.random(); }, 500);</script>";
const SPA: &str = "<!doctype html><title>spa</title>\n<div style=\"position:fixed;inset:0\">spa</div>\n<script>onclick = function () {\n  history.pushState({}, \"\", \"/spa/c?n=\" + Math.random());\n  setTimeout(function () { history.pushState({}, \"\", \"/spa/t?n=\" + Math.random()); }, 1000);\n};</script>";
const DEST: &str = "<!doctype html><title>dest</title>";
const ONCE: &str = "<!doctype html><title>nonce</title><p>served once";

#[derive(Default)]
pub struct NonceState {
    next: u64,
    issued: HashSet<String>,
    served: HashSet<String>,
    hits: u32,
    r_repeats: u32,
    dests: HashSet<Option<String>>,
    dest_repeats: u32,
    spa: [u32; 2],
    /// Every GET, as (time since the site started, raw request target).
    log: Vec<(Duration, String)>,
}

impl NonceState {
    /// The response to one request line; `now` is the time since the site started, for the log.
    pub fn respond(&mut self, request_line: &str, now: Duration) -> Vec<u8> {
        let mut parts = request_line.split(' ');
        let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
        if method != "GET" {
            return response("501 Unsupported method", &[], "text/html", b"");
        }
        self.log.push((now, target.to_string()));
        let path = target.split(['?', '#']).next().unwrap_or("");
        let n = query_param(url_query(target), "n", false);
        let page = |body: &str| response("200 OK", &[], "text/html", body.as_bytes());
        match path {
            "/link" => page(LINK),
            "/js" => page(JS),
            "/spa" => page(SPA),
            "/spa/c" => {
                self.spa[0] += 1;
                page(SPA)
            }
            "/spa/t" => {
                self.spa[1] += 1;
                page(SPA)
            }
            "/dest" => {
                if self.dests.contains(&n) {
                    self.dest_repeats += 1;
                }
                self.dests.insert(n);
                page(DEST)
            }
            "/r" => {
                self.hits += 1;
                let was_served = n.as_ref().is_some_and(|n| self.served.contains(n));
                if was_served {
                    self.r_repeats += 1;
                }
                match n {
                    Some(n) if self.issued.contains(&n) && !was_served => {
                        self.served.insert(n);
                        page(ONCE)
                    }
                    _ => {
                        self.next += 1;
                        self.issued.insert(self.next.to_string());
                        let to = format!("/r?n={}", self.next);
                        response("302 Found", &[("Location", &to)], "text/html", b"")
                    }
                }
            }
            _ => response("404 Not Found", &[], "text/html", b""),
        }
    }
}

/// A handle on the site's state, shared with its server threads.
#[derive(Clone, Default)]
pub struct Counters(Arc<Mutex<NonceState>>);

impl Counters {
    fn lock(&self) -> MutexGuard<'_, NonceState> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
    /// Requests for `/r` (python's `<hits-file>`).
    pub fn hits(&self) -> u32 {
        self.lock().hits
    }
    /// An already-served `/r?n=` asked for again (`.repeats` field 1).
    pub fn r_repeats(&self) -> u32 {
        self.lock().r_repeats
    }
    /// An already-seen `/dest?n=` asked for again (field 2).
    pub fn dest_repeats(&self) -> u32 {
        self.lock().dest_repeats
    }
    /// Distinct `/dest` urls asked for (field 3).
    pub fn dests(&self) -> usize {
        self.lock().dests.len()
    }
    /// `/spa/c` and `/spa/t` loads (fields 4 and 5).
    pub fn spa_clicks(&self) -> u32 {
        self.lock().spa[0]
    }
    pub fn spa_timers(&self) -> u32 {
        self.lock().spa[1]
    }
    /// GETs whose raw target is exactly `target` (the script's `grep -c " $1\$" hits.log`).
    pub fn served(&self, target: &str) -> usize {
        self.lock().log.iter().filter(|(_, t)| t == target).count()
    }
    /// The request log as `hits.log` had it ("<seconds> <target>" per line) and the `.repeats` line, for failures.
    pub fn report(&self) -> String {
        let s = self.lock();
        let mut out: Vec<String> = s.log.iter().map(|(t, p)| format!("{:.2} {p}", t.as_secs_f64())).collect();
        out.push(format!(
            "repeats: r {} dest {} dests {} spa_c {} spa_t {}",
            s.r_repeats,
            s.dest_repeats,
            s.dests.len(),
            s.spa[0],
            s.spa[1]
        ));
        out.join("\n")
    }
}

/// The running site. Dropping it stops the server.
pub struct Site {
    server: http::Server,
    counters: Counters,
}

impl Site {
    pub fn start() -> io::Result<Site> {
        let counters = Counters::default();
        let c = counters.clone();
        let t0 = Instant::now();
        let server = http::Server::start(Arc::new(move |line: &str| c.lock().respond(line, t0.elapsed())))?;
        Ok(Site { server, counters })
    }
    pub fn port(&self) -> u16 {
        self.server.port()
    }
    pub fn counters(&self) -> &Counters {
        &self.counters
    }
}
```

- [ ] **`nonce.rs` tests** (`#[cfg(test)] mod tests`). Each is a `#[test] fn`, and each is written to fail first against a stub that returns a 404 for everything.
  - `GET /r` gives a 302 with `Location: /r?n=1`, and `hits() == 1`.
  - `GET /r?n=1` gives 200 with the `served once` body, and `r_repeats() == 0`.
  - `GET /r?n=1` again gives a 302 to `/r?n=2`, and `r_repeats() == 1`.
  - `GET /r?n=99` (never issued) gives a 302 to `n=3`, and `r_repeats` is still 1.
  - `/dest?n=a` twice and `/dest?n=b` once give `dests() == 2` and `dest_repeats() == 1`. `/dest` with no `n` twice: `None` counts once, then repeats once.
  - `/spa/c` and `/spa/t` count separately and both serve the SPA body. `/spa` counts neither.
  - `/favicon.ico` gives a 404 and is logged. `served("/favicon.ico") == 1`, and `served("/js")` does not count `/js?x=1`.
  - `POST /r` gives a 501, is not logged, and leaves `hits` unchanged.
  - `/r?n=` (blank) is treated as no `n`: a 302, and no repeat.
  - Over a real socket, `Site::start()`: send `GET /r` and read a 302, then `GET /r?n=1` and read a 200.
- [ ] **Bodies against python (throwaway, not committed).**
  1. Start the old server: `python -P scripts/nonce_server.py .superpowers/two/ph 30 >.superpowers/two/ph.port </dev/null &`. It exits by itself after 30 s.
  2. Fetch `/link`, `/js`, `/spa`, `/dest?n=x` and `/r?n=1` (after a `/r`) with `curl -s` into `.superpowers/two/bodies/{link,js,spa,dest,once}.html`.
  3. Add a temporary `#[test]` comparing each constant with `include_str!("../../../.superpowers/two/bodies/<name>.html")`, run it, then delete it.
  4. Report the result, and that no `python.exe` is left.
- [ ] **`session.rs`.**

```rust
//! The two-instance test's JSON asserts (were scripts/compare_sessions.py and scripts/session_asserts.py), over
//! EXPORTSESSION documents (`{"nodes": [...], "local": ...}`) and EXPORTSTATE files (`mirror_groups`, `mirror_names`,
//! `mirror_texts`, `mirror_pos`: what that instance shows, by node id). Node ids are strings (the store's model); a
//! node without one is ignored by the id-based asserts.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde_json::Value;

pub fn nodes(doc: &Value) -> &[Value] {
    doc.get("nodes").and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

fn id(n: &Value) -> Option<&str> {
    n.get("id").and_then(Value::as_str)
}

/// A comparison key with python's `json.dumps(v, sort_keys=True)` equality: object keys sorted at every depth
/// (code-point order, which is UTF-8 byte order), scalars in serde's form, so `1` and `1.0` and `-0.0` stay distinct
/// as in python. Independent of serde_json's `preserve_order`.
pub fn canonical(v: &Value) -> String {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let inner: Vec<String> =
                keys.iter().map(|k| format!("{}:{}", Value::String((*k).clone()), canonical(&m[k.as_str()]))).collect();
            format!("{{{}}}", inner.join(","))
        }
        Value::Array(a) => format!("[{}]", a.iter().map(canonical).collect::<Vec<_>>().join(",")),
        other => other.to_string(),
    }
}

/// compare_sessions.py: the two documents' shared nodes are the same multiset (`local` is personal and ignored).
/// Ok(node count) or Err(what differs, for the failure line).
pub fn same_nodes(a: &Value, b: &Value) -> Result<usize, String> {
    let (Some(na), Some(nb)) =
        (a.get("nodes").and_then(Value::as_array), b.get("nodes").and_then(Value::as_array))
    else {
        return Err("an export has no nodes array".to_string());
    };
    let keys = |n: &[Value]| {
        let mut k: Vec<String> = n.iter().map(canonical).collect();
        k.sort();
        k
    };
    if keys(na.as_slice()) == keys(nb.as_slice()) {
        return Ok(na.len());
    }
    // Only for the failure line: which ids differ (a duplicated id shows as a count difference).
    let by_id = |n: &[Value]| -> BTreeMap<String, String> {
        n.iter().map(|x| (id(x).unwrap_or("<no id>").to_string(), canonical(x))).collect()
    };
    let (ia, ib) = (by_id(na.as_slice()), by_id(nb.as_slice()));
    let differ: BTreeSet<&str> =
        ia.keys().chain(ib.keys()).filter(|k| ia.get(*k) != ib.get(*k)).map(String::as_str).take(5).collect();
    Err(format!("sessions differ: A has {} nodes, B has {}; differing ids: {differ:?}", na.len(), nb.len()))
}

/// session_asserts.py count_kind (has_kinds is `count_kind(..) >= want`).
pub fn count_kind(doc: &Value, kind: &str) -> usize {
    nodes(doc).iter().filter(|n| n.get("type").and_then(Value::as_str) == Some(kind)).count()
}

/// session_asserts.py new_id: the id of the one node in `after` and not in `before`.
pub fn new_id(before: &Value, after: &Value) -> Result<String, String> {
    let had: HashSet<&str> = nodes(before).iter().filter_map(id).collect();
    let new: Vec<&str> = nodes(after).iter().filter_map(id).filter(|i| !had.contains(i)).collect();
    match new.as_slice() {
        [one] => Ok((*one).to_string()),
        _ => Err(format!("expected exactly 1 new node, got {}: {new:?}", new.len())),
    }
}

pub fn has_id(doc: &Value, wanted: &str) -> bool {
    nodes(doc).iter().any(|n| id(n) == Some(wanted))
}

/// session_asserts.py group_of: None if the id is missing, Some(None) at the top level (python prints "null" for a
/// missing, null or empty group), Some(Some(group)) otherwise. The last node with the id wins, as in python's dict.
pub fn group_of(doc: &Value, wanted: &str) -> Option<Option<String>> {
    nodes(doc).iter().rev().find(|n| id(n) == Some(wanted)).map(|n| {
        n.get("group").and_then(Value::as_str).filter(|g| !g.is_empty()).map(str::to_string)
    })
}

/// The script's way of printing a group_of result.
pub fn show_group(g: &Option<Option<String>>) -> String {
    match g {
        None => "<absent>".to_string(),
        Some(None) => "null".to_string(),
        Some(Some(g)) => g.clone(),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Field {
    Group,
    Name,
    Text,
    Pos,
}

impl Field {
    /// The EXPORTSTATE map.
    pub fn state_key(self) -> &'static str {
        match self {
            Field::Group => "mirror_groups",
            Field::Name => "mirror_names",
            Field::Text => "mirror_texts",
            Field::Pos => "mirror_pos",
        }
    }
    /// The EXPORTSESSION node key, and the word in messages.
    pub fn doc_key(self) -> &'static str {
        match self {
            Field::Group => "group",
            Field::Name => "name",
            Field::Text => "text",
            Field::Pos => "pos",
        }
    }
}

/// session_asserts.py same_value: all equal; lists elementwise within 1e-3 (f32 through JSON).
fn same_value(values: &[&Value]) -> bool {
    match values[0] {
        Value::Array(first) => values.iter().all(|v| {
            v.as_array().is_some_and(|v| {
                v.len() == first.len()
                    && v.iter().zip(first).all(|(a, b)| match (a.as_f64(), b.as_f64()) {
                        (Some(a), Some(b)) => (a - b).abs() <= 1e-3,
                        _ => false,
                    })
            })
        }),
        first => values.iter().all(|v| *v == first),
    }
}

fn show(v: Option<&Value>) -> String {
    v.map_or_else(|| "<absent>".to_string(), Value::to_string)
}

/// session_asserts.py field_agree: each id's field is present and the same in A's and B's EXPORTSTATE map and in the
/// document (a node without the key counts as null there). Ok or Err carries one line per id:
/// "<id> <field>: A shows <json>, B shows <json>, document <json>".
pub fn field_agree(field: Field, a: &Value, b: &Value, doc: &Value, ids: &[&str]) -> Result<Vec<String>, Vec<String>> {
    let map = |s: &Value| s.get(field.state_key()).and_then(Value::as_object).cloned().unwrap_or_default();
    let (ma, mb) = (map(a), map(b));
    let null = Value::Null;
    let mut d: HashMap<&str, &Value> = HashMap::new();
    for n in nodes(doc) {
        if let Some(i) = id(n) {
            d.insert(i, n.get(field.doc_key()).unwrap_or(&null));
        }
    }
    let mut ok = true;
    let mut lines = Vec::new();
    for i in ids {
        let seen = [ma.get(*i), mb.get(*i), d.get(i).copied()];
        lines.push(format!(
            "{i} {}: A shows {}, B shows {}, document {}",
            field.doc_key(),
            show(seen[0]),
            show(seen[1]),
            show(seen[2])
        ));
        match seen {
            [Some(x), Some(y), Some(z)] if same_value(&[x, y, z]) => {}
            _ => ok = false,
        }
    }
    if ok {
        Ok(lines)
    } else {
        Err(lines)
    }
}

/// session_asserts.py field_of: the id's value in that EXPORTSTATE map (None if absent).
pub fn field_of(state: &Value, field: Field, wanted: &str) -> Option<Value> {
    state.get(field.state_key())?.get(wanted).cloned()
}

/// session_asserts.py no_cycle: following `group` from any node never loops.
pub fn no_cycle(doc: &Value) -> Result<(), String> {
    let groups: BTreeMap<&str, Option<&str>> =
        nodes(doc).iter().filter_map(|n| Some((id(n)?, n.get("group").and_then(Value::as_str)))).collect();
    for &start in groups.keys() {
        let mut seen = HashSet::new();
        let mut cur = Some(start);
        while let Some(c) = cur {
            if !seen.insert(c) {
                return Err(format!("a group cycle through {c:?} (from {start:?})"));
            }
            cur = groups.get(c).copied().flatten();
        }
    }
    Ok(())
}

/// session_asserts.py kept_ids, inverted: the ids in `before` missing from `after`, sorted (empty = kept).
pub fn lost_ids(before: &Value, after: &Value) -> Vec<String> {
    let have: HashSet<&str> = nodes(after).iter().filter_map(id).collect();
    let mut lost: Vec<String> =
        nodes(before).iter().filter_map(id).filter(|i| !have.contains(i)).map(str::to_string).collect();
    lost.sort();
    lost.dedup();
    lost
}
```

- [ ] **`session.rs` tests.** Each is a `#[test] fn` over inline `json!` fixtures. Run them with and without `--features serde_json/preserve_order`.
  - `canonical`: `{"b":1,"a":[{"d":2,"c":3}]}` equals the same object written as `{"a":[{"c":3,"d":2}],"b":1}`, also when built by `serde_json::from_str` in each key order. `json!(1)` and `json!(1.0)` give different keys, as do `-0.0` and `0.0`.
  - `same_nodes`:
    - reordered nodes give `Ok(n)`;
    - a node duplicated in A but not B is an `Err`;
    - different `local` values are still `Ok`;
    - a changed field is an `Err` that names the id;
    - a missing `nodes` is an `Err`.
  - `new_id`: zero, one and two new nodes.
  - `group_of`: in a group, `null`, `""`, missing key, missing id. Also `show_group` for each.
  - `field_agree`:
    - all equal;
    - one differs;
    - absent in B's map;
    - absent from the document;
    - a document node without the key against `null` in both maps (agrees);
    - pos `[1.0, 2.0]` against `[1.0005, 2.0]` (agrees) and against `[1.002, 2.0]` (does not);
    - pos against `null` (does not);
    - the lines' text.
  - `field_of`: present, absent, null.
  - `no_cycle`: a chain (Ok), G and H in each other (Err), self-group (Err), group pointing at a missing id (Ok).
  - `lost_ids`: none, one.
  - `count_kind`.
- [ ] **Python differential (throwaway).**
  - Write the `same_nodes`, `new_id`, `group_of`, `field_agree`, `no_cycle` and `kept_ids` fixtures from the tests as files under `.superpowers/two/fixtures/`.
  - Run each through the python helpers, for example `python -P scripts/session_asserts.py field_agree pos fa.json fb.json doc.json n </dev/null; echo rc=$?` and `python -P scripts/compare_sessions.py a.json b.json </dev/null`. Never `python -`.
  - Tabulate python's exit code and output against the Rust test's expectation in the task report. Any disagreement is a port bug to fix, or a deliberate deviation to state. Known deviations:
    - the info lines print JSON (`"Both A"`, `null`), where python printed a repr (`'Both A'`, `None`);
    - python's `same_value` treats `1 == 1.0` as equal for scalars, while `Value ==` does not. No mirror field is a scalar number, so record it and move on.
- [ ] **Wire the modules.** In `main.rs` add `mod http; mod nonce; mod session;` beside `mod suggest;`, without `cfg`.
- [ ] **Proof.**
  - Run `cargo fmt --all`, `cargo fmt --all --check`, the CLAUDE.md clippy line, `cargo test -j 8 -p gate`, and `cargo test -j 8 -p gate --features serde_json/preserve_order`. All pass, with the new test count reported.
  - Because `suggest.rs` changed, run `gate smoke` once:
    1. build with `cargo build -j 8 -p manifold -p manifold-engine-webview2 -p gate`;
    2. launch `cmd //c start "" //b //belownormal bash .superpowers/two/run.sh smoke-t2 smoke`;
    3. require `exit=0` and 54 for `grep -c PASS`.
  - Check for leftovers with `Get-Process manifold -EA 0`. Any manifold.exe is either ours, which is a bug, or another session's, which you leave alone and note.
- [ ] **Commit:** `gate: http.rs server core (suggest.rs on it), nonce.rs site, session.rs asserts, with tests`.

### Task 3: the driver, plus scenarios 1, 2, 8, 9 and 10

**Files:** create `crates/gate/src/two/mod.rs` and `crates/gate/src/two/groups.rs` (s1, s2, s8, s9, s10 only); modify `check.rs` and `instance.rs` (the moves), `smoke.rs` (use them), and `main.rs`.

- [ ] **Move `Marks` from `smoke.rs` to `check.rs`, and make it `pub`.**

```rust
/// The timing marks: opening one ends the previous (`Section::next`), `end` closes the last.
pub struct Marks(Option<Section>);

impl Marks {
    pub fn start(name: &str) -> Marks {
        Marks(Some(Section::start(name)))
    }

    pub fn mark(&mut self, name: &str) {
        self.0 = self.0.take().map(|s| s.next(name));
    }

    pub fn end(&mut self) {
        self.0 = None;
    }
}
```

  In `smoke.rs`, delete its `Marks`, use `crate::check::Marks`, and replace `Marks(Some(Section::start("setup")))` with `Marks::start("setup")`. Drop any import the two moves leave unused in `smoke.rs` (likely `Section`, and `sleep` if nothing else uses it): clippy `-D warnings` fails on them.

- [ ] **Move `remove_with_retries`** from `smoke.rs` to `instance.rs` as `pub fn remove_dir_with_retries(dir: &Path)`, with the same body and the doc "Removes a work dir, retrying while a just-killed instance still holds files in it." `smoke.rs` calls the new name.
- [ ] **`two/mod.rs`.**

```rust
//! `gate two`: two headless instances (later a third, then a fourth, one at a time) on one isolated profile, exercising
//! the store's multi-writer merge end to end (was scripts/two-instance-test.sh; plan
//! docs/superpowers/plans/2026-10-10-rust-gate-two.md). The scenarios run in the script's order, 1, 2, 8 to 12, 3 to 7,
//! each building on the document the previous ones left. A failed precondition (`require`) stops the run at once; a
//! failed observation (`observe`) lets its scenario finish, then the run stops: later scenarios would only cascade.
//! `ENGINE` (default webview2) is the engine new tabs start in, which scenarios 3, 4, 5 and 7 navigate.

mod groups;
mod restart;
mod web;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::check::{check, exit_code, failures, note_failure, Marks};
use crate::instance::{log_tail, remove_dir_with_retries, seed_settings, wait_file, Instance};
use crate::link::no_verdict_words;
use crate::nonce::{Counters, Site};
use crate::session::{self, Field};
use crate::windows_main::{manifold_exe, work_dir};

/// An export the app never writes (the script's `wait_file`: 300 x 0.2 s).
const WAIT_FILE: Duration = Duration::from_secs(60);
/// Between poll rounds (the script's `sleep 0.2`): the app's cadence, not a fixed wait.
const PAUSE: Duration = Duration::from_millis(200);
/// A positive wait's deadline; it only matters when the check is failing (plan, risk R6).
const POSITIVE: Duration = Duration::from_secs(90);
const QUIT_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Who {
    A,
    B,
    C,
    D,
}

impl Who {
    fn name(self) -> &'static str {
        ["A", "B", "C", "D"][self as usize]
    }
    fn tag(self) -> &'static str {
        ["a", "b", "c", "d"][self as usize]
    }
}

/// Why the run stopped: a failed check (already printed) or a runner error (printed by `two`).
enum Halt {
    Failed,
    Error(io::Error),
}

impl From<io::Error> for Halt {
    fn from(e: io::Error) -> Halt {
        Halt::Error(e)
    }
}

type Step<T = ()> = Result<T, Halt>;

/// Prints `PASS:`/`FAIL:` and, on a failure, `detail` indented (never the verdict words: gate.sh counts them).
fn verdict(label: &str, ok: bool, detail: &str) -> bool {
    let ok = check(label, ok);
    if !ok {
        for line in no_verdict_words(detail).lines() {
            println!("    {line}");
        }
    }
    ok
}

/// A precondition: a failure stops the run here.
fn require(label: &str, ok: bool, detail: &str) -> Step {
    if verdict(label, ok, detail) {
        Ok(())
    } else {
        Err(Halt::Failed)
    }
}

/// An observation: a failure is counted, the scenario goes on to its end, then the run stops (`Two::scenario`).
fn observe(label: &str, ok: bool, detail: &str) -> bool {
    verdict(label, ok, detail)
}

/// Polls `cond` every PAUSE until it holds or `timeout` passes; whether it held.
fn wait_until(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if cond() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        sleep(PAUSE);
    }
}

/// Waits for an export and parses it.
fn read_json(path: &Path) -> Step<Value> {
    wait_file(path, WAIT_FILE)?;
    let v = serde_json::from_slice(&fs::read(path)?)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {e}", path.display())))?;
    Ok(v)
}

/// Informational lines, indented, never the verdict words.
fn info(text: &str) {
    for line in no_verdict_words(text).lines() {
        println!("    {line}");
    }
}

struct Two {
    exe: PathBuf,
    profile: PathBuf,
    /// Exports, test files and the instances' logs.
    out: PathBuf,
    insts: [Option<Instance>; 4],
    site: Option<Site>,
    marks: Marks,
}

impl Two {
    fn launch(&mut self, who: Who) -> Step {
        let inst = Instance::launch(&self.exe, &self.profile, &self.out, &format!("{}.log", who.tag()))?;
        println!("{}: pid {}", who.name(), inst.pid());
        self.insts[who as usize] = Some(inst);
        Ok(())
    }

    fn inst(&mut self, who: Who) -> io::Result<&mut Instance> {
        self.insts[who as usize]
            .as_mut()
            .ok_or_else(|| io::Error::other(format!("{} was never launched", who.name())))
    }

    fn send(&mut self, who: Who, line: &str) -> Step {
        self.inst(who)?.send(line)?;
        Ok(())
    }

    fn path(&self, file: &str) -> PathBuf {
        self.out.join(file)
    }

    /// Removes `<name>.json`, then sends `<cmd> <path>` without waiting, so A's and B's exports run side by side.
    fn request(&mut self, who: Who, cmd: &str, name: &str) -> Step<PathBuf> {
        let path = self.path(&format!("{name}.json"));
        let _ = fs::remove_file(&path);
        self.send(who, &format!("{cmd} {}", path.display()))?;
        Ok(path)
    }

    fn session(&mut self, who: Who, name: &str) -> Step<Value> {
        let p = self.request(who, "EXPORTSESSION", name)?;
        read_json(&p)
    }

    fn state(&mut self, who: Who, name: &str) -> Step<Value> {
        let p = self.request(who, "EXPORTSTATE", name)?;
        read_json(&p)
    }

    /// A no-op that still draws a frame (unlike SLEEP), so the instance's per-frame poll_store can pick up the other
    /// instance's edits. One file per instance (plan, risk R7).
    fn tick(&mut self, who: Who) -> Step {
        let p = self.path(&format!("{}-tick.json", who.tag()));
        self.send(who, &format!("EXPORTSTATE {}", p.display()))
    }

    /// ADDRESSBAR, WAITIDLE, ENTER: a typed navigation as the script types it.
    fn navigate(&mut self, who: Who, url: &str) -> Step {
        self.send(who, &format!("ADDRESSBAR {url}"))?;
        self.send(who, "WAITIDLE")?;
        self.send(who, "ENTER")
    }

    fn write_json(&self, file: &str, v: &Value) -> Step<PathBuf> {
        let path = self.path(file);
        fs::write(&path, v.to_string())?;
        Ok(path)
    }

    fn kill(&mut self, who: Who) {
        if let Some(i) = self.insts[who as usize].as_mut() {
            i.kill();
        }
    }

    fn quit(&mut self, who: Who) -> Step {
        self.inst(who)?.quit(QUIT_TIMEOUT)?;
        Ok(())
    }

    fn counters(&self) -> io::Result<Counters> {
        self.site.as_ref().map(|s| s.counters().clone()).ok_or_else(|| io::Error::other("the nonce site is not up"))
    }

    fn port(&self) -> io::Result<u16> {
        self.site.as_ref().map(Site::port).ok_or_else(|| io::Error::other("the nonce site is not up"))
    }

    /// Prints the header, runs the body, and prints `scenario N: ok<suffix>` only if nothing in it failed; any failure
    /// (a precondition's at once, an observation's at the end) stops the run.
    fn scenario(&mut self, mark: &str, n: u32, title: &str, body: impl FnOnce(&mut Two) -> Step<String>) -> Step {
        self.marks.mark(mark);
        println!("=== scenario {n}: {title} ===");
        let before = failures();
        let suffix = body(self)?;
        if failures() > before {
            return Err(Halt::Failed);
        }
        println!("scenario {n}: ok{suffix}");
        Ok(())
    }

    /// The script's wait_agree: exports both sessions until their shared nodes agree. A precondition. Returns A's.
    fn wait_agree(&mut self, label: &str, tag: &str) -> Step<Value> {
        let deadline = Instant::now() + POSITIVE;
        loop {
            self.tick(Who::A)?;
            self.tick(Who::B)?;
            let af = self.request(Who::A, "EXPORTSESSION", &format!("a{tag}"))?;
            let bf = self.request(Who::B, "EXPORTSESSION", &format!("b{tag}"))?;
            let (a, b) = (read_json(&af)?, read_json(&bf)?);
            match session::same_nodes(&a, &b) {
                Ok(n) => {
                    require(label, true, "")?;
                    info(&format!("{n} nodes"));
                    return Ok(a);
                }
                Err(e) if Instant::now() >= deadline => {
                    require(label, false, &e)?;
                }
                Err(_) => sleep(PAUSE),
            }
        }
    }

    /// The script's wait_field (wait_mirror for groups): until each id's field is the same in what A shows, what B
    /// shows and A's document. A precondition. Returns (A's EXPORTSTATE, the document).
    fn wait_field(&mut self, field: Field, label: &str, tag: &str, ids: &[&str]) -> Step<(Value, Value)> {
        let deadline = Instant::now() + POSITIVE;
        loop {
            self.tick(Who::A)?;
            self.tick(Who::B)?;
            let doc = self.session(Who::A, &format!("{tag}-doc"))?;
            self.tick(Who::B)?;
            let af = self.request(Who::A, "EXPORTSTATE", &format!("{tag}-a-state"))?;
            let bf = self.request(Who::B, "EXPORTSTATE", &format!("{tag}-b-state"))?;
            let (a, b) = (read_json(&af)?, read_json(&bf)?);
            match session::field_agree(field, &a, &b, &doc, ids) {
                Ok(lines) => {
                    require(label, true, "")?;
                    info(&lines.join("\n"));
                    return Ok((a, doc));
                }
                Err(lines) if Instant::now() >= deadline => {
                    require(label, false, &lines.join("\n"))?;
                }
                Err(_) => sleep(PAUSE),
            }
        }
    }

    /// The script's new_node, always on A: the id of the one node `cmd` adds. A precondition.
    fn new_node(&mut self, cmd: &str, label: &str, tag: &str) -> Step<String> {
        let before = self.session(Who::A, &format!("{tag}-before"))?;
        self.send(Who::A, cmd)?;
        let after = self.session(Who::A, &format!("{tag}-after"))?;
        let got = session::new_id(&before, &after);
        require(label, got.is_ok(), got.as_ref().err().map_or("", String::as_str))?;
        Ok(got.unwrap_or_default())
    }

    /// The script's show_local / show_local_field: each instance's own value right after a race. Informational:
    /// whether either had picked up the other's edit yet depends on frame timing.
    fn show_local(&mut self, field: Field, tag: &str, what: &str, ids: &[&str]) -> Step {
        let af = self.request(Who::A, "EXPORTSTATE", &format!("{tag}-a-local"))?;
        let bf = self.request(Who::B, "EXPORTSTATE", &format!("{tag}-b-local"))?;
        let (a, b) = (read_json(&af)?, read_json(&bf)?);
        for id in ids {
            let show = |s: &Value| session::field_of(s, field, id).map_or_else(|| "<absent>".to_string(), |v| v.to_string());
            info(&format!("{tag}: right after the {what}, {id}: A shows {}, B shows {}", show(&a), show(&b)));
        }
        Ok(())
    }
}

fn run(t: &mut Two) -> Step {
    t.launch(Who::A)?;
    t.launch(Who::B)?;
    groups::s1(t)?;
    groups::s2(t)?;
    groups::s8(t)?;
    groups::s9(t)?;
    groups::s10(t)?;
    groups::s11(t)?;
    groups::s12(t)?;
    web::s3(t)?;
    web::s4(t)?;
    web::s5(t)?;
    restart::s6(t)?;
    restart::s7(t)
}

pub fn two(parent: Option<PathBuf>) -> ExitCode {
    let fatal = |e: String| {
        eprintln!("{e}");
        ExitCode::from(1)
    };
    let exe = match manifold_exe() {
        Ok(exe) => exe,
        Err(e) => return fatal(e),
    };
    // The script's first mark comes after its freshness check.
    let marks = Marks::start("setup");
    let work = match work_dir(parent, "two") {
        Ok(w) => w,
        Err(e) => return fatal(format!("work dir: {e}")),
    };
    let (out, profile) = (work.join("out"), work.join("profile"));
    let engine = std::env::var("ENGINE").unwrap_or_else(|_| "webview2".to_string());
    if let Err(e) = fs::create_dir_all(&out).and_then(|()| seed_settings(&profile, &engine, "")) {
        return fatal(format!("setup: {e}"));
    }
    println!("two-instance test: exe {} profile {} engine {engine}", exe.display(), profile.display());
    let mut t = Two { exe, profile, out, insts: [None, None, None, None], site: None, marks };
    if let Err(Halt::Error(e)) = run(&mut t) {
        // Not a verdict line: gate.sh counts those words, and this failure is counted here.
        println!("ERROR: two stopped early: {e}");
        note_failure();
    }
    t.marks.end();
    let logs: Vec<PathBuf> = t.insts.iter().flatten().map(|i| i.log.clone()).collect();
    let site = t.site.as_ref().map(|s| s.counters().report());
    drop(t); // kills any instance still up and stops the site
    if failures() > 0 {
        for log in &logs {
            println!("--- tail of {}\n{}", log.display(), no_verdict_words(&log_tail(log, 30)));
        }
        if let Some(s) = site {
            println!("--- nonce site requests\n{}", no_verdict_words(&s));
        }
    }
    if std::env::var_os("KEEP").is_none() {
        remove_dir_with_retries(&work);
    } else {
        println!("kept {}", work.display());
    }
    println!("two-instance test: {}", if failures() == 0 { "ok" } else { "failed" });
    exit_code()
}
```

  The listing is the finished module. In Task 3, leave out `mod restart; mod web;`, `dump_urls` (Task 5) and the `run` lines for later scenarios, so `run` reads:

```rust
fn run(t: &mut Two) -> Step {
    t.launch(Who::A)?;
    t.launch(Who::B)?;
    groups::s1(t)?;
    groups::s2(t)?;
    groups::s8(t)?;
    groups::s9(t)?;
    groups::s10(t)
}
```

  Tasks 4, 5 and 6 each add their lines. `counters()` and `port()` are unused until Task 5; the crate's `#![allow(dead_code, ...)]` covers that.
- [ ] **`main.rs`.**
  - Add `#[cfg(windows)] mod two;`.
  - Add the arm `"two" => crate::two::two(parent),`.
  - The usage string becomes `usage: gate hello|link|smoke|two [WORKDIR_PARENT]`.
  - Add to the module doc's stage list: `two (scripts/two-instance-test.sh, until Task 8 deletes it)`. Task 8 rewrites that to plain `two`.
- [ ] **`two/groups.rs`, scenarios 1, 2, 8, 9 and 10.**

```rust
//! Scenarios 1, 2 and 8 to 12: the shared document. Instances A and B are up; nothing navigates.

use serde_json::json;

use super::{observe, require, Step, Two, Who, WAIT_FILE};
use crate::instance::wait_file;
use crate::session::{self, show_group, Field};
use crate::state::{GROUP, NOTE, TAB};

pub(super) fn s1(t: &mut Two) -> Step {
    t.scenario("s1-import", 1, "import + note", |t| {
        let pair = json!({"nodes": [
            {"id": "g", "type": GROUP, "name": "Pair"},
            {"id": "a", "type": TAB, "url": "manifold://welcome", "engine": "webview2", "pos": [-150.0, 0.0], "group": "g"},
            {"id": "b", "type": TAB, "url": "manifold://welcome", "engine": "webview2", "pos": [150.0, 0.0], "group": "g"}
        ]});
        let path = t.write_json("pair.json", &pair)?;
        t.send(Who::A, &format!("IMPORTGROUP {}", path.display()))?;
        t.send(Who::A, "WAITTABS")?;
        t.send(Who::B, "NEWNOTE")?;
        let doc = t.wait_agree("scenario 1: A and B agree on the shared nodes", "1")?;
        let (notes, tabs) = (session::count_kind(&doc, NOTE), session::count_kind(&doc, TAB));
        observe("scenario 1: the document has at least 1 note", notes >= 1, &format!("{notes} notes"));
        observe("scenario 1: the document has at least 2 tabs", tabs >= 2, &format!("{tabs} tabs"));
        Ok(String::new())
    })
}

pub(super) fn s2(t: &mut Two) -> Step {
    t.scenario("s2-compaction", 2, "compaction mid-run", |t| {
        // 200 notes of 500 chars each: ~100 KB of ops, past COMPACT_MIN (64 KiB) and twice a snapshot this small, so
        // A's save after the import compacts.
        let text = "x".repeat(500);
        let mut nodes = vec![json!({"id": "biggroup", "type": GROUP, "name": "Big"})];
        nodes.extend((1..=200).map(|i| json!({"id": format!("note{i}"), "type": NOTE, "text": text, "group": "biggroup"})));
        let path = t.write_json("big.json", &json!({ "nodes": nodes }))?;
        t.send(Who::A, &format!("IMPORTGROUP {}", path.display()))?;
        t.send(Who::A, "WAITTABS")?;
        // B edits only once A has compacted (the fresh profile had no `.prev`), so the edit lands on the rewritten file.
        let prev = t.profile.join("session.mnf.prev");
        require(
            "scenario 2: A's save after the import compacts the store (session.mnf.prev appears)",
            wait_file(&prev, WAIT_FILE).is_ok(),
            &format!("{} never appeared", prev.display()),
        )?;
        t.send(Who::B, "NEWNOTE")?;
        let doc = t.wait_agree("scenario 2: A and B agree on the shared nodes", "2")?;
        // A floor, not exact: the 200 imported notes plus B's notes.
        let notes = session::count_kind(&doc, NOTE);
        observe("scenario 2: the document has at least 201 notes", notes >= 201, &format!("{notes} notes"));
        Ok(String::new())
    })
}

pub(super) fn s8(t: &mut Two) -> Step {
    t.scenario("s8-move-into-group", 8, "a move into a group reaches the other instance", |t| {
        let g = t.new_node("NEWGROUP", "scenario 8: A's NEWGROUP adds exactly one node", "s8g")?;
        let n = t.new_node("NEWNOTE", "scenario 8: A's NEWNOTE adds exactly one node", "s8n")?;
        t.send(Who::A, &format!("MOVETOGROUP {n} {g}"))?;
        let doc = t.wait_agree("scenario 8: A and B agree on the shared nodes", "8")?;
        let got = session::group_of(&doc, &n);
        observe(
            "scenario 8: the document puts the note in the group",
            got.as_ref().is_some_and(|x| x.as_deref() == Some(g.as_str())),
            &format!("{n} is in {}, not {g}", show_group(&got)),
        );
        t.wait_field(Field::Group, "scenario 8: A, B and the document agree on the groups", "s8", &[&n, &g])?;
        Ok(String::new())
    })
}

/// "<id> in A's/B's group" for the ok-line, from where the document put it.
fn in_group(got: &Option<Option<String>>, a: &str) -> &'static str {
    if got.as_ref().is_some_and(|x| x.as_deref() == Some(a)) {
        "A's"
    } else {
        "B's"
    }
}

fn is_one_of(got: &Option<Option<String>>, a: &str, b: &str) -> bool {
    got.as_ref().is_some_and(|x| x.as_deref() == Some(a) || x.as_deref() == Some(b))
}

pub(super) fn s9(t: &mut Two) -> Step {
    t.scenario("s9-two-moves", 9, "both move one node into different groups", |t| {
        let ga = t.new_node("NEWGROUP", "scenario 9: A's first NEWGROUP adds exactly one node", "s9ga")?;
        let gb = t.new_node("NEWGROUP", "scenario 9: A's second NEWGROUP adds exactly one node", "s9gb")?;
        let n = t.new_node("NEWNOTE", "scenario 9: A's NEWNOTE adds exactly one node", "s9n")?;
        // B needs all three before it can offer the move.
        let (_, before) = t.wait_field(
            Field::Group,
            "scenario 9: A, B and the document agree on the groups before the moves",
            "s9-setup",
            &[&ga, &gb, &n],
        )?;
        // Back to back, so each move is likely made before that instance picks up the other's.
        t.send(Who::A, &format!("MOVETOGROUP {n} {ga}"))?;
        t.send(Who::B, &format!("MOVETOGROUP {n} {gb}"))?;
        t.show_local(Field::Group, "s9", "moves", &[&n])?;
        t.wait_agree("scenario 9: A and B agree on the shared nodes after both moves", "9")?;
        let (_, doc) = t.wait_field(
            Field::Group,
            "scenario 9: A, B and the document agree on the groups after both moves",
            "s9",
            &[&n, &ga, &gb],
        )?;
        let got = session::group_of(&doc, &n);
        observe(
            "scenario 9: the note ends in A's or B's group",
            is_one_of(&got, &ga, &gb),
            &format!("{n} ended in {}, neither {ga} (A's) nor {gb} (B's)", show_group(&got)),
        );
        let lost = session::lost_ids(&before, &doc);
        observe("scenario 9: no node is lost", lost.is_empty(), &format!("lost: {lost:?}"));
        let cycle = session::no_cycle(&doc);
        observe("scenario 9: no group cycle", cycle.is_ok(), cycle.as_ref().err().map_or("", String::as_str));
        Ok(format!(" ({n} in {} group)", in_group(&got, &ga)))
    })
}

pub(super) fn s10(t: &mut Two) -> Step {
    t.scenario("s10-cycle", 10, "A moves G into H while B moves H into G", |t| {
        let g = t.new_node("NEWGROUP", "scenario 10: A's first NEWGROUP adds exactly one node", "s10g")?;
        let h = t.new_node("NEWGROUP", "scenario 10: A's second NEWGROUP adds exactly one node", "s10h")?;
        let (_, before) = t.wait_field(
            Field::Group,
            "scenario 10: A, B and the document agree on the groups before the moves",
            "s10-setup",
            &[&g, &h],
        )?;
        t.send(Who::A, &format!("MOVETOGROUP {g} {h}"))?;
        t.send(Who::B, &format!("MOVETOGROUP {h} {g}"))?;
        t.show_local(Field::Group, "s10", "moves", &[&g, &h])?;
        t.wait_agree("scenario 10: A and B agree on the shared nodes after both moves", "10")?;
        let (_, doc) = t.wait_field(
            Field::Group,
            "scenario 10: A, B and the document agree on the groups after both moves",
            "s10",
            &[&g, &h],
        )?;
        let lost = session::lost_ids(&before, &doc);
        observe("scenario 10: no node is lost", lost.is_empty(), &format!("lost: {lost:?}"));
        let cycle = session::no_cycle(&doc);
        observe("scenario 10: no group cycle", cycle.is_ok(), cycle.as_ref().err().map_or("", String::as_str));
        let (gg, hg) = (show_group(&session::group_of(&doc, &g)), show_group(&session::group_of(&doc, &h)));
        Ok(format!(" (G in {gg}, H in {hg})"))
    })
}
```

  Implementer notes:
  - Task 3's import is `use serde_json::json;`. Task 4 makes it `use serde_json::{json, Value};` when `shown` needs `Value`.
  - `wait_field`'s first value is A's state.
  - The `before` of s9 and s10 is the setup's document, as the script's `cp "$out/s9-setup-doc.json" "$out/a9-before.json"`.
  - The script's s8 ids are `"$n8" "$g8"`; s9 ends with `"$n9" "$g9a" "$g9b"`. The orders are kept.

- [ ] **Green run.**
  - Build with `cargo build -j 8 -p manifold -p manifold-engine-webview2 -p gate`.
  - Launch `cmd //c start "" //b //belownormal bash .superpowers/two/run.sh t3-run1 two`.
  - Expect:
    - 27 `PASS:` lines (labels 1 to 27 of the table) and no `FAIL`;
    - the five `scenario N: ok` lines;
    - TIMING lines `setup s1-import s2-compaction s8-move-into-group s9-two-moves s10-cycle`;
    - `two-instance test: ok` and exit 0.
  - Check that no `manifold.exe` with the logged pids is left: `Get-Process -Id <pids> -EA 0` is empty.
- [ ] **Sabotage runs**, one at a time. For each:
  1. make the one-line edit;
  2. `cargo build -j 8 -p gate`;
  3. run `bash .superpowers/two/run.sh t3-sab-FN two`, launched as above;
  4. check the expectation;
  5. revert with `git checkout -- <file>` (only that file, nothing else uncommitted in it);
  6. check `git diff --exit-code crates/gate/src/<file>`;
  7. check that no pids are left.

  Each expectation is: exactly the named `FAIL:` line, the stop behaviour shown, exit 1, and the tails printed.
  - **F1, agreement** (`session.rs` `same_nodes`): change `if keys(na) == keys(nb)` to `if keys(na) == keys(nb) && na.is_empty()`.
    - `FAIL: scenario 1: A and B agree on the shared nodes` after about 90 s.
    - 0 PASS; no `scenario 1: ok`; TIMING `setup`, `s1-import` only.
  - **F2, kind count** (`groups.rs` s2): change `notes >= 201` to `notes >= 2010`.
    - `FAIL: scenario 2: the document has at least 201 notes`.
    - Scenario 2 runs to its end (no later check in it), there is no `scenario 2: ok`, and no `=== scenario 8`. 5 PASS.
  - **F3, new node** (`session.rs` `new_id`): change `[one] =>` to `[_, one] =>`.
    - `FAIL: scenario 8: A's NEWGROUP adds exactly one node`, stopping at once. 6 PASS.
  - **F4, group of** (`groups.rs` s8): change `Some(g.as_str())` to `Some("x")` in the observe.
    - `FAIL: scenario 8: the document puts the note in the group`, then `PASS: scenario 8: A, B and the document agree on the groups` (the scenario finishes). No `scenario 8: ok`, no `=== scenario 9`.
  - **F5, cycle and lost** (`session.rs`): `no_cycle` returns `Err("sabotage".into())` first thing, and `lost_ids` returns `vec!["sabotage".into()]` first thing.
    - In s9, both `FAIL: scenario 9: no node is lost` and `FAIL: scenario 9: no group cycle`, after `PASS: scenario 9: the note ends in A's or B's group`. Stops after s9.
- [ ] **Proof.** Run `cargo fmt --all`, the fmt check, the clippy line and both `cargo test` lines. Record the green run's wall time and per-section TIMING, CPU and Defender in the task report, along with each sabotage's log path and verdict.
- [ ] **Commit:** `gate two: driver and scenarios 1, 2, 8, 9, 10 (Marks and remove_dir_with_retries moved to check.rs and instance.rs)`.

### Task 4: scenarios 11 and 12

**Files:** modify `crates/gate/src/two/groups.rs` and `crates/gate/src/two/mod.rs` (only the two `run` lines).

- [ ] **Add `s11` and `s12`** to `groups.rs`, and change its import to `use serde_json::{json, Value};`.

```rust
/// The JSON text of an EXPORTSTATE value, as the script's `shown` printed it ("<absent>" if missing).
fn shown(v: &Option<Value>) -> String {
    v.as_ref().map_or_else(|| "<absent>".to_string(), Value::to_string)
}

pub(super) fn s11(t: &mut Two) -> Step {
    t.scenario("s11-rename", 11, "a group rename reaches the other instance", |t| {
        let g = t.new_node("NEWGROUP", "scenario 11: A's NEWGROUP adds exactly one node", "s11g")?;
        // B needs the group before it can rename it.
        t.wait_field(Field::Name, "scenario 11: A, B and the document agree on the new group's name", "s11-setup", &[&g])?;
        t.send(Who::A, &format!("RENAMEGROUP {g} Renamed by A"))?;
        t.wait_agree("scenario 11: A and B agree on the shared nodes after A's rename", "11")?;
        let (a, _) =
            t.wait_field(Field::Name, "scenario 11: A, B and the document agree on the name after A's rename", "s11", &[&g])?;
        let name = session::field_of(&a, Field::Name, &g);
        observe(
            "scenario 11: the group has A's name",
            name == Some(json!("Renamed by A")),
            &format!("{g} is named {}, not A's rename", shown(&name)),
        );
        // Back to back, so each rename is likely made before that instance picks up the other's.
        t.send(Who::A, &format!("RENAMEGROUP {g} Both A"))?;
        t.send(Who::B, &format!("RENAMEGROUP {g} Both B"))?;
        t.show_local(Field::Name, "s11c", "edits", &[&g])?;
        t.wait_agree("scenario 11: A and B agree on the shared nodes after both renames", "11c")?;
        let (a, _) = t.wait_field(
            Field::Name,
            "scenario 11: A, B and the document agree on the name after both renames",
            "s11c",
            &[&g],
        )?;
        let got = session::field_of(&a, Field::Name, &g);
        observe(
            "scenario 11: the group ends with A's or B's name",
            got == Some(json!("Both A")) || got == Some(json!("Both B")),
            &format!("{g} ended named {}, neither A's nor B's rename", shown(&got)),
        );
        Ok(format!(" ({g} named {})", shown(&got)))
    })
}

pub(super) fn s12(t: &mut Two) -> Step {
    t.scenario("s12-note-text", 12, "a note's text and a move reach the other instance", |t| {
        let n = t.new_node("NEWNOTE", "scenario 12: A's NEWNOTE adds exactly one node", "s12n")?;
        t.wait_field(Field::Text, "scenario 12: A, B and the document agree on the new note's text", "s12-setup", &[&n])?;
        t.send(Who::A, &format!("SETNOTETEXT {n} Typed by A"))?;
        t.wait_agree("scenario 12: A and B agree on the shared nodes after A's edit", "12")?;
        let (a, _) =
            t.wait_field(Field::Text, "scenario 12: A, B and the document agree on the text after A's edit", "s12", &[&n])?;
        let text = session::field_of(&a, Field::Text, &n);
        observe(
            "scenario 12: the note holds A's text",
            text == Some(json!("Typed by A")),
            &format!("{n} holds {}, not A's text", shown(&text)),
        );
        t.send(Who::A, &format!("SETNOTETEXT {n} Both typed A"))?;
        t.send(Who::B, &format!("SETNOTETEXT {n} Both typed B"))?;
        t.show_local(Field::Text, "s12c", "edits", &[&n])?;
        t.wait_agree("scenario 12: A and B agree on the shared nodes after both edits", "12c")?;
        let (a, _) =
            t.wait_field(Field::Text, "scenario 12: A, B and the document agree on the text after both edits", "s12c", &[&n])?;
        let got = session::field_of(&a, Field::Text, &n);
        observe(
            "scenario 12: the note ends with A's or B's text",
            got == Some(json!("Both typed A")) || got == Some(json!("Both typed B")),
            &format!("{n} ended holding {}, neither A's nor B's text", shown(&got)),
        );
        // The mirror and the document must stay agreed, not just reach agreement once: a later save on either side
        // (B's here, A's in wait_field's EXPORTSESSION) mustn't bring a losing text back (`commit_note_buffers` commits
        // any box that differs from the mirror).
        t.session(Who::B, "s12c2-b-save")?;
        let (a, _) = t.wait_field(
            Field::Text,
            "scenario 12: A, B and the document still agree on the text after B saves",
            "s12c2",
            &[&n],
        )?;
        let again = session::field_of(&a, Field::Text, &n);
        observe(
            "scenario 12: B's save leaves the winning text in place",
            again == got,
            &format!("{n} went from {} to {} with no edit", shown(&got), shown(&again)),
        );
        t.send(Who::A, &format!("MOVENODE {n} 321 -123"))?;
        t.wait_agree("scenario 12: A and B agree on the shared nodes after A's move", "12m")?;
        let (a, _) =
            t.wait_field(Field::Pos, "scenario 12: A, B and the document agree on the pos after A's move", "s12m", &[&n])?;
        let pos = session::field_of(&a, Field::Pos, &n);
        // Exact, as the script's string compare with '[321.0, -123.0]': f32 through serde writes 321.0, and an integer
        // 321 would fail both.
        observe(
            "scenario 12: the note is where A dropped it",
            pos == Some(json!([321.0, -123.0])),
            &format!("{n} is at {}, not where A dropped it", shown(&pos)),
        );
        Ok(format!(" ({n} holds {})", shown(&got)))
    })
}
```

  In `mod.rs`, `run` ends `groups::s10(t)?; groups::s11(t)?; groups::s12(t)`.

- [ ] **Green run** as in Task 3 (`run.sh t4-run1 two`). Expect 48 PASS (labels 1 to 48), 7 ok-lines, and the TIMING names through `s12-note-text`.
- [ ] **Sabotage**, with the same procedure as Task 3:
  - **F6, field agreement** (`session.rs` `field_agree`): add `if field == Field::Name { return Err(vec!["sabotage".into()]); }` as the first statement.
    - `FAIL: scenario 11: A, B and the document agree on the new group's name` after about 90 s, stopping at once. 28 PASS.
  - **F7, field value** (`groups.rs` s11): change `json!("Renamed by A")` in the first observe to `json!("Renamed by B")`.
    - `FAIL: scenario 11: the group has A's name`, then the rest of s11 passes (labels 33 to 35). No `scenario 11: ok`, and no `=== scenario 12`.
- [ ] **Proof:** fmt, clippy and tests as in Task 3; wall time and TIMING in the report.
- [ ] **Commit:** `gate two: scenarios 11 and 12`.

### Task 5: scenarios 3, 4 and 5 (the nonce site)

**Files:** create `crates/gate/src/two/web.rs`; modify `crates/gate/src/two/mod.rs` (`mod web;`, three `run` lines, and the `dump_urls` helper).

- [ ] **Add to `mod.rs`** the script's on-failure dump for s4: both instances' tab urls, plus the site's log.

```rust
    /// Scenario 4's failure dump: each instance's tab urls and the site's requests.
    fn dump_urls(&mut self, tag: &str) -> Step {
        for who in [Who::A, Who::B] {
            let doc = self.session(who, &format!("{}{tag}-urls", who.tag()))?;
            for n in session::nodes(&doc) {
                if let Some(url) = n.get("url").and_then(Value::as_str) {
                    info(&format!("{} {} {url}", who.name(), n.get("id").and_then(Value::as_str).unwrap_or("?")));
                }
            }
        }
        info(&self.counters()?.report());
        Ok(())
    }
```

- [ ] **`two/web.rs`.**

```rust
//! Scenarios 3 to 5: tabs on the nonce site (nonce.rs) and a local JSON file. A and B are up.

use std::fs;
use std::thread::sleep;
use std::time::{Duration, Instant};

use super::{observe, require, wait_until, Step, Two, Who, PAUSE, POSITIVE};
use crate::instance::file_url;
use crate::nonce::Site;
use crate::session;
use crate::state::JSON;

/// The script's 600 rounds of 0.2 s for A's and B's loads.
const FOLLOW: Duration = Duration::from_secs(150);

pub(super) fn s3(t: &mut Two) -> Step {
    t.scenario("s3-nonce-site", 3, "a url that changes on every load", |t| {
        t.site = Some(Site::start()?);
        let (c, port) = (t.counters()?, t.port()?);
        t.send(Who::A, "NEWTAB")?;
        t.send(Who::A, "WAITTABS")?;
        // The new tab is focused but shown in Graph view headless.
        t.send(Who::A, "TOGGLEVIEW")?;
        // Let the new tab's own first load settle, or its address-bar update overwrites the typed url before ENTER.
        t.send(Who::A, "WAITIDLE")?;
        t.navigate(Who::A, &format!("http://127.0.0.1:{port}/r"))?;
        // B's copy of A's new tab arrives asleep (spec "Lazy tabs"): once A's url has reached it, open it there, so B
        // loads it and follows from then on (scenario 4 too). Its landing is a host-started load, which is what keeps
        // B's own redirect local below. The script greps the raw export text; so does this.
        let needle = format!("127.0.0.1:{port}");
        let deadline = Instant::now() + POSITIVE;
        let reached = loop {
            t.tick(Who::B)?;
            let p = t.request(Who::B, "EXPORTSESSION", "b3")?;
            super::read_json(&p)?;
            if fs::read_to_string(&p)?.contains(&needle) {
                break true;
            }
            if Instant::now() >= deadline {
                break false;
            }
            sleep(PAUSE);
        };
        require("scenario 3: A's url reaches B's document", reached, "")?;
        t.send(Who::B, "QUICKSWITCHOPEN")?;
        t.send(Who::B, &format!("QUICKSWITCHTEXT 127.0.0.1:{port}"))?;
        t.send(Who::B, "QUICKSWITCHENTER")?;
        // Back to Graph view, where B was: C restores B's personal state, and scenario 7 toggles C into Tab view.
        t.send(Who::B, "TOGGLEVIEW")?;
        // A's load (a redirect, then the page), then B's (the same, as its copy follows): at least 4 requests.
        let followed = wait_until(FOLLOW, || c.hits() >= 4);
        require(
            "scenario 3: B follows A's navigation (at least 4 requests)",
            followed,
            &format!("only {} requests; B never followed A's navigation", c.hits()),
        )?;
        // Negative window (plan "Negative windows"): past HOST_LOAD_GRACE (5 s), then a window as long again; a bounce
        // is a round trip every second or two, so the count would keep climbing.
        sleep(Duration::from_secs(6));
        let settled = c.hits();
        sleep(Duration::from_secs(10));
        let now = c.hits();
        observe(
            "scenario 3: the instances stop navigating each other (no new request in 10 s)",
            now == settled,
            &format!("the instances keep navigating each other ({settled} -> {now} requests in 10 s)"),
        );
        Ok(format!(" ({settled} requests)"))
    })
}

pub(super) fn s4(t: &mut Two) -> Step {
    t.scenario("s4-click-vs-script", 4, "a click is shared, a script's navigation isn't", |t| {
        let (c, port) = (t.counters()?, t.port()?);
        let base = format!("http://127.0.0.1:{port}");
        let before = c.r_repeats();
        let link_before = c.served("/link");
        t.navigate(Who::A, &format!("{base}/link"))?;
        // The script's wait_served: one more request for the exact path, then A's tab finishes loading it.
        let seen = wait_until(POSITIVE, || c.served("/link") > link_before);
        require("scenario 4: the server sees A's /link load", seen, "the server never saw a request for /link")?;
        t.send(Who::A, "WAITIDLE")?;
        // The page is one window-sized link, so any click below the toolbar hits it. A lands on a served-once /r?n=
        // and shares it; B asks for it again.
        t.send(Who::A, "CLICK 400 300")?;
        let clicked = wait_until(POSITIVE, || c.r_repeats() > before);
        observe("scenario 4: B follows A's link click", clicked, "B never followed A's link click");
        t.navigate(Who::A, &format!("{base}/js"))?;
        // A's page and then B's (it follows the typed url) each navigate by script to their own /dest; had A's been
        // shared, B would ask for A's too.
        let both = wait_until(POSITIVE, || c.dests() >= 2);
        require(
            "scenario 4: A's and B's /js pages each navigate by script",
            both,
            &format!("only {} script navigations ran; expected A's and B's", c.dests()),
        )?;
        // Negative window: the page's timer fires 500 ms after load; 5 s lets a wrongly shared /dest reach B.
        sleep(Duration::from_secs(5));
        let repeats = c.dest_repeats();
        if !observe("scenario 4: B does not follow A's script navigation (5 s window)", repeats == 0, "") {
            t.dump_urls("4")?;
        }
        // A single-page app: a click's pushState is shared, a timer's isn't. Only B's load of a shared url asks the
        // server for /spa/c or /spa/t.
        let spa_before = c.served("/spa");
        t.navigate(Who::A, &format!("{base}/spa"))?;
        let seen = wait_until(POSITIVE, || c.served("/spa") > spa_before);
        require("scenario 4: the server sees A's /spa load", seen, "the server never saw a request for /spa")?;
        t.send(Who::A, "WAITIDLE")?;
        t.send(Who::A, "CLICK 400 300")?;
        let pushed = wait_until(POSITIVE, || c.spa_clicks() >= 1);
        require("scenario 4: B follows A's pushState from a click", pushed, "B never followed A's pushState from a click")?;
        // Negative window: the timer's push comes 1 s after the click; 5 s lets a wrongly shared one reach B.
        sleep(Duration::from_secs(5));
        let timers = c.spa_timers();
        observe(
            "scenario 4: B does not follow A's pushState from a timer (5 s window)",
            timers == 0,
            &format!("{timers} timer pushes were loaded"),
        );
        Ok(String::new())
    })
}

pub(super) fn s5(t: &mut Two) -> Step {
    t.scenario("s5-json-convert", 5, "a shared tab converting to a node", |t| {
        // A navigates the shared tab to a local JSON file (no network); B follows. Both engines load it and report
        // content, but the document must end up with exactly one more Json node, not one per instance.
        let file = t.path("c5.json");
        fs::write(&file, "{\"converted\": true}\n")?;
        let url = file_url(&file);
        let before = t.session(Who::A, "a5-before")?;
        let json_before = session::count_kind(&before, JSON);
        t.navigate(Who::A, &url)?;
        // No verdict of its own (as the script's loop): the count check after the window decides.
        let deadline = Instant::now() + POSITIVE;
        loop {
            t.tick(Who::A)?;
            let a5 = t.session(Who::A, "a5")?;
            if session::count_kind(&a5, JSON) > json_before || Instant::now() >= deadline {
                break;
            }
            sleep(PAUSE);
        }
        // Negative window: time for B to follow the url, load it and convert too, if it's going to.
        sleep(Duration::from_secs(8));
        let doc = t.wait_agree("scenario 5: A and B agree on the shared nodes after the conversion", "5")?;
        let json_after = session::count_kind(&doc, JSON);
        observe(
            "scenario 5: the document gains exactly one Json node",
            json_after == json_before + 1,
            &format!("Json nodes went {json_before} -> {json_after}, want exactly one more"),
        );
        Ok(String::new())
    })
}
```

  No visibility change is needed: a child module sees its parent's private items (`read_json`, `PAUSE`, `POSITIVE`, `Two`'s fields). Add `mod web;` to `mod.rs`. In `run`, `groups::s12(t)` gains a `?`, followed by `web::s3(t)?; web::s4(t)?; web::s5(t)`.

- [ ] **Green run** (`run.sh t5-run1 two`). Expect 60 PASS (labels 1 to 60), 10 ok-lines, and TIMING through `s5-json-convert`. Record the s3 `(N requests)` value.
- [ ] **Sabotage:**
  - **F8, positive counter** (`web.rs` s3): change `c.hits() >= 4` to `c.hits() >= 4000`.
    - `FAIL: scenario 3: B follows A's navigation (at least 4 requests)` after 150 s, stopping at once. The site's request log is printed in the tail. 49 PASS.
  - **F9, negative window** (`web.rs` s4): change `repeats == 0` to `repeats == 1`.
    - `FAIL: scenario 4: B does not follow A's script navigation (5 s window)`, then the url dump (`A`/`B` url lines and the site log, all indented), then labels 56 to 58 pass. No `scenario 4: ok`, no `=== scenario 5`.
- [ ] **Proof:** fmt, clippy and tests; wall time, TIMING, CPU and Defender in the report.
- [ ] **Commit:** `gate two: scenarios 3, 4, 5 on the in-process nonce site`.

### Task 6: scenarios 6 and 7 (kill, restart)

**Files:** create `crates/gate/src/two/restart.rs`; modify `crates/gate/src/two/mod.rs` (`mod restart;`, two `run` lines).

- [ ] **`two/restart.rs`.**

```rust
//! Scenarios 6 and 7: A is killed, B quits, C and then D open the same profile alone.
//!
//! A kill is TerminateProcess, not power loss: the store's on-disk file is never mid-write at that point (new_node's
//! last EXPORTSESSION saves first, immediately before the kill), so this exercises "a replica vanishes", not crash
//! recovery of a torn write.

use std::time::Duration;

use super::{observe, require, wait_until, Step, Two, Who, POSITIVE};
use crate::session;

/// One attempt at typing the url in scenario 7 (the script's 50 rounds of 0.2 s).
const ATTEMPT: Duration = Duration::from_secs(10);

pub(super) fn s6(t: &mut Two) -> Step {
    t.scenario("s6-kill", 6, "kill A", |t| {
        let n = t.new_node("NEWNOTE", "scenario 6: A's NEWNOTE adds exactly one node", "s6n")?;
        println!("A's new note id: {n}");
        t.kill(Who::A);
        t.quit(Who::B)?;
        t.launch(Who::C)?;
        let doc = t.session(Who::C, "c")?;
        observe(
            "scenario 6: a fresh instance on the profile has A's note",
            session::has_id(&doc, &n),
            &format!("{n:?} is not in C's document"),
        );
        Ok(String::new())
    })
}

pub(super) fn s7(t: &mut Two) -> Step {
    t.scenario("s7-restart", 7, "a restart reopens a tab's live url", |t| {
        // C is the only instance, so its personal state is the one on disk after it quits (two instances would each
        // overwrite the other's on save).
        let (c, port) = (t.counters()?, t.port()?);
        let dests = c.dests();
        let js_first = c.served("/js");
        t.send(Who::C, "NEWTAB")?;
        t.send(Who::C, "WAITTABS")?;
        t.send(Who::C, "TOGGLEVIEW")?;
        // Typed text is dropped if the view changes before the bar takes focus, and a new tab's first load can
        // overwrite it before ENTER; under load either loses the request, so type again once if the server never
        // sees /js.
        let mut requested = false;
        for _ in 0..2 {
            t.send(Who::C, "WAITIDLE")?;
            t.navigate(Who::C, &format!("http://127.0.0.1:{port}/js"))?;
            requested = wait_until(ATTEMPT, || c.served("/js") > js_first);
            if requested {
                break;
            }
        }
        require(
            "scenario 7: C requests the typed /js url",
            requested,
            &format!("the address bar text was lost\n{}", c.report()),
        )?;
        let navigated = wait_until(POSITIVE, || c.dests() > dests);
        require("scenario 7: C's /js page navigates to /dest", navigated, &c.report())?;
        // Let C see its tab settle on /dest before quitting saves it: the 2 s pass in the app, and the counters are
        // read right after sending it, as the script does.
        t.send(Who::C, "SLEEP 2")?;
        let js_before = c.served("/js");
        let dest_repeats = c.dest_repeats();
        t.quit(Who::C)?;
        t.launch(Who::D)?;
        t.send(Who::D, "WAITTABS")?;
        // D asks again for the /dest?n= C's script chose (a dest repeat).
        let reopened = wait_until(POSITIVE, || c.dest_repeats() > dest_repeats);
        require("scenario 7: D reopens C's /dest url", reopened, &c.report())?;
        let js_now = c.served("/js");
        observe(
            "scenario 7: D does not load the typed /js again",
            js_now == js_before,
            &format!("/js requests went {js_before} -> {js_now}"),
        );
        t.quit(Who::D)?;
        Ok(String::new())
    })
}
```

  Add `mod restart;` to `mod.rs`. `run` now ends `web::s5(t)?; restart::s6(t)?; restart::s7(t)`, which is Task 3's full listing.

- [ ] **Green run** (`run.sh t6-run1 two`). Expect:
  - `pass=66 fail=0` (by `grep -c`);
  - 12 ok-lines;
  - 13 TIMING names: `setup s1-import s2-compaction s8-move-into-group s9-two-moves s10-cycle s11-rename s12-note-text s3-nonce-site s4-click-vs-script s5-json-convert s6-kill s7-restart`;
  - `two-instance test: ok`;
  - no pids left.

  Then compare the sorted `PASS:` lines with the table. Build the label file from this plan, then diff:

```bash
awk -F' [|] ' '/^\| [0-9]+ \| scenario / { print $2 }' docs/superpowers/plans/2026-10-10-rust-gate-two.md | sort >.superpowers/two/labels.txt
wc -l <.superpowers/two/labels.txt   # 66
grep '^PASS: ' .superpowers/two/t6-run1.log | sed 's/^PASS: //' | sort | diff - .superpowers/two/labels.txt
```

  The diff must be empty. A mismatch is fixed in the code, or in the table with a stated reason, never by editing only one side silently.
- [ ] **Sabotage:**
  - **F10, has id** (`restart.rs` s6): change `session::has_id(&doc, &n)` to `session::has_id(&doc, &format!("{n}x"))`.
    - `FAIL: scenario 6: a fresh instance on the profile has A's note`. No `scenario 6: ok`, no `=== scenario 7`. Exit 1, and C's log tail is printed.
- [ ] **Proof:** fmt, clippy and tests; the full-run wall time and the 13 TIMING lines in the report.
- [ ] **Commit:** `gate two: scenarios 6 and 7; the port is complete (gate.sh still runs the script until Task 8)`.

### Task 7: proof runs (no code)

- [ ] **Preconditions** as in Task 1: no orphans, no cargo running, CPU and Defender recorded. The build is current.
- [ ] **Write `.superpowers/two/ab.sh`.** It runs six times in a fixed alternation: shell, rust, shell, rust, shell, rust. Each run goes through gate.sh's own `stage` shape, so the CPU join is the same. The CPU sampler runs throughout, and a calibration of forks runs at the start and end, as in `measure.sh`.

```bash
#!/usr/bin/env bash
# Task 7 A/B for docs/superpowers/plans/2026-10-10-rust-gate-two.md. Launch detached:
#   cmd //c start "" //b //belownormal bash .superpowers/two/ab.sh
cd "$(dirname "$0")/../.." || exit 1
d=.superpowers/two/ab
mkdir -p "$d"
rm -f "$d"/* 2>/dev/null
powershell -NoProfile -File "$(cygpath -aw scripts/cpu-sampler.ps1)" -Out "$(cygpath -aw "$d/cpu.log")" \
    -Stop "$(cygpath -aw "$d/cpu-stop")" </dev/null >/dev/null 2>&1 &
cal() {
    local t0 t1 x
    t0=${EPOCHREALTIME/[.,]/}
    for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do x=$(cat /dev/null); done
    t1=${EPOCHREALTIME/[.,]/}
    echo "fork_ms_$1 $(((t1 - t0) / 20000))${x:-}" >>"$d/ab.out"
}
one() { # NAME CMD...
    local name=$1 t0 rc
    shift
    t0=${EPOCHREALTIME/[.,]/}
    "$@" >"$d/$name.log" 2>&1
    rc=$?
    echo "$name exit=$rc secs=$(((${EPOCHREALTIME/[.,]/} - t0) / 1000000)) pass=$(grep -c PASS "$d/$name.log") fail=$(grep -c FAIL "$d/$name.log")" >>"$d/ab.out"
    bash scripts/gate-timing.sh collect "$name" "$d/$name.log" "$d/cpu.log" >>"$d/timing.out"
}
: >"$d/ab.out"
cal before
for i in 1 2 3; do
    one "shell-$i" bash scripts/two-instance-test.sh
    one "rust-$i" target/debug/gate two
done
cal after
touch "$d/cpu-stop"
echo done >"$d/ab.done"
```

  The sampler writes after a sample interval, so `collect` for the last run may lack its final samples. That is fine for a mean.

- [ ] **Launch and wait.** Launch `cmd //c start "" //b //belownormal bash .superpowers/two/ab.sh`. Wait for `ab.done`, with a deadline of 25 min (expected about 3 x 135 + 3 x 100 s, about 12 min).
- [ ] **Checks.**
  - All six exit 0.
  - Each rust run has `pass=66 fail=0`, and the three rust runs are consecutive greens. The shell runs show `pass=0 fail=0`, because the script prints no PASS lines.
  - The sorted `PASS:` lines of the three rust runs are identical to each other and to `labels.txt`.
  - Each rust log has the 13 TIMING names.
  - Both `fork_ms` values are at most 200. Otherwise report it, as in Task 1.
  - No stray pids.
- [ ] **Race winners.** From each log's ok-lines, record s9 (`in A's/B's group`), s10 (`G in .., H in ..`), s11 (`named ..`) and s12 (`holds ..`), for shell and rust.
- [ ] **Table.** For each of the 13 sections, the median shell seconds, the median rust seconds, the delta, and the mean CPU (from `timing.out`). Then the wall-time medians, and Defender state at the start and end.
  - Write the table into this plan's Measurements section.
  - Compare the saving with Task 1's `S_lo`.
  - If the rust median is not at least `S_lo - 5` s faster in total, investigate before Task 8 (which section did not gain, and why), and report.
- [ ] **Optional: ENGINE=cef.** If `target/debug/cef-runtime/manifold-engine-cef.exe` exists and is fresh (`manifold_exe` refuses a stale one), run one `ENGINE=cef target/debug/gate two` and one `ENGINE=cef bash scripts/two-instance-test.sh`, and report both. A red CEF run is reported, not fixed here. The gate does not run CEF.
- [ ] **Commit** the Measurements section: `plan: gate two A/B measurements`.

### Task 8: wire gate.sh, delete the replaced files, fix every reference, run the full gate

**Files:**
- modify: `scripts/gate.sh`, `scripts/gate-tier.sh`, `scripts/gate-tools-test.sh`, `README.md`, `CHANGELOG.md`, `CLAUDE.md`, `crates/gate/Cargo.toml`, `crates/gate/src/main.rs`, `crates/gate/src/check.rs`, `crates/gate/src/smoke.rs` (comment), `docs/superpowers/plans/2026-10-10-rust-gate-runner.md`;
- delete: `scripts/two-instance-test.sh`, `scripts/compare_sessions.py`, `scripts/session_asserts.py`, `scripts/nonce_server.py`, `scripts/timing.sh`.

- [ ] **`scripts/gate.sh`:**
  - line 63 becomes `stage "two-$i" "$out.two-$i.log" target/debug/gate two`;
  - lines 5 and 8 say `gate two x3` and `gate two x1` in place of `two-instance-test x3/x1`;
  - update the minutes on lines 7, 10 and 14 from Task 7's numbers and the smoke and link timings in the last full gate log (state the date and CPU).
- [ ] **`scripts/gate-timing.sh`** names none of the deleted files (checked 2026-10-10). Its only change is Task 1's fix; nothing to do here.
- [ ] **`scripts/gate-tier.sh`** lines 32 and 33 become:

```bash
    scripts/gate*.sh | crates/gate/* | scripts/fresh-exe.sh | scripts/winpid.sh | scripts/lib.sh | scripts/cpu-sampler.ps1)
```

  `scripts/fresh-exe.sh` stays: its users are other probes (launch-forms, drive.sh and others) and `gate-tools-test.sh`. Before deleting, grep for each name to confirm.

- [ ] **`scripts/gate-tools-test.sh`:**
  - delete the `timing_mark` block (lines 12 to 18 today: the comment, the `shellcheck source=timing.sh` line, the `. timing.sh` line, and its `check`);
  - the header line 3 becomes `# Checks gate-timing.sh, gate-tier.sh and fresh-exe.sh in about a second, without the app or git.`
- [ ] **Delete** the five files with `git rm`.
- [ ] **Grep for every deleted name** in the repo outside `target`, `.superpowers`, and the historical `docs/superpowers/specs` and `docs/superpowers/plans` (precedent, commit 59d5811).
  - Use the Grep tool with pattern `two-instance-test|compare_sessions|session_asserts|nonce_server|(^|[^-])timing\.sh|timing_mark`. The `[^-]` keeps `gate-timing.sh` out.
  - Fix each hit as below. What may remain:
    - CHANGELOG line 86 (history);
    - the "was scripts/..." provenance lines in the module docs of `two/mod.rs`, `nonce.rs`, `session.rs` and `main.rs`, following suggest.rs's "what `scripts/probe_server.py` answers" precedent.
  - List every remaining hit in the report.
- [ ] **README:**
  - **Line 397**, rewrite the paragraph as `gate two` (`crates/gate/src/two/`):
    - the scenarios with the script's numbers 1 to 12, in run order (1, 2, 8 to 12, 3 to 7), keeping the content of today's text;
    - the nonce site, now `crates/gate/src/nonce.rs` in process;
    - the asserts, now `crates/gate/src/session.rs`;
    - the stop rule in one sentence;
    - `ENGINE=cef` affects scenarios 3, 4, 5 and 7;
    - build with `cargo build -j 8 -p manifold -p manifold-engine-webview2 -p gate`.
  - **Line 399:** `two-instance-test.sh` becomes `gate two`, and the timings are updated from Task 7 and the final gate.
  - **Line 425:** the last sentence becomes `` `gate smoke`, `gate link` and `gate two` are Rust (`crates/gate`) and do not use it. ``
- [ ] **CHANGELOG.md:** in the bullet at line 81 (`scripts/gate.sh` is the pre-merge gate ...), name `gate two` with `gate smoke` and `gate link` as the Rust stages. Leave line 86, which is history.
- [ ] **CLAUDE.md "Gate"** (only if Decision 5 is yes): the build is `cargo build -j 8 -p manifold -p manifold-engine-webview2 -p gate`. That line predates `gate`, so `gate.sh`'s stages would run a stale `gate.exe` without it. If the decision is no, leave the file alone and say so in the report.
- [ ] **`crates/gate/Cargo.toml` description:** `The headless gate flows (link probe, smoke test, two-instance test) as one Rust program: ...`
- [ ] **`main.rs` module doc:** the stages are listed as `hello`, `link`, `smoke` and `two` (the two-instance test). The paths of the deleted scripts move into "was" wording.
- [ ] **`check.rs` lines 1, 66 and 74, and `smoke.rs:291`:** reword `timing.sh` and `timing_mark` references to "the `TIMING` format `scripts/gate-timing.sh` reads". `Marks`, now in check.rs, carries the doc from the Task 3 listing.
- [ ] **`docs/superpowers/plans/2026-10-10-rust-gate-runner.md` Task 5:** append `Decided 2026-10-10: ported; see 2026-10-10-rust-gate-two.md.`
- [ ] **Proof.**
  - Run `cargo fmt --all`, the fmt check, the clippy line, both `cargo test` lines, and `bash scripts/gate-tools-test.sh`.
  - Before committing, `bash scripts/gate-tier.sh HEAD` ends with `TIER full`.
  - Rerun the grep and see only the expected hits.
  - `git status --short` shows only the files listed (the 5 deletions included).
  - Build with `cargo build -j 8 -p manifold -p manifold-engine-webview2 -p gate`.
  - Run a full gate: `cmd //c start "" //b //belownormal bash scripts/gate.sh .superpowers/two/gate-full.log full`. gate.sh `cd`s to the repo first, so the relative path lands in `.superpowers/two/`.
  - Wait for `GATE done` (deadline 30 min). Then confirm:
    - every stage `exit=0`;
    - `two-1` to `two-3` each `pass=66 fail=0`;
    - smoke `pass=54`, link `pass=30`;
    - the TIMING lines now carry CPU numbers (Task 1's fix);
    - CPU and Defender reported.
- [ ] **Commit:** `gate: gate.sh runs gate two; delete two-instance-test.sh, compare_sessions.py, session_asserts.py, nonce_server.py, timing.sh; docs`.

## Decisions for the user

1. **New `PASS:` lines (66).** The two-N stage lines in the gate log gain `pass=66 fail=0`; today they carry no counts, because the script prints neither word on success. Recommend yes: the earlier plan's Task 5 left this open, and it makes a sabotage or a regression show by label.
2. **The P/O stop rule.** The alternative is "stop at the first failure", which is the script's `set -e` exactly. Recommend P/O: it adds the rest of a failing scenario's observations, and never runs a later scenario.
3. **Delete `scripts/timing.sh` and its gate-tools-test check.** Its last real user goes. Recommend yes, since keeping it would be a shim.
4. **The gate-timing.sh `ENVIRON` fix in Task 1,** as its own commit. Recommend yes: every gate log's CPU columns are blank today.
5. **Add `-p gate` to CLAUDE.md's Gate build line.** Recommend yes: gate.sh runs `target/debug/gate`, and a stale one would test old code.
6. **Positive-wait deadlines:** 90 s, 150 s for s3, and 10 s per s7 attempt, replacing round counts. Recommend yes: they only bound a failing run.
7. **One tick file per instance,** the only behaviour deviation. Recommend yes.
8. **Leave the historical specs and plans mentioning the deleted scripts.** Recommend yes, following commit 59d5811.
9. **The Task 1 calibration rule:** stop and ask if one MSYS fork costs over 200 ms. Recommend yes. This plan's own session measured 2.2 to 2.8 s per fork, which would inflate the measured saving several times over.

## Stress test

Five rounds were run on the draft. Each was a full adversarial pass looking for contradictions, missing steps, stale claims, unverified assumptions and Sonnet misreadings. Each round's finds were fixed in place.

**Round 1: the draft against the script and the python, line by line.**
- The setup waits of s9, s10, s11 and s12 were labelled "B shows ...". They check A, B and the document, so the labels were renamed (15, 23, 29, 37).
- `/spa/c` and `/spa/t` serve the SPA page and count. My notes had them as plain counters.
- A 100 ms counter poll would have started each negative window up to 100 ms closer to its event than the script does. All polls now use the script's 200 ms.
- Details printed under PASS lines were noise. Details now print only on FAIL, and the script's informational output goes through `info`.
- Checked that the 66 table labels and the code's label strings are the same set (extracted both, `comm -3` empty).

**Round 2: every command as a Sonnet implementer would type it.**
- `cmd //c start ... cmd /c "..."` passes `/c` through MSYS path conversion. Replaced by the `run.sh` wrapper with a `.done` file.
- The full gate's LOG used a Windows path. It is now repo-relative.
- `gate-tier.sh HEAD~1` was run before the commit exists. Now `HEAD`.
- Task 8's grep `timing\.sh` also matched `gate-timing.sh`. Now `(^|[^-])timing\.sh`.
- The expected leftover hits were wrong: the new module docs say "was scripts/...", per the suggest.rs precedent.
- Task 1's `git stash` proof was fragile. Now `git show HEAD:` into scratch.
- Task 1 called `collect` "after the fix" while the fix step came later. It works without the fix on relative paths, and now says so.
- The CLAUDE.md edit is now conditional on Decision 5.
- A body-constants unit test would have compared the constants with copies of themselves once python is deleted. Replaced by a throwaway curl comparison against the live python server.

**Round 3: compile and run the listings.**
- Extracted every code block from this file:
  - `bash -n` passes on the five complete bash blocks (the other two are deliberate fragments: a `case` line and an excerpt of an awk call);
  - `sum.awk` on a synthetic trace splits sections and does not time a pipeline's `tr`.
- Assembled `http.rs`, the new `suggest.rs`, `nonce.rs`, `session.rs`, `Marks`, `remove_dir_with_retries` and all four `two/` files into a scratch copy of `crates/gate` (own target dir, `-j 4`, deleted afterwards). Ran `cargo clippy --all-targets -- -D warnings` on it. It caught:
  - the test-only `Read`/`Write`/`TcpStream` imports left in `suggest.rs`;
  - an orphaned doc comment;
  - `items_after_test_module` for the moved items.

  All three are now in the Task 2 text. Clippy is then clean.
- By reading, I also replaced:
  - `filter().last()` (clippy `double_ended_iterator_last`) with `rev().find()`;
  - `dedup` on an unsorted chain with a `BTreeSet`;
  - an `unreachable!` in `new_node` with simpler code;
  - a placeholder `Value` import with per-task imports;
  - an unused helper in `measure.sh`.

  I also made Task 3's `run` body explicit.
- Quick tests of the listed code passed with and without `serde_json/preserve_order` (33 tests including the crate's existing ones):
  - `canonical` key order, `1` vs `1.0`, `-0.0`;
  - `same_nodes` with reordered and duplicated nodes;
  - `field_agree` at 1e-3 vs 2e-3, null against a missing doc key, absent in B;
  - `no_cycle` on G and H in each other, and on a dangling group;
  - `group_of("")`, `new_id`, and `[321, -123] != [321.0, -123.0]`;
  - the nonce issue, serve and repeat flow, blank `n=`, the `/dest` `None` repeat, POST 501 unlogged, 404 logged, exact `served`;
  - a socket round trip.

**Round 4: whole-plan consistency.**
- Decision 1 said the counts go "from pass=0". The gate log prints no counts for the script today, so it was reworded.
- `gate-timing.sh` was in the caller's reference list, but it names no deleted file. Task 8 now says so explicitly instead of silently skipping it.
- Re-derived the PASS count at each task's green run (27, 48, 60, 66) and at each sabotage (F1 0, F2 5, F3 6, F6 28, F8 49) from the table.
- Re-checked that each P/O in the table matches `require`/`observe` in the code.

**Round 5:** a full pass found nothing material.

**Still unverified (by design, until the tasks run):**
- the real saving (Task 1 measures it, Task 7 proves it);
- whether the faster rounds expose a race the script hid (R1);
- that `gate two` passes against the app at all. The listings compile, but no app run was made while writing the plan.
