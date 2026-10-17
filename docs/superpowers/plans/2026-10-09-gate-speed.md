---
status: done
---

# Gate speed Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `scripts/gate.sh` faster by first seeing where its time goes, then spending effort only where the numbers point:
- A: every scenario of `two-instance-test.sh` and every step group of `headless-smoke-test.sh` and `link-probe.sh` prints one `TIMING <name> <secs> <start>` line; `gate.sh` collects them, with the CPU load around each, into a table in its log and prints the slowest ten
- B: a third gate tier, `ui` (unit tests + `link-probe.sh`), and `scripts/gate-tier.sh`, which prints the tier a change needs from the paths it touches (advice only)
- C: a measured spike of `opt-level` 0, 1 and 3 for dependencies in the dev profile, with the decision rule written down before the first number exists

**Architecture:**
- A: one sourced bash helper (`timing_mark NAME` closes the open step and opens the next), one awk joiner (`gate-timing.sh collect`/`top`), and one PowerShell CPU sampler that runs beside the gate and is read back by timestamp. No test is slowed by it: the steps print a line, the sampler is a separate process, and the per-stage `powershell Get-Counter` calls of today (about 4 s each) go away.
- B: `gate.sh` gains `ui`; `gate-tier.sh` classifies each changed path by `case` globs plus one grep-able signal (does a "UI" module import the store, sync, mirror, session, process manager or net modules).
- C: dependencies only (`[profile.dev.package."*"]` already exists with `debug = false`; the spike adds `opt-level` through `cargo --config`, so no `Cargo.toml` changes unless the numbers say so).

**Tech Stack:** bash 5 (`EPOCHREALTIME`), awk, PowerShell 5.1, cargo (nightly-2026-09-23, `-j 8`).

**Source:** the user's go-ahead to fold A, B and C into one plan (2026-10-09). Every claim below was re-read against `576e583`. Line numbers are quoted for orientation only: find code by the quoted text. Paths are relative to the repo root.

## Global Constraints

**Branch and commits**
- Work on `main` (CLAUDE.md "Branches"), no branch. One commit per task with the exact message in its last step (Task 6 makes none). This plan file itself is left uncommitted for the user. The pre-commit hook only runs on staged `.rs` or `rustfmt.toml`, so script and doc commits skip it; never use `--no-verify`.
- Each implementer's commit message ends with that implementer's own `Co-Authored-By:` line. The controller's is `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`.
- No release, no tag, no merge question (this is on `main`).

**Scripts**
- Change files with the Edit tool. LF line endings (`.gitattributes`). New scripts are `#!/usr/bin/env bash`, carry a `Usage:` header, and use `# shellcheck source=` directives like their neighbors. `shellcheck` is not installed on this machine: the check is `bash -n FILE` plus the runnable checks each task names.
- Add no fixed sleeps. A wait is on an observable condition with a deadline (the sampler's stop wait in Task 3 is one).
- `timing_mark` names are one word and must not contain `PASS` or `FAIL` (`gate.sh` counts those words in each log).

**Python**
- Never run `python` in any way that can read stdin: no `python -`, no heredoc python, no python call tacked on as a no-op. Write the script to a file and run `python -P f.py < /dev/null`. None of these tasks needs new python. Every implementer prompt repeats this rule. Subagents never dispatch subagents.

**Builds and the machine**
- `-j 8` on every cargo command. C: is ~97% full (32 GB free on 2026-10-09): `df -h /c` before any build, and before Task 6 see its disk rule.
- A gate run, a baseline, or a spike timing needs a quiet machine: before each, `(Get-Counter '\Processor(_Total)\% Processor Time' -SampleInterval 2 -MaxSamples 2).CounterSamples[-1].CookedValue` under 20, `(Get-MpComputerStatus).RealTimeProtectionEnabled` recorded, and `Get-Process python, manifold, wsl -ErrorAction SilentlyContinue` shows no leftovers. Report CPU and Defender with every number.
- Runs are detached at BelowNormal (`cmd //c start "" //b //belownormal bash ...`), never Normal (LONG_RUNS.md). The gate is CPU-bound, so the GPU-job exception does not apply.
- Never kill another session's process. `C:\w\rt` is another session's worktree: do not touch it.

**What the gate shares, and must not (the WebView2 hazard)**
- A WebView2 host keeps its profile directory beside the host exe: `<exe>.WebView2/EBWebView/WV2Profile_manifold_<slot>` (`webview2_profile_dir` and its test in `process_manager.rs`). So two runs that use the same `target/debug/manifold-engine-webview2.exe` share and lock the same user-data folders.
- The probe scripts take their exe from `<repo>/target/debug` (`exe="$repo_root/target/debug/manifold.exe"`). Therefore: one gate per `target/debug` at a time; never run the dev app from a `target/debug` a gate is using (cargo also cannot overwrite a running exe); every extra worktree builds its own `target/` (a worktree's `target` is git-ignored) and gates from it.
- Copying the exes into another worktree is not the way: `require_fresh_exe` reads `manifold.d`, which lists absolute source paths of the repo it was built in, so a copy would be judged against the wrong tree's files and could pass while stale. Build in place.
- The shared-target-dir hazard: a `CARGO_TARGET_DIR` shared by several worktrees makes every session rebuild whatever the last session's sources or profile settings invalidated. That bites Task 7 (a changed dev profile re-fingerprints every dependency, once per target dir) and is why Task 6 uses a worktree's own `target/`.

## Decisions resolved by reading the code

1. **The gate's time is unmeasured per scenario; today's only per-stage figure is wall time.** `gate.sh` logs a start and end per stage and nothing inside. So no optimization is chosen in advance, with two exceptions that are visible in the source and sized below as estimates, not measurements.
2. **The CPU readings in `gate.sh` cost time themselves.** `cpu()` is `powershell ... Get-Counter -SampleInterval 2 -MaxSamples 2`, called at the start and end and before every stage (eight times for `full`), each at least 2 s of sampling plus a PowerShell start. The plan replaces the per-stage calls with a sampler and keeps the start and end ones. Estimated saving: 6 calls x 3-4 s = 20 s of ~7 min (an estimate: Task 5 measures the log's `took=` and the old figures).
3. **Fixed waits are in the two-instance script.** Scenario 3 sleeps 6 s then 10 s ("past HOST_LOAD_GRACE", then a window as long again), scenario 4 sleeps 5 s twice, scenario 5 sleeps 8 s, scenario 7 `SLEEP 2`: 36 s a run by the source, ~110 s of a `full` gate (an estimate: the polls around them add more). They prove an absence (nothing keeps navigating), so they cannot be polled. Whether they matter is for the Task 5 table to say; they are parked until then (see Parked).
4. **Why a sampler, not a CPU reading in each step.** The request is "CPU load at start of each step". A reading inside the step costs ~4 s and loads the machine being measured. The sampler (`cpu-sampler.ps1`, one process, one sample about every 4 s, tested to write `<unix seconds> <cpu %>` lines) is joined by timestamp afterwards: `cpu_start` is the last sample at or before the step began, `cpu_mean` the mean of those during it. Standalone runs of the three scripts print TIMING lines without CPU (nothing joins them); only the gate adds it.
5. **`TIMING` goes to stdout, and the gate's `last:` filters it.** `gate.sh` logs `tail -1` of each stage's output as the verdict; the EXIT trap prints the closing TIMING line after the final "ok", so `last:` must skip `TIMING ` lines (Task 3).
6. **`gate-tier.sh` never changes what `gate.sh` runs.** It prints a tier and a reason per path. The default base is `git merge-base HEAD main`, which is `HEAD` itself when working on `main` with everything committed (CLAUDE.md says to work on `main`): then the diff is empty, and the script says so and asks for a BASE (the commit the last gate ran at). Tracked changes and untracked files are both counted.
7. **The `ui` allowlist is derived, not guessed.** A grep for imports of `crate::(store|session_sync|mirror|session|process_manager|net)` over `crates/manifold/src/*.rs` (2026-10-09) shows the big UI files write the store (`card.rs` renames groups through `app.mirror.set`, `graph_ui.rs` imports `store::model` and calls `insert_placeholder_node`, `node_menu.rs` and `popups.rs` have 42 and 13 hits of a wider pattern that also counts `tabs`, `jar` and `ipc_protocol`). Files with none that are drawing, input or view state: `bookmarks`, `click_count`, `failure_toasts`, `graph_layout`, `history`, `image_view`, `input`, `json_level_ui`, `json_view`, `pdf_render`, `rgba`, `shortcuts`, `theme`, `view_modes`, `zoom_chip`. That is the initial list, and `gate-tier.sh` re-checks it at run time: a listed file that now imports a core module is demoted to `light`. So `ui` is narrow on purpose. Most UI changes (cards, menus, popups, `main.rs`) are `light`, as today. Whether to widen it is an open question for the user.
8. **`full` is by path, with the user's list as the core**: sync, store, mirror, session, engine, `process_manager`, `net`. Mapped to files: `session_sync.rs` and `session_sync/`, `store/`, `mirror.rs`, `session.rs`, `process_manager.rs`, `net.rs`, every `crates/manifold-engine-*` and `crates/ipc-protocol`, `Cargo.toml`/`Cargo.lock`/`.cargo`, and the gate's own scripts (the two-instance test and its python helpers, `gate*.sh`, `timing.sh`, `fresh-exe.sh`, `winpid.sh`, `lib.sh`). Added by me, from "engine" and "net": `local_state.rs` (personal state, saved), `engine_install.rs`, `redirect_fetch.rs`, `favicon_fetch.rs`, `jar.rs`. Anything under `crates/manifold/src` on no list is `light`. Over-gating is cheap, under-gating is not; the lists are open for the user to edit.
9. **`dev` already has `[profile.dev.package."*"] debug = false`.** And `"*"` means every package except workspace members: `manifold`, `ipc-protocol` and the engine crates stay at `opt-level 0` in every variant, which is what an incremental dev loop wants. Build scripts and proc macros use `[profile.dev.build-override]` (default `opt-level 0`) and are not touched. Release is untouched, so the wave 2 benches (`cargo test --release`), their pins (counts and goldens) and `json-pan-profile.sh` (release exe) do not go stale (Task 7 lists what does).
10. **The gate's build command may not build the WebView2 host.** `crates/manifold/Cargo.toml` has no dependency on `manifold-engine-webview2`, and the README builds the four crates explicitly, so `cargo build -j 8 -p manifold` (CLAUDE.md "Gate") leaves `target/debug/manifold-engine-webview2.exe` as it was. `fresh-exe.sh` checks only `manifold.d` (and the CEF copy). A gate after an engine or `ipc-protocol` change could therefore run an old host. Not verified by running; recorded as a finding and an open question, not changed here. The spike builds both crates, because the smoke test needs the host.

## Parked, with why

- **Parallel two-instance repeats.** Tried as parallel runs on 2026-10-07: 53 min, all failed on timing. Running the three repeats at once also needs one exe copy per job (the WebView2 hazard above) and triples the load. Revisit only with per-job worktrees and a core budget.
- **A release or optimized gate profile.** Superseded by C (optimize only the dependencies, keep debug assertions and overflow checks on the crates under test). A release build also costs 13 min cold (the log in `target/` says so) and a second target tree on a 97%-full disk.
- **Gate priority Normal.** Violates LONG_RUNS.md; CPU-bound jobs stay BelowNormal.
- **Shrinking the fixed waits in scenarios 3, 4, 5 (Decision 3).** Parked until Task 5's table shows what they cost; each guards a bounce loop, so a shorter window weakens the check. Decide with numbers.
- **Skipping the repeated two-instance runs in `full` by path.** `full` exists to catch flaky ordering in sync; not touched.

## Per-task summary

| # | What | Kind | Model | Depends on | Proof |
|---|---|---|---|---|---|
| 1 | `timing.sh`, `gate-timing.sh`, `gate-tier.sh`, `gate-tools-test.sh` | mechanical (code given) | haiku | none | `bash scripts/gate-tools-test.sh` all PASS |
| 2 | Add `timing_mark` calls to the three scripts | mechanical (anchored edits) | haiku | 1 | `bash -n`; mark counts; a real `link-probe.sh` run prints its TIMING lines |
| 3 | `cpu-sampler.ps1` and the new `gate.sh` (collect, table, `ui` mode) | judgment | sonnet | 1, 2 | `bash -n`; sampler smoke test; `gate.sh LOG ui` end to end |
| 4 | Docs for the three tiers and the helpers (numbers left as marked blanks) | mechanical | haiku | 1, 3 | read-back; blank list below |
| 5 | Baseline: measure `full`, `light`, `ui` once each; read the table; fill the blanks | controller + judgment | controller | 1-4 | gate logs; CPU and Defender reported |
| 6 | Spike C: opt-level 0/1/3, 3 alternating rounds | judgment | sonnet runs, controller decides | 5 | the results table; the rule applied as written |
| 7 | Apply the decision and re-baseline (conditional on 6) | mechanical | haiku | 6 | before/after table; `gate-tier.sh` says full; stale-number list done |
| 8 | Acceptance: full gate with the new gate | controller | controller | 1-7 | `GATE done`, all `exit=0`, table present |

Mechanical tasks (1, 2, 4, 7) have complete code or exact edits below; a reviewer checks every step landed and nothing else changed. Judgment tasks give the design and the checks and leave small details to the implementer, who reports anything that differs.

---

### Task 1: The helper scripts and their check

**Model:** haiku. **Kind:** mechanical. **Proof:** `bash scripts/gate-tools-test.sh` prints 12 PASS lines and exits 0 (it ran green in a scratch copy while this plan was written).

**Files:**
- Create: `scripts/timing.sh`, `scripts/gate-timing.sh`, `scripts/gate-tier.sh`, `scripts/gate-tools-test.sh`

**Interfaces:**
- Produces: `timing_mark [NAME]` (sourced); `gate-timing.sh collect TAG RUNLOG [CPULOG]` and `top LOG [N]`; `gate-tier.sh [BASE]` and `--stdin`; the test.
- Consumes: `check` from `scripts/lib.sh`.

- [ ] **Step 1: Create `scripts/timing.sh`.**

```bash
# Sourced by the scripts whose steps the gate times. A step is the run of commands between two marks.
#   timing_mark NAME   ends the open step, if any, printing   TIMING <name> <seconds> <start epoch>   and opens NAME
#   timing_mark        ends the open step and opens none (call it at the end and first in the EXIT trap)
# NAME is one word and must not contain PASS or FAIL (gate.sh counts those words). Needs bash 5 (EPOCHREALTIME).
# Call it only from the top level of a script, never inside a $(...) or a pipeline: the open step is shell state.
_timing_name=""
_timing_t0=0
timing_mark() {
    local now=${EPOCHREALTIME/[.,]/} d
    if [ -n "$_timing_name" ]; then
        d=$((now - _timing_t0))
        printf 'TIMING %s %d.%03d %d\n' "$_timing_name" $((d / 1000000)) $((d % 1000000 / 1000)) $((_timing_t0 / 1000000))
    fi
    _timing_name="${1:-}"
    _timing_t0=$now
    return 0
}
```

- [ ] **Step 2: Create `scripts/gate-timing.sh`.**

```bash
#!/usr/bin/env bash
# Usage: scripts/gate-timing.sh collect TAG RUNLOG [CPULOG]   prints RUNLOG's TIMING lines as
#            TIMING TAG/name secs start cpu_start=% cpu_mean=%
#        scripts/gate-timing.sh top LOG [N]                  the N (default 10) slowest TIMING lines of LOG
# CPULOG is gate.sh's sampler file, lines "<unix seconds> <total cpu %>". cpu_start is the last sample taken at or
# before the step began, cpu_mean the mean of the samples taken during it; "-" where there is none.
case "${1:-}" in
collect)
    grep '^TIMING ' "$3" | awk -v tag="$2" -v cpulog="${4:-/dev/null}" '
        BEGIN { while ((getline l < cpulog) > 0) { split(l, f, " "); n++; ts[n] = f[1]; cp[n] = f[2] } close(cpulog) }
        {
            s = $4; e = s + $3; start = "-"; sum = 0; cnt = 0
            for (i = 1; i <= n; i++) {
                if (ts[i] <= s) start = cp[i]
                if (ts[i] >= s && ts[i] <= e) { sum += cp[i]; cnt++ }
            }
            printf "TIMING %s/%s %s %s cpu_start=%s cpu_mean=%s\n", tag, $2, $3, s, start, (cnt ? sprintf("%.1f", sum / cnt) : "-")
        }'
    ;;
top)
    grep '^TIMING ' "$2" | sort -g -r -k3,3 | head -n "${3:-10}" | awk '{ printf "%8ss  %-36s %s %s\n", $3, $2, $5, $6 }'
    ;;
*) echo "usage: gate-timing.sh collect TAG RUNLOG [CPULOG] | top LOG [N]" >&2; exit 2 ;;
esac
```

- [ ] **Step 3: Create `scripts/gate-tier.sh`.**

```bash
#!/usr/bin/env bash
# Usage: scripts/gate-tier.sh [BASE]     the tier to gate the changes since BASE (default: merge-base with main)
#        scripts/gate-tier.sh --stdin    classify the paths on stdin, one per line (what gate-tools-test.sh uses)
# Prints one line per changed path, strictest first, then `TIER ui|light|full`. It only advises: nothing here runs or
# changes the gate. The strictest tier of any path wins. Changed means tracked files that differ from BASE in the
# working tree, plus untracked files. On `main` with everything committed the default base is HEAD and nothing differs:
# pass the commit the last gate ran at. Edit the lists below when a module changes its role.
set -u
cd "$(dirname "$0")/.." || exit 1
src="${TIER_SRC:-crates/manifold/src}"
# Modules a UI-only change may touch: drawing, input and view state, none of which write the store or reach an engine.
# A file here that imports a core module is demoted to light (classify checks the file itself).
ui_files=" bookmarks click_count failure_toasts graph_layout history image_view input json_level_ui json_view pdf_render rgba shortcuts theme view_modes zoom_chip "
core_import='crate::(store|session_sync|mirror|session|process_manager|net)\b'

rank() { case $1 in ui) echo 1 ;; light) echo 2 ;; full) echo 3 ;; esac; }

# classify PATH: "<tier> <reason>"
classify() {
    local b
    case "$1" in
    Cargo.toml | Cargo.lock | .cargo/* | rust-toolchain* | crates/*/Cargo.toml | crates/ipc-protocol/* | crates/manifold-engine-*/*)
        echo "full dependency, protocol or engine crate" ;;
    "$src"/session_sync.rs | "$src"/session_sync/* | "$src"/store/* | "$src"/mirror.rs | "$src"/session.rs | "$src"/local_state.rs | \
        "$src"/process_manager.rs | "$src"/net.rs | "$src"/engine_install.rs | "$src"/redirect_fetch.rs | "$src"/favicon_fetch.rs | "$src"/jar.rs)
        echo "full sync, store, mirror, session, engine or network core" ;;
    scripts/two-instance-test.sh | scripts/compare_sessions.py | scripts/session_asserts.py | scripts/nonce_server.py | scripts/gate*.sh | \
        scripts/timing.sh | scripts/fresh-exe.sh | scripts/winpid.sh | scripts/lib.sh | scripts/cpu-sampler.ps1)
        echo "full the gate's own scripts and their helpers" ;;
    "$src"/*/*) echo "light a module directory on no list" ;;
    "$src"/*.rs)
        b=$(basename "$1" .rs)
        case "$ui_files" in
        *" $b "*)
            if [ -f "$1" ] && grep -qE "$core_import" "$1"; then echo "light $b is on the ui list but imports a core module"; else echo "ui $b: drawing or input, no core import"; fi
            return ;;
        esac
        echo "light $b is on neither the ui nor the full list" ;;
    scripts/* | .githooks/* | .github/*) echo "light script or tooling" ;;
    *.md | docs/* | .superpowers/* | .gitignore | .gitattributes | rustfmt.toml | LICENSE*) echo "ui documentation or config only (nothing to test)" ;;
    *) echo "light path on no list" ;;
    esac
}

paths() {
    if [ "${1:-}" = "--stdin" ]; then tr '\n' '\0'; return; fi
    { git diff --name-only -z "$base" --; git ls-files -z --others --exclude-standard; }
}

base=""
if [ "${1:-}" != "--stdin" ]; then base="${1:-$(git merge-base HEAD main)}"; fi
best=0 tier=ui n=0 lines=""
while IFS= read -r -d '' p; do
    read -r t reason <<<"$(classify "$p")"
    n=$((n + 1))
    lines+="$(rank "$t") $t $p ($reason)"$'\n'
    if [ "$(rank "$t")" -gt "$best" ]; then best=$(rank "$t"); tier=$t; fi
done < <(paths "$@")
if [ "$n" -eq 0 ]; then
    echo "no changed files since ${base:-stdin}; pass the commit the last gate ran at (git log) as BASE" >&2
    exit 3
fi
printf '%s' "$lines" | sort -r | cut -d' ' -f2-
echo "TIER $tier"
```

- [ ] **Step 4: Create `scripts/gate-tools-test.sh`.**

```bash
#!/usr/bin/env bash
# Usage: scripts/gate-tools-test.sh
# Checks timing.sh, gate-timing.sh and gate-tier.sh in about a second, without the app or git.
set -u
repo_root="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=lib.sh
. "$repo_root/scripts/lib.sh"
t="$(mktemp -d)"
trap 'rm -rf "$t"' EXIT
status=0

# timing_mark: one TIMING line per closed step, name then seconds with three decimals then a start epoch; a mark
# with no step open prints nothing.
# shellcheck source=timing.sh
. "$repo_root/scripts/timing.sh"
got="$({ timing_mark one; timing_mark two; timing_mark; timing_mark; } | sed -E 's/ [0-9]+\.[0-9]{3} [0-9]{10}$/ SECS EPOCH/')"
check "timing_mark prints one line per closed step" "TIMING one SECS EPOCH
TIMING two SECS EPOCH" "$got"

# collect: cpu_start is the last sample at or before the start, cpu_mean the mean of those inside the step.
printf 'TIMING setup 12.345 100\nTIMING s1 30.000 112\nnoise\nTIMING s2 3.5 142\n' >"$t/run.log"
printf '99 5.0\n101 10.0\n105 20.0\n130 40.0\n143 90.0\n150 80.0\n' >"$t/cpu.log"
check "collect joins each step with the cpu samples" "TIMING g/setup 12.345 100 cpu_start=5.0 cpu_mean=15.0
TIMING g/s1 30.000 112 cpu_start=20.0 cpu_mean=40.0
TIMING g/s2 3.5 142 cpu_start=40.0 cpu_mean=90.0" "$(bash "$repo_root/scripts/gate-timing.sh" collect g "$t/run.log" "$t/cpu.log")"
check "collect without a cpu log says -" "TIMING g/setup 12.345 100 cpu_start=- cpu_mean=-" "$(bash "$repo_root/scripts/gate-timing.sh" collect g "$t/run.log" | head -1)"
bash "$repo_root/scripts/gate-timing.sh" collect g "$t/run.log" "$t/cpu.log" >"$t/all.log"
check "top lists the slowest first" "g/s1 g/setup" "$(bash "$repo_root/scripts/gate-timing.sh" top "$t/all.log" 2 | awk '{printf "%s%s", sep, $2; sep=" "}')"

# gate-tier --stdin: the strictest path wins; a ui-listed file that imports a core module is demoted.
mkdir -p "$t/src"
echo 'use egui;' >"$t/src/theme.rs"
echo 'use crate::store::model::X;' >"$t/src/zoom_chip.rs"
s="$t/src"
tier() { TIER_SRC="$s" bash "$repo_root/scripts/gate-tier.sh" --stdin | tail -1; }
check "docs only is ui" "TIER ui" "$(printf '%s\n' README.md docs/x.md | tier)"
check "a ui-listed file with no core import is ui" "TIER ui" "$(printf '%s\n' "$s/theme.rs" | tier)"
check "a ui-listed file that imports store is light" "TIER light" "$(printf '%s\n' "$s/zoom_chip.rs" | tier)"
check "an unlisted source file is light" "TIER light" "$(printf '%s\n' "$s/main.rs" | tier)"
check "a probe script is light" "TIER light" "$(printf '%s\n' scripts/link-probe.sh | tier)"
check "the store is full" "TIER full" "$(printf '%s\n' "$s/store/journal.rs" | tier)"
check "the strictest path wins" "TIER full" "$(printf '%s\n' README.md "$s/main.rs" crates/ipc-protocol/src/lib.rs | tier)"
check "an empty list is refused" "3" "$(printf '' | TIER_SRC="$s" bash "$repo_root/scripts/gate-tier.sh" --stdin >/dev/null 2>&1; echo $?)"
exit $status
```

- [ ] **Step 5: Check.** `bash -n` on all four; `bash scripts/gate-tools-test.sh` (expect 12 PASS, exit 0). Make the `gate-tier.sh` test fail once on purpose to prove it can: change `"$src"/store/*` to `"$src"/storez/*`, see "the store is full" FAIL, revert.
- [ ] **Step 6: Dry run on the real tree (read-only).** `bash scripts/gate-tier.sh HEAD~5` prints a plausible list and a `TIER` line; `bash scripts/gate-tier.sh` on a clean `main` prints the "pass a BASE" message and exits 3.
- [ ] **Step 7: Commit:** `gate tools: timing_mark, TIMING collector, tier advisor, and their check`.

**Blast radius:** four new files, nothing sourced or called yet. `gate-tier.sh`'s lists are the only judgment: the implementer re-runs the Decision 7 grep (below) and reports any file whose result changed since 2026-10-09:

```bash
cd crates/manifold/src && for f in bookmarks click_count failure_toasts graph_layout history image_view input json_level_ui json_view pdf_render rgba shortcuts theme view_modes zoom_chip; do grep -cE 'crate::(store|session_sync|mirror|session|process_manager|net)\b' $f.rs | sed "s/^/$f /"; done
```

Every count must be 0 (2026-10-09). A nonzero file leaves the list.

---

### Task 2: Mark the steps in the three scripts

**Model:** haiku. **Kind:** mechanical. **Proof:** `bash -n` on each; `grep -c '^ *timing_mark'` equals 15, 13 and 8; a real `link-probe.sh` run (about 2 min, `cargo build -j 8 -p manifold` first) prints its TIMING lines and its verdict is unchanged.

**Files:**
- Modify: `scripts/two-instance-test.sh`, `scripts/headless-smoke-test.sh`, `scripts/link-probe.sh`

**Interfaces:** consumes `timing_mark` (Task 1). Produces TIMING lines on stdout, in file order, that sum to the script's wall time from `setup` on.

Every edit below is "insert this line above the quoted line", unless it says otherwise. Markers are top-level only. Add no other change. Each script also gets, directly under its existing `winpid.sh`/`fresh-exe.sh` source lines, and before the `require_fresh_exe` call:

```bash
# shellcheck source=timing.sh
. "$repo_root/scripts/timing.sh"
```

and `timing_mark setup` on the line after `require_fresh_exe "$exe" || exit 1`.

- [ ] **Step 1: `two-instance-test.sh`.**
  - Above `echo "=== scenario 1: import + note ==="`: `timing_mark s1-import`
  - Above `echo "=== scenario 2: compaction mid-run ==="`: `timing_mark s2-compaction`
  - Above `echo "=== scenario 8: ...`: `timing_mark s8-move-into-group`
  - Above `echo "=== scenario 9: ...`: `timing_mark s9-two-moves`
  - Above `echo "=== scenario 10: ...`: `timing_mark s10-cycle`
  - Above `echo "=== scenario 11: ...`: `timing_mark s11-rename`
  - Above `echo "=== scenario 12: ...`: `timing_mark s12-note-text`
  - Above `echo "=== scenario 3: ...`: `timing_mark s3-nonce-site`
  - Above `echo "=== scenario 4: ...`: `timing_mark s4-click-vs-script`
  - Above `echo "=== scenario 5: ...`: `timing_mark s5-json-convert`
  - Above `echo "=== scenario 6: kill A ==="`: `timing_mark s6-kill`
  - Above `echo "=== scenario 7: ...`: `timing_mark s7-restart`
  - Above `echo "two-instance test: ok"`: `timing_mark`
  - In `cleanup()`, on the line after `local status=$?`: `timing_mark`
  - Add one header comment line under `# Usage:`: `# Prints a TIMING line per scenario (scripts/timing.sh); gate.sh collects them.`
  - Note for the reader: the scenarios run in the order 1, 2, 8-12, 3-7, so the lines come out in that order, and scenario 6's step holds the start of instance C.
- [ ] **Step 2: `headless-smoke-test.sh`.** Source and `setup` as above (after `require_fresh_exe "$exe" || exit 1`). Then:
  - Above the comment block that starts `# Pin a known window size so the click coordinates below`: `timing_mark launch` (the step holds the root tab's spawn wait)
  - Above `# The app launches into Graph view; with only the root tab, the layout`: `timing_mark address-bar`
  - Above `# Group drill-in: a second Escape (no dropdown now) leaves to Graph view,`: `timing_mark group-drill`
  - Above `# Tab drops inside a group.`: `timing_mark group-drops`
  - Above `# Border colors, read off a screenshot`: `timing_mark borders`
  - Above `# Recently Closed keeps nesting`: `timing_mark reopen-nested`
  - Above `# Drag and drop: \`ADDFILE <path> X Y\``: `timing_mark file-drops`
  - Above `# Entering the dropped JSON file`: `timing_mark json-level`
  - Above `# The remaining \`ADDFILE\` kinds`: `timing_mark content-kinds`
  - Above `# Engine-side conversion, no network`: `timing_mark conversion`
  - Above the bare `echo` that precedes `if [ "$fail" -eq 0 ]; then`: `timing_mark`
  - As the first line of `cleanup()`: `timing_mark`
  - Add to the header comment: `# Prints a TIMING line per step group (scripts/timing.sh); gate.sh collects them.`
  - The `# Card helpers.` block (function definitions) sits between `address-bar` and `group-drill` and is counted in `address-bar`; that is accepted.
- [ ] **Step 3: `link-probe.sh`.** Source and `setup` as above (the step holds the profile seed and the `--import-json`). Then:
  - Above `start log1`: `timing_mark run1-link`
  - Above `start log2`: `timing_mark run2-restart-unlink`
  - Above `start log3`: `timing_mark run3-gestures`
  - Above `start log4`: `timing_mark run4-persists`
  - Above `# Prints "<link count> <first link's id> <its from> <its to>" for one EXPORTSTATE file.`: `timing_mark verdicts` (the many `py` checks)
  - Above the last line `exit $status`: `timing_mark`
  - As the first line of `cleanup()`: `timing_mark`
- [ ] **Step 4: Check.**
  - `bash -n` on the three.
  - `grep -c '^ *timing_mark' scripts/two-instance-test.sh scripts/headless-smoke-test.sh scripts/link-probe.sh` gives 15, 13, 8 (one per step, plus `setup`, plus the closing mark, plus the trap's).
  - `grep -n 'timing_mark' scripts/*.sh | grep -E 'PASS|FAIL'` prints nothing.
  - `cargo build -j 8 -p manifold`, then `bash scripts/link-probe.sh` detached at BelowNormal: expect 6 TIMING lines (`setup`, `run1-link`, `run2-restart-unlink`, `run3-gestures`, `run4-persists`, `verdicts`, and none else), every PASS as before, exit 0, and the six times add up to within 5 s of the script's wall time. Paste the TIMING lines into the report.
- [ ] **Step 5: Commit:** `gate: print a TIMING line per scenario and step group`.

**Blast radius:** `gate.sh` still prints `tail -1` of each log (fixed in Task 3); until then a gate's `last:` text shows a TIMING line. Run Task 3 straight after.

---

### Task 3: The sampler and the new `gate.sh`

**Model:** sonnet. **Kind:** judgment (process lifetime, quoting). **Proof:** `bash -n`; the sampler smoke test; `gate.sh LOG ui` end to end (a few minutes; first `cargo build -j 8 -p manifold`).

**Files:**
- Create: `scripts/cpu-sampler.ps1`
- Replace: `scripts/gate.sh`

**Interfaces:** consumes `timing.sh` lines in the stage logs, `gate-timing.sh`, `cpu-sampler.ps1`. Produces in LOG: per stage a `GATE <name> start`, a `GATE <name> exit=<rc> end <time> took=<s>s last: <line> [pass= fail=]`, a `TIMING gate/<name> <secs> <start>`, and the stage's own TIMING lines as `TIMING <name>/<step> ...`; then `GATE slowest steps` and ten lines; `LOG.cpu` holds `<unix seconds> <cpu %>`. Stage names: `unit` (ui only), `two-1`..`two-3`, `smoke`, `launch-forms`, `link`.

- [ ] **Step 1: Create `scripts/cpu-sampler.ps1`.**

```powershell
# Usage: powershell -NoProfile -File scripts/cpu-sampler.ps1 -Out FILE -Stop FILE [-MaxMinutes 90]
# Appends "<unix seconds> <total cpu %>" to Out about every 4 s until the Stop file exists (gate.sh creates it) or
# MaxMinutes pass, so it cannot outlive a gate that crashed.
param([string]$Out, [string]$Stop, [int]$MaxMinutes = 90)
$deadline = (Get-Date).AddMinutes($MaxMinutes)
while (-not (Test-Path -LiteralPath $Stop) -and (Get-Date) -lt $deadline) {
    $v = (Get-Counter '\Processor(_Total)\% Processor Time' -SampleInterval 2 -MaxSamples 2 -ErrorAction SilentlyContinue).CounterSamples[-1].CookedValue
    if ($null -ne $v) { '{0} {1}' -f [DateTimeOffset]::UtcNow.ToUnixTimeSeconds(), [math]::Round($v, 1) | Add-Content -LiteralPath $Out -Encoding ascii }
}
```

- [ ] **Step 2: Replace `scripts/gate.sh`** with the following. Compared with today's file: the header gains `ui` and the new outputs; the mode `case` gains `ui`; the per-stage `cpu()` calls are replaced by the sampler and `stage()`; `last:` skips TIMING lines; the stage names are `two-N`, `smoke`, `launch-forms`, `link`; the start and `done` lines keep `cpu()` and `defender()`. The `~FULL_MIN`, `DATE`, `LIGHT_MIN`, `UI_MIN` words are filled in Task 5 from measurements; until then leave them as written.

```bash
#!/usr/bin/env bash
# Usage: scripts/gate.sh LOG [full|light|ui]    (scripts/gate-tier.sh says which tier a change needs)
#
# The pre-merge gate: sequential, so load from other sessions is the only thing that stretches it.
#   full  (default): two-instance-test x3 + headless-smoke-test x1 + launch-forms-probe + link-probe. Use for any
#                    change to sync, the store, the mirror, session save/restore or the engines. ~FULL_MIN min on a
#                    quiet machine (measured DATE).
#   light:           two-instance-test x1 + headless-smoke-test x1 + launch-forms-probe + link-probe. Allowed when
#                    only UI code that writes the store or reaches an engine changed (gestures, menus), or scripts:
#                    ~LIGHT_MIN min quiet. The repeats in `full` exist to catch flaky ordering in sync, which
#                    UI-only changes cannot reach.
#   ui:              `cargo test` of the three CI crates + link-probe. For changes to drawing, input and view-state
#                    modules that import no store, sync or network module (the list is in gate-tier.sh), and docs:
#                    ~UI_MIN min quiet.
# Build first (`cargo build -j 8 -p manifold`): every probe stage runs the existing exe. Launch detached and low priority:
#   cmd //c start "" //b //belownormal bash scripts/gate.sh LOG [mode]
# Progress and verdicts go to LOG; each stage's own output to LOG.two-N.log, LOG.smoke.log, LOG.launch-forms.log,
# LOG.link.log and LOG.unit.log. After each stage LOG gets its TIMING lines (see scripts/gate-timing.sh) and at the end
# the slowest ten; LOG.cpu holds the machine's CPU load sampled every few seconds while it ran. Running the runs in
# parallel was tried (2026-10-07) and was worse: 53 min, all four runs failed on timing waits under load.
cd "$(dirname "$0")/.." || exit 1
out="$1"
mode="${2:-full}"
case "$mode" in full) runs=3 ;; light) runs=1 ;; ui) runs=0 ;; *) echo "mode must be full, light or ui" >&2; exit 2 ;; esac
[ -n "$out" ] || { echo "usage: scripts/gate.sh LOG [full|light|ui]" >&2; exit 2; }
cpu() { powershell -NoProfile -Command "[math]::Round((Get-Counter '\Processor(_Total)\% Processor Time' -SampleInterval 2 -MaxSamples 2).CounterSamples[-1].CookedValue,1)" 2>/dev/null | tr -d '\r'; }
defender() { powershell -NoProfile -Command "(Get-MpComputerStatus).RealTimeProtectionEnabled" 2>/dev/null | tr -d '\r'; }

# The CPU sampler runs until the stop file appears (the EXIT trap makes it) or its own 90-minute limit.
stop="$out.cpu-stop"
rm -f "$stop" "$out.cpu"
powershell -NoProfile -File "$(cygpath -aw scripts/cpu-sampler.ps1)" -Out "$(cygpath -aw "$out.cpu")" -Stop "$(cygpath -aw "$stop")" \
    </dev/null >/dev/null 2>&1 &
sampler=$!
end_sampler() {
    touch "$stop"
    for _ in $(seq 20); do kill -0 "$sampler" 2>/dev/null || break; sleep 0.5; done
    rm -f "$stop"
}
trap end_sampler EXIT

# stage NAME LOGFILE CMD...: runs CMD with its output in LOGFILE, then writes the stage's verdict line, its own TIMING
# line (gate/NAME) and the TIMING lines CMD printed, each with the CPU load around it.
stage() {
    local name=$1 log=$2 t0=$SECONDS s0 rc pf=""
    shift 2
    s0=$(date +%s)
    echo "GATE $name start $(date +%H:%M:%S)" >>"$out"
    "$@" >"$log" 2>&1
    rc=$?
    grep -qE 'PASS|FAIL' "$log" && pf=" pass=$(grep -c PASS "$log") fail=$(grep -c FAIL "$log")"
    echo "GATE $name exit=$rc end $(date +%H:%M:%S) took=$((SECONDS - t0))s last: $(grep -v '^TIMING ' "$log" | tail -1)$pf" >>"$out"
    printf 'TIMING gate/%s %d %d\n' "$name" $((SECONDS - t0)) "$s0" >>"$out"
    bash scripts/gate-timing.sh collect "$name" "$log" "$out.cpu" >>"$out"
}

echo "GATE start ($mode) $(date +%H:%M:%S) cpu=$(cpu)% defender_rt=$(defender)" >>"$out"
if [ "$mode" = ui ]; then
    stage unit "$out.unit.log" cargo test -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2
fi
for i in $(seq "$runs"); do
    stage "two-$i" "$out.two-$i.log" bash scripts/two-instance-test.sh
done
if [ "$mode" != ui ]; then
    stage smoke "$out.smoke.log" bash scripts/headless-smoke-test.sh
    stage launch-forms "$out.launch-forms.log" bash scripts/launch-forms-probe.sh
fi
stage link "$out.link.log" bash scripts/link-probe.sh
echo "GATE slowest steps (seconds, cpu at start, mean cpu):" >>"$out"
bash scripts/gate-timing.sh top "$out" 10 >>"$out"
echo "GATE done $(date +%H:%M:%S) cpu=$(cpu)% defender_rt=$(defender)" >>"$out"
```

- [ ] **Step 3: Sampler smoke test (no gate).** From a scratch directory run the sampler with a short loop in bash: start it in the background with `-Out` and `-Stop` paths from `cygpath -aw`, wait (poll up to 20 s, no fixed sleep) until the Out file has 2 lines, `touch` the stop file, poll up to 10 s for the process to exit. Expect lines like `1791573937 62.7`, the process gone, no `powershell` left (`Get-Process powershell` shows only your own shell's).
- [ ] **Step 4: End to end.** `cargo build -j 8 -p manifold`, then `cmd //c start "" //b //belownormal bash scripts/gate.sh target/gate-ui-try.log ui`, watching for the `GATE done` line with a Monitor until-loop (not the launcher). Expect: `GATE unit` and `GATE link` blocks, `exit=0` each, `TIMING gate/unit`, `TIMING gate/link`, the link probe's six `TIMING link/...` lines each with `cpu_start=` and `cpu_mean=` numbers, `GATE slowest steps` with up to ten lines, `GATE done`, no `target/gate-ui-try.log.cpu-stop` left, and `Get-Process powershell` shows no sampler. Paste the log. If `cargo test` fails or is slow for a reason of its own (it is a new stage), report it; do not change the stage's command.
- [ ] **Step 5: `last:` filter.** In that log the `GATE link ... last:` text must not start with `TIMING`.
- [ ] **Step 6: Commit:** `gate.sh: ui tier, per-stage timing table, CPU sampler replaces the per-stage readings`.

**Blast radius:** the old stage names (`GATE two-instance run 1`, `GATE smoke start`) are gone from new logs. Nothing in the repo parses them (grep of `GATE ` outside `docs/superpowers/plans` and `.superpowers` finds none); watchers in plans look for `GATE done`, which is kept last. `gate.sh` still exits 0 whatever the stages did, as before (the verdicts are in the log); that is left alone.

---

### Task 4: Describe the three tiers

**Model:** haiku. **Kind:** mechanical. **Proof:** read-back; `grep -n 'FULL_MIN\|LIGHT_MIN\|UI_MIN\|DATE' scripts/gate.sh README.md CLAUDE.md` lists exactly the blanks Task 5 fills.

**Files:**
- Modify: `CLAUDE.md` ("Gate" section), `README.md` (the `scripts/gate.sh` paragraph, the `launch-forms-probe.sh` paragraph's last sentence, and a new paragraph after the `scripts/lib.sh` one)

- [ ] **Step 1: `CLAUDE.md` "Gate".** Replace the section body with:

```
Before merging or tagging, run `scripts/gate-tier.sh [BASE]` (it prints the tier the changed paths need and why; it advises, nothing more), then `scripts/gate.sh LOG TIER` after `cargo build -j 8 -p manifold`, detached at BelowNormal; see README "scripts/gate.sh". Tiers: `full` (~FULL_MIN min quiet) for sync, store, mirror, session, engine, process manager, net, dependency and gate-script changes; `light` (~LIGHT_MIN min) for UI code that writes the store or reaches an engine, and other scripts; `ui` (~UI_MIN min: unit tests and the link probe) for the drawing, input and view-state modules listed in `gate-tier.sh`, and docs. Report CPU load and Defender state with the result.
```

(Keep it to that paragraph; the blanks are filled in Task 5.)

- [ ] **Step 2: `README.md`, the `scripts/gate.sh` paragraph.** Rewrite it to say: modes `full` (the default), `light` and `ui` and what each runs; the verdict lines per stage now include `took=`, a `TIMING gate/<stage>` line and the stage's own `TIMING <stage>/<step>` lines with `cpu_start` and `cpu_mean`, then `GATE slowest steps` (ten lines), and `GATE done` last; `LOG.cpu` is the sampler's file; `LOG.unit.log` for `ui`; `scripts/gate-tier.sh` and what it does and does not do; the durations as `~FULL_MIN`, `~LIGHT_MIN`, `~UI_MIN` (measured DATE, quiet machine, CPU and Defender noted); keep the existing sentences about the 2026-10-07 parallel experiment and the 11.5 min loaded figure. Keep the sentence that it records CPU load and Defender state at the start and end (the `cpu=` and `defender_rt=` fields of the start and done lines are unchanged).
- [ ] **Step 3: `README.md`, the `launch-forms-probe.sh` paragraph.** Its last sentence says both probes run in `gate.sh` "in `light` and `full`". Change to: `launch-forms-probe.sh` runs in `light` and `full`; `link-probe.sh` in all three tiers.
- [ ] **Step 4: `README.md`, after the `scripts/lib.sh` paragraph.** Add: `bash scripts/gate-tools-test.sh` checks `timing.sh`, `gate-timing.sh` and `gate-tier.sh` in about a second without the app; `timing_mark` is how a script reports a step's time (`two-instance-test.sh` per scenario, `headless-smoke-test.sh` per step group, `link-probe.sh` per run), and `TIMING <name> <secs> <start>` is the format.
- [ ] **Step 5: Commit:** `docs: gate tiers (full, light, ui), the tier advisor and the timing table`.

**Blast radius:** CLAUDE.md is the project instruction file every session reads, so its wording is the contract; the user reads Step 1 before it is committed.

---

### Task 5: Baseline, and what the table says

**Model:** controller (measuring on a quiet machine) with a sonnet read of the table. **Kind:** judgment. **Proof:** three gate logs, each with its `GATE start`/`done` CPU and Defender, and the blanks filled.

**Files:** `scripts/gate.sh` (header numbers), `CLAUDE.md`, `README.md` (the blanks). No code.

- [ ] **Step 1: Preconditions.** Tasks 1 to 4 committed. The Global Constraints quiet-machine checks; `df -h /c`; `cargo build -j 8 -p manifold -p manifold-engine-webview2` (both, per Decision 10) once, then confirm the gate's own command alone would not have rebuilt the host: `touch crates/manifold-engine-webview2/src/main.rs` (mtime only, nothing to revert), run `cargo build -j 8 -p manifold`, and see whether `target/debug/manifold-engine-webview2.exe`'s mtime moved. Record yes or no. This settles Decision 10.
- [ ] **Step 2: Run `full`, then `light`, then `ui`**, one at a time, each detached at BelowNormal to `target/gate-base-<tier>.log`, each only after the previous one's `GATE done` and a fresh quiet check. Three runs in one sitting; that is about 15 minutes of a quiet machine, estimated from the figures in the README (7 + 4 + 2) and not yet measured. If the machine is not quiet at the start of a run, wait on the condition with a deadline of 10 minutes; if it never is, run anyway and say so in the log's report line.
- [ ] **Step 3: Read the tables.** For `full`: the ten slowest steps, how much of each stage the scenarios in it explain (they sum to within the stage's `took=`), what fraction of the `full` wall is the three two-instance runs, and the `cpu_mean` of the slowest steps (low CPU in a slow step means waiting, not computing, so C cannot help it). Estimate, from the source, the share of the fixed waits of Decision 3 in scenarios 3, 4, 5 and 7. Write four lines of findings (not a report file) into the Outcome section of this plan.
- [ ] **Step 4: Fill the blanks.** `FULL_MIN`, `LIGHT_MIN`, `UI_MIN` from the three `took` totals (minutes, one decimal, with the date as `DATE`), in `gate.sh`'s header, `CLAUDE.md` and `README.md`. State "one run each, quiet machine, CPU <n>%, Defender <state>".
- [ ] **Step 5: Commit:** `docs: measured gate durations for the three tiers`.
- [ ] **Step 6: Tell the user the expectation for Task 6**: if the slowest steps are low-CPU waits, C's 25% smoke threshold is unlikely to be met and the spike may be skipped on the user's word; if they are CPU-bound (`cpu_mean` high) it is worth running. Wait for the go-ahead (Task 6 takes hours).

**Blast radius:** the three gate logs stay under `target/` (git-ignored, `*.log`). Leftovers check afterwards.

---

### Task 6: Spike C, dependency opt-level in the dev profile

**Model:** sonnet runs the harness; the controller applies the rule. **Kind:** judgment. **Proof:** the results table below filled from the logs, and the decision rule applied as written.

**Files:** none in the repo. A throwaway worktree `C:\w\gs` and a harness in the scratchpad.

**The question:** does `opt-level` 1 or 3 on dependencies make the smoke test and link probe (which run the debug exe, whose frame code is egui and wgpu) fast enough to pay for slower dependency builds?

**Decision rule (fixed before any number exists).** Compare each variant's medians (3 rounds) with variant 0's:
- D1: the smoke test's time (the sum of its TIMING lines, `setup` through the last step; its wall clock is recorded beside it) drops by at least 25%.
- D2: the cold dependency build takes at most 2.0 times variant 0's. It is the empty-`target/` build of `-p manifold -p manifold-engine-webview2` ("cold") minus the time to rebuild just those workspace crates afterwards ("ws-rebuild", which is the same at every opt-level because the workspace crates stay at 0). The plain cold time is reported too.
- D3: the incremental rebuild of the `manifold` crate (the second one after the edit) takes at most 1.10 times variant 0's.
- D4: all three runs of the variant pass the same checks as variant 0 (the same `pass=` count and `fail=0`) in both the smoke test and the link probe. A run with a FAIL is rerun in a fresh slot once; a second failing run disqualifies the variant. (The README records `headless-smoke-test.sh` as flaky on graph interactions; a flake is not a verdict, a repeat is.)
- D5: its `target/` after the cold build is at most 2.0 times variant 0's.
- Adopt the lowest `opt-level` that meets D1 to D5, unless a higher one meets all five and its smoke drop is at least 10 percentage points larger than the lower one's. If none meets all five, adopt nothing and record the negative result (Task 7 Step 5).
- A number within 10% of its threshold (a smoke drop of 22.5% to 27.5%, D2 between 1.8x and 2.2x) is rerun once, as an extra round of that variant pair on a quiet machine, before deciding.
- Reported but not decisive: the link probe's drop, `cargo test -p manifold` build and run times, each smoke step group's change. If D1 fails by a hair and these look good, the controller tells the user and does not adopt on them.

**Disk and time.** The existing `target/` is 28 GB, but 18 GB of it is `target/debug/build` from CEF and Servo, which `-p manifold -p manifold-engine-webview2` does not build. The expected size of one such target is therefore a guess of 3 to 6 GB and is not measured; the first cold build measures it. Only one variant's `target/` exists at a time (it is wiped at the start of each slot), so the peak is one target plus the source tree. Rule: `df -h /c` before the first slot, and stop and report if free space is under 12 GB at any slot start or if the first cold target exceeds 8 GB. Expected duration: unknown until the probe slot (round 1, variant 0) is timed; the whole spike is then about 9 times that slot, times 1.3 for the optimized variants. If that is over 3 hours, stop and ask the user before continuing.

- [ ] **Step 1: Worktree.** `git worktree add --detach C:/w/gs <HEAD sha>` (the commit with Tasks 1 to 5, so the smoke test and link probe print TIMING). Short path on purpose (memory: long worktree paths break the CEF build; this build has no CEF but keeps the path short). The worktree has its own git-ignored `target/`; do not set `CARGO_TARGET_DIR` into the main repo's. Run `cargo fetch --locked` in it once (untimed) and use `--offline` on every timed cargo command so network noise is out.
- [ ] **Step 2: Prove each variant takes effect, once, untimed.** In `C:/w/gs`, with an empty `target/`, run `cargo build -j 8 --offline -v -p ipc-protocol --config 'profile.dev.package."*".opt-level=1'` (it builds only ipc-protocol's few dependencies) and in the verbose output find the rustc line of one dependency (for example `serde`): it must have `-C opt-level=1` and `-C debuginfo` absent or 0; the `ipc-protocol` line must have `-C opt-level=0` or none. Wipe `target/` and repeat for 3. Whether that quoted key is accepted by `--config` is unverified. Repeat for 3. If `--config` with that quoted key is rejected, the fallback is an untracked `C:/w/gs/.cargo/config.toml` edit adding the same table (note this in the report; never in the main repo). Variant 0 is the same command with no `--config`.
- [ ] **Step 3: The harness** (scratchpad, not committed): a bash driver `spike-driver.sh LOG` launched once as `cmd //c start "" //b //belownormal bash spike-driver.sh LOG` so every child is BelowNormal. It runs rounds 1 to 3 with the variant order rotated: round 1 `0 1 3`, round 2 `1 3 0`, round 3 `3 0 1`. For each slot:
  1. wait, on an observable condition with a 10-minute deadline, for CPU under 20% (the Get-Counter reading), then record Defender and the time;
  2. in `C:/w/gs`: `git checkout -- .`, `rm -rf target`;
  3. `cold`: `cargo build -j 8 --offline -p manifold -p manifold-engine-webview2 $flags`, timed; then `du -sm target` (the D5 size);
  4. `ws-rebuild`: `cargo clean -p manifold -p ipc-protocol -p manifold-engine-webview2`, then the same build as `cold`, timed (D2's subtrahend);
  5. `incr1`, `incr2`: apply the same one-line semantic edit to a leaf function in `crates/manifold/src` (the implementer picks one line once, for example a numeric literal in `zoom_chip.rs`, records it, and every slot uses it, toggling the literal between two values), rebuild the same way, timed each;
  6. `test-build`: `cargo test -j 8 --offline --no-run -p manifold $flags`; `test-run`: `cargo test -j 8 --offline -p manifold $flags`; both timed;
  7. `bash scripts/headless-smoke-test.sh` and `bash scripts/link-probe.sh`, each to its own log, each timed by wall clock and by its TIMING lines (the exe is fresh from the last `incr` build); their `pass=`/`fail=` counts are kept for D4;
  8. a leftovers check (`Get-Process python, manifold, wsl`), then the row appended to the results file.
  Timing uses `EPOCHREALTIME` differences in the driver and writes one CSV row per step: `round,variant,step,secs,cpu_before,defender`. Do not use `Measure-Command`; a PowerShell host would add its own load.
- [ ] **Step 4: The probe slot.** Run only round 1, variant 0 first. Report its numbers and the size, restate the expected total, and continue only if under the limits above.
- [ ] **Step 5: Run the rest, then compute medians** per variant and step (three values each; print all three beside the median). A slot that was not quiet (CPU over 20% at its start for more than the wait deadline, or a leftover process) is marked and rerun at the end.
- [ ] **Step 6: Fill this table** in the Outcome section (medians; all three values in brackets), then apply the rule and write down which clause decided it.

| Step | opt 0 | opt 1 | opt 3 | ratio 1/0 | ratio 3/0 |
|---|---|---|---|---|---|
| cold build | | | | | |
| ws-rebuild | | | | | |
| cold dependency build (cold - ws-rebuild) | | | | | |
| target size (MB) | | | | | |
| incremental rebuild (2nd) | | | | | |
| `cargo test` build | | | | | |
| `cargo test` run | | | | | |
| smoke wall | | | | | |
| link-probe wall | | | | | |
| smoke pass/fail | | | | | |

- [ ] **Step 7: Clean up.** `git worktree remove --force C:/w/gs`, `Get-Process python, manifold, wsl` shows none of ours, and `df -h /c` is back to where it began.

**Blast radius:** none in the repo. The spike runs for hours on a machine other sessions use; the quiet checks will force waits.

---

### Task 7: Apply the decision (only if Task 6 adopts a variant), and re-baseline

**Model:** haiku. **Kind:** mechanical. **Proof:** before and after numbers from the same quiet-machine procedure; `gate-tier.sh` calls the change `full`; the stale-number list below done.

**Files:** `Cargo.toml`, `README.md`, `CLAUDE.md`, `scripts/gate.sh` (header), this plan's Outcome.

- [ ] **Step 1: If nothing was adopted, skip to Step 5.**
- [ ] **Step 2: `Cargo.toml`.** Add one line under the existing `[profile.dev.package."*"]` table, with a comment saying why:

```toml
[profile.dev.package."*"]
debug = false
# Dependencies are optimized (the app itself stays at opt-level 0 for fast rebuilds): the debug exe spends its
# frames in egui and wgpu. Spike 2026-10-09: smoke <X>% faster, cold build <Y>x. See docs/superpowers/plans/2026-10-09-gate-speed.md.
opt-level = <N>
```

  `<N>`, `<X>`, `<Y>` from the decision.
- [ ] **Step 3: Tell the user what this does to everyone**, in the commit message and the report: one cold rebuild of every dependency in each `target/` (every worktree, and every session on a shared `CARGO_TARGET_DIR`; sessions on an older checkout flip the shared directory back and forth until they rebase, so announce it first); the CI cache key (`Swatinem/rust-cache`) changes once and CI's cold build takes about D2's ratio longer; and `.githooks/pre-commit`'s clippy is a check build, which I expect is unaffected but have not verified: time one clippy before and after, same procedure, and report.
- [ ] **Step 4: Re-baseline.** On a quiet machine, `cargo build -j 8 -p manifold -p manifold-engine-webview2`, then `full`, `light`, `ui` once each to `target/gate-after-<tier>.log` as in Task 5. Replace the three durations (`gate.sh` header, `CLAUDE.md`, `README.md`) with the new ones and keep the old ones in the Outcome.
- [ ] **Step 5: The stale-number list** (exactly these; everything else stays). If adopted, edit each; if not, add the negative result to the README entry of Step 6 and leave them.
  - `scripts/gate.sh` header and `CLAUDE.md` "Gate": the three durations (done in Step 4).
  - `README.md` `scripts/gate.sh` paragraph: the durations and the "11.5 minutes at 35-90% CPU" loaded figure (keep it, date it as before C).
  - `README.md`: `link-probe.sh` "about 2 min", `jar-probe.sh` "about 5 min", `chrome-shots.sh` "about 5 min for all 46", `focus-loss-probe.sh` "about 80 s", `launch-forms-probe.sh` "about 6 s", `bad-url-probe.sh` "under a minute ... about 3 when it fails": re-time each once with `timing`-style wall clock, or mark it "before 2026-10-09's dev-profile change" if not re-timed.
  - `README.md` `scale-test.sh`: its recorded "restored in" times (a debug exe); memory figures are not expected to move, check one.
  - `README.md` "Running": the debug 13-16 fps against release 60 fps numbers are a dated debugging trail already marked superseded; add one dated line saying the dependencies are now optimized in dev, without changing the old numbers.
  - `docs/superpowers/plans/2026-10-07-wave-2-performance.md`: nothing edited. Its benches are `cargo test --release` and `json-pan-profile.sh` runs the release exe, so its timing numbers do not depend on the dev profile; its pins are counts and goldens; Task 16's "about 6 minutes" is a historical instruction. Confirm both claims with `grep -n 'cargo test' docs/superpowers/plans/2026-10-07-wave-2-performance.md | grep -v release` (expect only non-timing runs) before leaving it alone.
  - Any `#[test]` that asserts on a measured duration: `grep -rn 'elapsed()' crates/*/src | grep -E 'assert|<'` and judge each; optimized dependencies make code faster, so a "faster than" bound only gets more slack, but a "slower than" bound would break.
- [ ] **Step 6: README entry** under the gate paragraph: what the spike found (the table), the rule, the decision, and (if not adopted) the negative result in one sentence so nobody repeats it.
- [ ] **Step 7: Commit:** `dev profile: optimize dependencies (opt-level N), spike numbers in the plan` or, if not adopted, `docs: dev-profile dependency opt-level spike, not adopted`.

**Blast radius:** every build tree on the machine rebuilds its dependencies once. That is the point of announcing it.

---

### Task 8: Acceptance

**Model:** controller. **Kind:** judgment. **Proof:** a full gate run with the new gate, quiet machine.

- [ ] **Step 1:** `bash scripts/gate-tier.sh <base before Task 1>` prints `TIER full` (the gate scripts changed). Use it.
- [ ] **Step 2:** `cargo build -j 8 -p manifold -p manifold-engine-webview2`, quiet-machine checks, `cmd //c start "" //b //belownormal bash scripts/gate.sh target/gate-accept.log full`, Monitor until `GATE done`.
- [ ] **Step 3: Accept if** all three `two-N`, `smoke`, `launch-forms`, `link` show `exit=0` and the smoke `fail=0`; each stage's own TIMING lines sum to within 10 s of its `took=` (the rest is script start-up and cleanup); every scenario and step group of Tasks 1 to 2 appears; `GATE slowest steps` has ten lines; no `.cpu-stop` file and no sampler process remain. Report CPU and Defender with the durations.
- [ ] **Step 4:** `Get-Process python, manifold, wsl -ErrorAction SilentlyContinue` and `git worktree list` for leftovers; fill the Outcome; tell the user what is uncommitted.

---

## Outcome

Done 2026-10-09 (commits `ff21e63`, `9bee1e4`, `9014cc8`, `2f37641`, `195c89a`, `be837b1`). Tasks 7 and 8 not run: Task 6 adopted nothing.

**A and B (landed).** Every probe step prints `TIMING`; `gate.sh` collects them with a CPU sampler and prints the slowest ten; `gate.sh LOG ui` and `scripts/gate-tier.sh` exist. Extra, decided with the user: the freshness check and the documented gate build now cover the WebView2 host (Decision 10 fixed, `2f37641`).

**Baseline** (one run each, 30-35% CPU from other sessions, Defender off, so upper bounds): `ui` 2 min (unit 11 s, link 100 s), `light` 8.5 min (two-instance 156 s, smoke 237 s, launch-forms 18 s, link 83 s), `full` 11 min (two-instance 136/111/106 s, smoke 210 s, link 70 s). Slowest steps: smoke `json-level` 55 s, `group-drops` 40 s, `reopen-nested` 32 s, link `run4-persists` 26 s. The fixed waits of scenarios 3, 4, 5 are about 34 s of each ~110 s two-instance run (about 100 s of `full`): left parked, each guards a bounce loop.

**Spike C: negative result, nothing adopted.** Dependencies' `opt-level` 0 / 1 / 3 in the dev profile, three rotated rounds each in a throwaway worktree, medians (seven of nine slots were non-quiet, so the absolute numbers are noisy; the direction is not):

| Step | opt 0 | opt 1 | opt 3 |
|---|---|---|---|
| cold dependency build (s) | 97 | 247 (2.6x) | 287 (3.0x) |
| smoke, sum of TIMING (s) | 245 | 207 (-16%) | 197 (-20%) |
| link probe (s) | 72 | 64 | 76 |
| incremental rebuild, 2nd (s) | 7.4 | 5.2 | 5.1 |
| `cargo test` build (s) | 90 | 114 | 113 |
| target size (MB) | 1380 | 1134 | 1141 |

All runs passed the same checks (smoke 54/0, link 30/0). Rule as written: D1 (smoke drop at least 25%) fails for both (16%, 20%; 20% is outside the 22.5-27.5% rerun band); D2 (cold dependency build at most 2.0x) fails for both (2.6x, 3.0x). Individual smoke times swung 166-357 s within one variant, as large as the effect. `Cargo.toml` is unchanged.

## Review rounds

Three rounds, then a fourth read of everything edited in rounds 2 and 3 that found nothing material. Each round read the plan against the files (`gate.sh`, the three scripts, `lib.sh`, `fresh-exe.sh`, `winpid.sh`, `Cargo.toml`, `.cargo/config.toml`, CLAUDE.md, the README paragraphs), not against memory. The new scripts were run in a scratch directory outside the repo (not the gate, not cargo, not the app): `timing.sh`, `gate-timing.sh`, `gate-tier.sh` and `gate-tools-test.sh` (12 PASS), `cpu-sampler.ps1` (wrote three samples, exited on the stop file) and a `bash -n` of the new `gate.sh`.

1. **Drafting against the files.**
   - Caught that the per-stage `cpu()` calls are themselves ~4 s each (eight calls in a `full` gate), so Task 3 replaces six of them with a sampler instead of adding more readings.
   - Caught that a closing TIMING line printed by an EXIT trap would become the stage's `tail -1`, breaking the `last:` verdict text; Task 3 filters it.
   - Caught that `[profile.dev.package."*"]` already exists with `debug = false` and covers only non-workspace crates, so the spike adds only `opt-level` through `--config` and the app crates stay unoptimized.
   - Caught that the wave 2 benches are `--release` (and `json-pan-profile.sh` runs the release exe), so they do not go stale; the stale list in Task 7 is the gate and probe durations and the scale-test times.
   - Caught that the WebView2 user-data folder sits beside the host exe (`webview2_profile_dir`), which is the real reason a gate cannot share `target/debug` with a running app or another gate.
   - Caught that `cargo build -p manifold` does not build the WebView2 host (no dependency edge; the README lists four crates), so the gate may run an old host. Recorded as Decision 10 and an open question, not fixed.
   - Caught that the big UI files (`card.rs`, `graph_ui.rs`, `node_menu.rs`, `popups.rs`) write the store, so the `ui` allowlist is the 15 files with no store/sync/mirror/session/process/net import, re-checked at run time.
   - Caught that on `main` the default base `merge-base HEAD main` is `HEAD`, giving an empty diff; `gate-tier.sh` now says so and takes a BASE.
2. **Read-through of the written plan.**
   - Fixed a contradiction in Global Constraints ("leave everything uncommitted" against a commit step per task).
   - Fixed Task 2's expected link-probe TIMING count (listed six names, said seven).
   - Fixed the fixed-wait total (36 s by the source, not 34).
   - Fixed D2: the cold build includes the workspace crates, which are the same at every opt-level and would dilute the ratio; added a `ws-rebuild` step and defined the cold dependency build as cold minus it.
   - Replaced a muddled "after a `cargo clean -p serde`" in Task 6 Step 2 with a clean-target `-v` check, and marked the `--config` quoted-key acceptance as unverified.
   - Rewrote a muddled sentence about keeping the CPU/Defender line in the README paragraph.
3. **Anchors and tolerances.**
   - Re-checked every quoted anchor in the three scripts and the README against the current files, and recounted the `timing_mark` expectations (15, 13, 8).
   - Fixed the `node_menu.rs`/`popups.rs` hit counts (42 and 13, of a wider pattern).
   - Removed a vacuous acceptance check (a stage's `TIMING gate/` line equals its `took=` by construction) in favor of its own TIMING lines summing to within 10 s, and loosened the link-probe sum tolerance to 5 s.
   - Pointed Task 5's host-staleness test at a `touch` (mtime only) rather than an edit to revert.
4. **Fourth read: no material findings.** Soft spots left on purpose and flagged in-task: the quoted-key `--config` form, the 3 to 6 GB target size guess and the duration of the spike (both measured by the probe slot before it continues), whether `cargo test` as a `ui` stage is quick enough to justify a `ui` tier (Task 5 measures it), and the `full`-list additions beyond the user's (`local_state`, `engine_install`, `redirect_fetch`, `favicon_fetch`, `jar`).
