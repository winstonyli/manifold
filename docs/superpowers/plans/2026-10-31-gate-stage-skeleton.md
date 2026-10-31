---
status: done
---

# Gate stage skeleton Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every `gate` stage that takes a work dir runs through one function, `run_stage`. It finds `manifold.exe`, makes the work dir, runs the stage body, prints the stopped-early line, prints the registered log tails on failure, keeps the dir on failure and prints the closing line. This replaces the 21 `probes::setup`/`finish` call pairs and the nine hand-written Parent frames, and deletes `hello` and `hold`.

**Architecture:**
- `run_stage(name, parent, needs_exe, body)` lives in `main.rs` `windows_main`, beside `manifold_exe` and `work_dir`; the dispatcher owns the skeleton.
- A stage body has the type `fn(&mut Ctx) -> io::Result<()>`, and `Run::Work(Body)` replaces both `Run::Work` and `Run::Parent`.
- Instance logs reach the failure printout through a process-wide registry in `instance.rs`. `Instance::launch_env` fills it, and so do the few stages with extra logs.
- Work dirs move to `<parent>/gate-runs/<stage>-<pid>`, and `work_dir` prunes entries of that dir older than three days.

**Tech Stack:** Rust (edition of the workspace), the `gate` crate only (`crates/gate`), std only (no new dependency).

**Spec:** `docs/superpowers/specs/2026-10-31-gate-stage-skeleton-design.md`

## Global Constraints

- Scope: `crates/gate` plus the docs in Task 10. No other crate changes.
- `run_stage` steps, in order:
  1. `manifold_exe()` (when `needs_exe`), then `work_dir(parent, name)`. On error, print `{e}` (or `work dir: {e}`), print `{name}: failed`, and return exit 1.
  2. Read `before = failures()`, then run the body. On `Err(e)`, print `ERROR: {name} stopped early: {e}` and call `note_failure()`.
  3. `marks.end()`. `failed = stage_failed(errored, before, failures())`: the body returned `Err` or the count rose during it (an earlier failure in the process does not fail this stage; the exit code still reads the global count).
  4. If `failed`, take the registry (`take_logs()`) and print every registered log. A passing run does not touch the registry.
  5. If `keep_dir(failed, KEEP set)` is false, call `remove_dir_with_retries`.
  6. Print the last line, `{name}: ok` or `{name}: failed (kept {work})`, and return `exit_code()`.
- Log tail: `--- {path}` then the last 20 lines through `no_verdict_words`, for each non-empty registered file. Missing and empty files are skipped.
- Work dir: `<WORKDIR_PARENT or %TEMP%>/gate-runs/<stage>-<pid>`. Children of `gate-runs` last modified more than 3 days ago are removed by `work_dir`; nothing outside `gate-runs` is touched.
- `QUIT_TIMEOUT` is 60 s and `BELOW_NORMAL` is `0x4000`. Each gets one `pub(crate)` definition, in `crates/gate/src/instance.rs`.
- Exit codes:
  - A failed stage exits 1.
  - Usage errors of `scale`, `package` and `install-check` stay exit 2, checked before `run_stage`.
  - The `chrome-shots` config errors (were exit 2) become body errors and exit 1. They now happen after `run_stage` made the work dir, so a bad `SIZE` in `chrome-shots` or `shots-aa` creates and keeps an empty work dir (it exited before any dir existed). Accepted; it is item 8 of the spec's behaviour changes.
- No line the dispatcher prints contains `PASS` or `FAIL` (`gate all` counts those words per line). The closing line uses lowercase `failed`.
- Repo rules:
  - Run `cargo fmt --all` before each commit.
  - `cargo clippy -j 8 -p manifold -p manifold-hosts -p manifold-view-protocol -p manifold-pages -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings` must pass at the end of every task. `.githooks/pre-commit` runs `cargo fmt --all --check` and exactly this 7-package clippy whenever a `.rs` file is staged, so each Rust commit is its own clippy run; the tasks therefore run clippy explicitly only in Tasks 2 and 10 (Task 2 for the large conversion, Task 10 before the end-of-plan gate). The docs-only commits (Task 10) do not trigger the hook.
  - Never commit with `--no-verify`.
  - Don't silence a lint; fix it, or `#[allow(..., reason = "...")]` with a reason.
- Tests never read or write the real `%APPDATA%\manifold`. Unit tests here use `std::env::temp_dir().join(format!("gate-<name>-test-{}", std::process::id()))`, the pattern of `instance.rs`'s `seeded_settings_skip_the_demo`.
- The edit loop runs the quickest targeted check: `cargo test -p gate <filter>` (or `cargo check -p gate`), and the commit hook's clippy. The pre-tag gate runs at the end of the plan only (Task 10), plus one baseline run before any edit (Task 1 Step 1). It is never run per edit.
- Never push, tag or bump. Commit per task with `git add` of the named files. Run `git diff --stat` before each commit. End each commit message with `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`.
- Never run `python` reading stdin (`python -`, heredocs). If a script is ever needed, write it to a file and run `python -P f.py < /dev/null`. This plan needs none.
- Edit files with the Edit tool. Every "replace" below is an exact `old_string` -> `new_string` edit. Line numbers are as of commit `7e6010d5`; match on the text, not the number.

### Plan decisions (folded into the spec; listed so the reasons stay with the plan)

1. **`Ctx` has no `args` field.** `scale`, `package` and `install-check` keep their own parsed arguments in the closure they pass to `run_stage`.
   - `run_stage` takes `body: impl FnOnce(&mut Ctx) -> io::Result<()>`; the `Body` fn pointer type is what `Run::Work` stores.
   - `Ctx` is `{ exe, work, marks }`, and the signature is `run_stage(name, parent, needs_exe, body)`.
2. **`two` takes the marks with `std::mem::take(&mut ctx.marks)`** and owns them in `Two` as today. It does not hold `&mut Marks`, so `Two` needs no lifetime parameter. The last section still ends when the body returns.
3. **Deletions land with their last caller**, so every commit passes clippy's dead-code lint:
   - `LAYOUT_KEEP` goes in Task 3 (layout).
   - `Marks::start` goes in Task 7 (smoke and two).
   - `probes::setup`/`probes::finish` go in Task 8 (scale and package).
   - `Instance::log_tail` (the method) goes in Task 9 (hello).
4. **`package` and `install-check` keep `guarded`** inside their body, so their `FAIL: package: ...` verdict lines stay. Only the frame changes.
5. **GATE15 is done only in part.** The spec covers its `Marks` part, which this plan does. The `check`/`check_eq`/`check_is` bodies and `default_jar_id`/`default_jar_node` stay listed in the cleanup plan.
6. **Behavioural proof has a baseline run before any edit** (Task 1 Step 1): a `full` gate at `7e6010d5` plus `dblprobe` and `scale 2`. Task 10 runs the same set after, and diffs the verdict lines. `full` covers every tier, so it also satisfies whatever `gate tier` advises.
7. **The spec's prune test "keeps a non-`gate-` dir"** becomes a sibling dir beside `gate-runs`, which pruning must not touch. Only `gate-runs` is ever pruned.
8. **The spec's "README also documents `KEEP`" is wrong.** `git grep -n KEEP README.md` finds only `LAYOUT_KEEP`. Task 10 adds the first README description of `KEEP` and of the kept dir.

## Review Focus

1. **Missing or stale `manifold.exe`.**
   - Expected: the error line, then `<stage>: failed`, exit 1. No work dir is created and the body never runs.
   - Pinned by `a_setup_failure_runs_no_body_and_makes_no_dir` (Task 2).
2. **A user's `WORKDIR_PARENT` holds their own dirs.**
   - Expected: pruning removes nothing outside `<parent>/gate-runs`.
   - Pinned by `work_dirs_live_under_gate_runs_and_old_ones_are_pruned` (Task 1).
3. **Logs outside the work dir** (`smoke`'s `HEADLESS_TEST_OUT`, `bad_url`'s per-case TEMP host logs, `two`'s `out/`).
   - Expected: they still appear in a failed run's tails, because registration happens at `File::create` in `launch_env`, wherever the log lives.
   - Pinned by `launch_registers_its_log_once_and_take_empties_the_list` (Task 2). It launches a missing exe, so the log is created and registered and the spawn fails.
4. **`KEEP=1` on a passing run.**
   - Expected: the dir is kept and the last line is still `<stage>: ok`.
   - Pinned by `a_failed_run_or_keep_keeps_the_work_dir` and `the_early_stop_and_closing_lines_carry_no_verdict_word` (Task 2).
5. **A body that prints its own closing text** (`smoke`'s "All checks passed / Some checks failed").
   - Expected: the dispatcher's `smoke: ok|failed (kept ...)` is still the last line. `gate all`'s `last:` headline shows it, and smoke's own line appears only on the body's `Ok` path.
   - Checked in Task 10 Step 6 (the `last:` lines of the end-of-plan gate).

---

### Task 1: Shared pieces with no new consumer (Marks, no_verdict_words, QUIT_TIMEOUT, BELOW_NORMAL, gate-runs and pruning)

**Files:**
- Modify: `crates/gate/src/check.rs`, `crates/gate/src/instance.rs`, `crates/gate/src/main.rs`
- Modify (imports and consts only): `crates/gate/src/link.rs`, `closed.rs`, `unknown_type.rs`, `trail.rs`, `smoke.rs`, `two/mod.rs`, `probes.rs`, `shots.rs`, `dblprobe.rs`, `layout.rs`, `launch_forms.rs`, `profile.rs`
- Modify: `docs/superpowers/plans/2026-10-31-gate-stage-skeleton.md` (status)
- Test: `crates/gate/src/check.rs` (tests module), `crates/gate/src/main.rs` (tests module)

**Interfaces:**
- Consumes: nothing new.
- Produces:
  - `#[derive(Default)] pub struct Marks(Option<Section>)`; `Marks::mark(&mut self, name: &str)` opens a section even on an empty `Marks`.
  - `crate::check::no_verdict_words(text: &str) -> String`.
  - `crate::instance::QUIT_TIMEOUT: Duration` (60 s) and `crate::instance::BELOW_NORMAL: u32` (0x4000), both `pub(crate)`.
  - `crate::windows_main::work_dir(parent: Option<PathBuf>, stage: &str) -> std::io::Result<PathBuf>`, which now returns `<parent>/gate-runs/<stage>-<pid>`.
  - `windows_main::prune(runs: &Path, now: SystemTime)` (private).

- [ ] **Step 1: Baseline run before any edit (behavioural proof, plan decision 6)**

Check that nothing else holds the target dir. Check free disk (`df -h /c`). C: is nearly full, so stop and report if it has under 15 GB free. Then launch from bash in the repo root:

```bash
powershell -NoProfile -File "$USERPROFILE/.claude/runs/run.ps1" -Kind gate -Name skeleton-base -Detach -Command 'cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate && target\debug\gate.exe all "%RUN_DIR%\gate-log.txt" full'
```

Expected: the wrapper prints a run id and returns. Wait with a Monitor until-loop (deadline 45 min, no fixed sleeps) that exits when `gate-log.txt` in the newest `"$LOCALAPPDATA"/claude-runs/manifold/*-skeleton-base` dir contains `GATE done` OR that dir's `run.json` has an `"end"` that is no longer `null` (a crashed wrapper or a failed build never writes `GATE done`). Then read the exit code from `run.json`; without `GATE done` the baseline did not finish, so stop and report.

Do not edit any file under `crates/` while it runs: `gate` refuses a `manifold.exe` older than any source file. Then run the two stages `gate all` does not run, one after the other:

```bash
ROUNDS=3 powershell -NoProfile -File "$USERPROFILE/.claude/runs/run.ps1" -Kind probe -Name skeleton-base-dblprobe -Detach -Command 'target\debug\gate.exe dblprobe'
powershell -NoProfile -File "$USERPROFILE/.claude/runs/run.ps1" -Kind probe -Name skeleton-base-scale -Detach -Command 'target\debug\gate.exe scale 2'
```

Launch them one at a time with `-Detach`, each followed by the same watcher as the gate run: a Monitor until-loop (deadline 20 min, no fixed sleeps) that exits when that run's `run.json` `"end"` is no longer `null`. Read the exit code from `run.json`; a missing `"end"` at the deadline means it hung, so stop and report.

Expected: each ends with exit 0, and its `out.log` holds the PASS lines. Note the three run ids for Task 10. If the baseline gate itself fails a stage, stop and report it: the before/after diff needs a known baseline.

- [ ] **Step 2: Mark the plan active**

In `docs/superpowers/plans/2026-10-31-gate-stage-skeleton.md` replace `status: draft` with `status: active` (front matter, line 2).

- [ ] **Step 3: Write the failing tests**

In `crates/gate/src/check.rs`, inside `mod tests` (after `timing_line_truncates_to_milliseconds`), add:

```rust
    #[test]
    fn a_mark_on_empty_marks_opens_a_section() {
        let mut m = Marks::default();
        assert!(m.0.is_none());
        m.mark("a");
        assert_eq!(m.0.as_ref().map(|s| s.name.as_str()), Some("a"));
        m.mark("b");
        assert_eq!(m.0.as_ref().map(|s| s.name.as_str()), Some("b"));
        m.end();
        assert!(m.0.is_none());
        m.mark("c");
        assert_eq!(m.0.as_ref().map(|s| s.name.as_str()), Some("c"), "a mark after end opens a section");
    }

    #[test]
    fn diagnostics_never_carry_the_verdict_words() {
        let t = no_verdict_words("PASS: x\nFAIL: y");
        assert!(!t.contains("PASS") && !t.contains("FAIL"));
    }
```

In `crates/gate/src/main.rs`, inside `windows_main`'s `mod tests` (after `install_checks_usage_lists_the_installable_hosts`), add `use std::os::windows::fs::OpenOptionsExt;` to the module's imports if it is not there, and the test below (it relies on `use super::*` for `Path`, `Duration` and `SystemTime`; add them to the imports if the module imports by name). `File::set_modified` is used on a directory, which Windows opens for writing only with `FILE_FLAG_BACKUP_SEMANTICS` (`0x0200_0000`); `fresh.rs` and `sync_deps.rs` tests already use `set_modified` on files.

```rust
        #[test]
        fn work_dirs_live_under_gate_runs_and_old_ones_are_pruned() {
            let root = std::env::temp_dir().join(format!("gate-prune-test-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            let runs = root.join("gate-runs");
            let (old, mid) = (runs.join("old-1"), runs.join("mid-1"));
            let sibling = root.join("gate-mine");
            std::fs::create_dir_all(&old).unwrap();
            std::fs::create_dir_all(&mid).unwrap();
            std::fs::create_dir_all(&sibling).unwrap();
            // Directories open for writing only with backup semantics (FILE_FLAG_BACKUP_SEMANTICS).
            let backdate = |dir: &Path, age: Duration| {
                let f = std::fs::OpenOptions::new().write(true).custom_flags(0x0200_0000).open(dir).unwrap();
                f.set_modified(SystemTime::now() - age).unwrap();
            };
            backdate(&old, Duration::from_secs(4 * 24 * 60 * 60));
            backdate(&mid, Duration::from_secs(2 * 24 * 60 * 60));
            backdate(&sibling, Duration::from_secs(4 * 24 * 60 * 60));
            let fresh = runs.join("fresh-1");
            std::fs::create_dir_all(&fresh).unwrap();
            prune(&runs, SystemTime::now());
            assert!(!old.exists(), "a dir older than three days goes");
            assert!(mid.is_dir() && fresh.is_dir(), "a dir two days old and a fresh one stay");
            assert!(sibling.is_dir(), "nothing outside gate-runs is touched, however old");
            let work = work_dir(Some(root.clone()), "t").unwrap();
            assert_eq!(work, runs.join(format!("t-{}", std::process::id())), "the work dir lives under gate-runs");
            assert!(work.is_dir() && mid.is_dir(), "work_dir makes its dir and keeps younger ones");
            let _ = std::fs::remove_dir_all(&root);
        }
```

- [ ] **Step 4: Run them to verify they fail**

Run: `cargo test -p gate -j 8 a_mark_on_empty`
Expected: the build fails. The errors include ``no function or associated item named `default` found for struct `Marks` ``, ``cannot find function `no_verdict_words` in this scope`` and ``cannot find function `prune` in this scope``.

- [ ] **Step 5: Marks gets Default; mark opens a section on an empty Marks; no_verdict_words moves to check.rs**

In `crates/gate/src/check.rs` replace:

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
```

with:

```rust
/// The timing marks: opening one ends the previous (`Section::next`), `end` closes the last. `Marks::default()` is
/// empty; the first `mark` opens a section.
#[derive(Default)]
pub struct Marks(Option<Section>);

impl Marks {
    pub fn start(name: &str) -> Marks {
        Marks(Some(Section::start(name)))
    }

    pub fn mark(&mut self, name: &str) {
        drop(self.0.take());
        self.0 = Some(Section::start(name));
    }
```

(`Marks::start` stays until Task 7 removes its last users.) The old `mark` did nothing once `end` had run; the new one opens a section after `end` too. No caller marks after `end`: `git grep -n 'Marks\|marks' -- crates/gate/src` at `7e6010d5` shows `smoke` calling `marks.end()` last, and `two` calling `t.marks.end()` as its last use (after Task 7, `two` moves the marks into `Two` and `drop(t)` ends them; `run_stage` ends `ctx.marks`, empty by then, after the body). So the change is unobservable. Then, directly above `/// A timed step: prints its `TIMING` line when dropped`, add:

```rust
/// Replaces the verdict words, so a log line that happens to contain one is not counted by gate all.
pub fn no_verdict_words(text: &str) -> String {
    text.replace("PASS", "P-ASS").replace("FAIL", "F-AIL")
}

```

In `crates/gate/src/link.rs`:
- Delete the function. Remove:

  ```rust
  /// Replaces the verdict words, so a log line that happens to contain one is not counted by gate all.
  pub(crate) fn no_verdict_words(text: &str) -> String {
      text.replace("PASS", "P-ASS").replace("FAIL", "F-AIL")
  }

  ```

- Delete its test. Remove:

  ```rust

      #[test]
      fn diagnostics_never_carry_the_verdict_words() {
          let t = no_verdict_words("PASS: x\nFAIL: y");
          assert!(!t.contains("PASS") && !t.contains("FAIL"));
      }
  ```

- Replace `use crate::check::{check_eq, exit_code, failures, note_failure, Section};` with `use crate::check::{check_eq, exit_code, failures, no_verdict_words, note_failure, Section};`.

Update the importers:
- `closed.rs`:
  - `use crate::check::{check, check_is, exit_code, failures, note_failure, Section};` becomes `use crate::check::{check, check_is, exit_code, failures, no_verdict_words, note_failure, Section};`.
  - `use crate::link::{bad, no_verdict_words};` becomes `use crate::link::bad;`.
- `unknown_type.rs`:
  - `use crate::check::{check, check_eq, exit_code, failures, note_failure, Section};` becomes `use crate::check::{check, check_eq, exit_code, failures, no_verdict_words, note_failure, Section};`.
  - `use crate::link::{bad, no_verdict_words, num};` becomes `use crate::link::{bad, num};`.
- `trail.rs`:
  - `use crate::check::{check_is, exit_code, failures, note_failure, Section};` becomes `use crate::check::{check_is, exit_code, failures, no_verdict_words, note_failure, Section};`.
  - `use crate::link::{bad, no_verdict_words};` becomes `use crate::link::bad;`.
- `smoke.rs`:
  - `use crate::check::{exit_code, fail_line, failures, note_failure, pass_line, Marks};` becomes `use crate::check::{exit_code, fail_line, failures, no_verdict_words, note_failure, pass_line, Marks};`.
  - `use crate::link::{bad, no_verdict_words, truthy};` becomes `use crate::link::{bad, truthy};`.
- `two/mod.rs`:
  - `use crate::check::{check, exit_code, failures, note_failure, Marks};` becomes `use crate::check::{check, exit_code, failures, no_verdict_words, note_failure, Marks};`.
  - Delete the line `use crate::link::no_verdict_words;`.

- [ ] **Step 6: One QUIT_TIMEOUT and one BELOW_NORMAL, in instance.rs**

In `crates/gate/src/instance.rs` replace:

```rust
/// `BELOW_NORMAL_PRIORITY_CLASS`: the instance never competes with the user's work (LONG_RUNS.md).
const BELOW_NORMAL: u32 = 0x4000;
```

with:

```rust
/// `BELOW_NORMAL_PRIORITY_CLASS`: the instance never competes with the user's work (LONG_RUNS.md).
pub(crate) const BELOW_NORMAL: u32 = 0x4000;
/// How long an instance may take to exit after QUIT before `Instance::quit` kills it.
pub(crate) const QUIT_TIMEOUT: Duration = Duration::from_secs(60);
```

Delete the local copies and import the shared one:

| File | Remove (exact) | Import edit | Also |
|---|---|---|---|
| `link.rs` | `/// How long an instance may take to exit after QUIT.\nconst QUIT_TIMEOUT: Duration = Duration::from_secs(60);\n` | `use crate::instance::{log_tail, seed_settings, Instance};` -> `use crate::instance::{log_tail, seed_settings, Instance, QUIT_TIMEOUT};` | delete `use std::time::Duration;` |
| `closed.rs` | `/// How long an instance may take to exit after QUIT.\nconst QUIT_TIMEOUT: Duration = Duration::from_secs(60);\n` | `use crate::instance::{log_tail, seed_settings, Instance};` -> `use crate::instance::{log_tail, seed_settings, Instance, QUIT_TIMEOUT};` | delete `use std::time::Duration;` |
| `unknown_type.rs` | `/// How long an instance may take to exit after QUIT.\nconst QUIT_TIMEOUT: Duration = Duration::from_secs(60);\n` | `use crate::instance::{log_tail, seed_settings, Instance};` -> `use crate::instance::{log_tail, seed_settings, Instance, QUIT_TIMEOUT};` | delete `use std::time::Duration;` |
| `trail.rs` | `/// How long the instance may take to exit after QUIT.\nconst QUIT_TIMEOUT: Duration = Duration::from_secs(60);\n` | `use crate::instance::{log_tail, seed_settings, Instance};` -> `use crate::instance::{log_tail, seed_settings, Instance, QUIT_TIMEOUT};` | delete `use std::time::Duration;` |
| `smoke.rs` | `/// How long the instance may take to exit after QUIT (the script waits without a limit).\nconst QUIT_TIMEOUT: Duration = Duration::from_secs(60);\n` | `use crate::instance::{file_url, log_tail, remove_dir_with_retries, seed_settings, Instance};` -> `use crate::instance::{file_url, log_tail, remove_dir_with_retries, seed_settings, Instance, QUIT_TIMEOUT};` | keep `Duration` (the polls use it) |
| `two/mod.rs` | `const QUIT_TIMEOUT: Duration = Duration::from_secs(60);\n` | `use crate::instance::{log_tail, remove_dir_with_retries, seed_settings, wait_file, Instance};` -> `use crate::instance::{log_tail, remove_dir_with_retries, seed_settings, wait_file, Instance, QUIT_TIMEOUT};` | keep `Duration` |
| `probes.rs` | `const QUIT_TIMEOUT: Duration = Duration::from_secs(30);\n\n` | `use crate::instance::{remove_dir_with_retries, seed_settings, Instance, STEP_TIMEOUT};` -> `use crate::instance::{remove_dir_with_retries, seed_settings, Instance, QUIT_TIMEOUT, STEP_TIMEOUT};` | keep `Duration` |
| `shots.rs` | `const QUIT_TIMEOUT: Duration = Duration::from_secs(30);\n` | `use crate::instance::{seed_settings, Instance};` -> `use crate::instance::{seed_settings, Instance, QUIT_TIMEOUT};` | delete `use std::time::Duration;` |

Inline quits:
- `dblprobe.rs`:
  - Replace `    a.quit(Duration::from_secs(60)).map(|_| ())` with `    a.quit(QUIT_TIMEOUT).map(|_| ())`.
  - Replace `use crate::instance::{seed_settings, Instance};` with `use crate::instance::{seed_settings, Instance, QUIT_TIMEOUT};`.
  - Delete `use std::time::Duration;`.
- `layout.rs`:
  - Replace `            let status = inst.quit(Duration::from_secs(30))?;` with `            let status = inst.quit(QUIT_TIMEOUT)?;`.
  - Replace `use crate::instance::{seed_settings, Instance};` with `use crate::instance::{seed_settings, Instance, QUIT_TIMEOUT};`.
  - Delete `use std::time::Duration;`.
- Leave `main.rs`'s `hello` (`quit(Duration::from_secs(30))`; Task 9 deletes it) and `stress.rs:124` (`quit(Duration::from_secs(15))`, the 10 s bound under test) unchanged.

BELOW_NORMAL:
- `launch_forms.rs`:
  - Remove ``/// `BELOW_NORMAL_PRIORITY_CLASS`, as in `instance.rs`.\nconst BELOW_NORMAL: u32 = 0x4000;\n``.
  - Replace `use crate::instance::{job, remove_dir_with_retries};` with `use crate::instance::{job, remove_dir_with_retries, BELOW_NORMAL};`.
- `profile.rs`:
  - Remove `const BELOW_NORMAL: u32 = 0x4000;\n`.
  - Replace `use serde_json::{json, Value};\n` (the import before it) with `use serde_json::{json, Value};\n\nuse crate::instance::BELOW_NORMAL;\n`.

Then check that no copy is left:

Run: `git grep -n -E 'const (QUIT_TIMEOUT|BELOW_NORMAL)|quit\(Duration' -- crates/gate`
Expected: exactly `crates/gate/src/instance.rs` (two `const` lines), `crates/gate/src/main.rs` (`quit(Duration::from_secs(30))` in hello) and `crates/gate/src/stress.rs` (`quit(Duration::from_secs(15))`).

- [ ] **Step 7: work_dir under gate-runs, with pruning**

In `crates/gate/src/main.rs` replace `    use std::time::Duration;` (inside `windows_main`) with `    use std::time::{Duration, SystemTime};`, then replace:

```rust
    /// A fresh work dir (log, exports, profile) under `parent`, default the temp dir.
    pub(crate) fn work_dir(parent: Option<PathBuf>, stage: &str) -> std::io::Result<PathBuf> {
        let dir = parent.unwrap_or_else(std::env::temp_dir).join(format!("gate-{stage}-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }
```

with:

```rust
    /// How old a dir under `gate-runs` may get before `work_dir` removes it.
    const RUN_DIR_MAX_AGE: Duration = Duration::from_secs(3 * 24 * 60 * 60);

    /// A fresh work dir (log, exports, profile): `<parent>/gate-runs/<stage>-<pid>`, `parent` defaulting to the temp
    /// dir. Dirs under `gate-runs` older than `RUN_DIR_MAX_AGE` are removed first (a failed run keeps its dir, so they
    /// would pile up); nothing outside `gate-runs` is touched.
    pub(crate) fn work_dir(parent: Option<PathBuf>, stage: &str) -> std::io::Result<PathBuf> {
        let runs = parent.unwrap_or_else(std::env::temp_dir).join("gate-runs");
        prune(&runs, SystemTime::now());
        let dir = runs.join(format!("{stage}-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    /// Removes the children of `runs` last modified more than `RUN_DIR_MAX_AGE` before `now`.
    fn prune(runs: &Path, now: SystemTime) {
        for entry in std::fs::read_dir(runs).into_iter().flatten().flatten() {
            let modified = entry.metadata().and_then(|m| m.modified());
            if modified.is_ok_and(|t| now.duration_since(t).is_ok_and(|age| age > RUN_DIR_MAX_AGE)) {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
```

Then confirm nothing looks a work dir up by its old name:

Run: `git grep -n -E 'gate-\{stage\}|"gate-(hello|hold|launch-forms|layout|link|smoke|two|unknown-type|closed|dblprobe|trail|scale|package|install-check)' -- crates scripts`
Expected: no output.

- [ ] **Step 8: Run the tests to verify they pass**

Run: `cargo test -p gate -j 8 -- a_mark_on_empty diagnostics_never_carry work_dirs_live_under`
Expected: `test result: ok. 3 passed`.

Run: `cargo test -p gate -j 8`
Expected: `test result: ok.`, 0 failed.

- [ ] **Step 9: Format, lint, commit**

```bash
cargo fmt --all
git diff --stat
git add crates/gate/src docs/superpowers/plans/2026-10-31-gate-stage-skeleton.md
git commit -m "gate: shared Marks, no_verdict_words, QUIT_TIMEOUT and BELOW_NORMAL; work dirs under gate-runs with 3-day pruning" -m "Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

Expected: the pre-commit hook's fmt check and clippy pass (the commit goes through). `git diff --stat` lists only `crates/gate/src/*` files and the plan.

---

### Task 2: The log registry, run_stage, and the 21 Work stages

**Files:**
- Modify: `crates/gate/src/instance.rs` (registry, `launch_env`)
- Modify: `crates/gate/src/main.rs` (`Run`, `Stage::work`, dispatch, `Ctx`, `Body`, `run_stage` and helpers, tests)
- Modify (stage conversions): `crates/gate/src/probes.rs` (`page_kind`, `hscroll`, `key`, `bad_url`, `failed_spawn`, `host_logs`), `stress.rs` (`unresponsive`), `focus.rs`, `download.rs`, `shots.rs` (3 stages, `config`, `run`), `click_count.rs`, `iframe_typing.rs`, `page_text.rs`, `surface.rs`, `search.rs`, `suggest_groups.rs`, `labels.rs`, `arrange.rs`, `sleep.rs`, `jar.rs`
- Test: `crates/gate/src/instance.rs` and `crates/gate/src/main.rs` tests modules

**Interfaces:**
- Consumes (Task 1):
  - `Marks::default()` and `Marks::mark`.
  - `no_verdict_words`.
  - `work_dir`.
- Produces:
  - `crate::instance::register_log(path: &Path)` and `crate::instance::take_logs() -> Vec<PathBuf>` (`pub`).
  - `pub(crate) struct crate::windows_main::Ctx { pub exe: PathBuf, pub work: PathBuf, pub marks: Marks }`.
  - `type Body = fn(&mut Ctx) -> io::Result<()>` (private to `windows_main`).
  - `pub(crate) fn crate::windows_main::run_stage(name: &str, parent: Option<PathBuf>, needs_exe: bool, body: impl FnOnce(&mut Ctx) -> io::Result<()>) -> ExitCode`.
  - `Stage::work(name: &'static str, f: Body) -> Stage`.
  - Every Work stage function is now `pub(crate) fn NAME(ctx: &mut Ctx) -> io::Result<()>`.

- [ ] **Step 1: Write the failing tests**

In `crates/gate/src/instance.rs`, inside `mod tests`, add:

```rust
    #[test]
    fn launch_registers_its_log_once_and_take_empties_the_list() {
        // One test, not two: the registry is process-wide and tests run in parallel.
        let dir = std::env::temp_dir().join(format!("gate-register-test-{}", std::process::id()));
        let missing_exe = dir.join("no-such-manifold.exe");
        let log = dir.join("x.log");
        assert!(Instance::launch(&missing_exe, &dir.join("profile"), &dir, "x.log").is_err(), "the spawn fails");
        let taken = take_logs();
        assert!(taken.contains(&log), "launch_env registers the log at File::create: {taken:?}");
        // A second launch truncates the same file: still one entry.
        assert!(Instance::launch(&missing_exe, &dir.join("profile"), &dir, "x.log").is_err());
        assert!(Instance::launch(&missing_exe, &dir.join("profile"), &dir, "x.log").is_err());
        let taken = take_logs();
        assert_eq!(taken.iter().filter(|p| **p == log).count(), 1, "deduplicated: {taken:?}");
        assert!(!take_logs().contains(&log), "take empties the list");
        let _ = fs::remove_dir_all(&dir);
    }
```

In `crates/gate/src/main.rs`, inside `windows_main`'s `mod tests`, add:

```rust
        #[test]
        fn a_failed_run_or_keep_keeps_the_work_dir() {
            assert!(!keep_dir(false, false));
            assert!(keep_dir(true, false));
            assert!(keep_dir(false, true));
            assert!(keep_dir(true, true));
        }

        #[test]
        fn the_early_stop_and_closing_lines_carry_no_verdict_word() {
            let e = std::io::Error::other("export s1 never appeared");
            assert_eq!(early_stop_line("link", &e), "ERROR: link stopped early: export s1 never appeared");
            let work = Path::new(r"C:\t\gate-runs\link-7");
            assert_eq!(closing_line("link", false, work), "link: ok");
            assert_eq!(closing_line("link", true, work), r"link: failed (kept C:\t\gate-runs\link-7)");
            let quoting = std::io::Error::other("host said FAIL: no PASS");
            assert_eq!(early_stop_line("x", &quoting), "ERROR: x stopped early: host said F-AIL: no P-ASS");
            for line in [early_stop_line("x", &e), early_stop_line("x", &quoting), closing_line("x", true, work)] {
                assert!(!line.contains("PASS") && !line.contains("FAIL"), "{line}");
            }
        }

        #[test]
        fn log_tails_mask_the_verdict_words_and_skip_empty_and_missing_logs() {
            let dir = std::env::temp_dir().join(format!("gate-tails-test-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let (full, empty, missing) = (dir.join("full.log"), dir.join("empty.log"), dir.join("missing.log"));
            let lines: Vec<String> = (1..=25).map(|i| format!("line {i}")).collect();
            std::fs::write(&full, format!("{}\nPASS: x\nFAIL: y\n", lines.join("\n"))).unwrap();
            std::fs::write(&empty, "").unwrap();
            let text = log_tails(&[full.clone(), empty, missing]);
            assert!(text.starts_with(&format!("--- {}\n", full.display())), "{text}");
            assert!(!text.contains("PASS") && !text.contains("FAIL"), "{text}");
            assert!(text.contains("P-ASS: x") && text.contains("F-AIL: y"), "{text}");
            assert!(text.contains("line 8\n") && !text.contains("line 7\n"), "the last 20 lines only: {text}");
            assert!(!text.contains("empty.log") && !text.contains("missing.log"), "{text}");
            let _ = std::fs::remove_dir_all(&dir);
        }

        /// Under `cargo test` the exe is `target/debug/deps/gate-*.exe`, with no `manifold.exe` beside it, so
        /// `manifold_exe()` fails: the setup-failure path, which touches no process-wide counter.
        #[test]
        fn a_setup_failure_runs_no_body_and_makes_no_dir() {
            let root = std::env::temp_dir().join(format!("gate-setup-test-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            let ran = std::cell::Cell::new(false);
            let _ = run_stage("t", Some(root.clone()), true, |_| {
                ran.set(true);
                Ok(())
            });
            assert!(!ran.get(), "the body never runs");
            assert!(!root.join("gate-runs").exists(), "no work dir is made");
        }

        /// A stage fails if its body errored or the failure count rose during the body; a count already above zero
        /// (another unit test failing a check on purpose) does not matter.
        #[test]
        fn a_stage_fails_only_on_an_error_or_a_rise_during_its_body() {
            assert!(!stage_failed(false, 0, 0));
            assert!(!stage_failed(false, 3, 3), "earlier failures do not fail this stage");
            assert!(stage_failed(false, 3, 4));
            assert!(stage_failed(true, 3, 3));
        }

        /// The success path: the body runs with its work dir made, and the dir is gone afterwards. `needs_exe` is
        /// false, so no `manifold.exe` is needed. `check.rs`'s `a_failed_check_is_counted` raises the process-wide
        /// failure count in parallel, and if that lands inside `run_stage`'s before/after window the stage counts as
        /// failed and keeps its dir. So the test reads `failures()` around the call and asserts "the dir is gone" only
        /// when the count did not move (and `KEEP` is unset: it keeps the dir by design). The exit code is not
        /// asserted: it reads the process-wide count (Step 6 checks that a passing stage exits 0).
        #[test]
        fn a_passing_body_runs_in_its_work_dir_and_leaves_none_behind() {
            let root = std::env::temp_dir().join(format!("gate-ok-test-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            let seen = std::cell::RefCell::new(None);
            let before = failures();
            let _ = run_stage("t", Some(root.clone()), false, |ctx| {
                *seen.borrow_mut() = Some((ctx.work.clone(), ctx.work.is_dir()));
                Ok(())
            });
            let after = failures();
            let (work, existed) = seen.borrow().clone().expect("the body ran");
            assert_eq!(work, root.join("gate-runs").join(format!("t-{}", std::process::id())));
            assert!(existed, "the work dir exists inside the body");
            if before == after && std::env::var_os("KEEP").is_none() {
                assert!(!work.exists(), "a passing run deletes its work dir");
            }
            let _ = std::fs::remove_dir_all(&root);
        }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p gate -j 8 launch_registers`
Expected: the build fails. The errors include ``cannot find function `register_log` in this scope``, ``cannot find function `keep_dir` in this scope``, ``cannot find function `log_tails` in this scope``, ``cannot find function `stage_failed` in this scope`` and ``cannot find function `run_stage` in this scope``.

- [ ] **Step 3: The registry in instance.rs**

In `crates/gate/src/instance.rs`:
- Replace `use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};` with `use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};\nuse std::sync::{Mutex, PoisonError};`.
- Directly above ``/// Removes a work dir, retrying while a just-killed instance still holds files in it.``, add:

  ```rust
  /// The logs a failed stage prints the tails of (`run_stage`), in the order first registered.
  static LOGS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

  /// Adds `path` to the logs a failed stage prints; a path already there stays one entry (a later launch truncated it).
  pub fn register_log(path: &Path) {
      let mut logs = LOGS.lock().unwrap_or_else(PoisonError::into_inner);
      if !logs.iter().any(|p| p == path) {
          logs.push(path.to_path_buf());
      }
  }

  /// Empties the registered logs and returns them.
  pub fn take_logs() -> Vec<PathBuf> {
      std::mem::take(&mut *LOGS.lock().unwrap_or_else(PoisonError::into_inner))
  }

  ```

- In `launch_env` replace `        let out = File::create(&log)?;` with `        let out = File::create(&log)?;\n        register_log(&log);`.

- [ ] **Step 4: Ctx, run_stage and its helpers in main.rs**

In `crates/gate/src/main.rs` (inside `windows_main`), replace the imports:

```rust
    use std::path::{Path, PathBuf};
    use std::process::ExitCode;
    use std::time::{Duration, SystemTime};

    use crate::check::{check, check_eq, exit_code, note_failure, Section};
    use crate::fresh::require_fresh_exe;
    use crate::instance::{seed_settings, Instance};
```

with:

```rust
    use std::fmt::{Display, Write as _};
    use std::io;
    use std::path::{Path, PathBuf};
    use std::process::ExitCode;
    use std::time::{Duration, SystemTime};

    use crate::check::{check, check_eq, exit_code, failures, no_verdict_words, note_failure, Marks, Section};
    use crate::fresh::require_fresh_exe;
    use crate::instance::{log_tail, remove_dir_with_retries, seed_settings, take_logs, Instance};
```

Replace the `Run` enum and `Stage::work`:

```rust
    /// How a stage takes its arguments: the manifold exe and a fresh work dir (made from the work-dir parent argument),
    /// the work-dir parent, the rest of the command line, or nothing.
    enum Run {
        Work(fn(&Path, &Path) -> ExitCode),
```

with:

```rust
    /// What a stage that takes `[WORKDIR_PARENT]` runs inside `run_stage`.
    type Body = fn(&mut Ctx) -> io::Result<()>;

    /// How a stage takes its arguments: a body run by `run_stage` (with the manifold exe and a fresh work dir made from
    /// the work-dir parent argument), the work-dir parent, the rest of the command line, or nothing.
    enum Run {
        Work(Body),
```

and:

```rust
        /// A stage that takes `[WORKDIR_PARENT]` and is run with `manifold.exe` and its work dir (named after the stage).
        const fn work(name: &'static str, f: fn(&Path, &Path) -> ExitCode) -> Self {
```

with:

```rust
        /// A stage that takes `[WORKDIR_PARENT]`; `run_stage` runs its body with `manifold.exe` and its work dir.
        const fn work(name: &'static str, f: Body) -> Self {
```

Replace the dispatch arm:

```rust
            Some(Stage { name, run: Run::Work(f), .. }) => {
                match crate::probes::setup(rest.first().map(PathBuf::from), name) {
                    Ok((exe, work)) => f(&exe, &work),
                    Err(code) => code,
                }
            }
```

with:

```rust
            Some(Stage { name, run: Run::Work(f), .. }) => run_stage(name, rest.first().map(PathBuf::from), true, *f),
```

Directly above `fn launch(exe: &Path, work: &Path) -> std::io::Result<Instance> {`, add:

```rust
    /// What a stage body gets: `manifold.exe` (empty when the stage does not need it), its fresh work dir, and the
    /// timing marks (empty: the first `marks.mark(name)` opens a section).
    pub(crate) struct Ctx {
        pub exe: PathBuf,
        pub work: PathBuf,
        pub marks: Marks,
    }

    /// Lines of each registered log a failed stage prints.
    const LOG_TAIL_LINES: usize = 20;

    /// The one frame of every stage that takes a work dir. Finds `manifold.exe` (when `needs_exe`) and makes the work
    /// dir, runs `body`, prints an error it returns, prints the registered logs if anything failed, keeps the dir after
    /// a failure (or with `KEEP` set) and deletes it otherwise, and ends with `{name}: ok` or
    /// `{name}: failed (kept {dir})`, the line `gate all` reports.
    pub(crate) fn run_stage(
        name: &str,
        parent: Option<PathBuf>,
        needs_exe: bool,
        body: impl FnOnce(&mut Ctx) -> io::Result<()>,
    ) -> ExitCode {
        let exe = if needs_exe {
            match manifold_exe() {
                Ok(exe) => exe,
                Err(e) => return setup_failed(name, &e),
            }
        } else {
            PathBuf::new()
        };
        let work = match work_dir(parent, name) {
            Ok(w) => w,
            Err(e) => return setup_failed(name, &format!("work dir: {e}")),
        };
        let mut ctx = Ctx { exe, work, marks: Marks::default() };
        let before = failures();
        let errored = if let Err(e) = body(&mut ctx) {
            println!("{}", early_stop_line(name, &e));
            note_failure();
            true
        } else {
            false
        };
        ctx.marks.end();
        let failed = stage_failed(errored, before, failures());
        // Only a failed run drains the registry (one process runs one stage); a passing run, e.g. a unit test, leaves it.
        let logs = if failed { take_logs() } else { Vec::new() };
        if failed {
            print!("{}", log_tails(&logs));
        }
        if !keep_dir(failed, std::env::var_os("KEEP").is_some()) {
            remove_dir_with_retries(&ctx.work);
        }
        println!("{}", closing_line(name, failed, &ctx.work));
        exit_code()
    }

    /// Setup failed before any dir existed: says why, then the stage's closing line.
    fn setup_failed(name: &str, e: &str) -> ExitCode {
        eprintln!("{e}");
        println!("{name}: failed");
        ExitCode::from(1)
    }

    /// Whether this stage failed: its body returned `Err`, or the process-wide failure count rose while the body ran.
    /// Comparing before and after keeps a failure noted earlier in the process (a unit test that fails a check on
    /// purpose) from failing every later stage; `gate`'s exit code still reads the global count.
    fn stage_failed(errored: bool, before: u32, after: u32) -> bool {
        errored || after > before
    }

    /// Whether the work dir stays: after a failure, or when `KEEP` is set.
    fn keep_dir(failed: bool, keep_env: bool) -> bool {
        failed || keep_env
    }

    /// The line for a body's error. Not a verdict line: `gate all` counts those words, and the error is counted by
    /// `note_failure`. The error text is masked too, since it may quote a log line or a child's output.
    fn early_stop_line(name: &str, e: &dyn Display) -> String {
        format!("ERROR: {name} stopped early: {}", no_verdict_words(&e.to_string()))
    }

    /// A stage's last line.
    fn closing_line(name: &str, failed: bool, work: &Path) -> String {
        if failed {
            format!("{name}: failed (kept {})", work.display())
        } else {
            format!("{name}: ok")
        }
    }

    /// `--- <path>` and the last `LOG_TAIL_LINES` lines of each non-empty log in `logs`, the verdict words masked.
    fn log_tails(logs: &[PathBuf]) -> String {
        let mut out = String::new();
        for log in logs.iter().filter(|p| std::fs::metadata(p).is_ok_and(|m| m.len() > 0)) {
            let _ = writeln!(out, "--- {}\n{}", log.display(), no_verdict_words(&log_tail(log, LOG_TAIL_LINES)));
        }
        out
    }

```

Run: `cargo check -p gate -j 8 --message-format short`
Expected: errors only. `Run::Work` now stores `Body`, so each of the 21 `Stage::work(...)` rows in `STAGES` is a type mismatch until its stage is converted in Step 5, and nothing else: no error in `instance.rs`, in `run_stage` or its helpers, or in `Ctx`. Any other error is a typo in Steps 3-4; fix it now, while the edit is small.

- [ ] **Step 5: Convert the 21 Work stages**

Cadence: run `cargo check -p gate -j 8 --message-format short` after every three or four files (it takes seconds; the full test and clippy are for Step 6). Each run's errors must shrink to the `STAGES` rows of the stages not yet converted and the unconverted files' own errors; a new error in a file just converted is a mistake in that file.

Every Work stage gets the same five edits:
1. Its signature `pub(crate) fn NAME(exe: &Path, work: &Path) -> ExitCode {` becomes `pub(crate) fn NAME(ctx: &mut Ctx) -> io::Result<()> {`.
2. The next line is a new `    let (exe, work) = (ctx.exe.as_path(), ctx.work.as_path());`. The body below then compiles unchanged.
3. Its closing `    finish(work)\n}` becomes `    Ok(())\n}`.
4. `finish` leaves its `crate::probes` import, and `use crate::windows_main::Ctx;` is added. `cargo fmt` orders imports.
5. Its unused `ExitCode`/`Path` imports go, and `use std::io;` is added where missing.

Exact per-file import edits:

| File | Std import edit | Probes import edit |
|---|---|---|
| `arrange.rs` | `use std::path::Path;\nuse std::process::ExitCode;` -> `use std::io;` | `use crate::probes::{finish, guarded, import_session, quit};` -> `use crate::probes::{guarded, import_session, quit};\nuse crate::windows_main::Ctx;` |
| `click_count.rs` | `use std::path::Path;\nuse std::process::ExitCode;` -> `use std::io;` | `use crate::probes::{finish, for_each_view, quit, tab_node, Site};` -> `use crate::probes::{for_each_view, quit, tab_node, Site};\nuse crate::windows_main::Ctx;` |
| `iframe_typing.rs` | `use std::path::Path;\nuse std::process::ExitCode;` -> `use std::io;` | `use crate::probes::{finish, for_each_view, quit, tab_node, Site};` -> `use crate::probes::{for_each_view, quit, tab_node, Site};\nuse crate::windows_main::Ctx;` |
| `labels.rs` | `use std::path::Path;\nuse std::process::ExitCode;` -> `use std::io;` | `use crate::probes::{finish, guarded, import_session, quit};` -> `use crate::probes::{guarded, import_session, quit};\nuse crate::windows_main::Ctx;` |
| `page_text.rs` | `use std::path::Path;\nuse std::process::ExitCode;` -> `use std::io;` | `use crate::probes::{finish, for_each_view, quit, tab_node, Site};` -> `use crate::probes::{for_each_view, quit, tab_node, Site};\nuse crate::windows_main::Ctx;` |
| `search.rs` | `use std::path::Path;\nuse std::process::ExitCode;` -> `use std::io;` | `use crate::probes::{finish, for_each_view, quit, tab_node, Site};` -> `use crate::probes::{for_each_view, quit, tab_node, Site};\nuse crate::windows_main::Ctx;` |
| `suggest_groups.rs` | `use std::path::Path;\nuse std::process::ExitCode;` -> `use std::io;` | `use crate::probes::{finish, guarded, import_session, quit};` -> `use crate::probes::{guarded, import_session, quit};\nuse crate::windows_main::Ctx;` |
| `surface.rs` | `use std::path::Path;\nuse std::process::ExitCode;` -> `use std::io;` | `use crate::probes::{finish, guarded, quit, seed};` -> `use crate::probes::{guarded, quit, seed};\nuse crate::windows_main::Ctx;` |
| `download.rs` | `use std::process::{Command, ExitCode, Stdio};` -> `use std::process::{Command, Stdio};` | `use crate::probes::{finish, guarded, quit, seed, tab_node, views};` -> `use crate::probes::{guarded, quit, seed, tab_node, views};\nuse crate::windows_main::Ctx;` |
| `focus.rs` | `use std::process::{Command, ExitCode, Stdio};` -> `use std::process::{Command, Stdio};` | `use crate::probes::{finish, guarded, quit, seed};` -> `use crate::probes::{guarded, quit, seed};\nuse crate::windows_main::Ctx;` |
| `sleep.rs` | `use std::process::{Command, ExitCode, Stdio};` -> `use std::process::{Command, Stdio};` | `use crate::probes::{finish, guarded, quit, seed, tab_node, views, Site};` -> `use crate::probes::{guarded, quit, seed, tab_node, views, Site};\nuse crate::windows_main::Ctx;` |
| `jar.rs` | delete `use std::process::ExitCode;` | `use crate::probes::{default_jar_id, default_jar_node, finish, guarded, import_session};` -> `use crate::probes::{default_jar_id, default_jar_node, guarded, import_session};\nuse crate::windows_main::Ctx;` |
| `shots.rs` | delete `use std::process::ExitCode;` | `use crate::probes::{finish, import_session, quit};` -> `use crate::probes::{import_session, quit};\nuse crate::windows_main::Ctx;` |
| `stress.rs` (only `unresponsive`; `scale` is Task 8) | none (`ExitCode` stays for `scale`) | add `use crate::windows_main::Ctx;` after `use crate::state::{State, TAB};` (`finish` and `setup` stay for `scale`) |
| `probes.rs` | none (`ExitCode` stays for `setup`/`finish` until Task 8) | `use crate::windows_main::{manifold_exe, work_dir};` -> `use crate::windows_main::{manifold_exe, work_dir, Ctx};`; `use crate::instance::{remove_dir_with_retries, seed_settings, Instance, QUIT_TIMEOUT, STEP_TIMEOUT};` -> `use crate::instance::{register_log, remove_dir_with_retries, seed_settings, Instance, QUIT_TIMEOUT, STEP_TIMEOUT};` |

Signatures to replace (edit 1 and 2 above). The line to add after each is always `    let (exe, work) = (ctx.exe.as_path(), ctx.work.as_path());`:
- `arrange.rs`: `pub(crate) fn arrange(exe: &Path, work: &Path) -> ExitCode {`
- `click_count.rs`: `pub(crate) fn click_count(exe: &Path, work: &Path) -> ExitCode {`
- `iframe_typing.rs`: `pub(crate) fn iframe_typing(exe: &Path, work: &Path) -> ExitCode {`
- `labels.rs`: `pub(crate) fn labels(exe: &Path, work: &Path) -> ExitCode {`
- `page_text.rs`: `pub(crate) fn page_text(exe: &Path, work: &Path) -> ExitCode {`
- `search.rs`: `pub(crate) fn search(exe: &Path, work: &Path) -> ExitCode {`
- `suggest_groups.rs`: `pub(crate) fn suggest_groups(exe: &Path, work: &Path) -> ExitCode {`
- `surface.rs`: `pub(crate) fn surface(exe: &Path, work: &Path) -> ExitCode {`
- `download.rs`: `pub(crate) fn download(exe: &Path, work: &Path) -> ExitCode {`
- `focus.rs`: `pub(crate) fn focus_loss(exe: &Path, work: &Path) -> ExitCode {`
- `sleep.rs`: `pub(crate) fn sleep(exe: &Path, work: &Path) -> ExitCode {`
- `jar.rs`: `pub(crate) fn jar(exe: &Path, work: &Path) -> ExitCode {`
- `stress.rs`: `pub(crate) fn unresponsive(exe: &Path, work: &Path) -> ExitCode {`
- `probes.rs`: `pub(crate) fn page_kind(exe: &Path, work: &Path) -> ExitCode {`, `pub(crate) fn hscroll(exe: &Path, work: &Path) -> ExitCode {`, `pub(crate) fn key(exe: &Path, work: &Path) -> ExitCode {`, `pub(crate) fn bad_url(exe: &Path, work: &Path) -> ExitCode {` and `pub(crate) fn failed_spawn(exe: &Path, work: &Path) -> ExitCode {`.
- `shots.rs`: `pub(crate) fn chrome_shots(exe: &Path, work: &Path) -> ExitCode {`, `pub(crate) fn shots_aa(exe: &Path, work: &Path) -> ExitCode {` and `pub(crate) fn chrome_shots_sizes(exe: &Path, work: &Path) -> ExitCode {`.

Endings (edit 3):
- In `probes.rs` and `shots.rs` replace every `    finish(work)\n}` (5 and 3 occurrences, `replace_all`) with `    Ok(())\n}`.
- In every other listed file replace its one `    finish(work)\n}` with `    Ok(())\n}`.
- `stress.rs`'s `    finish(&work)\n}` (in `scale`) stays.

The early returns become errors:
- `probes.rs` (`page_kind`), `page_text.rs` and `search.rs` each have:

  ```rust
      let site = match Site::start() {
          Ok(s) => s,
          Err(e) => {
              eprintln!("site: {e}");
              return ExitCode::from(1);
          }
      };
  ```

  Replace it with:

  ```rust
      let site = Site::start().map_err(|e| io::Error::other(format!("site: {e}")))?;
  ```

- `focus.rs`: replace

  ```rust
      let Some(script) = exe.ancestors().nth(3).map(|root| root.join("scripts").join("foreground.ps1")) else {
          eprintln!("cannot find the repo root from {}", exe.display());
          return ExitCode::from(1);
      };
  ```

  with

  ```rust
      let script = exe
          .ancestors()
          .nth(3)
          .map(|root| root.join("scripts").join("foreground.ps1"))
          .ok_or_else(|| io::Error::other(format!("cannot find the repo root from {}", exe.display())))?;
  ```

- `jar.rs`: replace

  ```rust
      let server = match cookie_server(log.clone()) {
          Ok(s) => s,
          Err(e) => {
              eprintln!("cookie server: {e}");
              return ExitCode::from(1);
          }
      };
  ```

  with `    let server = cookie_server(log.clone()).map_err(|e| io::Error::other(format!("cookie server: {e}")))?;`. Then replace

  ```rust
      if let Err(e) = seed_settings(&profile, "webview2", &more) {
          eprintln!("settings: {e}");
          return ExitCode::from(1);
      }
  ```

  with `    seed_settings(&profile, "webview2", &more).map_err(|e| io::Error::other(format!("settings: {e}")))?;`.

`bad_url` registers its host logs (and the 15-line dump goes). In `probes.rs` replace `host_logs`:

```rust
/// The host logs (`manifold-<tab>.log`) the case's own TEMP holds.
fn host_logs(tmp: &Path) -> Vec<String> {
    fs::read_dir(tmp)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            e.file_name().to_string_lossy().starts_with("manifold-") && e.path().extension().is_some_and(|x| x == "log")
        })
        .filter_map(|e| fs::read_to_string(e.path()).ok())
        .collect()
}
```

with:

```rust
/// The host logs (`manifold-<tab>.log`) the case's own TEMP holds.
fn host_logs(tmp: &Path) -> Vec<PathBuf> {
    fs::read_dir(tmp)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            e.file_name().to_string_lossy().starts_with("manifold-") && e.path().extension().is_some_and(|x| x == "log")
        })
        .map(|e| e.path())
        .collect()
}
```

and in `bad_url` replace

```rust
                quit(&mut inst, &label);
                let logs = host_logs(&tmp);
```

with

```rust
                quit(&mut inst, &label);
                let paths = host_logs(&tmp);
                paths.iter().for_each(|p| register_log(p));
                let logs: Vec<String> = paths.iter().filter_map(|p| fs::read_to_string(p).ok()).collect();
```

and delete the dump:

```rust
                if failures() > 0 {
                    println!("--- {label} host log");
                    for l in &logs {
                        println!(
                            "{}",
                            l.lines()
                                .rev()
                                .take(15)
                                .collect::<Vec<_>>()
                                .into_iter()
                                .rev()
                                .collect::<Vec<_>>()
                                .join("\n")
                        );
                    }
                }
```

`failures` stays imported in `probes.rs`, since `finish` still uses it until Task 8.

`shots.rs`: the config is a body error, and `run` drops its own tail and says `ERROR:`. Replace

```rust
/// A `chrome-shots` run from `SIZE`, `VIEWS` and `CROP_PX`, into `out`; `None` (and said) when `SIZE` isn't "W H".
fn config(out: PathBuf) -> Option<Config> {
    let Some(size) = parse_size(&std::env::var("SIZE").unwrap_or_else(|_| "1280 800".into())) else {
        eprintln!("SIZE is \"W H\"");
        return None;
    };
    let views = std::env::var("VIEWS").unwrap_or_else(|_| DEFAULT_VIEWS.into());
    Some(Config {
```

with

```rust
/// A `chrome-shots` run from `SIZE`, `VIEWS` and `CROP_PX`, into `out`; an error when `SIZE` isn't "W H".
fn config(out: PathBuf) -> io::Result<Config> {
    let size = parse_size(&std::env::var("SIZE").unwrap_or_else(|_| "1280 800".into()))
        .ok_or_else(|| io::Error::other("SIZE is \"W H\""))?;
    let views = std::env::var("VIEWS").unwrap_or_else(|_| DEFAULT_VIEWS.into());
    Ok(Config {
```

Then make these replacements:
- In `chrome_shots`, replace

  ```rust
      let Some(cfg) = config(std::env::var_os("OUT").map_or_else(|| default_out(exe, "shots"), PathBuf::from)) else {
          return ExitCode::from(2);
      };
  ```

  with `    let cfg = config(std::env::var_os("OUT").map_or_else(|| default_out(exe, "shots"), PathBuf::from))?;`.
- In `shots_aa`, replace `        let Some(cfg) = config(out.join(leaf)) else { return ExitCode::from(2) };` with `        let cfg = config(out.join(leaf))?;`.
- In `run`, replace

  ```rust
      let result = Shots::start(exe, work, cfg).and_then(|mut s| {
          let result = s.run_views(exe);
          if result.is_err() {
              println!("--- tail of {}\n{}", s.inst.log.display(), s.inst.log_tail(30));
          }
          result
      });
      if let Err(e) = result {
          println!("FAIL: chrome-shots stopped early: {e}");
  ```

  with

  ```rust
      // Not a body error: the sheet below is still written. Not a verdict line either (gate all counts those words);
      // the failure is counted here, and run_stage prints the instance's log.
      let result = Shots::start(exe, work, cfg).and_then(|mut s| s.run_views(exe));
      if let Err(e) = result {
          println!("ERROR: chrome-shots stopped early: {e}");
  ```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p gate -j 8 -- launch_registers a_failed_run_or_keep the_early_stop log_tails_mask a_setup_failure a_stage_fails_only a_passing_body stage_names_are_unique`
Expected: `test result: ok. 8 passed`.

Run: `cargo test -p gate -j 8`
Expected: `test result: ok.`, 0 failed.

Run: `git grep -n -E 'finish\(work\)|fn [a-z_]+\(exe: &Path, work: &Path\) -> ExitCode' -- crates/gate`
Expected: no output. The remaining `finish(&work)` calls are in `stress.rs` and `package.rs`, which Task 8 converts.

Then the manual failure path through a body `Err` (the unit tests cannot reach it: a body that fails here would raise the process-wide count and the exit code reads it). `chrome-shots` with a bad `SIZE` fails in its body before any instance starts. It needs a fresh `manifold.exe`:

```bash
cargo build -j 8 -p manifold -p gate
SIZE=bad target/debug/gate.exe chrome-shots; echo "exit $?"
```

Expected:
- `ERROR: chrome-shots stopped early: SIZE is "W H"`.
- The last line `chrome-shots: failed (kept <TEMP>\gate-runs\chrome-shots-<pid>)`; no `PASS`/`FAIL` in any line.
- `exit 1`.
- The kept dir exists and is empty (spec behaviour change 8). Delete it by hand.

Also confirm the success path's exit code once: `target/debug/gate.exe dblprobe; echo "exit $?"` is slow, so use Task 10 Step 6's `dblprobe` run for that (it must exit 0 and end in `dblprobe: ok`).

- [ ] **Step 7: Format, lint, commit**

```bash
cargo fmt --all
cargo clippy -j 8 -p manifold -p manifold-hosts -p manifold-view-protocol -p manifold-pages -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings
git diff --stat
git add crates/gate/src
git commit -m "gate: run_stage frames the 21 Work stages; logs registered at launch and tailed on failure; failed runs keep their dir" -m "Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

Expected: no clippy warning. If clippy reports an unused `exe` or `work` binding in a converted stage, drop that half of the `let (exe, work)` line. No stage at `7e6010d5` is expected to need this.

---

### Task 3: launch-forms and layout on run_stage (and LAYOUT_KEEP goes)

**Files:**
- Modify: `crates/gate/src/launch_forms.rs`, `crates/gate/src/layout.rs`, `crates/gate/src/main.rs` (two `STAGES` rows)

**Interfaces:**
- Consumes (Task 2):
  - `Ctx`.
  - `register_log(path: &Path)`.
  - `Stage::work(name, Body)`.
- Produces:
  - `pub(crate) fn launch_forms(ctx: &mut Ctx) -> io::Result<()>`.
  - `pub(crate) fn layout(ctx: &mut Ctx) -> io::Result<()>`.

- [ ] **Step 1: launch-forms**

In `crates/gate/src/launch_forms.rs` replace the imports:

```rust
use std::fs::{self, File};
use std::io::Write;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::check::{check_eq, exit_code, failures, Section};
use crate::instance::{job, remove_dir_with_retries, BELOW_NORMAL};
use crate::windows_main::{manifold_exe, work_dir};
```

with:

```rust
use std::fs::{self, File};
use std::io::{self, Write};
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::check::check_eq;
use crate::instance::{job, register_log, BELOW_NORMAL};
use crate::windows_main::Ctx;
```

In `run`, replace

```rust
    let open = |ext: &str| File::create(work.join(format!("{name}.{ext}")));
    let (Ok(out), Ok(err)) = (open("out"), open("err")) else {
        return check_eq(&format!("{name} exits {want}"), want, -1);
    };
```

with

```rust
    let path = |ext: &str| work.join(format!("{name}.{ext}"));
    let (Ok(out), Ok(err)) = (File::create(path("out")), File::create(path("err"))) else {
        return check_eq(&format!("{name} exits {want}"), want, -1);
    };
    register_log(&path("out"));
    register_log(&path("err"));
```

Delete `tail` entirely:

```rust
fn tail(path: &Path) -> String {
    let text = fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(20)..].join("\n")
}

```

Replace the whole `pub(crate) fn launch_forms(parent: Option<PathBuf>) -> ExitCode { ... }`, from its signature to its closing `}`, with:

```rust
pub(crate) fn launch_forms(ctx: &mut Ctx) -> io::Result<()> {
    ctx.marks.mark("setup");
    let (exe, work) = (&ctx.exe, &ctx.work);
    let session = work.join("session.json");
    let quit = work.join("quit.txt");
    let mnf = work.join("s.mnf");
    fs::write(&session, r#"{"nodes":[{"id":"g","type":"application/vnd.manifold.group","name":"G"}]}"#)?;
    fs::write(&quit, "QUIT\n")?;
    fs::create_dir_all(work.join("p"))?;
    fs::create_dir_all(work.join("bad"))?;
    fs::create_dir_all(work.join("drive").join("appdata").join("manifold"))?;
    let manifold = || Command::new(exe);

    let drive_appdata = work.join("drive").join("appdata");
    let mut import_for_drive = manifold();
    import_for_drive
        .arg("--import-json")
        .arg(&session)
        .arg("--out")
        .arg(drive_appdata.join("manifold").join("session.mnf"));
    run(work, "drive-import", 0, import_for_drive, None);
    ctx.marks.mark("drive");
    let mut headless = manifold();
    headless.arg("--headless").env("APPDATA", &drive_appdata);
    run(work, "drive", 0, headless, Some("QUIT\n"));

    ctx.marks.mark("script");
    let mut c = manifold();
    c.arg("--headless").arg("--profile-dir").arg(work.join("p")).arg("--script").arg(&quit);
    run(work, "script", 0, c, None);
    ctx.marks.mark("tools");
    let mut c = manifold();
    c.arg("--import-json").arg(&session).arg("--out").arg(&mnf);
    run(work, "import", 0, c, None);
    for tool in ["dump", "check"] {
        let mut c = manifold();
        c.arg(format!("--{tool}")).arg(&mnf);
        run(work, tool, 0, c, None);
    }
    ctx.marks.mark("refused");
    let mut c = manifold();
    c.arg("--headless").arg("--profile-dir").arg(work.join("bad")).arg("--hedless");
    run(work, "refused", 2, c, None);
    let named =
        fs::read_to_string(work.join("refused.err")).unwrap_or_default().contains(r#"unknown argument "--hedless""#);
    check_eq("refused names the argument", 1, i32::from(named));
    Ok(())
}
```

(`&ctx.exe`/`&ctx.work` and `ctx.marks` are disjoint field borrows. If the borrow checker objects because `exe`/`work` are captured across `ctx.marks.mark`, clone them: `let (exe, work) = (ctx.exe.clone(), ctx.work.clone());` and pass `&work`.)

- [ ] **Step 2: layout**

In `crates/gate/src/layout.rs` replace the imports:

```rust
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::Value;

use crate::check::{check, exit_code, failures, note_failure, Section};
use crate::instance::{seed_settings, Instance, QUIT_TIMEOUT};
use crate::state::{State, GROUP, NOTE, TAB};
use crate::windows_main::{manifold_exe, work_dir};
```

with:

```rust
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::check::{check, Section};
use crate::instance::{seed_settings, Instance, QUIT_TIMEOUT};
use crate::state::{State, GROUP, NOTE, TAB};
use crate::windows_main::Ctx;
```

Replace the `LAYOUT_KEEP` block:

```rust
            // Kept for a look: the run's work dir is deleted unless KEEP is set.
            if let Some(keep) = std::env::var_os("LAYOUT_KEEP") {
                let dir = PathBuf::from(keep);
                let _ = std::fs::create_dir_all(&dir);
                let stem = format!("{width}x{height}-{}", sc.name);
                let _ = std::fs::copy(&shot.over_cards, dir.join(format!("{stem}-cards.png")));
                let _ = std::fs::copy(&bare, dir.join(format!("{stem}-bare.png")));
            }
```

with:

```rust
            // Both PNGs stay in the work dir, which a failed run keeps.
```

Delete the per-group tail in `run`:

```rust
            if failures() > 0 {
                println!(
                    "--- tail of {}
{}",
                    inst.log.display(),
                    inst.log_tail(30)
                );
            }
```

Replace the whole `pub(crate) fn layout(parent: Option<PathBuf>) -> ExitCode { ... }` with:

```rust
pub(crate) fn layout(ctx: &mut Ctx) -> io::Result<()> {
    ctx.marks.mark("setup");
    ctx.marks.mark("layout");
    run(&ctx.exe, &ctx.work)
}
```

- [ ] **Step 3: The STAGES rows**

In `crates/gate/src/main.rs` replace `        Stage::parent("launch-forms", crate::launch_forms::launch_forms),` with `        Stage::work("launch-forms", crate::launch_forms::launch_forms),` and `        Stage::parent("layout", crate::layout::layout),` with `        Stage::work("layout", crate::layout::layout),`.

- [ ] **Step 4: Check**

Run: `cargo test -p gate -j 8 stage_names_are_unique`
Expected: `test result: ok. 1 passed`.

Run: `git grep -n LAYOUT_KEEP -- crates`
Expected: no output.

- [ ] **Step 5: Format, lint, commit**

```bash
cargo fmt --all
git diff --stat
git add crates/gate/src/launch_forms.rs crates/gate/src/layout.rs crates/gate/src/main.rs
git commit -m "gate: launch-forms and layout run on run_stage; LAYOUT_KEEP goes (the kept dir holds the PNGs)" -m "Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

Expected: the pre-commit hook passes; three files changed.

---

### Task 4: link and unknown-type on run_stage, seeding through probes::seed

**Files:**
- Modify: `crates/gate/src/link.rs`, `crates/gate/src/unknown_type.rs`, `crates/gate/src/main.rs` (two `STAGES` rows)

**Interfaces:**
- Consumes:
  - `Ctx`.
  - `Marks::mark`.
  - `crate::probes::seed(exe: &Path, profile: &Path, view_kind: &str, nodes: &[Value]) -> io::Result<()>`.
- Produces:
  - `pub fn link(ctx: &mut Ctx) -> io::Result<()>`.
  - `pub fn unknown_type(ctx: &mut Ctx) -> io::Result<()>`.
  - `link::bad` and `link::num` are unchanged for their importers.

- [ ] **Step 1: link**

In `crates/gate/src/link.rs` replace the imports:

```rust
use std::io;
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};

use serde_json::Value;

use crate::check::{check_eq, exit_code, failures, no_verdict_words, note_failure, Section};
use crate::instance::{log_tail, seed_settings, Instance, QUIT_TIMEOUT};
use crate::state::{round_half_even, State};
use crate::windows_main::{manifold_exe, work_dir};
```

with:

```rust
use std::io;
use std::path::PathBuf;

use serde_json::Value;

use crate::check::{check_eq, Marks};
use crate::instance::{Instance, QUIT_TIMEOUT};
use crate::probes::seed;
use crate::state::{round_half_even, State};
use crate::windows_main::Ctx;
```

Replace `Run::setup`:

```rust
    /// The setup: the profile's settings and the session built from the seed (`--import-json`).
    fn setup(&self) -> io::Result<()> {
        seed_settings(&self.profile, "webview2", "")?;
        let seed = self.profile.join("seed.json");
        let tab = |id: &str, x: f64, y: f64| {
            serde_json::json!({"id": id, "type": "application/vnd.manifold.tab", "url": "about:blank",
                "view_kind": "webview2", "pos": [x, y]})
        };
        let doc = serde_json::json!({"nodes": [tab("t1", 0.0, 0.0), tab("t2", 400.0, 0.0), tab("t3", 0.0, 400.0)]});
        std::fs::write(&seed, doc.to_string())?;
        let status = Command::new(&self.exe)
            .arg("--import-json")
            .arg(&seed)
            .arg("--out")
            .arg(self.profile.join("session.mnf"))
            .stdout(Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!("--import-json exited with {status}")))
        }
    }
```

with:

```rust
    /// The setup: the profile's settings and the session built from three tabs (`probes::seed`).
    fn setup(&self) -> io::Result<()> {
        let tab = |id: &str, x: f64, y: f64| {
            serde_json::json!({"id": id, "type": "application/vnd.manifold.tab", "url": "about:blank",
                "view_kind": "webview2", "pos": [x, y]})
        };
        seed(&self.exe, &self.profile, "webview2", &[tab("t1", 0.0, 0.0), tab("t2", 400.0, 0.0), tab("t3", 0.0, 400.0)])
    }
```

Replace

```rust
fn drive(run: &Run, sec: &mut Option<Section>) -> io::Result<Exports> {
    let mut next = |name: &str| *sec = sec.take().map(|s| s.next(name));
```

with

```rust
fn drive(run: &Run, m: &mut Marks) -> io::Result<Exports> {
    let mut next = |name: &str| m.mark(name);
```

Replace the whole `pub fn link(parent: Option<PathBuf>) -> ExitCode { ... }` with:

```rust
pub fn link(ctx: &mut Ctx) -> io::Result<()> {
    ctx.marks.mark("setup");
    let run = Run { exe: ctx.exe.clone(), profile: ctx.work.join("profile"), work: ctx.work.clone() };
    run.setup()?;
    let exports = drive(&run, &mut ctx.marks)?;
    ctx.marks.mark("verdicts");
    verdicts(&exports)
}
```

- [ ] **Step 2: unknown-type**

In `crates/gate/src/unknown_type.rs` replace the imports:

```rust
use std::io;
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};

use serde_json::Value;

use crate::check::{check, check_eq, exit_code, failures, no_verdict_words, note_failure, Section};
use crate::instance::{log_tail, seed_settings, Instance, QUIT_TIMEOUT};
use crate::link::{bad, num};
use crate::state::{State, GROUP, TAB};
use crate::windows_main::{manifold_exe, work_dir};
```

with:

```rust
use std::io;
use std::path::PathBuf;

use serde_json::Value;

use crate::check::{check, check_eq, Marks};
use crate::instance::{Instance, QUIT_TIMEOUT};
use crate::link::{bad, num};
use crate::probes::seed;
use crate::state::{State, GROUP, TAB};
use crate::windows_main::Ctx;
```

Replace `Run::setup`:

```rust
    /// The profile's settings and the session built from the seed (`--import-json`, which gives fresh ids).
    fn setup(&self) -> io::Result<()> {
        seed_settings(&self.profile, "webview2", "")?;
        let seed = self.profile.join("seed.json");
        let doc = serde_json::json!({"nodes": [
            {"id": "t", "type": TAB, "url": "about:blank", "view_kind": "webview2", "pos": [0.0, 0.0]},
            {"id": "g", "type": GROUP, "name": "G", "pos": [300.0, 300.0]},
            {"id": "u", "type": FUTURE, "pos": [400.0, 0.0]},
        ]});
        std::fs::write(&seed, doc.to_string())?;
        let status = Command::new(&self.exe)
            .arg("--import-json")
            .arg(&seed)
            .arg("--out")
            .arg(self.profile.join("session.mnf"))
            .stdout(Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!("--import-json exited with {status}")))
        }
    }
```

with:

```rust
    /// The profile's settings and the session built from the seed (`probes::seed`; the import gives fresh ids).
    fn setup(&self) -> io::Result<()> {
        let nodes = [
            serde_json::json!({"id": "t", "type": TAB, "url": "about:blank", "view_kind": "webview2", "pos": [0.0, 0.0]}),
            serde_json::json!({"id": "g", "type": GROUP, "name": "G", "pos": [300.0, 300.0]}),
            serde_json::json!({"id": "u", "type": FUTURE, "pos": [400.0, 0.0]}),
        ];
        seed(&self.exe, &self.profile, "webview2", &nodes)
    }
```

Replace

```rust
fn drive(run: &Run, sec: &mut Option<Section>) -> io::Result<()> {
    let mut next = |name: &str| *sec = sec.take().map(|s| s.next(name));
```

with

```rust
fn drive(run: &Run, m: &mut Marks) -> io::Result<()> {
    let mut next = |name: &str| m.mark(name);
```

Replace the whole `pub fn unknown_type(parent: Option<PathBuf>) -> ExitCode { ... }` with:

```rust
pub fn unknown_type(ctx: &mut Ctx) -> io::Result<()> {
    ctx.marks.mark("setup");
    let run = Run { exe: ctx.exe.clone(), profile: ctx.work.join("profile"), work: ctx.work.clone() };
    run.setup()?;
    drive(&run, &mut ctx.marks)
}
```

- [ ] **Step 3: STAGES rows**

In `crates/gate/src/main.rs` replace `        Stage::parent("link", crate::link::link),` with `        Stage::work("link", crate::link::link),` and `        Stage::parent("unknown-type", crate::unknown_type::unknown_type),` with `        Stage::work("unknown-type", crate::unknown_type::unknown_type),`.

- [ ] **Step 4: Check**

Run: `cargo test -p gate -j 8 -- link:: unknown_type:: stage_names_are_unique`
Expected: `test result: ok.`, 0 failed (link's and unknown_type's own unit tests plus the STAGES test).

- [ ] **Step 5: Format, lint, commit**

```bash
cargo fmt --all
git diff --stat
git add crates/gate/src/link.rs crates/gate/src/unknown_type.rs crates/gate/src/main.rs
git commit -m "gate: link and unknown-type run on run_stage and seed through probes::seed" -m "Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

Expected: the pre-commit hook passes; three files changed.

---

### Task 5: closed and dblprobe on run_stage

**Files:**
- Modify: `crates/gate/src/closed.rs`, `crates/gate/src/dblprobe.rs`, `crates/gate/src/main.rs` (two `STAGES` rows)

**Interfaces:**
- Consumes:
  - `Ctx`.
  - `Marks::mark`.
  - `probes::seed`.
- Produces:
  - `pub fn closed(ctx: &mut Ctx) -> io::Result<()>`.
  - `pub fn dblprobe(ctx: &mut Ctx) -> io::Result<()>`.

- [ ] **Step 1: closed**

In `crates/gate/src/closed.rs` replace the imports:

```rust
use std::io;
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};

use serde_json::Value;

use crate::check::{check, check_is, exit_code, failures, no_verdict_words, note_failure, Section};
use crate::instance::{log_tail, seed_settings, Instance, QUIT_TIMEOUT};
use crate::link::bad;
use crate::state::{State, GROUP, NOTE};
use crate::windows_main::{manifold_exe, work_dir};
```

with:

```rust
use std::io;
use std::path::PathBuf;

use serde_json::Value;

use crate::check::{check, check_is, Marks};
use crate::instance::{Instance, QUIT_TIMEOUT};
use crate::link::bad;
use crate::probes::seed;
use crate::state::{State, GROUP, NOTE};
use crate::windows_main::Ctx;
```

In `Run::setup` replace

```rust
    fn setup(&self) -> io::Result<()> {
        seed_settings(&self.profile, "webview2", "")?;
        let seed = self.profile.join("seed.json");
        let mut nodes = vec![
```

with

```rust
    fn setup(&self) -> io::Result<()> {
        let mut nodes = vec![
```

and replace

```rust
        std::fs::write(&seed, serde_json::json!({ "nodes": nodes }).to_string())?;
        let status = Command::new(&self.exe)
            .arg("--import-json")
            .arg(&seed)
            .arg("--out")
            .arg(self.profile.join("session.mnf"))
            .stdout(Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!("--import-json exited with {status}")))
        }
    }
```

with

```rust
        seed(&self.exe, &self.profile, "webview2", &nodes)
    }
```

Replace its doc line ``    /// The profile's settings and the session built from the seed (`--import-json`, which gives fresh ids; a`` with ``    /// The profile's settings and the session built from the seed (`probes::seed`, which gives fresh ids; a``.

Replace

```rust
fn drive(run: &Run, sec: &mut Option<Section>) -> io::Result<()> {
    let mut next = |name: &str| *sec = sec.take().map(|s| s.next(name));
```

with

```rust
fn drive(run: &Run, m: &mut Marks) -> io::Result<()> {
    let mut next = |name: &str| m.mark(name);
```

Replace the whole `pub fn closed(parent: Option<PathBuf>) -> ExitCode { ... }` with:

```rust
pub fn closed(ctx: &mut Ctx) -> io::Result<()> {
    ctx.marks.mark("setup");
    let run = Run { exe: ctx.exe.clone(), profile: ctx.work.join("profile"), work: ctx.work.clone() };
    run.setup()?;
    drive(&run, &mut ctx.marks)
}
```

- [ ] **Step 2: dblprobe**

In `crates/gate/src/dblprobe.rs` replace the imports:

```rust
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use crate::check::{check, exit_code, failures, note_failure, Section};
use crate::instance::{seed_settings, Instance, QUIT_TIMEOUT};
use crate::link::bad;
use crate::state::{GROUP, NOTE};
use crate::windows_main::{manifold_exe, work_dir};
```

with:

```rust
use std::io;
use std::path::Path;

use serde_json::json;

use crate::check::check;
use crate::instance::{Instance, QUIT_TIMEOUT};
use crate::link::bad;
use crate::probes::seed;
use crate::state::{GROUP, NOTE};
use crate::windows_main::Ctx;
```

In `drive` replace

```rust
    let profile = work.join("profile");
    seed_settings(&profile, "webview2", "")?;
    let seed = profile.join("seed.json");
    let nodes = serde_json::json!([
        {"id": "g", "type": GROUP, "name": "G", "pos": [0.0, 0.0]},
        {"id": "in", "type": NOTE, "name": "inside", "text": "inside", "group": "g", "pos": [0.0, 0.0]},
        {"id": "n", "type": NOTE, "name": "n", "text": "outside", "pos": [400.0, 0.0]},
    ]);
    std::fs::write(&seed, serde_json::json!({ "nodes": nodes }).to_string())?;
    let status = Command::new(exe)
        .arg("--import-json")
        .arg(&seed)
        .arg("--out")
        .arg(profile.join("session.mnf"))
        .stdout(Stdio::null())
        .status()?;
    if !status.success() {
        return Err(bad(format!("--import-json exited with {status}")));
    }
```

with

```rust
    let profile = work.join("profile");
    let nodes = [
        json!({"id": "g", "type": GROUP, "name": "G", "pos": [0.0, 0.0]}),
        json!({"id": "in", "type": NOTE, "name": "inside", "text": "inside", "group": "g", "pos": [0.0, 0.0]}),
        json!({"id": "n", "type": NOTE, "name": "n", "text": "outside", "pos": [400.0, 0.0]}),
    ];
    seed(exe, &profile, "webview2", &nodes)?;
```

Replace the whole `pub fn dblprobe(parent: Option<PathBuf>) -> ExitCode { ... }` with:

```rust
pub fn dblprobe(ctx: &mut Ctx) -> io::Result<()> {
    ctx.marks.mark("dblprobe");
    drive(&ctx.exe, &ctx.work)
}
```

- [ ] **Step 3: STAGES rows**

In `crates/gate/src/main.rs` replace `        Stage::parent("closed", crate::closed::closed),` with `        Stage::work("closed", crate::closed::closed),` and `        Stage::parent("dblprobe", crate::dblprobe::dblprobe),` with `        Stage::work("dblprobe", crate::dblprobe::dblprobe),`.

- [ ] **Step 4: Check**

Run: `cargo test -p gate -j 8 -- closed:: stage_names_are_unique`
Expected: `test result: ok.`, 0 failed.

- [ ] **Step 5: Format, lint, commit**

```bash
cargo fmt --all
git diff --stat
git add crates/gate/src/closed.rs crates/gate/src/dblprobe.rs crates/gate/src/main.rs
git commit -m "gate: closed and dblprobe run on run_stage and seed through probes::seed" -m "Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

Expected: the pre-commit hook passes; three files changed.

---

### Task 6: trail on run_stage

**Files:**
- Modify: `crates/gate/src/trail.rs`, `crates/gate/src/main.rs` (one `STAGES` row)

**Interfaces:**
- Consumes:
  - `Ctx`.
  - `Marks::mark`.
  - `crate::probes::import_session(exe: &Path, profile: &Path, nodes: &[Value]) -> io::Result<()>`.
  - `crate::instance::seed_settings`.
- Produces: `pub fn trail(ctx: &mut Ctx) -> io::Result<()>`.

- [ ] **Step 1: Imports**

In `crates/gate/src/trail.rs` replace:

```rust
use std::io;
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};
use std::sync::Arc;

use serde_json::Value;

use crate::check::{check_is, exit_code, failures, no_verdict_words, note_failure, Section};
use crate::http::{response, Handler, Server};
use crate::instance::{log_tail, seed_settings, Instance, QUIT_TIMEOUT};
use crate::link::bad;
use crate::state::{State, TAB};
use crate::windows_main::{manifold_exe, work_dir};
```

with:

```rust
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;

use crate::check::{check_is, Marks};
use crate::http::{response, Server};
use crate::instance::{seed_settings, Instance, QUIT_TIMEOUT};
use crate::link::bad;
use crate::probes::import_session;
use crate::state::{State, TAB};
use crate::windows_main::Ctx;
```

- [ ] **Step 2: Run::setup through import_session**

Replace:

```rust
        seed_settings(&self.profile, &self.engine, &format!(", \"home_page\": \"{}/b1\"", self.base))?;
        let seed = self.profile.join("seed.json");
        let tab = serde_json::json!({"id": "a", "type": TAB, "url": format!("{}/a", self.base), "view_kind": self.engine,
            "pos": [0.0, 0.0]});
        std::fs::write(&seed, serde_json::json!({ "nodes": [tab] }).to_string())?;
        let status = Command::new(&self.exe)
            .arg("--import-json")
            .arg(&seed)
            .arg("--out")
            .arg(self.profile.join("session.mnf"))
            .stdout(Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!("--import-json exited with {status}")))
        }
```

with:

```rust
        seed_settings(&self.profile, &self.engine, &format!(", \"home_page\": \"{}/b1\"", self.base))?;
        let tab = serde_json::json!({"id": "a", "type": TAB, "url": format!("{}/a", self.base), "view_kind": self.engine,
            "pos": [0.0, 0.0]});
        import_session(&self.exe, &self.profile, &[tab])
```

- [ ] **Step 3: drive and the frame**

Replace

```rust
fn drive(run: &Run, sec: &mut Option<Section>) -> io::Result<()> {
    let mut next = |name: &str| *sec = sec.take().map(|s| s.next(name));
```

with

```rust
fn drive(run: &Run, m: &mut Marks) -> io::Result<()> {
    let mut next = |name: &str| m.mark(name);
```

Replace the whole `pub fn trail(parent: Option<PathBuf>) -> ExitCode { ... }` with:

```rust
pub fn trail(ctx: &mut Ctx) -> io::Result<()> {
    ctx.marks.mark("setup");
    fn page(line: &str) -> String {
        let path = line.split(' ').nth(1).unwrap_or("/").to_owned();
        let script = if path == "/back" { "<script>setTimeout(() => history.back(), 500)</script>" } else { "" };
        format!("<!doctype html><title>{path}</title><p>{path}</p>{script}")
    }
    let site = |e: io::Error| io::Error::other(format!("site: {e}"));
    let server =
        Server::start(Arc::new(move |line: &str| response("200 OK", &[], "text/html", page(line).as_bytes())))
            .map_err(site)?;
    // A second origin (another port) whose `/favicon.ico` is a red 16x16 PNG; the first origin's pages keep their tiles.
    let mut png = Vec::new();
    let red = image::RgbaImage::from_pixel(16, 16, image::Rgba([220, 40, 40, 255]));
    red.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).map_err(io::Error::other)?;
    let fav_server = Server::start(Arc::new(move |line: &str| {
        if line.split(' ').nth(1) == Some("/favicon.ico") {
            return response("200 OK", &[], "image/png", &png);
        }
        response("200 OK", &[], "text/html", page(line).as_bytes())
    }))
    .map_err(site)?;
    let run = Run {
        exe: ctx.exe.clone(),
        profile: ctx.work.join("profile"),
        work: ctx.work.clone(),
        base: server.base(),
        alias: fav_server.base(),
        engine: crate::probes::engine(),
    };
    run.setup()?;
    drive(&run, &mut ctx.marks)
}
```

- [ ] **Step 4: STAGES row**

In `crates/gate/src/main.rs` replace `        Stage::parent("trail", crate::trail::trail),` with `        Stage::work("trail", crate::trail::trail),`.

- [ ] **Step 5: Check**

Run: `cargo test -p gate -j 8 -- trail:: stage_names_are_unique`
Expected: `test result: ok.`, 0 failed.

- [ ] **Step 6: Format, lint, commit**

```bash
cargo fmt --all
git diff --stat
git add crates/gate/src/trail.rs crates/gate/src/main.rs
git commit -m "gate: trail runs on run_stage; its site start-up errors are body errors" -m "Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

Expected: the pre-commit hook passes; two files changed. The `page` fn item after a statement was already there before, and `clippy::items_after_statements` is pedantic, so it is not enabled.

---

### Task 7: smoke and two on run_stage (Marks::start goes)

**Files:**
- Modify: `crates/gate/src/smoke.rs`, `crates/gate/src/two/mod.rs`, `crates/gate/src/check.rs`, `crates/gate/src/main.rs` (two `STAGES` rows)

**Interfaces:**
- Consumes:
  - `Ctx`.
  - `Marks` (`Default`, `mark`, `end`).
  - `register_log`.
- Produces:
  - `pub fn smoke(ctx: &mut Ctx) -> io::Result<()>`.
  - `pub fn two(ctx: &mut Ctx) -> io::Result<()>`.
  - `Marks::start` no longer exists.

- [ ] **Step 1: smoke**

In `crates/gate/src/smoke.rs` make these replacements:
- Delete the line `use std::process::ExitCode;`.
- Replace `use crate::check::{exit_code, fail_line, failures, no_verdict_words, note_failure, pass_line, Marks};` with `use crate::check::{fail_line, failures, note_failure, pass_line, Marks};`.
- Replace `use crate::instance::{file_url, log_tail, remove_dir_with_retries, seed_settings, Instance, QUIT_TIMEOUT};` with `use crate::instance::{file_url, seed_settings, Instance, QUIT_TIMEOUT};`.
- Replace `use crate::windows_main::{manifold_exe, work_dir};` with `use crate::windows_main::Ctx;`.

Replace the whole `pub fn smoke(parent: Option<PathBuf>) -> ExitCode { ... }` with:

```rust
pub fn smoke(ctx: &mut Ctx) -> io::Result<()> {
    ctx.marks.mark("setup");
    let repo = repo_root(&ctx.exe).ok_or_else(|| bad(format!("no checkout found above {}", ctx.exe.display())))?;
    let out =
        std::env::var_os("HEADLESS_TEST_OUT").map_or_else(|| repo.join(r"target\headless-test-output"), PathBuf::from);
    fs::create_dir_all(&out)
        .and_then(|()| {
            for e in fs::read_dir(&out)?.flatten() {
                if e.path().extension().is_some_and(|x| x == "png" || x == "json") {
                    fs::remove_file(e.path())?;
                }
            }
            Ok(())
        })
        .map_err(|e| bad(format!("{}: {e}", out.display())))?;
    // A fresh profile of its own, so the real %APPDATA%\manifold is never read or written. `has_shown_first_launch_demo`
    // skips the demo scene, so the app starts with just the root tab.
    let profile = ctx.work.join("profile");
    seed_settings(&profile, "webview2", "")?;
    // Address-bar suggestions come from a local server, not DuckDuckGo, so those checks don't depend on the network.
    let server = Server::start()?;
    let url = server.url();
    let inst = Instance::launch_env(&ctx.exe, &profile, &out, "smoke.log", &[("MANIFOLD_SUGGESTIONS_URL", &url)])?;
    let mut sm = Smoke { inst, out: out.clone(), repo };
    drive(&mut sm, &mut ctx.marks)?;
    drop(sm); // kills the instance if it is still up
    drop(server);
    println!();
    if failures() == 0 {
        println!("All checks passed. Screenshots in {}", out.display());
    } else {
        println!("Some checks failed. Screenshots in {}", out.display());
    }
    Ok(())
}
```

- [ ] **Step 2: two**

In `crates/gate/src/two/mod.rs` make these replacements:
- Delete the line `use std::process::ExitCode;`.
- Replace `use crate::check::{check, exit_code, failures, no_verdict_words, note_failure, Marks};` with `use crate::check::{check, failures, no_verdict_words, Marks};`.
- Replace `use crate::instance::{log_tail, remove_dir_with_retries, seed_settings, wait_file, Instance, QUIT_TIMEOUT};` with `use crate::instance::{register_log, seed_settings, wait_file, Instance, QUIT_TIMEOUT};`.
- Replace `use crate::windows_main::{manifold_exe, work_dir};` with `use crate::windows_main::Ctx;`.

Replace the whole `pub fn two(parent: Option<PathBuf>) -> ExitCode { ... }` with:

```rust
pub fn two(ctx: &mut Ctx) -> io::Result<()> {
    // The script's first mark comes after its freshness check (run_stage's).
    ctx.marks.mark("setup");
    let (out, profile) = (ctx.work.join("out"), ctx.work.join("profile"));
    let view_kind = std::env::var("ENGINE").unwrap_or_else(|_| "webview2".to_string());
    fs::create_dir_all(&out)?;
    seed_settings(&profile, &view_kind, "")?;
    println!("two-instance test: exe {} profile {} engine {view_kind}", ctx.exe.display(), profile.display());
    // `Two` owns the marks for the run; the last section ends when it is dropped below.
    let marks = std::mem::take(&mut ctx.marks);
    let mut t = Two { exe: ctx.exe.clone(), profile, out, insts: [None, None, None, None], site: None, marks };
    let result = run(&mut t);
    // The nonce site's request counters, shown with the logs if the run failed.
    if let Some(site) = &t.site {
        let report = ctx.work.join("nonce-site.txt");
        if fs::write(&report, site.counters().report()).is_ok() {
            register_log(&report);
        }
    }
    drop(t); // ends the last mark, kills any instance still up and stops the site
    match result {
        // A check already failed and was counted.
        Ok(()) | Err(Halt::Failed) => Ok(()),
        Err(Halt::Error(e)) => Err(e),
    }
}
```

- [ ] **Step 3: Marks::start goes**

In `crates/gate/src/check.rs` remove:

```rust
    pub fn start(name: &str) -> Marks {
        Marks(Some(Section::start(name)))
    }

```

Run: `git grep -n 'Marks::start' -- crates`
Expected: no output.

- [ ] **Step 4: STAGES rows**

In `crates/gate/src/main.rs` replace `        Stage::parent("smoke", crate::smoke::smoke),` with `        Stage::work("smoke", crate::smoke::smoke),` and `        Stage::parent("two", crate::two::two),` with `        Stage::work("two", crate::two::two),`.

Run: `git grep -n 'Stage::parent' -- crates/gate`
Expected: exactly one line, `Stage::parent("hello", hello),` (the `const fn parent` definition has no `Stage::` prefix). `hold` is a struct literal with `Run::Parent`. Task 9 deletes all three.

- [ ] **Step 5: Check**

Run: `cargo test -p gate -j 8 -- smoke:: two:: a_mark_on_empty stage_names_are_unique`
Expected: `test result: ok.`, 0 failed.

- [ ] **Step 6: Format, lint, commit**

```bash
cargo fmt --all
git diff --stat
git add crates/gate/src/smoke.rs crates/gate/src/two/mod.rs crates/gate/src/check.rs crates/gate/src/main.rs
git commit -m "gate: smoke and two run on run_stage; their closing lines give way to run_stage's; Marks::start goes" -m "Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

Expected: the pre-commit hook passes; four files changed. If `PathBuf` becomes unused in `smoke.rs`, check with `grep -n PathBuf crates/gate/src/smoke.rs`; it is used by `repo_root` and `Smoke` at `7e6010d5`, so it should stay.

---

### Task 8: scale, package and install-check call run_stage; probes::setup and probes::finish go

**Files:**
- Modify: `crates/gate/src/stress.rs` (`scale`), `crates/gate/src/package.rs` (`package`, `install_check`), `crates/gate/src/probes.rs` (delete `setup`, `finish`, imports)

**Interfaces:**
- Consumes: `run_stage(name: &str, parent: Option<PathBuf>, needs_exe: bool, body: impl FnOnce(&mut Ctx) -> io::Result<()>) -> ExitCode`.
- Produces: the same `fn(&[String]) -> ExitCode` signatures for the three `Run::Args` stages; usage errors still exit 2 before any dir exists.

- [ ] **Step 1: scale**

In `crates/gate/src/stress.rs` replace `use crate::probes::{finish, guarded, quit, seed, setup, tab_node};` with `use crate::probes::{guarded, quit, seed, tab_node};`, and `use crate::windows_main::Ctx;` with `use crate::windows_main::{run_stage, Ctx};`. Then replace everything in `scale` after the usage check, that is from

```rust
    let (exe, work) = match setup(None, "scale") {
```

through the function's final `    finish(&work)\n}`, with:

```rust
    run_stage("scale", None, true, |ctx| {
        let (exe, work) = (ctx.exe.as_path(), ctx.work.as_path());
        let engine = crate::probes::engine();
        let page = medium_page();
        let server = http::Server::start(Arc::new(move |_: &str| response("200 OK", &[], "text/html", page.as_bytes())))
            .map_err(|e| io::Error::other(format!("site: {e}")))?;
        for n in counts {
            let _t = Section::start(&format!("scale-{n}"));
            guarded(&format!("{n} tabs"), || {
                let nodes: Vec<Value> = (1..=n)
                    .map(|i| {
                        json!({"id": format!("t{i}"), "type": TAB, "url": format!("{}/p?i={i}", server.base()),
                               "view_kind": engine, "pos": [(i % 10 * 300) as f64, (i / 10 * 200) as f64]})
                    })
                    .collect();
                let profile = work.join(format!("appdata-{n}"));
                seed(exe, &profile, &engine, &nodes)?;
                let start = Instant::now();
                let mut inst = Instance::launch(exe, &profile, work, &format!("scale-{n}.log"))?;
                inst.send("WAITTABS")?;
                let spawned = inst.export_state(&format!("spawned-{n}"))?;
                let elapsed = start.elapsed().as_secs_f64();
                let count = |loaded| spawned.cards().iter().filter(|c| State::card_is(c, TAB, Some(loaded))).count();
                let (live, asleep) = (count(true), count(false));
                let want = format!("{n} tabs: {live} live and {asleep} asleep, expected 1 and {}", n - 1);
                if !check(&want, live == 1 && live + asleep == n) {
                    return Ok(());
                }
                println!("{engine}, {n} tabs: restored in {elapsed:.1}s, 1 live and {asleep} asleep");
                inst.send("SLEEP 15")?;
                inst.export_state(&format!("settled-{n}"))?;
                for line in powershell(work, "memory", MEMORY, &["-Root", &inst.pid().to_string()])? {
                    println!("{line}");
                }
                quit(&mut inst, &format!("{n} tabs"));
                Ok(())
            });
        }
        Ok(())
    })
}
```

- [ ] **Step 2: package and install-check**

In `crates/gate/src/package.rs` replace `use crate::probes::{finish, guarded, quit, seed, tab_node, Site};` with `use crate::probes::{guarded, quit, seed, tab_node, Site};`, and `use crate::windows_main::work_dir;` with `use crate::windows_main::run_stage;`. In `package` replace

```rust
    let work = match work_dir(None, "package") {
        Ok(w) => w,
        Err(e) => {
            eprintln!("work dir: {e}");
            return ExitCode::from(1);
        }
    };
    let _t = Section::start("package");
    guarded("package", || run(&repo, &exe_src, &work));
    finish(&work)
}
```

with

```rust
    run_stage("package", None, false, |ctx| {
        let _t = Section::start("package");
        guarded("package", || run(&repo, &exe_src, &ctx.work));
        Ok(())
    })
}
```

and in `install_check` replace

```rust
    let work = match work_dir(None, "install-check") {
        Ok(w) => w,
        Err(e) => {
            eprintln!("work dir: {e}");
            return ExitCode::from(1);
        }
    };
    let _t = Section::start("install-check");
    guarded("install-check", || install(engine, &files, Path::new(exe_src), &work));
    finish(&work)
}
```

with

```rust
    run_stage("install-check", None, false, |ctx| {
        let _t = Section::start("install-check");
        guarded("install-check", || install(engine, &files, Path::new(exe_src), &ctx.work));
        Ok(())
    })
}
```

- [ ] **Step 3: Delete probes::setup and probes::finish**

In `crates/gate/src/probes.rs` remove:

```rust
/// `manifold.exe` and a fresh work dir, or the exit code to stop with.
pub(crate) fn setup(parent: Option<PathBuf>, name: &str) -> Result<(PathBuf, PathBuf), ExitCode> {
    let exe = manifold_exe().map_err(|e| {
        eprintln!("{e}");
        ExitCode::from(1)
    })?;
    let work = work_dir(parent, name).map_err(|e| {
        eprintln!("work dir: {e}");
        ExitCode::from(1)
    })?;
    Ok((exe, work))
}

pub(crate) fn finish(work: &Path) -> ExitCode {
    if failures() == 0 && std::env::var_os("KEEP").is_none() {
        remove_dir_with_retries(work);
    }
    exit_code()
}

```

Then the imports:
- Replace `use std::process::{Command, ExitCode, Stdio};` with `use std::process::{Command, Stdio};`.
- Replace `use crate::check::{check, check_eq, exit_code, failures, note_failure, Section};` with `use crate::check::{check, check_eq, note_failure, Section};`.
- Replace `use crate::instance::{register_log, remove_dir_with_retries, seed_settings, Instance, QUIT_TIMEOUT, STEP_TIMEOUT};` with `use crate::instance::{register_log, seed_settings, Instance, QUIT_TIMEOUT, STEP_TIMEOUT};`.
- Replace `use crate::windows_main::{manifold_exe, work_dir, Ctx};` with `use crate::windows_main::Ctx;`.
- Keep `PathBuf`: `host_logs` returns it.

Run: `git grep -n -E 'probes::(setup|finish)|\bfinish\(&?work\)|setup\(None' -- crates/gate`
Expected: no output.

- [ ] **Step 4: Check**

Run: `cargo test -p gate -j 8 -- install_checks_usage stage_names_are_unique package::`
Expected: `test result: ok.`, 0 failed.

- [ ] **Step 5: Format, lint, commit**

```bash
cargo fmt --all
git diff --stat
git add crates/gate/src/stress.rs crates/gate/src/package.rs crates/gate/src/probes.rs
git commit -m "gate: scale, package and install-check frame through run_stage; probes::setup and probes::finish go" -m "Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

Expected: the pre-commit hook passes; three files changed.

---

### Task 9: Delete hello, hold, launch(), Run::Parent, Stage::parent, the Option in Stage.usage, and Instance::log_tail

**Files:**
- Modify: `crates/gate/src/main.rs`, `crates/gate/src/instance.rs`, `crates/gate/src/check.rs` (`Section::next`)

**Interfaces:**
- Consumes: nothing new.
- Produces:
  - `struct Stage { name: &'static str, usage: &'static str, run: Run }`.
  - `enum Run { Work(Body), Args(fn(&[String]) -> ExitCode), Bare(fn() -> ExitCode) }`.

- [ ] **Step 1: Delete the stages and their helpers**

In `crates/gate/src/main.rs`:
- Delete the `STAGES` rows `        Stage::parent("hello", hello),` and `        Stage { name: "hold", usage: None, run: Run::Parent(hold) },`.
- Delete the functions `launch`, `hello` and `hold` entirely: from `    fn launch(exe: &Path, work: &Path) -> std::io::Result<Instance> {` through the closing `    }` of `hold`, including the doc comment ``    /// Launches an instance and then sleeps, so a test can kill `gate` itself and look for leaked processes.``.
- Delete the `Stage::parent` constructor:

  ```rust
          /// A stage that takes `[WORKDIR_PARENT]` and makes its own work dir.
          const fn parent(name: &'static str, f: fn(Option<PathBuf>) -> ExitCode) -> Self {
              Self { name, usage: Some("[WORKDIR_PARENT]"), run: Run::Parent(f) }
          }

  ```

- Delete the variant `        Parent(fn(Option<PathBuf>) -> ExitCode),` and the dispatch arm `            Some(Stage { run: Run::Parent(f), .. }) => f(rest.first().map(PathBuf::from)),`.
- Replace the `Run` doc comment:

  ```rust
      /// How a stage takes its arguments: a body run by `run_stage` (with the manifold exe and a fresh work dir made from
      /// the work-dir parent argument), the work-dir parent, the rest of the command line, or nothing.
  ```

  with:

  ```rust
      /// How a stage takes its arguments: a body run by `run_stage` (with the manifold exe and a fresh work dir made from
      /// the work-dir parent argument), the rest of the command line, or nothing.
  ```

- [ ] **Step 2: Stage.usage is a plain string**

In `crates/gate/src/main.rs` replace:

```rust
    /// One `gate <name>` stage. `usage` is what follows the name in the usage text (`None`: a test aid, not listed).
    struct Stage {
        name: &'static str,
        usage: Option<&'static str>,
        run: Run,
    }
```

with:

```rust
    /// One `gate <name>` stage. `usage` is what follows the name in the usage text.
    struct Stage {
        name: &'static str,
        usage: &'static str,
        run: Run,
    }
```

Then:
- In `Stage::work` replace `            Self { name, usage: Some("[WORKDIR_PARENT]"), run: Run::Work(f) }` with `            Self { name, usage: "[WORKDIR_PARENT]", run: Run::Work(f) }`.
- In `Stage::args` replace `            Self { name, usage: Some(usage), run: Run::Args(f) }` with `            Self { name, usage, run: Run::Args(f) }`.
- In `listed_as` replace `            self.usage = Some(usage);` with `            self.usage = usage;`.
- Replace `        Stage { name: "sync-runtime-deps", usage: Some(""), run: Run::Bare(crate::sync_deps::sync_runtime_deps) },` with `        Stage { name: "sync-runtime-deps", usage: "", run: Run::Bare(crate::sync_deps::sync_runtime_deps) },`.
- In `usage()` replace

  ```rust
          let listed: Vec<String> = STAGES
              .iter()
              .filter_map(|s| s.usage.map(|u| format!("gate {} {u}", s.name).trim_end().to_owned()))
              .collect();
  ```

  with

  ```rust
          let listed: Vec<String> =
              STAGES.iter().map(|s| format!("gate {} {}", s.name, s.usage).trim_end().to_owned()).collect();
  ```

In the tests:
- Replace

  ```rust
              for stage in STAGES.iter().filter(|s| s.usage.is_some()) {
                  assert!(usage.contains(&format!("gate {}", stage.name)), "{} missing from {usage}", stage.name);
              }
              assert!(!usage.contains("gate hold"), "the test aid is not listed");
  ```

  with

  ```rust
              for stage in STAGES {
                  assert!(usage.contains(&format!("gate {}", stage.name)), "{} missing from {usage}", stage.name);
              }
  ```

- In `install_checks_usage_lists_the_installable_hosts` replace `.and_then(|s| s.usage).unwrap_or("")` with `.map_or("", |s| s.usage)`.

- [ ] **Step 3: Imports and the module comment**

Replace the `windows_main` imports:

```rust
    use crate::check::{check, check_eq, exit_code, failures, no_verdict_words, note_failure, Marks, Section};
    use crate::fresh::require_fresh_exe;
    use crate::instance::{log_tail, remove_dir_with_retries, seed_settings, take_logs, Instance};
```

with:

```rust
    use crate::check::{exit_code, failures, no_verdict_words, note_failure, Marks};
    use crate::fresh::require_fresh_exe;
    use crate::instance::{log_tail, remove_dir_with_retries, take_logs};
```

(`Duration` stays for `RUN_DIR_MAX_AGE`; `Path` stays for `prune` and `closing_line`.) In the crate doc comment at the top of `main.rs` replace ``//! Stages: `hello` (one instance: launch, WAITTABS, export, QUIT), `launch-forms` `` with ``//! Stages: `launch-forms` ``, keeping the rest of that line unchanged.

- [ ] **Step 4: Instance::log_tail goes**

Run: `git grep -n '\.log_tail(' -- crates/gate`
Expected: no output (its last user was `hello`).

In `crates/gate/src/instance.rs` remove:

```rust
    /// The last `n` lines of the instance's log, for the diagnostics printed after a failure.
    pub fn log_tail(&self, n: usize) -> String {
        log_tail(&self.log, n)
    }

```

and replace the free function's doc ``/// The last `n` lines of a log file (empty if unreadable): `Instance::log_tail` for an instance already gone.`` with ``/// The last `n` lines of a log file (empty if unreadable).``.

- [ ] **Step 4b: Section::next goes**

`Section::next` is dead once `hello` goes (the bin crate has no public API, and clippy runs with `-D warnings`). Its other callers were converted to `Marks::mark` by Tasks 3-6, and `Marks::mark` (Task 1) uses `drop` plus `Section::start`.

Run: `git grep -n -E 'Section::next|\.next\("' -- crates/gate`
Expected: only the doc comments in `check.rs`.

In `crates/gate/src/check.rs`:
- Remove:

  ```rust

      /// Ends this step (printing it) and opens `name`.
      pub fn next(self, name: &str) -> Section {
          drop(self);
          Section::start(name)
      }
  ```

- Replace the doc ``/// scripts' EXIT trap). `Section::start("a")` then `.next("b")` ends "a" and opens "b".`` with ``/// scripts' EXIT trap). `Marks` opens one after another.``.
- Replace the doc ``/// The timing marks: opening one ends the previous (`Section::next`), `end` closes the last. `Marks::default()` is`` with ``/// The timing marks: opening one ends the previous, `end` closes the last. `Marks::default()` is``.

- [ ] **Step 5: Check**

Run: `git grep -n -E '\bhello\b|\bhold\b|Run::Parent|Stage::parent|usage: Some|usage: None' -- crates/gate/src/main.rs`
Expected: no output.

Run: `cargo test -p gate -j 8`
Expected: `test result: ok.`, 0 failed.

- [ ] **Step 6: Format, lint, commit**

```bash
cargo fmt --all
git diff --stat
git add crates/gate/src/main.rs crates/gate/src/instance.rs crates/gate/src/check.rs
git commit -m "gate: delete the hello and hold stages, Run::Parent, Stage::parent, launch() and Instance::log_tail; usage is a plain string" -m "Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

Expected: the pre-commit hook passes ; three files changed.

---

### Task 10: Docs, the end-of-plan gate, and one deliberately failing run

**Files:**
- Modify: `README.md` (lines 462 and 480), `docs/superpowers/plans/2026-10-30-cleanup.md` (GATE1, GATE2, GATE4, GATE10, GATE15), `docs/superpowers/specs/2026-10-31-gate-stage-skeleton-design.md` (status), `docs/superpowers/plans/2026-10-31-gate-stage-skeleton.md` (status)

**Interfaces:**
- Consumes: the finished code of Tasks 1-9 and the baseline run ids from Task 1 Step 1.
- Produces: no code.

- [ ] **Step 1: README**

In `README.md` (the `gate all` paragraph) replace

```
and each stage's output in `LOG.<step>.log` (`LOG.two-N.log` for the `gate two` runs). The steps after
```

with

```
and each stage's output in `LOG.<step>.log` (`LOG.two-N.log` for the `gate two` runs). Each stage that takes `[WORKDIR_PARENT]` (and `scale`, `package` and `install-check`) works in `<WORKDIR_PARENT, default %TEMP%>\gate-runs\<stage>-<pid>` and ends with one line, `<stage>: ok` or `<stage>: failed (kept <dir>)`, which `gate all` shows as the stage's `last:`. A passing run deletes its dir unless `KEEP=1` is set. A failing run keeps it and prints the last 20 lines of every log its instances wrote. Dirs under `gate-runs` older than three days are removed when the next stage starts. The steps after
```

In the `gate layout` paragraph replace `A failure prints the differing pixels' box; `LAYOUT_KEEP=<dir>` copies the failing PNG pairs there.` with `A failure prints the differing pixels' box and the two PNGs' paths, which stay in the kept work dir.`

- [ ] **Step 2: Cleanup plan outcomes**

Get the commit ids of Tasks 1-9: `git log --format='%h %s' -10`. Call the first and last of them `<T1>` and `<T9>`. In `docs/superpowers/plans/2026-10-30-cleanup.md`:
- After the GATE1 line (the one ending `**Own plan 2026-10-31** (...).`), add a new line ``  **Outcome:** Done 2026-10-31 (<T1>..<T9>): one `run_stage` frames every stage that takes a work dir; a failed run keeps its dir under `gate-runs` (3-day pruning) and prints every registered log.``.
- After the GATE2 line, add ``  **Outcome:** Done 2026-10-31 (<T1>..<T9>): the seed copies use `probes::seed`/`import_session`; one `QUIT_TIMEOUT` (60 s) and one `BELOW_NORMAL` in `instance.rs`.``.
- After the GATE4 line, add ``  **Outcome:** Done 2026-10-31 (<T9>): `hello`, `hold`, `launch()` and `Stage.usage: Option` deleted; `listed_as` stays.``. First confirm the GATE4 line already cites `docs/runs.md:267` and says `listed_as` stays (it does at `7e6010d5`).
- After the GATE15 line, add ``  **Outcome:** Partly done 2026-10-31 (<T1>, <T7>): `Marks` is the one timing type (`Default`, `mark` opens a section). Left: the `check`/`check_eq`/`check_is` bodies and `default_jar_id`/`default_jar_node`.``. Here `<T7>` is Task 7's commit.
- In the GATE10 line:
  - Replace `**GATE10. Undocumented env knobs and a second keep hook.**` with `**GATE10. Undocumented env knobs.**`.
  - Replace ` ~30 lines. Medium. (`LAYOUT_KEEP` goes with GATE1's plan.)` with ` ~30 lines. Medium.`.

- [ ] **Step 3: Docs check**

Run: `git grep -n -E 'LAYOUT_KEEP|gate hello|gate hold|probes::(setup|finish)|Run::Parent|Stage::parent|gate-\{stage\}|two-instance test: (ok|failed)' -- . ':!docs/runs.md' ':!docs/superpowers/plans/2026-10-1*' ':!docs/superpowers/plans/2026-10-29-host-table.md'`

Expected: matches only in the spec, this plan, and the GATE1 description of the cleanup plan, which is history now that its Outcome line is added. Any other match is stale: fix it in this step. Also confirm `PARKED.md`'s GATE-HELPERS line ("one of three argument shapes") is still true. `Run` now has `Work`, `Args` and `Bare`, so it needs no edit.

- [ ] **Step 4: Mark the spec and plan done, commit the docs**

Make this edit (the spec already matches the shipped design; its amendments were applied when the plan was written):
- In `docs/superpowers/specs/2026-10-31-gate-stage-skeleton-design.md` replace `status: draft` with `status: done`.
- In this plan replace `status: active` with `status: done`.

```bash
git diff --stat
git add README.md docs/superpowers/plans/2026-10-30-cleanup.md docs/superpowers/specs/2026-10-31-gate-stage-skeleton-design.md docs/superpowers/plans/2026-10-31-gate-stage-skeleton.md
git commit -m "docs: gate stage skeleton done (README work dirs and KEEP, cleanup outcomes, LAYOUT_KEEP gone)" -m "Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 5: The end-of-plan gate**

Run the 7-package clippy once (Global Constraints; the docs commits above did not trigger the hook): `cargo clippy -j 8 -p manifold -p manifold-hosts -p manifold-view-protocol -p manifold-pages -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings`. Expected: no warning.

Run: `cargo build -j 8 -p gate && target/debug/gate.exe tier 7e6010d5`
(`7e6010d5` is the commit the plan was written against, so the advice covers everything Tasks 1-9 changed.) Expected: a tier name. Record it. Then run `full` regardless: it covers every tier and matches the baseline (plan decision 6). Check `Get-Process python, manifold, gate -ErrorAction SilentlyContinue` for leftovers first, then launch from bash:

```bash
powershell -NoProfile -File "$USERPROFILE/.claude/runs/run.ps1" -Kind gate -Name skeleton-end -Detach -Command 'cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate && target\debug\gate.exe all "%RUN_DIR%\gate-log.txt" full'
```

Expected: a run id. Wait with a Monitor until-loop (deadline 45 min) that exits on `GATE done` in its `gate-log.txt` OR when its `run.json` `"end"` is no longer `null`; then read the exit code from `run.json` (no `GATE done` means the run died: report it). Set the row's headline from the log's last lines (CLAUDE.md "Gate"). Report the CPU load and Defender state that `run.json` recorded.

- [ ] **Step 6: Verdicts before and after**

```bash
B=$(ls -d "$LOCALAPPDATA"/claude-runs/manifold/*-skeleton-base | tail -1)
A=$(ls -d "$LOCALAPPDATA"/claude-runs/manifold/*-skeleton-end | tail -1)
verdicts() { grep -E '^ *(PASS|FAIL):' "$1" | sed -E 's/[0-9]+(\.[0-9]+)?/N/g'; }
for f in "$B"/gate-log.txt.*.log; do s=${f##*/gate-log.txt.}; diff <(verdicts "$f") <(verdicts "$A/gate-log.txt.$s") > "$A/verdict-diff.$s.txt" || echo "DIFF $s"; done
for f in "$B"/gate-log.txt.*.log; do s=${f##*/gate-log.txt.}; echo "$s base=$(grep -c -E '^ *PASS:' "$f") end=$(grep -c -E '^ *PASS:' "$A/gate-log.txt.$s")"; done
grep -E '^GATE .* last:' "$A/gate-log.txt"
```

The digit masking in `verdicts` hides a changed count in a label, so the loop also compares the PASS-line count per step; a count that differs between base and end is a `DIFF` too (read both logs).

Expected:
- No `DIFF` line (numbers inside labels are masked) and every step's `base=` equals its `end=`.
- Every `last:` of a step that runs through `run_stage` reads `<stage>: ok`. The `two-N` steps end in `two: ok` and the download-cef step in `download: ok` (the stage name, not the step name). Skip the `unit` step: it is `cargo test`, not a `run_stage` stage, so its `last:` is unchanged.
- `GATE done` and an overall pass.
- A `DIFF` is a behaviour change: read its `verdict-diff.*.txt`, and fix or report it before going on.

Then the two stages outside `gate all`:

```bash
ROUNDS=3 powershell -NoProfile -File "$USERPROFILE/.claude/runs/run.ps1" -Kind probe -Name skeleton-end-dblprobe -Detach -Command 'target\debug\gate.exe dblprobe'
powershell -NoProfile -File "$USERPROFILE/.claude/runs/run.ps1" -Kind probe -Name skeleton-end-scale -Detach -Command 'target\debug\gate.exe scale 2'
```

Launch them one at a time with `-Detach`, each followed by the same `run.json` `"end"` watcher as Task 1 Step 1 (deadline 20 min).

Expected:
- Exit 0 for both.
- Their PASS lines match the `skeleton-base-*` runs after the same number masking.
- Their last lines are `dblprobe: ok` and `scale: ok`.
- `ls "$TEMP/gate-runs"` shows no dir from these runs.

`package` and `install-check` are not run: they need a release exe and the network. Their verdict lines come from the unchanged `guarded` calls (plan decision 4).

- [ ] **Step 7: One deliberately failing run, with pruning**

Make an old dir that must be pruned:

```powershell
$old = Join-Path $env:TEMP 'gate-runs\prune-probe-1'; New-Item -ItemType Directory -Force $old | Out-Null; (Get-Item $old).LastWriteTime = (Get-Date).AddDays(-4)
```

Then run from bash. The `bad` entry fails deterministically after the first size has launched an instance:

```bash
SIZES='400 300,bad' VIEWS=webview2 OUT="$TEMP/shots-fail-probe" powershell -NoProfile -File "$USERPROFILE/.claude/runs/run.ps1" -Kind probe -Name skeleton-fail -Command 'target\debug\gate.exe chrome-shots-sizes'
```

Expected in the run's `out.log`:
1. `FAIL: SIZES entry "bad" is not "W H"`.
2. At least one `--- <path>` header followed by log lines in which `PASS`/`FAIL` appear only as `P-ASS`/`F-AIL`.
3. The last line `chrome-shots-sizes: failed (kept <TEMP>\gate-runs\chrome-shots-sizes-<pid>)`.
4. Exit code 1.

Then check:
- `Test-Path "$env:TEMP\gate-runs\prune-probe-1"` is `False` (pruned).
- The kept dir exists and holds `400x300\`.
- Delete the kept dir and `$TEMP/shots-fail-probe` by hand.
- `Get-Process manifold, gate -ErrorAction SilentlyContinue` lists nothing of this run.

- [ ] **Step 8: Record the runs in the outcome**

Append the run ids of `skeleton-base`, `skeleton-end` and `skeleton-fail` to the GATE1 Outcome line in `docs/superpowers/plans/2026-10-30-cleanup.md`, as `(runs <base id>, <end id>, <fail id>)`. Then:

```bash
git diff --stat
git add docs/superpowers/plans/2026-10-30-cleanup.md
git commit -m "docs: cleanup plan GATE1 cites the skeleton gate runs" -m "Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

Expected: one file changed. Do not push, tag or bump.
