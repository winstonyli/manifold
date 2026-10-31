---
status: draft
---

# Gate stage skeleton

Cleanup items GATE1, GATE2, GATE4 (`hold` and `hello`) and GATE15 of `docs/superpowers/plans/2026-10-30-cleanup.md`.
Decisions (user, 2026-10-31): the dispatcher owns the skeleton (a1); a failed run keeps its work dir (b1); `hello` is
deleted with `hold` (c1). Reviewed against the code by three independent readers (facts, design fit, blast radius); the
corrections are folded in below.

## Problem

Every `gate <stage>` repeats one frame: find `manifold.exe`, make a work dir, run a body, print a "stopped early"
line, tail logs on failure, clean up, return the exit code. Two shapes exist. The 21 `Run::Work` stages get
`(exe, work)` from `probes::setup` and end with `probes::finish(work)` (21 call sites; `scale`, `package` and `install-check`, which are `Run::Args`, add three more). The 11 `Run::Parent` stages
(`hello`, `hold`, `launch-forms`, `layout`, `link`, `smoke`, `two`, `unknown-type`, `closed`, `dblprobe`, `trail`)
hand-write the frame and have drifted:

- Cleanup after a failure: `finish` and `launch-forms` keep the dir; `layout`, `link`, `closed`, `unknown-type`,
  `trail`, `dblprobe` and `hello` delete it (`remove_dir_all`), `smoke` and `two` delete it with
  `remove_dir_with_retries` (the retry exists because a just-killed instance still holds files). The evidence
  (screenshots, logs) is lost; `layout` carries a `LAYOUT_KEEP` copy-out only for that.
- The stopped-early line: `ERROR:` in ten places, `FAIL:` in `shots.rs:195`, and `link` has a second wording.
- Log tails: 20 or 30 lines; some through `no_verdict_words` (so `gate all`, which counts `PASS`/`FAIL` per line,
  does not count log text), some not (`hello`, `layout`, `launch-forms`); `dblprobe` has none; `link` and `unknown-type`
  hard-code their log names.
- Leaks (work dir made, then an early `return 1` without cleanup or `exit_code()`): `trail.rs:466`, `:478`;
  `smoke.rs:1309`; `two/mod.rs:336-342`; `launch_forms.rs:103`; `Site::start` failures in `probes.rs:302-308`
  (`page_kind`), `page_text.rs:15-19`, `search.rs:13-17`; the early returns in `jar.rs:736-752` and `focus.rs:293`.
- Copies of `probes::seed`/`import_session` in `link.rs` (`Run::setup`), `closed.rs`, `unknown_type.rs`, `trail.rs`
  and `dblprobe.rs`; `QUIT_TIMEOUT` declared eight times (60 s in six places, 30 s in `probes.rs` and `shots.rs`);
  `BELOW_NORMAL` three times (`instance.rs`, `launch_forms.rs`, `profile.rs`).
- `Marks` (`check.rs`) is a wrapper over `Option<Section>`; `link`, `closed`, `trail` and `unknown-type` thread a bare
  `Option<Section>` and a closure instead.

## Design

One dispatcher path for every stage that takes a work dir. A stage is a body:

```rust
type Body = fn(&mut Ctx) -> io::Result<()>;

pub(crate) struct Ctx {
    pub exe: PathBuf,          // empty when the stage does not need manifold.exe
    pub work: PathBuf,
    pub args: Vec<String>,     // the command line after the stage name, minus WORKDIR_PARENT when the stage takes one
    pub marks: Marks,          // empty: a stage that wants TIMING lines calls `marks.mark("name")`
}
```

`Run::Work` and `Run::Parent` merge into `Run::Work(Body)`: the stage takes `[WORKDIR_PARENT]` and needs a fresh
`manifold.exe`. The 21 Work stages and the 9 Parent stages that remain after `hello` and `hold` go are all
`Run::Work`; `Stage::parent` and `Run::Parent` go. Three stages that today call `probes::setup`/`work_dir` and
`finish` from a `Run::Args` stage keep their argument parsing and usage errors (exit 2, before any dir exists) and then
call `run_stage` themselves: `scale` (needs the exe, args are `TABS...`) and `package`/`install-check` (no
`manifold.exe`; args in `ctx.args`). So `run_stage(name, parent, needs_exe, args, body)` is a plain function and
`Run::Work` is one caller of it. `probes::setup` (22 callers) and `probes::finish` (24) are replaced by it.

`run_stage(name, parent, needs_exe, args, body)`:

1. `manifold_exe()` if the stage needs it, then `work_dir(parent, name)`; an error prints as `probes::setup`
   does today (`{e}`, `work dir: {e}`) and returns exit 1 after printing `{name}: failed`. Nothing exists yet to clean up.
2. Run `body(&mut ctx)`. `Err(e)` prints `ERROR: {name} stopped early: {e}` (not a verdict word) and calls
   `note_failure()`.
3. `ctx.marks.end()`. Everything the body owned (instances, servers, sections) is dropped when it returns.
4. If `failures() > 0`: print the registered logs (below).
5. `cleanup(&work)`: `keep_dir(failures() > 0, KEEP set)` decides; if not kept, `remove_dir_with_retries`.
6. Print the closing line `{name}: ok`, or `{name}: failed (kept {work})` when the dir was kept after a failure,
   as the last line (`gate all` takes a stage's last non-`TIMING` line as its headline; today `smoke` and `two`
   print their own closing line, which become redundant and are deleted). Return `exit_code()`.

`keep_dir(failed: bool, keep_env: bool) -> bool` is a pure function so it can be tested without the process-wide
`failures()` counter.

### Log tails: one registry, no scan, no name lists

`Instance::launch_env` (which `launch` calls) records the log path right after its `File::create` succeeds, in a
process-wide ordered set in `instance.rs` (`instance::register_log(path)` is the entry; a repeated name is one entry,
the file having been truncated by the later launch). Any other log file a stage wants shown calls
`register_log` itself: `launch-forms`' `run()` registers its `NAME.out`/`NAME.err`, `bad_url` registers its host
logs (`host_logs` returns their paths instead of their text; registered right after `quit`, so the 15-line dump
becomes the common 20), `two` registers the nonce-site report it writes into its work dir before returning. On failure step 4 drains
the set, and for each non-empty file prints `--- {path}` and its last 20 lines through `no_verdict_words` (moved from
`link.rs` to `check.rs`). Instance logs live in different places (`work`, `work/out` for `two`, per-case subdirs for
`bad_url`, `shots-aa` and `chrome-shots-sizes`, `HEADLESS_TEST_OUT` for `smoke`) and some have no extension (`log1`),
which is why a directory scan was rejected in review.

This subsumes every per-stage tail loop, `launch_forms::tail`, the `log1..log5` lists in `link` and `unknown-type`,
`layout`'s per-group tail (the file is still readable after the instance is gone) and `bad_url`'s host-log tail, and
gives `dblprobe` a tail. A stage no longer prints its own tails (this includes the 30-line tail in `shots.rs:190`).
The printer is a function over a list of paths, so its unit test does not touch the global set.

### Pruning

Keeping failed dirs fills `%TEMP%` (`gate all` passes no `WORKDIR_PARENT`, and this machine's C: is nearly full).
Work dirs move to `<parent>/gate-runs/<stage>-<pid>`, and `work_dir` first removes children of `gate-runs` last
modified more than three days ago. Only that subdir is pruned: `gate-shots-<hex>` (a reused, content-addressed
cache), `gate-sync-*`, `gate-release-*` and a user's own `gate-*` dirs are never touched. Dirs kept with `KEEP=1`
are pruned the same way. Nothing in `gate` looks a work dir up by its old `gate-<stage>-<pid>` name (the plan
greps to confirm).

### Smaller unifications that land with it

- `Marks` becomes the one timing type: `Marks::default()` is empty and `mark(name)` on an empty `Marks` starts a
  section. `link`, `closed`, `trail`, `unknown-type`, `smoke` and `two` use `ctx.marks` (`two` holds
  `&mut Marks` instead of owning one). Stages that open a `Section` per view or per case keep doing so.
- Every stage that seeds a profile uses `probes::seed`/`import_session`. The copies in `link`, `closed`,
  `unknown_type`, `trail` and `dblprobe` are deleted. `launch-forms` keeps its two `--import-json` calls: the import is
  what it checks.
- `QUIT_TIMEOUT` and `BELOW_NORMAL` each get one `pub(crate)` definition in `instance.rs`; the quit timeout is 60 s
  (the stages using the 30 s constants of `probes.rs` and `shots.rs` move to 60 s, which only lengthens a hung quit;
  the plan greps `quit(` for inline durations).
- The early `return 1` leaks above become `?` in the body, including `scale`'s `Server::start` failure
  (`stress.rs:173`). `shots.rs`'s `config()` returns `io::Result`, so a bad config is a body error.
- `two`'s `Halt::Failed` (a check already failed and was counted) maps to `Ok(())`, `Halt::Error(e)` to `Err(e)`.
- `LAYOUT_KEEP` is deleted: the screenshots stay in the kept dir. README line 480 describes it and goes with it.

### Deleted

- Stages `hold` and `hello`. Nothing runs them: not `gate all`, any tier, the README, CLAUDE.md or PARKED.md. The only
  references are the `main.rs` module comment, `docs/runs.md:267` (a historical usage string, history stays) and old
  plans (`2026-10-10-rust-gate-runner.md`, `-gate-two.md`, `2026-10-14-unknown-type-card.md`, `2026-10-29-host-table.md`,
  all history). With them: `launch()`, `Stage.usage: Option`, and the test assertion `!usage.contains("gate hold")`.
  `Stage::listed_as` stays (`focus-loss`, `sleep`, `jar`). The cleanup plan's GATE4 line citing `docs/runs.md:258` and
  saying `listed_as` goes with `hold` is corrected to `:267` and `usage: Option` only.
- The nine hand-written frames (about 120 to 160 lines), the 22 `setup` and 24 `finish` call sites, and
  `probes::setup`/`probes::finish`. `main.rs:6`'s module comment (it names `hello`) is edited.

## What stays per stage

Everything that is the stage's own: sections, checks, per-view and per-case `guarded` blocks, `smoke`'s repo root,
`HEADLESS_TEST_OUT` and local suggestions server, and its "All checks passed / Some checks failed. Screenshots in ..."
line, printed only on the body's `Ok` path from `failures()` (on `Err` the dispatcher's last line says it failed). `shots.rs`'s `run()` keeps going after an error to write its sheet, so
it is not a body `Err`: it prints `ERROR: chrome-shots stopped early` (was `FAIL:`, which `gate all` counted) and calls
`note_failure()` itself; its own log tail is deleted.

## Behaviour changes (all deliberate)

1. A failed `layout`, `link`, `closed`, `unknown-type`, `trail`, `dblprobe`, `smoke` or `two` run keeps its work dir,
   named on the last line; the pruning rule bounds the pile.
2. Every dispatched stage's last line is `{name}: ok` or `{name}: failed (kept ...)`. `gate all`'s per-stage headline (`last:`)
   follows; `smoke`'s and `two`'s own closing lines are gone.
3. The dispatcher emits no `TIMING` line. The converted Parent stages open their own first mark (`setup`) so their
   `TIMING setup` lines stay.
4. Failed runs print every registered log, not only the ones a stage chose, so a failing multi-instance stage prints
   more lines than before.
5. The quit timeout of the stages on the 30 s constants goes to 60 s.
6. `hello`, `hold` and `gate hello` no longer exist.
7. Exit codes: a failed stage exits 1. Config errors that exited 2 (`shots.rs:129`, `:141`) become body errors and exit
   1; usage errors of `scale`, `package` and `install-check` stay exit 2 (checked before `run_stage`); `gate all` only tests `rc != 0` (`all.rs:165`) and logs the code.

## Testing

- Unit tests: `keep_dir` over its four inputs; the log printer over a list of paths; the early-stop line text; the log printer on a temp dir with a non-empty
  registered log containing `PASS`/`FAIL` (masked), an empty registered log (skipped) and a missing one (skipped);
  `Marks::mark` on an empty `Marks` starts a section; `work_dir` pruning removes an old `gate-*` dir and keeps a fresh
  one and a non-`gate-` dir.
- The README step-name test and `stage_names_are_unique_and_the_usage_lists_the_public_ones` stay (the `gate hold`
  assertion goes).
- Behavioural proof: for each converted stage, its PASS/FAIL verdict lines before and after the change are identical
  (diff of `gate <stage>` output with `TIMING` and path lines filtered), run once at the end rather than per edit, plus
  the tier-`ui` gate once, plus one deliberately failing run (a bogus `VIEWS` or a stage with a forced `check(false)`)
  to see the tails, the `kept` line and the pruning.

## Out of scope

`cfg(windows)` scaffolding (GATE5, decided against), the Python-port leftovers (GATE3), the repo-root lookups (GATE7),
`gate ab` load waiting (GATE8), `gate dblprobe`'s own fate (GATE9) and the env-knob documentation (GATE10; its "second keep hook" clause goes with
`LAYOUT_KEEP` and is trimmed from the cleanup plan in the same change).

## Open questions for the plan

- Whether `gate-runs` should live under `target/` instead of `%TEMP%` (outside the repo is current behaviour; the
  plan keeps it).
- `README.md` also documents `KEEP`; the plan adds the "a failed run keeps its dir" sentence there.
