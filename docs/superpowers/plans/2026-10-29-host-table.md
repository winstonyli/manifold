---
status: draft
---

# One Table per Host Implementation Plan

> **For agentic workers:** Execute inline, in task order (each task consumes the one before; Task 8 Step 9 dispatches the one final whole-branch review). Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every engine host's layout facts (exe name, the OS it is built for, dev build folder, delivery, sidecar files, runtime folder) live in one row of a new zero-dependency crate, `manifold-hosts`, read by the app, its build script, the gate and, through two new gate subcommands, the release workflow.

**Architecture:**
- `crates/manifold-hosts` holds `Os`, `Delivery`, `BuildOutput`, `Sidecar`, `Runtime`, `Host`, the five rows (`SERVO`, `CEF`, `WEBKIT`, `WEBVIEW2`, `PDFIUM`, in `ALL`), `by_id`, and every derived name and path. Each method whose answer depends on the OS takes the OS manifold runs on as an argument.
- The app's `ViewDesc` points at its row (`host: &'static Host`). `build.rs` generates the embedded-host table from the `Embedded` rows. `host_pin::parse` checks each manifest entry with `Host::pinned_under`.
- The gate syncs every host with one `sync_host`, and its other stages read file names from the rows. `gate release-pins` and `gate release-embedded` replace `release.yml`'s PowerShell host table and embedded copy. `gate release-verify` checks the manifest against the table with `manifest_gaps`.

**Tech Stack:**
- Rust (`nightly-2026-09-23` per `rust-toolchain.toml`, edition 2021). The leaf crate uses `std` only.
- The gate's `serde_json` (`preserve_order`) and `sha2` (`crates/gate/Cargo.toml`).
- GitHub Actions `release.yml` (`pwsh`) and `ci.yml`.
- `~/.claude/runs/run.ps1` for recorded runs, and WSL Ubuntu for the leaf crate's Linux test run.

**Spec:** `docs/superpowers/specs/2026-10-29-host-table-design.md` ("One table per host"). Its decisions H1-H14 are approved and binding, and its "Order of work" (steps 1-7) is this plan's commit structure: Task N is step N. Where a line below restates the spec briefly, the spec wins on detail.

**Base commit:** 10444ae5 ("docs: host table spec: stress round 5 ..."). Every line number below, and every one the spec cites, was read at 10444ae5.
- The last commit there that touches code, `.github`, `.githooks`, `Cargo.*` or `CLAUDE.md` is 5a015745, which is older than the spec's first draft (b620c932). So the spec's code citations hold at 10444ae5.
- Lines drift as tasks land, and another session may commit in this tree. Find each edit by the code quoted with it, not by its line number.
- Step S2 under "Start: before Task 1" records the commit the work actually starts from, as B.

**Dates.** The plan was drafted on 2026-10-29. Dates in new text are the day of the edit.

## Approved decisions (the spec, restated)

- **H1. A new leaf crate, `manifold-hosts`.** It has zero dependencies, uses `std` only and has no `cfg`.
  - `manifold` (as a normal and a build dependency) and `gate` depend on it. Nothing else does: no host crate, not `manifold-view-protocol`, not `manifold-pages`.
  - So a table edit rebuilds the app and the gate, and no host.
- **H2. `ViewDesc` gets `host: &'static Host`.** It replaces `name`, `binary`, `source`, `host_os`, `runtime_dir` and `dev_dir`, and `HostSource` is deleted.
  - This amends `docs/decisions/2026-10-23-host-runtimes.md:68`-`69`. The facts are still plain data, in a table of their own.
- **H3. The row key is `id`, not `name`.** It is the engine's store name and the manifest's first key.
  - `desc().name` becomes `desc().host.id`.
  - The names guard's `desc().name` exemption goes: `names/tests.rs:470` and `:477` are edited, and `:495` is deleted.
- **H4. PDFium is a fifth row with no `ViewKind`.** It is embedded exactly as WebView2 is, so the embedding code, the freshness check and `release_verify` handle both in one loop.
- **H5. `gate release-pins` and `gate release-embedded` replace `release.yml`'s PowerShell host logic.**
  - `release-pins` replaces `:74`-`101`, and `release-embedded` replaces `:107`-`112` and `:117`-`123`.
  - The `cargo build` lines and their CRT flags stay in the YAML.
- **H6. The embedded hash records are renamed `<stem>.embedded-<id>.sha256`** (`embedded-webview2` and `embedded-pdfium`; they were `embedded-host` and `embedded-pdf-view`).
  - Their writer, the draft-release upload (`release.yml:144`) and the reader (`release_verify.rs:74`, `:81`) change in one commit, the last with code (Task 6).
- **H7. `build.rs` generates the embedded-host code from the rows.**
  - The input variable is `Host::embed_var()` (`MANIFOLD_<ID>_HOST`) and the blob is `<id>-host.bin`; both are today's names.
  - The generated `OUT_DIR/embedded.rs` carries each hash as a literal. So the `MANIFOLD_<ID>_SHA256` rustc-env values, `embedded_host::Embedded` and `build.rs`'s `HOSTS` tuple are deleted.
- **H8. CEF's runtime marker moves into the row** (`Runtime.lib`: `libcef.dll` on Windows, `libcef.so` elsewhere). `CefPlatform.runtime_lib` is deleted.
- **H9. One sync rule for every host.**
  - A host's own build folder is searched before `target/`'s (`find_first`).
  - A host whose build folder is missing is skipped. The run fails only if no host returned `Synced`.
  - A part found in no build output is kept if it is already in place. It is an error if it is not in place and the host exe is built, and skipped otherwise.
- **H10. The Linux `webkit-host` job keeps two literals** (`release.yml:25`, `:29`), tied to the WebKit row by a layout test. The gate does not compile on Linux.
- **H11. The exe suffix is decided at run time, by `Os::exe_suffix`.**
  - `Os` moves from `process_manager/runtime.rs` into the leaf: it is moved, not copied, and callers import it from the leaf.
  - Every OS-dependent `Host` method takes the OS manifold runs on as an argument. None reads `cfg` or calls `Os::here()` itself.
  - `CefPlatform.exe_suffix` and `host_exe` are deleted.
  - WebKit's host file name on Windows has no `.exe`, which fixes `view_install.rs:846`.
- **H12. Checks.**
  - Layout tests in the leaf, plus a golden test.
  - `ViewKind` maps onto the rows.
  - Two checks of a manifest against one table: `host_pin::parse` through `Host::pinned_under`, and `release_verify` through `manifest_gaps`, with a test that they agree.
  - One `tier.rs` Full arm for every crate but the app, which covers `crates/manifold-hosts` and any later crate (amended 2026-10-30, Q-tier).
- **H13. Parked with this design, not done in it:** the app's CEF-only runtime code, the repeated `-p` crate lists, and per-host environment variable names. They are in PARKED.md, "Host table (2026-10-29)".
- **H14. Release proof needs a push.**
  - The `release.yml` change first runs in the workflow's manual dry run, which needs the branch pushed, and pushing is the user's call.
  - The dry run does not reach the draft-release step, so the upload line (`:144`) first runs on a real tag.

## Global Constraints

- **Start condition.**
  - `git status --short -- crates README.md PARKED.md ROADMAP.md CLAUDE.md docs .github .githooks Cargo.toml Cargo.lock` must print nothing before the Start steps and before each commit, apart from the current task's own files.
  - This is the guard against another session's work. If anything else is listed, stop and report it. Never stage, revert or edit another agent's changes.
  - At 10444ae5 those paths were clean. Every `git add` below names its paths.
  - An untracked file outside those paths (at the repo root, for instance) is not this plan's. Never stage it, and leave it alone.
    - `run.ps1` marks every run's commit `-dirty` while `git status --porcelain -- . ':!docs/runs.md'` (`run.ps1:63`) lists anything, untracked files included. At 10444ae5 it listed nothing, so an Expected line below that says "no `-dirty`" holds only while it still lists nothing.
- **CPU and priority.** Before a foreground cargo build or test expected to run over about 30 s, check `%LOCALAPPDATA%\cpu-leases\*.json` for a live `exclusive` lease (its pid alive and `until` in the future) and wait for it. `-j 8` caps build jobs only; the whole-crate `cargo test` lines for `-p manifold` and `-p gate` below (and Task 8 Step 2's multi-crate line) carry `-- --test-threads 8` for that reason. A filtered run needs none, and standalone `-p manifold-hosts` lines (a dozen small tests, no dependencies) and its WSL line carry none. The three `Downloads::start()` tests in `shots_net.rs` share one fixed port (`DOWNLOADS_PORT`, `:122`), so a whole-crate gate run that fails `a_killed_download_server_refuses_the_retry` may be that overlap: rerun it alone with `cargo test -j 8 -p gate shots_net -- --test-threads=1` and report it before treating it as a defect. The gate runs go through `run.ps1` (Task 8 Step 12), which sets BelowNormal.
- **One cargo per target dir.**
  - Before any cargo command, run `Get-Process cargo -ErrorAction SilentlyContinue` in PowerShell. If one is running, it is another session's build in `target/`: wait for it to exit.
  - Before a WSL cargo run, `wsl.exe -d Ubuntu -e pgrep -af "[c]argo"` must print nothing.
- **Commands.** Every cargo line uses `-j 8` (CLAUDE.md).
  - The clippy line is CLAUDE.md's "Formatting and lints" command. Task 1 adds `-p manifold-hosts` to it, to `.githooks/pre-commit:7` and to `ci.yml:16` together. After Task 1 it is `cargo clippy -j 8 -p manifold -p manifold-hosts -p manifold-view-protocol -p manifold-pages -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings`. Every task's clippy step runs that line. Before Task 1 lands, the same line without `-p manifold-hosts`.
  - The test lines are listed below. The app has no lib target, hence `--bin manifold`.
    - `cargo test -j 8 -p manifold-hosts`
    - `cargo test -j 8 -p manifold --bin manifold -- --test-threads 8`
    - `cargo test -j 8 -p gate -- --test-threads 8`
  - The gate build line (CLAUDE.md "Gate") is `cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate`.
- **Commit structure.**
  - Tasks 1-7 are the spec's "Order of work" steps 1-7, one commit each. Task 2 is written as parts 2A and 2B, with one commit at the end of 2B.
  - Every commit leaves the tree building with its tests green.
  - `Cargo.lock` is staged in every commit that adds the crate or a dependency on it (Tasks 1, 2 and 3). `release.yml` and `ci.yml` build `--locked`, and the pre-commit hook does not.
- **Names.**
  - The leaf's public API is exactly the spec's "The crate" section, and its rows are exactly the five listed there.
  - A name a task needs that the spec does not give is a question for the user, not an invention.
  - The gate names are fixed by the spec: `HostSync`, `sync_host`, `run_in`, `find_first` (`pub(crate)`), `release_pins_in`, `release_embedded_in`, `embedded_record` and `manifest_gaps`. `Pinned` and `manifest_files` keep their names and become `pub(crate)`.
- **Formatting and commits.**
  - Run `cargo fmt --all` before each commit, and check `git diff --stat` first.
  - The pre-commit hook runs fmt and clippy. Never bypass it with `--no-verify`. Never write an `#[allow]` without a `reason`.
  - Commit messages end with the `Co-Authored-By:` trailer that your own session's system reminder gives. It names your session's model, which differs between the planner, implementers and reviewers.
    - Each commit block writes the trailer as the placeholder `<attribution>`. Put that line in its place.
    - A message ending in the literal placeholder is wrong. After each commit, check it with `git log -1 --format=%B | grep -v '^$' | tail -n 1`.
    - If it is wrong, fix it before going on with `git commit --amend -F -` and the corrected message in a heredoc. A bare `--amend` opens an editor, which this environment can't use.
  - Never push, tag or bump.
  - No CHANGELOG edit: the entry is written at the bump (spec "Docs").
  - No migration code and no compatibility shims. In particular, `process_manager::runtime` does not re-export `Os` (H11).
- **Tests and the user's files.**
  - No test reads or writes the real `%APPDATA%` (CLAUDE.md "Tests and the user's profile").
    - App tests use `crate::store::testutil::temp_dir`, and gate tests use `std::env::temp_dir()`.
    - The leaf's `tests/layout.rs` only reads repo files under `CARGO_MANIFEST_DIR/../..` and writes nothing.
  - Tests run on Linux too: CI's Linux job runs `-p manifold`, `-p manifold-view-protocol` and, after Task 1, `-p manifold-hosts` (`ci.yml:24`). A test that builds a host file name uses an explicit `Os::Windows` or `Os::Linux`, or `Os::here()` on both sides of the comparison, never an `.exe` literal against `Os::here()`.
  - No run writes into the user's Downloads folder. Each run step diffs it before and after.
- **Python.** Run Python only as `python -P f.py < /dev/null`, with the script written to a file first. No step here needs Python. Repeat this rule in every subagent brief that may edit files.
- **Models.** Set `model` on every Agent call.
  - Implementers follow the user's WORKING_STYLE tiers. Per-task reviewers, fix rounds and re-reviews use Sonnet.
  - The one final whole-branch review (Task 8) uses Opus.
  - Haiku is never a reviewer, and subagents do not spawn subagents.
- **Hosts.**
  - No host crate's source changes, and this plan builds neither CEF nor Servo.
  - The sync comparison uses the hosts already built in this tree: `target/debug/manifold-view-cef.exe` with `target/debug/cef-runtime/`, and `target-servo/debug/manifold-view-servo.exe` with its two DLLs.
  - WebKit is touched only through the leaf's WSL test run.
- **Disk.**
  - C: is near full and its free space swings with other sessions' builds (5 GB on 2026-10-29, 26 to 28 GB on 2026-10-30). Check it with `df -h /c | tail -n 1` before every build and run.
  - Under 2 GB free, stop and ask the user. Delete nothing to make room.
  - Make no new target dir.
- **While a run is live.** Never edit sources while a run started from this repo is live.
- **Release proof (H14).** Never push. The `release.yml` dry run is the user's next step after this plan, and the report says so.

### Pre-run check (before every `run.ps1` launch)

1. In PowerShell, run `Get-Process cargo, gate, manifold -ErrorAction SilentlyContinue | Select-Object Id, ProcessName, Path`.
   - Any `cargo` or `gate` belongs to another session: wait for it to exit.
   - A `manifold` whose `Path` is this repo's `target\debug\manifold.exe` holds files a build or a sync replaces: ask the user to close it.
   - Any other `manifold` is the user's own app: leave it.
2. Find unfinished runs.
   - `grep -nE "T[0-9:]+ \|  \|  \|" docs/runs.md | cut -c1-100` lists the rows with an empty `end`.
   - For each, find the run's `wrapper pid N` (in its notes cell, or in `"notes"` in its `run.json`) and run `tasklist //FI "PID eq N"`.
   - A live wrapper is a run in progress: wait for it.
   - A dead one is a lost run. Close it as RUNS.md "Finding and closing" says, and commit that on its own (`runs: close <n> lost rows`).
3. Check for leases (LONG_RUNS.md "CPU leases"; the gate drives a GPU-drawn app too). This lists live leases and changes nothing:

```powershell
foreach ($d in "$env:LOCALAPPDATA\cpu-leases", "$env:LOCALAPPDATA\gpu-leases") { Get-ChildItem $d -Filter *.json -ErrorAction SilentlyContinue | ForEach-Object { $l = Get-Content $_.FullName -Raw | ConvertFrom-Json; if ((Get-Process -Id $l.pid -ErrorAction SilentlyContinue) -and $l.until -gt [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()) { "$d $($l.kind) pid $($l.pid) until $($l.until) $($l.purpose)" } } }
```

   - A line with `exclusive` is someone else's timing run. Wait until it ends (its `until` is the latest it can last, at most 15 minutes), then check again.
   - `shared` leases are reported with the result, not waited for.
   - Both folders were empty on 2026-10-29.
   - No run in this plan is `-Timed`, and none takes a lease.
4. Check the disk: `df -h /c | tail -n 1`. Under 2 GB free, stop and ask.
5. Save the user's Downloads folder listing: `ls -A "$USERPROFILE/Downloads" > "<scratchpad>/downloads-before-<run name>.txt"`. `<scratchpad>` is your session's scratchpad directory.

### Waiting for a run

`RUN` in the steps below is the run's folder, `$LOCALAPPDATA/claude-runs/manifold/<YYYY-MM-DD>-<Name>`: the launch day and the run's `-Name`, with `-2`, `-3` appended if the name was already used that day.

A run started without `-Detach` returns when it ends and prints no run id, so find its folder by name: `RUN=$(ls -dt "$LOCALAPPDATA"/claude-runs/manifold/????-??-??-<Name>* | head -1)` (newest first). Check that `echo "$RUN"` printed a folder.

A detached run prints `run <id> started: wrapper pid N, raw <dir>`, and `<id>` below is that id. Wait for it with this command as a background Bash call (`run_in_background: true`), deadline 4500 s for the full gate:

```bash
RUN="$LOCALAPPDATA/claude-runs/manifold/<id>"; deadline=$((SECONDS+4500)); until grep -Eq '"end": +"' "$RUN/run.json" || [ $SECONDS -ge $deadline ]; do sleep 5; done; grep -E '"(exit|headline|commit)"' "$RUN/run.json"; tail -n 15 "$RUN/out.log"
```

`run.ps1` writes `run.json` with Windows PowerShell's `ConvertTo-Json`, which puts two spaces after each colon (`"exit":  0,`). So read each field's value, never its spacing; a pattern that matches one uses ` +`, as the loop's `"end": +"` does. "`run.json`'s `"exit"` is `0`" below means that value.

After every run, `ls -A "$USERPROFILE/Downloads" | diff "<scratchpad>/downloads-before-<run name>.txt" -` must print nothing. Any difference is a stop-and-report: name each new file.

A gate stage that fails keeps its work folder (`<run folder>/gate-<stage>-<pid>`); `probes::finish` deletes it only on a pass. Remove it once its output has been read: `rm -rf -- "$RUN"/gate-<stage>-*`.

### Leaf tests on Linux (WSL)

```bash
wsl.exe -d Ubuntu -- bash -lc 'cd "/mnt/c/Users/Winston Li/Documents/GitHub/manifold" && CARGO_TARGET_DIR=target-linux cargo test --locked -j 8 -p manifold-hosts'
```

- The leaf has no dependencies, so this compiles in seconds even though `target-linux` holds no app build (on 2026-10-29 it held only the WebKit host and `render_stub`). Use a 600000 ms timeout.
- Never run `-p manifold` there. That is a cold build of the whole app on a nearly full disk, and CI's Linux job runs it after a push.

### Start: before Task 1

- [ ] **Step S1: Check the start condition, cargo and the disk**

Run:
- the start-condition `git status` line (expected: no output);
- `Get-Process cargo -ErrorAction SilentlyContinue` (expected: nothing);
- `df -h /c | tail -n 1` (expected: at least 2 GB free);
- `git status --porcelain -- . ':!docs/runs.md'` (expected: nothing; a line listed here is not this plan's, see Global Constraints "Start condition", and makes later runs read `-dirty`).

- [ ] **Step S2: Record B**

Run: `git rev-parse --short HEAD`
Expected: a commit hash. That commit is **B**, the base for `gate tier <B>` in Task 8 and for the final review's diff. Step S4 writes it into this plan's header.

- [ ] **Step S3: Pre-run capture: `gate sync-runtime-deps` before the change**

The spec's verification compares this command's output before and after the work. This step records the "before".

1. Build the gate at B: `cargo build -j 8 -p gate`. Expected: `Finished`.
2. Do the pre-run check. Then run the sync once unrecorded to reach a steady state: `target/debug/gate.exe sync-runtime-deps; echo "exit $?"`.
   - Expected: `exit 0`. It may print `recopied ...` lines if the tree was behind.
   - Exit 1 is a stop-and-report: the before state must be a passing sync, or the comparison means nothing.
3. Record the run that counts, from bash:

```bash
powershell -NoProfile -File "$USERPROFILE/.claude/runs/run.ps1" -Kind probe -Name sync-before -Command 'target\debug\gate.exe sync-runtime-deps'
RUN=$(ls -dt "$LOCALAPPDATA"/claude-runs/manifold/????-??-??-sync-before* | head -1); echo "$RUN"
grep -E '"(exit|commit)"' "$RUN/run.json"; cat "$RUN/out.log"; cat "$RUN/err.log"
```

Expected:
- `run.json`'s `"exit"` is `0`, and its `"commit"` is B with no `-dirty`.
- `out.log` holds:
  - `sync-runtime-deps: target-servo/debug/libEGL.dll already current` and the same line for `libGLESv2.dll`;
  - one `sync-runtime-deps: cef-runtime/<name> already current` line per file of CEF's runtime;
  - `sync-runtime-deps: cef-runtime/manifold-view-cef.exe already current`;
  - then `sync-runtime-deps: everything already current, nothing to do`.
- `err.log` is empty.

This is a record, not a gate. Whatever it holds is the "before"; write down any line that differs from the expectation in the report. The diff in Task 8 is between the two records.

On 2026-10-29 Servo's DLLs in `target-servo/debug/build/mozangle/ad274970f12e1e29/out/`, in `target/debug/build/mozangle/ad274970f12e1e29/out/` and in `target-servo/debug/` had the same SHA-256 (libEGL.dll `63f91749...`, libGLESv2.dll `2cf89920...`). So the change of DLL source (H9) should not change a line of output on this tree.

- [ ] **Step S4: Set the spec and this plan active, and commit with the run row**

1. In `docs/superpowers/specs/2026-10-29-host-table-design.md`, change the front matter's `status: draft` to `status: active`.
2. In this plan's repo copy, `docs/superpowers/plans/2026-10-29-host-table.md` (tracked since 07e3f325):
   - change its front matter to `status: active`;
   - add one line under **Base commit**: `Started at <B> (<date>).`
3. Commit:

```bash
cargo fmt --all
git diff --stat; git status --short -- docs
git add docs/runs.md docs/superpowers/specs/2026-10-29-host-table-design.md docs/superpowers/plans/2026-10-29-host-table.md
git commit -F - <<'EOF'
docs: host table: spec and plan active; runs: sync-before

<attribution>
EOF
git log -1 --format=%B | grep -v '^$' | tail -n 1
```

Expected:
- `cargo fmt --all` changes nothing.
- `git diff --stat` lists only `docs/runs.md` (the `sync-before` row), the spec and the plan. `git status --short -- docs` shows nothing else.
- The last command prints the trailer line, not `<attribution>`.

## Review Focus

These are the inputs and conditions the spec implies that no ordinary test run exercises, most likely to bite first. Each line names the test or step that pins it.

1. **The release subcommands' stdout is piped into `$GITHUB_ENV`.** The YAML runs `& target/debug/gate.exe release-pins ... | Add-Content $env:GITHUB_ENV`, and only the dry run executes it (H14).
   - Any stdout line that is not `NAME=VALUE` corrupts the job's environment. Examples: a progress line from a reused helper (`sync_file`'s progress lines print to stdout, `sync_deps.rs:76` and `:80`), the manifest text, or a debug print.
   - The dispatcher itself adds nothing for `Stage::args` stages (`main.rs:216`).
   - Expected: stdout is exactly the lines the core returns. Everything else, including the manifest text, goes to stderr.
   - Pinned in Task 5: the wrapper prints only the core's returned `Vec<String>`, and the core calls no printing helper. Also pinned in Task 8 Step 7, which runs `release-embedded` on this machine's `target/release` hosts and checks every stdout line against `^[A-Z0-9_]+=.+$`.
2. **Tests that pass on Windows and fail on CI's Linux job.** Locally only `-p manifold-hosts` runs on Linux (WSL); `-p manifold` runs there only in CI after a push (`ci.yml:24`).
   - An `.exe` literal, or a Windows-only path shape, in a new or rewritten app test fails only there.
   - Pinned in Task 2: the `host_pin` tests build file names with `Os::Windows`, and `two_embedded_hosts_extract_side_by_side` compares with `PDFIUM.installed_in(Os::here(), ..)` (spec "Tests").
   - Pinned in Task 1: the golden test checks both OSes' literals in one binary.
   - Pinned in Task 8: Step 4 runs the leaf in WSL, and Step 9's reviewer reads every new test for OS-shaped literals.
3. **A build with a real embed.**
   - The generated `embedded.rs` has a non-empty branch (a real blob, a real hash literal, `ALL`'s order). That branch runs only in a release build and in the dedicated test run. Dev builds and the gate embed nothing (README `:390`).
   - The embed test skips a host whose blob is empty (`embedded_host.rs:307`-`323`), so a run that embedded nothing still passes.
   - Pinned in Task 2: `the_embedded_hosts_extract_to_their_pinned_hashes` loops the `Embedded` rows.
   - Pinned in Task 8 Step 6, which builds with both variables set and checks that each `<id>-host.bin` is as large as its exe, so the test did not skip.
4. **`release-pins` meets its real inputs first on the release runner.** It cannot run here: there is no release build of CEF or Servo in `target/release` on 2026-10-29. The inputs are:
   - Servo's sidecars under `target/release/build` in whichever cargo layout the runner's nightly writes (this machine holds both `build/mozangle/<hash>/out/` and `build/mozangle-<hash>/`);
   - WebKit's host from the artifact download at `webkit-host/manifold-view-webkitgtk` (`release.yml:26`-`29` uploads that one file, `:53`-`56` downloads it into `webkit-host`, and `:77` reads that path today);
   - printed paths that the later `cargo build -p manifold` step reads from the environment, so they must be absolute.

   Pinned in Task 5: `release_pins_in`'s temp-tree test puts a Servo sidecar under `target/release/build/mozangle/<hash>/out/` and WebKit's host at `FOREIGN/manifold-view-webkitgtk`, and asserts that every returned path is absolute and has no `\\?\` prefix. Task 3's `find_first` tests cover the other layout. Not run before the dry run (H14), and the report says so.
5. **The `WITH_ENGINES` steps of `gate all`.**
   - The `SITE_HOST` predicate (`all.rs:318`-`326`, now `built_for(Os::here()) != Os::here()`) and `kill_hosts`'s `by_id(view)` lookup (`sleep.rs:87`) have no test of their own.
   - The full gate does not set `WITH_ENGINES` (CLAUDE.md "Gate"), so neither runs in Task 8.
   - Expected: Servo steps keep the local page address, and only WebKit's gets the VM address.
   - Pinned only by the golden `built_for` and `file_name` literals (Task 1), the `manifold-view-{` grep (Task 8 Step 5) and the final reviewer's reading (Task 8 Step 9).

## File Structure

Paths are relative to the repo root. Line ranges are at 10444ae5.

- **Start** (before Task 1)
  - `docs/superpowers/specs/2026-10-29-host-table-design.md`: front matter `active`.
  - `docs/superpowers/plans/2026-10-29-host-table.md` (this plan's repo copy, tracked as a draft since 07e3f325): set `active`, start commit B.
  - `docs/runs.md`: the `sync-before` row.
- **Task 1: the leaf crate**
  - `crates/manifold-hosts/Cargo.toml` (new): package `manifold-hosts`, `version.workspace`, `edition.workspace`, `[lints] workspace = true`, no dependencies of any kind.
  - `crates/manifold-hosts/src/lib.rs` (new): `HOSTS_DIR`, `CEF_RUNTIME_DIR`, `Os`, `Delivery`, `BuildOutput`, `Sidecar`, `Runtime`, `Host`, the five rows, `ALL`, `by_id`, the `Host` methods, and the golden, `Os` and `pinned_under` unit tests.
  - `crates/manifold-hosts/tests/layout.rs` (new): checks the rows against `crates/<exe>/Cargo.toml`, `.gitignore`, `release.yml` and the other crates' manifests.
  - `Cargo.toml`: the root `members` (`:3`-`13`) and `[workspace.dependencies]` (`:23`-`24`).
  - `Cargo.lock`: the new package.
  - `README.md`: the crate-map line (`:9`-`18`) and `-p manifold-hosts` on the CI line (`:35`); Task 7 only checks both.
  - `crates/gate/src/tier.rs`: one Full arm for every crate but the app (it replaces the protocol, pages, engine-crate and gate arms), and its test cases beside `:282`.
  - `.github/workflows/ci.yml` (`:16`, `:17`, `:24`), `.githooks/pre-commit` (`:7`) and `CLAUDE.md` ("Formatting and lints"): `-p manifold-hosts`.
- **Task 2: the app reads the table** (parts 2A and 2B, one commit)
  - `crates/manifold/Cargo.toml`: `manifold-hosts` as a normal and a build dependency.
  - `crates/manifold/build.rs`: generates `OUT_DIR/embedded.rs` from the `Embedded` rows. The `HOSTS` tuple and the `MANIFOLD_<ID>_SHA256` values are deleted.
  - `crates/manifold/src/embedded_host.rs`: `include!`s the generated table and reads it through `embedded(host)`. `ensure` and `ensure_with` take `&Host`. `Embedded` and its statics are deleted, and the tests move onto the rows.
  - `crates/manifold/src/process_manager.rs`: `ViewDesc.host`. `HostSource` and `CEF_RUNTIME_DIR` are deleted. Also `prepare_webview2` (`:1011`), `pdf_view_exe` (`:1212`), `from_name` and `Serialize` (`:238`, `:256`), the CEF exe path (`:644`) and the docs.
  - `crates/manifold/src/process_manager/runtime.rs`: `Os` and its tests move out to the leaf.
  - `crates/manifold/src/process_manager/availability.rs`: `sibling_binary`, `host_file_name`, `beside_exe`, `in_place_paths` and the `"engine-hosts"` literal are deleted, replaced by the row's methods.
  - `crates/manifold/src/process_manager/tests.rs`: the in-place tests are rewritten over the rows, with a PDFium case added.
  - `crates/manifold/src/view_install.rs` and `crates/manifold/src/view_install/tests.rs`: Download/runtime checks, the pin platform, `host_file_names` (WebKit has no `.exe`), the marker names and the `runtime_lib` reads.
  - `crates/manifold/src/settings.rs` (`Os` import at `:189`, `:392`) and `crates/manifold/src/settings_page.rs` (`:472`, the runtime marker hint).
  - `crates/manifold/src/pdf_render.rs` (`:140`): the not-installed hint takes the exe name from `PDFIUM.exe`; its text is unchanged.
  - `crates/manifold/src/cef_platform.rs`: `runtime_lib`, `exe_suffix`, `host_exe` and its test are deleted.
  - `crates/manifold/src/host_pin.rs`: the exact-set check through `Host::pinned_under`, and the doc at `:12`-`13`.
  - `crates/manifold/src/debug_control.rs` (`:452`), `crates/manifold/src/debug_export.rs` (`:420`) and `crates/manifold/src/session_sync/tests/misc.rs` (`:201`): `desc().host.id`.
  - `crates/manifold/src/names/tests.rs` (`:470`, `:477`, `:495`): the exemption goes (H3).
  - `crates/manifold/src/deleted_names.rs`: a second `#[test]` with the app-crate needles.
  - `Cargo.lock`.
- **Task 3: one sync for every host**
  - `crates/gate/Cargo.toml`: the `manifold-hosts` dependency.
  - `crates/gate/src/sync_deps.rs`: `HostSync`, `sync_host`, `run_in` and `find_first`. `SERVO_DLLS`, `SERVO_EXE`, `sync_servo`, `sync_cef` and `find_newest` are deleted, and the tests are rewritten.
  - `Cargo.lock`.
- **Task 4: the other gate readers**
  - `crates/gate/src/fresh.rs`: a runtime-copy check per host with a runtime, and the dep-info list from `[&WEBVIEW2, &PDFIUM]`.
  - `crates/gate/src/package.rs`: `installed_files` and `install` take owned names; the engine list, pins, extracted-host checks and `HOSTS_DIR` come from the rows.
  - `crates/gate/src/main.rs`: the `install-check` usage test in `windows_main::tests`.
  - `crates/gate/src/jar.rs` (`:481`, `:744`), `probes.rs` (`:560`-`561`), `shots.rs` (`:74`, `:1590`-`1704`) and `stress.rs` (`:40`): names from the rows.
  - `crates/gate/src/sleep.rs` (`:87`, `kill_hosts` through `by_id`) and `crates/gate/src/all.rs` (`:318`-`326`, the `SITE_HOST` predicate).
- **Task 5: the release subcommands**
  - `crates/gate/src/release_hosts.rs` (new, `cfg(windows)`): `release_pins_in`, `release_embedded_in`, `embedded_record`, the two stage wrappers and their temp-tree and name tests.
  - `crates/gate/src/main.rs`: `mod release_hosts`, and `release-pins` and `release-embedded` in `STAGES`.
- **Task 6: the verifier and the workflow, one commit**
  - `crates/gate/src/release_verify.rs`: the record names from `embedded_record`, one embedded loop, `manifest_gaps`, the `pub(crate)` visibilities, and the round-trip, upload-glob and agreement tests.
  - `crates/gate/src/release_hosts.rs` (Task 5's file): the imports and the round-trip, agreement and upload-glob tests.
  - `.github/workflows/release.yml`: `:74`-`101` and `:107`-`123` are replaced by the two subcommands, `manifold.exe` gets a step of its own, and the upload at `:144` takes the records by pattern.
  - `README.md` "Releases" bullets (`:387`-`390`): the subcommands, the record names, the printed variables, and the stale `hosts.yml`.
- **Task 7: the remaining docs**
  - `crates/manifold/src/session_sync.rs` (`:121`) and `crates/manifold/src/store/model.rs` (`:227`): doc comments that still say `ENGINES` become `VIEW_KINDS`. An addition beyond the spec's "Docs" list, comment-only.
  - `README.md`: `:22` (`Host.os` through `Host::built_for`), `:33` (checked), and `:80` ("Runtime-file placement": search order, per-host skip, a part kept in place, the CEF error).
  - `PARKED.md`: GATE-CEF-DIR (`:61`), "One table per host" (`:157`) and "Servo's DLLs come from the newest mozangle output ..." (`:158`) are deleted. "The `-p` crate lists are repeated" gains its evidence, `:156` is re-cited to `Host::in_place`, `:87` is re-read and reworded, and `process_manager.rs` line numbers are refreshed.
  - `docs/decisions/2026-10-23-host-runtimes.md` (`:3`, `:7`-`9`, `:18`, `:68`-`69`) and `docs/decisions/2026-10-18-hosts-in-the-release.md` (`:8`): amendment notes; the decisions themselves are unchanged.
- **Task 8: final checks and the full gate**
  - `docs/runs.md`: the `sync-after` and `gate-host-table` rows, and `sleep-history-rerun-table` if run.
  - The spec's and this plan's front matter: `status: done`.

---

### Task 1: The leaf crate `manifold-hosts`

Spec step 1 ("The crate"; Tests, "Golden table", "`Os`", "Layout" and "Tier"; Tooling; H1, H11). It adds a new crate `crates/manifold-hosts` with:
- `Os`;
- the five rows and their helpers;
- the golden, `Os` and layout tests.

It also covers the tooling around the crate:
- `Cargo.lock`;
- the `tier.rs` Full arm for every crate but the app;
- `-p manifold-hosts` in `ci.yml`, the pre-commit hook and CLAUDE.md's clippy command (together, so they never disagree);
- README's crate map and CI line.

Nothing reads the table yet. The leaf's `Os` is additive: `crates/manifold/src/process_manager/runtime.rs:14`-`62` (`runtime::Os`) and its test (`:198`-`208`) stay until Task 2 deletes them, so for one commit the tree has two `Os` types with the same variants.

How to work:
- Make every edit with the Edit tool. The old text is exact; a line hint ("at HEAD :109") is the line at `10444ae5`.
- Run cargo with `-j 8`, one cargo at a time on `target/`.
- Run no Python.
- Change nothing beyond this task; report every deviation.

**Files:**
- Create: `crates/manifold-hosts/Cargo.toml`.
- Create: `crates/manifold-hosts/src/lib.rs` (the table, the helpers and their unit tests).
- Create: `crates/manifold-hosts/tests/layout.rs` (checks against the repo's files).
- Modify: `Cargo.toml`:
  - `members`, at HEAD :5-6;
  - `[workspace.dependencies]`, at HEAD :23.
- Modify: `Cargo.lock` (cargo adds a four-line `manifold-hosts` package entry on the first cargo command).
- Modify: `crates/gate/src/tier.rs`:
  - the Full arm's replaced part, at HEAD :107-113 (the arm starts at :104), and the gate arm it makes dead, at HEAD :122-124 (deleted);
  - its tests, at HEAD :282.
- Modify: `.github/workflows/ci.yml` :16, :17, :24.
- Modify: `.githooks/pre-commit` :7.
- Modify: `CLAUDE.md` :11 ("Formatting and lints").
- Modify: `README.md`:
  - the crate map, after :17;
  - the CI paragraph, :35.
- Touched and restored in Step 11 only (to watch the layout tests fail): `.gitignore` :2 and `.github/workflows/release.yml` :144.

**Interfaces:**
- Consumes: nothing from other tasks. At test time, `tests/layout.rs` reads these files:
  - `crates/<exe>/Cargo.toml` for each row;
  - `.gitignore`;
  - `.github/workflows/release.yml` (:25, :29, :66, :72, :105, :144 at HEAD);
  - every `crates/*/Cargo.toml`.
- Produces (crate `manifold-hosts`, lib `manifold_hosts`, `std` only, no `cfg`), exactly the spec's "The crate":

```rust
pub const HOSTS_DIR: &str = "engine-hosts";
pub const CEF_RUNTIME_DIR: &str = "cef-runtime";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Os { Windows, Linux, Mac, Other }
impl Os {
    pub const ALL: [Os; 4] = [Os::Windows, Os::Linux, Os::Mac, Os::Other];
    pub fn from_consts(os: &str) -> Os;
    pub fn here() -> Os;                       // Os::from_consts(std::env::consts::OS)
    pub fn name(self) -> &'static str;         // "Windows" | "Linux" | "macOS" | "another OS"
    pub fn pin_platform(self) -> &'static str; // "windows-x64" | "linux-x64" | "macos-x64" | "other-x64"
    pub fn exe_suffix(self) -> &'static str;   // ".exe" | "" | "" | ""
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Delivery { Download, Embedded }
#[derive(Debug)] pub struct BuildOutput { pub krate: &'static str, pub tail: &'static str }
#[derive(Debug)] pub struct Sidecar { pub file: &'static str, pub os: Os, pub from: BuildOutput }
#[derive(Debug)] pub struct Runtime { pub dir: &'static str, pub lib: fn(Os) -> &'static str, pub from: BuildOutput }
#[derive(Debug)] pub struct Host {
    pub id: &'static str, pub exe: &'static str, pub os: Option<Os>, pub dev_dir: Option<&'static str>,
    pub delivery: Delivery, pub sidecars: &'static [Sidecar], pub runtime: Option<Runtime>,
}
pub static SERVO: Host; pub static CEF: Host; pub static WEBKIT: Host; pub static WEBVIEW2: Host; pub static PDFIUM: Host;
pub static ALL: [&Host; 5];                    // [&SERVO, &CEF, &WEBKIT, &WEBVIEW2, &PDFIUM]
pub fn by_id(id: &str) -> Option<&'static Host>;
impl Host {
    pub fn built_for(&self, running: Os) -> Os;
    pub fn file_name(&self, running: Os) -> String;
    pub fn pin_platform(&self, running: Os) -> &'static str;
    pub fn beside(&self, running: Os, exe_dir: &Path) -> PathBuf;
    pub fn in_place(&self, running: Os, exe_dir: &Path, dev_folders: bool) -> Vec<PathBuf>;
    pub fn installed_in(&self, running: Os, hosts_dir: &Path) -> PathBuf;
    pub fn runtime_in(&self, hosts_dir: &Path) -> Option<PathBuf>;
    pub fn runtime_lib(&self, running: Os) -> Option<&'static str>;
    pub fn sidecars_for(&self, running: Os) -> impl Iterator<Item = &Sidecar>;
    pub fn pinned_files(&self, running: Os) -> Vec<String>;
    pub fn installed_files(&self, running: Os) -> Vec<String>;
    pub fn pinned_under(&self, platform: &str) -> Option<Vec<String>>;
    pub fn embed_var(&self) -> String;
}
```

- Also produces:
  - the root `[workspace.dependencies]` entry `manifold-hosts = { path = "crates/manifold-hosts" }`. Tasks 2 and 3 add `manifold-hosts.workspace = true` to `crates/manifold/Cargo.toml` (`[dependencies]` and `[build-dependencies]`) and `crates/gate/Cargo.toml`;
  - the layout test `only_the_app_and_the_gate_depend_on_the_table_and_it_on_nothing`. It fails if any other `crates/*/Cargo.toml` names `manifold-hosts`;
  - `tier.rs`'s one Full arm for every crate but the app, which covers `crates/manifold-hosts/**` and later crates;
  - `-p manifold-hosts` in `ci.yml` :16, :17 and :24, `.githooks/pre-commit` :7 and CLAUDE.md's clippy command, all placed right after `-p manifold`. Later tasks' clippy line is `cargo clippy -j 8 -p manifold -p manifold-hosts -p manifold-view-protocol -p manifold-pages -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings`;
  - README's crate-map line starting `manifold-hosts` and `-p manifold-hosts` on README's CI line (Task 7 checks both).

- [ ] **Step 1: Start**

- Run the start check. Both commands must print nothing:
  - `git status --short -- crates Cargo.toml Cargo.lock README.md CLAUDE.md .github .githooks .gitignore docs`;
  - `Get-Process cargo -ErrorAction SilentlyContinue` (PowerShell).

- B is the commit Step S2 recorded, not HEAD (HEAD is Step S4's commit now). Do not recompute it. Run `git diff --stat 10444ae5 HEAD -- Cargo.toml Cargo.lock crates .github .githooks CLAUDE.md README.md .gitignore`. Expected: empty. If it prints anything, re-find every anchor this task quotes in those files before going on.
- Run `ls crates/manifold-hosts`. Expected: `No such file or directory`. If the folder exists, stop and report.

Step S4 already committed this plan as `active`; this task's only commit is Step 20's.

- [ ] **Step 2: The crate's manifest and its place in the workspace**

Create `crates/manifold-hosts/Cargo.toml`:

```toml
[package]
name = "manifold-hosts"
version.workspace = true
edition.workspace = true
description = "The host table: one row per engine host's layout, for manifold, its build script and the gate."

[lints]
workspace = true
```

It has no `[dependencies]`, `[build-dependencies]` or `[dev-dependencies]` section, and must never get one (H1; Step 9's layout test checks it).

In `Cargo.toml`, at HEAD :5-6, replace:

```toml
    "crates/manifold-pages",
    "crates/manifold",
```

with:

```toml
    "crates/manifold-pages",
    "crates/manifold-hosts",
    "crates/manifold",
```

In `Cargo.toml`, at HEAD :23, replace:

```toml
manifold-pages = { path = "crates/manifold-pages" }
```

with:

```toml
manifold-hosts = { path = "crates/manifold-hosts" } # std only; only manifold and gate depend on it (its tests/layout.rs)
manifold-pages = { path = "crates/manifold-pages" }
```

Nothing uses the workspace dependency until Tasks 2 and 3. Cargo allows it, but on a recent toolchain cargo may print, on each command (tests, clippy, the hook's clippy), one ``warning: unused workspace dependency `manifold-hosts` `` (`cargo::unused_workspace_dependencies`, warn by default) until Task 2 adds `manifold-hosts.workspace = true`. It is cargo's lint, not a clippy finding, and `-D warnings` does not turn it into an error. Leave the entry in.

- [ ] **Step 3: Write `lib.rs`: the types and the failing tests**

Create `crates/manifold-hosts/src/lib.rs` with the docs, the constants, the types and the test module only. Steps 5-7 add the code between them.

```rust
//! The host table: one row per engine host, saying what its exe is called, where a dev tree builds it, how a user
//! gets it and what must sit beside it, with the file names and paths derived from that. It is the one copy of a
//! host's layout for the app, its build script and the gate, so they cannot drift apart.
//!
//! `std` only, no dependencies (`tests/layout.rs` checks it) and no `cfg`: every answer that depends on the OS takes
//! the OS manifold runs on (`running`) as an argument, so one test binary checks every OS's answers. A build script
//! uses only `id`, `delivery` and `embed_var()`, never a method taking an OS: there `Os::here()` would be the build
//! machine, not the target. Rows are compared by `id` (`Host` holds a `fn`, so it has no `PartialEq`).

use std::path::{Path, PathBuf};

/// The folder under the profile's view data directory that holds downloaded and extracted hosts.
pub const HOSTS_DIR: &str = "engine-hosts";
/// CEF's runtime folder: beside its host in a dev tree, under `HOSTS_DIR` when downloaded.
pub const CEF_RUNTIME_DIR: &str = "cef-runtime";

/// An operating system a host is built for, or manifold runs on. `Other` is any not named here (FreeBSD and the like):
/// only the engines built for whichever OS manifold runs on (Servo, CEF) can run there.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Os {
    Windows,
    Linux,
    Mac,
    Other,
}

/// How a user gets a host.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Delivery {
    /// Pinned in the release's manifest and downloaded into `HOSTS_DIR` on demand.
    Download,
    /// Carried inside manifold.exe and extracted to `HOSTS_DIR/<id>/<file>` at first use.
    Embedded,
}

/// Where cargo leaves a file or folder a host needs besides its exe: `<build>/<krate>-*/out/<tail>` or
/// `<build>/<krate>/*/out/<tail>` (older cargo nightlies use the first layout, newer ones the second).
#[derive(Debug)]
pub struct BuildOutput {
    pub krate: &'static str,
    pub tail: &'static str,
}

/// A file that sits beside the host exe when it runs on `os`.
#[derive(Debug)]
pub struct Sidecar {
    pub file: &'static str,
    pub os: Os,
    pub from: BuildOutput,
}

/// A runtime folder beside the host, holding the runtime and a copy of the host exe.
#[derive(Debug)]
pub struct Runtime {
    pub dir: &'static str,
    /// The file whose presence marks a runtime folder, per OS the host is built for.
    pub lib: fn(Os) -> &'static str,
    /// Where a dev build's runtime comes from (Windows: cef-dll-sys's `cef_windows_x86_64`).
    pub from: BuildOutput,
}

/// One engine host's row.
#[derive(Debug)]
pub struct Host {
    /// The engine's store name and the manifest key.
    pub id: &'static str,
    /// Cargo package and bin name, without a suffix; also the `crates/<exe>/` folder.
    pub exe: &'static str,
    /// The OS it is built for; None: whichever OS manifold runs on.
    pub os: Option<Os>,
    /// A dev tree's build folder beside `target/`, when not `target/` itself (`<dev_dir>/debug/<file>`).
    pub dev_dir: Option<&'static str>,
    pub delivery: Delivery,
    pub sidecars: &'static [Sidecar],
    pub runtime: Option<Runtime>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_os_name_from_the_standard_library_reads_as_one_of_four() {
        assert_eq!(Os::from_consts("windows"), Os::Windows);
        assert_eq!(Os::from_consts("linux"), Os::Linux);
        assert_eq!(Os::from_consts("macos"), Os::Mac);
        for other in ["freebsd", "openbsd", ""] {
            assert_eq!(Os::from_consts(other), Os::Other, "{other:?}");
        }
        let facts: Vec<_> = Os::ALL.into_iter().map(|os| (os, os.name(), os.pin_platform(), os.exe_suffix())).collect();
        assert_eq!(
            facts,
            [
                (Os::Windows, "Windows", "windows-x64", ".exe"),
                (Os::Linux, "Linux", "linux-x64", ""),
                (Os::Mac, "macOS", "macos-x64", ""),
                (Os::Other, "another OS", "other-x64", ""),
            ]
        );
    }

    #[test]
    fn the_rows_in_their_order() {
        let rows: Vec<_> = ALL.iter().map(|host| (host.id, host.exe, host.os, host.dev_dir, host.delivery)).collect();
        assert_eq!(
            rows,
            [
                ("servo", "manifold-view-servo", None, Some("target-servo"), Delivery::Download),
                ("cef", "manifold-view-cef", None, None, Delivery::Download),
                ("webkit", "manifold-view-webkitgtk", Some(Os::Linux), Some("target-linux"), Delivery::Download),
                ("webview2", "manifold-view-webview2", Some(Os::Windows), None, Delivery::Embedded),
                ("pdfium", "manifold-view-pdfium", None, None, Delivery::Embedded),
            ]
        );
        assert_eq!((HOSTS_DIR, CEF_RUNTIME_DIR), ("engine-hosts", "cef-runtime"));
    }

    /// Where a dev build leaves each part a host needs besides its exe, and that exactly CEF has a runtime.
    #[test]
    fn where_a_dev_build_leaves_each_part() {
        let sidecars: Vec<_> = SERVO
            .sidecars
            .iter()
            .map(|sidecar| (sidecar.file, sidecar.os, sidecar.from.krate, sidecar.from.tail))
            .collect();
        assert_eq!(
            sidecars,
            [
                ("libEGL.dll", Os::Windows, "mozangle", "libEGL.dll"),
                ("libGLESv2.dll", Os::Windows, "mozangle", "libGLESv2.dll"),
            ]
        );
        for host in ALL {
            assert_eq!(host.sidecars.is_empty(), host.id != "servo", "only Servo has sidecars: {}", host.id);
            assert_eq!(host.runtime.is_some(), host.id == "cef", "only CEF has a runtime: {}", host.id);
        }
        let runtime = CEF.runtime.as_ref().expect("CEF has a runtime");
        assert_eq!(
            (runtime.dir, runtime.from.krate, runtime.from.tail),
            ("cef-runtime", "cef-dll-sys", "cef_windows_x86_64")
        );
        assert_eq!(Os::ALL.map(runtime.lib), ["libcef.dll", "libcef.so", "libcef.so", "libcef.so"]);
    }

    /// One row's derived values as manifold sees them running on `running`, written out by hand.
    struct Golden {
        host: &'static Host,
        running: Os,
        built_for: Os,
        file_name: &'static str,
        pin_platform: &'static str,
        /// The file names of `sidecars_for`.
        sidecars: &'static [&'static str],
        pinned_files: &'static [&'static str],
        installed_files: &'static [&'static str],
        runtime_lib: Option<&'static str>,
        /// Under the hosts folder.
        runtime_in: Option<&'static str>,
        /// Under the hosts folder, one component per entry.
        installed_in: &'static [&'static str],
        /// Under `manifold.exe`'s folder, one component per entry.
        beside: &'static [&'static str],
        /// The dev folder `in_place` walks with `dev_folders`, if any.
        walks: Option<&'static str>,
        embed_var: &'static str,
    }

    static GOLDEN: [Golden; 10] = [
        Golden {
            host: &SERVO,
            running: Os::Windows,
            built_for: Os::Windows,
            file_name: "manifold-view-servo.exe",
            pin_platform: "windows-x64",
            sidecars: &["libEGL.dll", "libGLESv2.dll"],
            pinned_files: &["manifold-view-servo.exe", "libEGL.dll", "libGLESv2.dll"],
            installed_files: &["manifold-view-servo.exe", "libEGL.dll", "libGLESv2.dll"],
            runtime_lib: None,
            runtime_in: None,
            installed_in: &["manifold-view-servo.exe"],
            beside: &["manifold-view-servo.exe"],
            walks: Some("target-servo"),
            embed_var: "MANIFOLD_SERVO_HOST",
        },
        Golden {
            host: &SERVO,
            running: Os::Linux,
            built_for: Os::Linux,
            file_name: "manifold-view-servo",
            pin_platform: "linux-x64",
            sidecars: &[],
            pinned_files: &["manifold-view-servo"],
            installed_files: &["manifold-view-servo"],
            runtime_lib: None,
            runtime_in: None,
            installed_in: &["manifold-view-servo"],
            beside: &["manifold-view-servo"],
            walks: Some("target-servo"),
            embed_var: "MANIFOLD_SERVO_HOST",
        },
        Golden {
            host: &CEF,
            running: Os::Windows,
            built_for: Os::Windows,
            file_name: "manifold-view-cef.exe",
            pin_platform: "windows-x64",
            sidecars: &[],
            pinned_files: &["manifold-view-cef.exe"],
            installed_files: &["manifold-view-cef.exe", "cef-runtime/libcef.dll"],
            runtime_lib: Some("libcef.dll"),
            runtime_in: Some("cef-runtime"),
            installed_in: &["manifold-view-cef.exe"],
            beside: &["cef-runtime", "manifold-view-cef.exe"],
            walks: None,
            embed_var: "MANIFOLD_CEF_HOST",
        },
        Golden {
            host: &CEF,
            running: Os::Linux,
            built_for: Os::Linux,
            file_name: "manifold-view-cef",
            pin_platform: "linux-x64",
            sidecars: &[],
            pinned_files: &["manifold-view-cef"],
            installed_files: &["manifold-view-cef", "cef-runtime/libcef.so"],
            runtime_lib: Some("libcef.so"),
            runtime_in: Some("cef-runtime"),
            installed_in: &["manifold-view-cef"],
            beside: &["cef-runtime", "manifold-view-cef"],
            walks: None,
            embed_var: "MANIFOLD_CEF_HOST",
        },
        Golden {
            host: &WEBKIT,
            running: Os::Windows,
            built_for: Os::Linux,
            file_name: "manifold-view-webkitgtk",
            pin_platform: "linux-x64",
            sidecars: &[],
            pinned_files: &["manifold-view-webkitgtk"],
            installed_files: &["manifold-view-webkitgtk"],
            runtime_lib: None,
            runtime_in: None,
            installed_in: &["manifold-view-webkitgtk"],
            beside: &["manifold-view-webkitgtk"],
            walks: Some("target-linux"),
            embed_var: "MANIFOLD_WEBKIT_HOST",
        },
        Golden {
            host: &WEBKIT,
            running: Os::Linux,
            built_for: Os::Linux,
            file_name: "manifold-view-webkitgtk",
            pin_platform: "linux-x64",
            sidecars: &[],
            pinned_files: &["manifold-view-webkitgtk"],
            installed_files: &["manifold-view-webkitgtk"],
            runtime_lib: None,
            runtime_in: None,
            installed_in: &["manifold-view-webkitgtk"],
            beside: &["manifold-view-webkitgtk"],
            walks: Some("target-linux"),
            embed_var: "MANIFOLD_WEBKIT_HOST",
        },
        Golden {
            host: &WEBVIEW2,
            running: Os::Windows,
            built_for: Os::Windows,
            file_name: "manifold-view-webview2.exe",
            pin_platform: "windows-x64",
            sidecars: &[],
            pinned_files: &["manifold-view-webview2.exe"],
            installed_files: &["manifold-view-webview2.exe"],
            runtime_lib: None,
            runtime_in: None,
            installed_in: &["webview2", "manifold-view-webview2.exe"],
            beside: &["manifold-view-webview2.exe"],
            walks: None,
            embed_var: "MANIFOLD_WEBVIEW2_HOST",
        },
        Golden {
            host: &WEBVIEW2,
            running: Os::Linux,
            built_for: Os::Windows,
            file_name: "manifold-view-webview2.exe",
            pin_platform: "windows-x64",
            sidecars: &[],
            pinned_files: &["manifold-view-webview2.exe"],
            installed_files: &["manifold-view-webview2.exe"],
            runtime_lib: None,
            runtime_in: None,
            installed_in: &["webview2", "manifold-view-webview2.exe"],
            beside: &["manifold-view-webview2.exe"],
            walks: None,
            embed_var: "MANIFOLD_WEBVIEW2_HOST",
        },
        Golden {
            host: &PDFIUM,
            running: Os::Windows,
            built_for: Os::Windows,
            file_name: "manifold-view-pdfium.exe",
            pin_platform: "windows-x64",
            sidecars: &[],
            pinned_files: &["manifold-view-pdfium.exe"],
            installed_files: &["manifold-view-pdfium.exe"],
            runtime_lib: None,
            runtime_in: None,
            installed_in: &["pdfium", "manifold-view-pdfium.exe"],
            beside: &["manifold-view-pdfium.exe"],
            walks: None,
            embed_var: "MANIFOLD_PDFIUM_HOST",
        },
        Golden {
            host: &PDFIUM,
            running: Os::Linux,
            built_for: Os::Linux,
            file_name: "manifold-view-pdfium",
            pin_platform: "linux-x64",
            sidecars: &[],
            pinned_files: &["manifold-view-pdfium"],
            installed_files: &["manifold-view-pdfium"],
            runtime_lib: None,
            runtime_in: None,
            installed_in: &["pdfium", "manifold-view-pdfium"],
            beside: &["manifold-view-pdfium"],
            walks: None,
            embed_var: "MANIFOLD_PDFIUM_HOST",
        },
    ];

    /// `base` joined with each of `parts` in turn.
    fn under(base: &Path, parts: &[&str]) -> PathBuf {
        parts.iter().fold(base.to_path_buf(), |path, part| path.join(part))
    }

    /// `manifold.exe`'s folder in the golden cases (`d4/d3/d2/d1/exe`), then its four parents, nearest first.
    fn exe_dir_and_parents() -> Vec<PathBuf> {
        let parts = ["d4", "d3", "d2", "d1", "exe"];
        (1..=parts.len()).rev().map(|n| parts[..n].iter().collect::<PathBuf>()).collect()
    }

    /// Every row's derived values, for Windows and for Linux, against the literals above. Expected paths are built
    /// from literal components with `Path::join`, so they compare equal on either OS. An edit to a row changes a
    /// literal here by hand, so the app and the gate cannot agree on a typo silently.
    #[test]
    fn every_rows_derived_values_are_the_ones_written_here() {
        for host in ALL {
            for running in [Os::Windows, Os::Linux] {
                assert!(
                    GOLDEN.iter().any(|golden| golden.host.id == host.id && golden.running == running),
                    "no golden case for {} on {running:?}",
                    host.id
                );
            }
        }
        let hosts = Path::new("hosts");
        let exe_dirs = exe_dir_and_parents();
        let exe_dir = &exe_dirs[0];
        for golden in &GOLDEN {
            let (host, running) = (golden.host, golden.running);
            let case = format!("{} on {running:?}", host.id);
            assert_eq!(host.built_for(running), golden.built_for, "{case}");
            assert_eq!(host.file_name(running), golden.file_name, "{case}");
            assert_eq!(host.pin_platform(running), golden.pin_platform, "{case}");
            let sidecars: Vec<&str> = host.sidecars_for(running).map(|sidecar| sidecar.file).collect();
            assert_eq!(sidecars, golden.sidecars, "{case}");
            assert_eq!(host.pinned_files(running), golden.pinned_files, "{case}");
            assert_eq!(host.installed_files(running), golden.installed_files, "{case}");
            assert_eq!(host.runtime_lib(running), golden.runtime_lib, "{case}");
            assert_eq!(host.runtime_in(hosts), golden.runtime_in.map(|dir| hosts.join(dir)), "{case}");
            assert_eq!(host.installed_in(running, hosts), under(hosts, golden.installed_in), "{case}");
            let beside = under(exe_dir, golden.beside);
            assert_eq!(host.beside(running, exe_dir), beside, "{case}");
            assert_eq!(host.in_place(running, exe_dir, false), std::slice::from_ref(&beside), "{case}");
            let mut walked: Vec<PathBuf> = match golden.walks {
                Some(dev) => exe_dirs.iter().map(|dir| under(dir, &[dev, "debug", golden.file_name])).collect(),
                None => Vec::new(),
            };
            walked.push(beside);
            assert_eq!(host.in_place(running, exe_dir, true), walked, "{case}");
            assert_eq!(host.embed_var(), golden.embed_var, "{case}");
        }
    }

    /// The release is built on Windows, where a host built for another OS runs in a VM (the gate's `SITE_HOST`
    /// case): only WebKit, whose file keeps its Linux name there (H11).
    #[test]
    fn only_webkit_is_built_for_another_os_than_windows() {
        let foreign: Vec<&str> =
            ALL.iter().filter(|host| host.built_for(Os::Windows) != Os::Windows).map(|host| host.id).collect();
        assert_eq!(foreign, ["webkit"]);
        assert_eq!(WEBKIT.file_name(Os::Windows), "manifold-view-webkitgtk");
    }

    fn files(names: &[&str]) -> Option<Vec<String>> {
        Some(names.iter().map(|name| name.to_string()).collect())
    }

    #[test]
    fn a_manifest_may_list_a_host_only_under_the_platform_it_is_built_for() {
        assert_eq!(CEF.pinned_under("windows-x64"), files(&["manifold-view-cef.exe"]));
        assert_eq!(
            SERVO.pinned_under("windows-x64"),
            files(&["manifold-view-servo.exe", "libEGL.dll", "libGLESv2.dll"])
        );
        assert_eq!(WEBKIT.pinned_under("linux-x64"), files(&["manifold-view-webkitgtk"]));
        assert_eq!(WEBKIT.pinned_under("windows-x64"), None, "built for Linux");
        for unknown in ["windows-arm64", ""] {
            assert_eq!(CEF.pinned_under(unknown), None, "{unknown:?}");
        }
        for embedded in [&WEBVIEW2, &PDFIUM] {
            assert_eq!(embedded.pinned_under("windows-x64"), None, "{} is embedded", embedded.id);
        }
        // The table is OS-agnostic: the app looks up only `Os::here()`'s platform.
        assert_eq!(CEF.pinned_under("macos-x64"), files(&["manifold-view-cef"]));
        assert_eq!(CEF.pinned_under("linux-x64"), files(&["manifold-view-cef"]));
        assert_eq!(SERVO.pinned_under("linux-x64"), files(&["manifold-view-servo"]));
        assert_eq!(CEF.pinned_under("other-x64"), None, "no host is pinned for Os::Other");
    }

    #[test]
    fn a_row_is_found_by_its_id_ignoring_case() {
        for host in ALL {
            assert_eq!(by_id(host.id).map(|found| found.id), Some(host.id));
            assert_eq!(by_id(&host.id.to_ascii_uppercase()).map(|found| found.id), Some(host.id));
        }
        assert!(by_id("wkwebview").is_none());
        assert!(by_id("").is_none());
    }
}
```

- [ ] **Step 4: Run the tests to see them fail**

Run: `cargo test -j 8 -p manifold-hosts`

Expected: the build fails, because nothing the tests call exists yet. The errors are:
- `error[E0425]: cannot find value `SERVO` in this scope`, and the same for `CEF`, `WEBKIT`, `WEBVIEW2`, `PDFIUM` and `ALL`;
- `error[E0425]: cannot find function `by_id` in this scope`.

rustc may also list `error[E0599]` for `Os::from_consts`, `Os::ALL` and the `Host` methods. Any other error means this step's code was mistyped: fix the test module, not the expected values. This first cargo command also writes the `manifold-hosts` entry into `Cargo.lock`.

- [ ] **Step 5: `Os`'s methods**

In `crates/manifold-hosts/src/lib.rs`, replace:

```rust
pub enum Os {
    Windows,
    Linux,
    Mac,
    Other,
}
```

with:

```rust
pub enum Os {
    Windows,
    Linux,
    Mac,
    Other,
}

impl Os {
    /// In the order a refusal lists them ("needs Windows or Linux").
    pub const ALL: [Os; 4] = [Os::Windows, Os::Linux, Os::Mac, Os::Other];

    /// The OS of a `std::env::consts::OS` value.
    pub fn from_consts(os: &str) -> Os {
        match os {
            "windows" => Os::Windows,
            "linux" => Os::Linux,
            "macos" => Os::Mac,
            _ => Os::Other,
        }
    }

    /// The OS manifold is running on.
    pub fn here() -> Os {
        Os::from_consts(std::env::consts::OS)
    }

    /// Its name in a refusal ("needs Windows or Linux").
    pub fn name(self) -> &'static str {
        match self {
            Os::Windows => "Windows",
            Os::Linux => "Linux",
            Os::Mac => "macOS",
            Os::Other => "another OS",
        }
    }

    /// The manifest's name for the platform a host built for this OS is pinned under.
    pub fn pin_platform(self) -> &'static str {
        match self {
            Os::Windows => "windows-x64",
            Os::Linux => "linux-x64",
            Os::Mac => "macos-x64",
            // No host is pinned for it, so a lookup finds nothing.
            Os::Other => "other-x64",
        }
    }

    /// The suffix of an exe built for this OS.
    pub fn exe_suffix(self) -> &'static str {
        match self {
            Os::Windows => ".exe",
            Os::Linux | Os::Mac | Os::Other => "",
        }
    }
}
```

- [ ] **Step 6: The rows, `ALL` and `by_id`**

In `crates/manifold-hosts/src/lib.rs`, replace:

```rust
    pub sidecars: &'static [Sidecar],
    pub runtime: Option<Runtime>,
}
```

with:

```rust
    pub sidecars: &'static [Sidecar],
    pub runtime: Option<Runtime>,
}

/// The file that marks CEF's runtime folder, per OS the host is built for: `libcef.dll` on Windows, `libcef.so`
/// elsewhere (a macOS CEF would need an app-bundle layout, not written yet).
fn cef_runtime_lib(os: Os) -> &'static str {
    match os {
        Os::Windows => "libcef.dll",
        Os::Linux | Os::Mac | Os::Other => "libcef.so",
    }
}

/// Servo: built for whichever OS manifold runs on, in its own build folder; on Windows ANGLE's two DLLs, from
/// mozangle's build output, sit beside it.
pub static SERVO: Host = Host {
    id: "servo",
    exe: "manifold-view-servo",
    os: None,
    dev_dir: Some("target-servo"),
    delivery: Delivery::Download,
    sidecars: &[
        Sidecar { file: "libEGL.dll", os: Os::Windows, from: BuildOutput { krate: "mozangle", tail: "libEGL.dll" } },
        Sidecar {
            file: "libGLESv2.dll",
            os: Os::Windows,
            from: BuildOutput { krate: "mozangle", tail: "libGLESv2.dll" },
        },
    ],
    runtime: None,
};

/// CEF: built for whichever OS manifold runs on, in `target/`. Its runtime folder holds the runtime and the copy of
/// the host a tab runs. CEF ships its own `libEGL.dll`/`libGLESv2.dll` (ANGLE), and Windows resolves an exe's implicit
/// DLL dependencies from the exe's own folder first, so a CEF host in one folder with another ANGLE user would load
/// whichever copy was put there last.
pub static CEF: Host = Host {
    id: "cef",
    exe: "manifold-view-cef",
    os: None,
    dev_dir: None,
    delivery: Delivery::Download,
    sidecars: &[],
    runtime: Some(Runtime {
        dir: CEF_RUNTIME_DIR,
        lib: cef_runtime_lib,
        from: BuildOutput { krate: "cef-dll-sys", tail: "cef_windows_x86_64" },
    }),
};

/// WebKitGTK: built for Linux only (run through WSL on Windows), in its own build folder.
pub static WEBKIT: Host = Host {
    id: "webkit",
    exe: "manifold-view-webkitgtk",
    os: Some(Os::Linux),
    dev_dir: Some("target-linux"),
    delivery: Delivery::Download,
    sidecars: &[],
    runtime: None,
};

/// WebView2: Windows only, carried inside manifold.exe. A lone exe: its loader is statically linked by the
/// `webview2-com` crate.
pub static WEBVIEW2: Host = Host {
    id: "webview2",
    exe: "manifold-view-webview2",
    os: Some(Os::Windows),
    dev_dir: None,
    delivery: Delivery::Embedded,
    sidecars: &[],
    runtime: None,
};

/// The PDF renderer: no engine of its own (no `ViewKind` reaches it), carried inside manifold.exe.
pub static PDFIUM: Host = Host {
    id: "pdfium",
    exe: "manifold-view-pdfium",
    os: None,
    dev_dir: None,
    delivery: Delivery::Embedded,
    sidecars: &[],
    runtime: None,
};

/// Every row: `ViewKind::ALL`'s order, then PDFium.
pub static ALL: [&Host; 5] = [&SERVO, &CEF, &WEBKIT, &WEBVIEW2, &PDFIUM];

/// The row whose `id` is `id`, ignoring ASCII case as `ViewKind::from_name` does.
pub fn by_id(id: &str) -> Option<&'static Host> {
    ALL.iter().copied().find(|host| host.id.eq_ignore_ascii_case(id))
}
```

- [ ] **Step 7: `Host`'s methods**

In `crates/manifold-hosts/src/lib.rs`, replace:

```rust
#[cfg(test)]
mod tests {
```

with:

```rust
impl Host {
    /// The OS this host is built for when manifold runs on `running`.
    pub fn built_for(&self, running: Os) -> Os {
        self.os.unwrap_or(running)
    }

    /// The one file-name derivation: `exe` plus the suffix of the OS it is built for.
    pub fn file_name(&self, running: Os) -> String {
        format!("{}{}", self.exe, self.built_for(running).exe_suffix())
    }

    /// The manifest platform it is pinned under: that of the OS it is built for.
    pub fn pin_platform(&self, running: Os) -> &'static str {
        self.built_for(running).pin_platform()
    }

    /// Beside manifold.exe in `exe_dir`: flat, or inside the runtime folder for a host with one.
    pub fn beside(&self, running: Os, exe_dir: &Path) -> PathBuf {
        let file = self.file_name(running);
        match &self.runtime {
            Some(runtime) => exe_dir.join(runtime.dir).join(file),
            None => exe_dir.join(file),
        }
    }

    /// Where it runs in place from, in order: with `dev_folders`, `<dev_dir>/debug/<file>` in `exe_dir` and its four
    /// parents, nearest first; then `beside(running, exe_dir)`. The dev folder comes first: it is where a dev tree
    /// builds the host, and a stray copy left beside the exe must not shadow a fresh build.
    pub fn in_place(&self, running: Os, exe_dir: &Path, dev_folders: bool) -> Vec<PathBuf> {
        let file = self.file_name(running);
        let mut paths: Vec<PathBuf> = match self.dev_dir.filter(|_| dev_folders) {
            Some(dev) => exe_dir.ancestors().take(5).map(|dir| dir.join(dev).join("debug").join(&file)).collect(),
            None => Vec::new(),
        };
        paths.push(self.beside(running, exe_dir));
        paths
    }

    /// Where it is installed under `hosts_dir`: flat for `Download`, `<id>/<file>` for `Embedded`.
    pub fn installed_in(&self, running: Os, hosts_dir: &Path) -> PathBuf {
        let file = self.file_name(running);
        match self.delivery {
            Delivery::Download => hosts_dir.join(file),
            Delivery::Embedded => hosts_dir.join(self.id).join(file),
        }
    }

    /// `hosts_dir/<runtime.dir>` for a host with a runtime.
    pub fn runtime_in(&self, hosts_dir: &Path) -> Option<PathBuf> {
        self.runtime.as_ref().map(|runtime| hosts_dir.join(runtime.dir))
    }

    /// The runtime's marker file for the OS it is built for, for a host with a runtime.
    pub fn runtime_lib(&self, running: Os) -> Option<&'static str> {
        self.runtime.as_ref().map(|runtime| (runtime.lib)(self.built_for(running)))
    }

    /// The sidecars that sit beside it: those whose `os` is the one it is built for. The one filter, used by
    /// `pinned_files` and by the gate's `sync_host`.
    pub fn sidecars_for(&self, running: Os) -> impl Iterator<Item = &Sidecar> {
        let os = self.built_for(running);
        self.sidecars.iter().filter(move |sidecar| sidecar.os == os)
    }

    /// The files a manifest pins for it: its exe, then the file names of `sidecars_for(running)`.
    pub fn pinned_files(&self, running: Os) -> Vec<String> {
        let mut files = vec![self.file_name(running)];
        files.extend(self.sidecars_for(running).map(|sidecar| sidecar.file.to_owned()));
        files
    }

    /// `pinned_files`, plus `<runtime.dir>/<runtime_lib>` for a host with a runtime: what an install leaves on disk.
    pub fn installed_files(&self, running: Os) -> Vec<String> {
        let mut files = self.pinned_files(running);
        if let Some(runtime) = &self.runtime {
            files.push(format!("{}/{}", runtime.dir, (runtime.lib)(self.built_for(running))));
        }
        files
    }

    /// The files a manifest must list for this host under the platform string `platform`, or None when it may not
    /// list it there: the host is not `Download`, no `os` in `Os::ALL` has `pin_platform()` equal to `platform`, that
    /// `os` is `Os::Other` (no host is pinned for it), or that `os` is not the one this host is built for
    /// (`self.pin_platform(os) != platform`). The predicate the app's manifest parser uses; the gate's check of a
    /// release's manifest is stricter.
    pub fn pinned_under(&self, platform: &str) -> Option<Vec<String>> {
        if self.delivery != Delivery::Download {
            return None;
        }
        let os = Os::ALL.into_iter().find(|os| os.pin_platform() == platform)?;
        if os == Os::Other || self.pin_platform(os) != platform {
            return None;
        }
        Some(self.pinned_files(os))
    }

    /// The build variable that names this host's file for embedding: `MANIFOLD_<ID>_HOST`.
    pub fn embed_var(&self) -> String {
        format!("MANIFOLD_{}_HOST", self.id.to_ascii_uppercase())
    }
}

#[cfg(test)]
mod tests {
```

- [ ] **Step 8: Run the tests to see them pass**

Run: `cargo test -j 8 -p manifold-hosts`

Expected:
- `running 7 tests` for `src/lib.rs`, then `test result: ok. 7 passed; 0 failed`;
- `Doc-tests manifold_hosts` with `running 0 tests`.

If a golden assertion fails, the message names the row and OS (`cef on Linux`). Compare the row in Step 6 against the spec's "The crate" before touching a literal. A literal changes only where the spec says the derived value is different.

Run: `git diff -- Cargo.lock`

Expected: exactly four added lines, between the `name = "manifold"` package entry and the `name = "manifold-pages"` one:

```
+[[package]]
+name = "manifold-hosts"
+version = "0.9.0"
+
```

- [ ] **Step 9: Write the layout tests**

Create `crates/manifold-hosts/tests/layout.rs`:

```rust
//! The table against the repo's own files: the host crates' names, the ignored build folders, the release workflow,
//! and who may depend on the table. Plain substring matching on whole lines, leading whitespace trimmed and `#`
//! comment lines skipped: the leaf has no TOML, YAML or regex parser.

use manifold_hosts::{Delivery, Os, ALL};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The repo root: this crate is `crates/manifold-hosts`.
fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

/// The lines that carry content: leading whitespace trimmed, blank and `#` comment lines dropped.
fn lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines().map(str::trim_start).filter(|line| !line.is_empty() && !line.starts_with('#'))
}

/// A manifest's header lines (those starting `[`), each with its content lines up to the next header.
fn sections(text: &str) -> Vec<(&str, Vec<&str>)> {
    let mut sections: Vec<(&str, Vec<&str>)> = Vec::new();
    for line in lines(text) {
        if line.starts_with('[') {
            sections.push((line, Vec::new()));
        } else if let Some((_, body)) = sections.last_mut() {
            body.push(line);
        }
    }
    sections
}

/// Whether `line` holds `needle` followed by the line's end, whitespace or a quote, so a longer name never counts.
fn has_word(line: &str, needle: &str) -> bool {
    line.match_indices(needle).any(|(at, _)| {
        line[at + needle.len()..].chars().next().is_none_or(|next| next.is_whitespace() || next == '"' || next == '\'')
    })
}

fn release_yml() -> String {
    read(&repo().join(".github").join("workflows").join("release.yml"))
}

/// `crates/<exe>/Cargo.toml` has `name = "<exe>"` under `[package]`, and under each `[[bin]]` too.
#[test]
fn every_host_crate_is_named_for_its_exe() {
    for host in ALL {
        let path = repo().join("crates").join(host.exe).join("Cargo.toml");
        let text = read(&path);
        let name = format!("name = \"{}\"", host.exe);
        let mut packages = 0;
        for (header, body) in sections(&text) {
            let names: Vec<&str> =
                body.iter().copied().filter(|line| line.starts_with("name ") || line.starts_with("name=")).collect();
            if header == "[package]" {
                packages += 1;
            }
            if header == "[package]" || header == "[[bin]]" {
                assert_eq!(names, [name.as_str()], "{}: {header}", path.display());
            }
        }
        assert_eq!(packages, 1, "{}: one [package]", path.display());
    }
}

/// Every dev build folder is ignored: `.gitignore` has a `/<dev_dir>` line.
#[test]
fn every_dev_folder_is_ignored() {
    let text = read(&repo().join(".gitignore"));
    for host in ALL {
        let Some(dev) = host.dev_dir else { continue };
        let ignored = format!("/{dev}");
        assert!(lines(&text).any(|line| line.trim_end() == ignored), ".gitignore has no {ignored} line ({})", host.id);
    }
}

/// `release.yml` builds every host (`-p <exe>`), and its Linux job takes each host built for Linux from
/// `target/release/<file>` (H10: the two literals the gate cannot write there).
#[test]
fn the_release_builds_every_host() {
    let text = release_yml();
    for host in ALL {
        let build = format!("-p {}", host.exe);
        assert!(lines(&text).any(|line| has_word(line, &build)), "release.yml has no {build}");
        if host.os == Some(Os::Linux) {
            let built = format!("target/release/{}", host.file_name(Os::Linux));
            assert!(lines(&text).any(|line| has_word(line, &built)), "release.yml has no {built}");
        }
    }
}

/// The draft release uploads every platform the release pins. The `gh release create` line's `-Match '-(...)-x64$'`
/// alternation must equal, as a set, the OS parts of `pin_platform(Os::Windows)` over the `Download` rows, since the
/// release is built on Windows.
#[test]
fn the_release_uploads_every_platform_it_pins() {
    let text = release_yml();
    let line = lines(&text).find(|line| line.contains("gh release create")).expect("a gh release create line");
    let (_, rest) = line.split_once("-Match '-(").unwrap_or_else(|| panic!("no -Match '-( on: {line}"));
    let (alternation, _) = rest.split_once(")-x64$'").unwrap_or_else(|| panic!("no )-x64$' on: {line}"));
    let uploaded: BTreeSet<&str> = alternation.split('|').collect();
    let pinned: BTreeSet<&str> = ALL
        .iter()
        .filter(|host| host.delivery == Delivery::Download)
        .map(|host| host.pin_platform(Os::Windows).strip_suffix("-x64").expect("an -x64 platform"))
        .collect();
    assert_eq!(uploaded, pinned, "{line}");
}

/// Only the app and the gate depend on the table, and the table on nothing (H1). No other `crates/*/Cargo.toml`
/// names `manifold-hosts` on a content line. The leaf's own manifest has an empty body under every header that
/// names dependencies, target-specific ones included.
#[test]
fn only_the_app_and_the_gate_depend_on_the_table_and_it_on_nothing() {
    let mut read_the_leaf = false;
    for entry in std::fs::read_dir(repo().join("crates")).expect("read crates/") {
        let dir = entry.expect("an entry of crates/").path();
        let manifest = dir.join("Cargo.toml");
        if !manifest.is_file() {
            continue;
        }
        let text = read(&manifest);
        match dir.file_name().and_then(|name| name.to_str()).unwrap_or_default() {
            "manifold-hosts" => {
                read_the_leaf = true;
                for (header, body) in sections(&text) {
                    if header.contains("dependencies") {
                        assert!(body.is_empty(), "{}: {header} lists {body:?}", manifest.display());
                    }
                }
            }
            "manifold" | "gate" => {}
            _ => assert!(
                !lines(&text).any(|line| line.contains("manifold-hosts")),
                "{} names manifold-hosts",
                manifest.display()
            ),
        }
    }
    assert!(read_the_leaf, "crates/manifold-hosts/Cargo.toml was not read");
}
```

- [ ] **Step 10: Run the layout tests**

Run: `cargo test -j 8 -p manifold-hosts --test layout`

Expected: `running 5 tests`, then `test result: ok. 5 passed; 0 failed`.

A failure here names a file and line the table disagrees with. Check the row against the spec first, then the file at HEAD. Change neither the file nor the test to make it pass without reporting it.

- [ ] **Step 11: Watch two layout tests fail, then restore**

The layout tests pass against today's files, so this step breaks two of those files on purpose to see that the tests can fail. `.gitignore` and `release.yml` were clean in Step 1 and are restored from HEAD at the end of this step.

In `.gitignore`, at HEAD :2, replace:

```
/target-linux
```

with:

```
/target-lnx
```

In `.github/workflows/release.yml`, at HEAD :144, replace:

```
-Match '-(windows|linux)-x64$'
```

with:

```
-Match '-(windows)-x64$'
```

Run: `cargo test -j 8 -p manifold-hosts --test layout`

Expected: `test result: FAILED. 3 passed; 2 failed`, failing:
- `every_dev_folder_is_ignored`, with `.gitignore has no /target-linux line (webkit)`;
- `the_release_uploads_every_platform_it_pins`, with `assertion `left == right` failed`. The left is `{"windows"}` and the right is `{"linux", "windows"}`.

Restore both files and check:

```bash
git checkout HEAD -- .gitignore .github/workflows/release.yml
git diff --quiet HEAD -- .gitignore .github/workflows/release.yml && echo restored
```

Expected: `restored`.

Run: `cargo test -j 8 -p manifold-hosts --test layout`

Expected: `test result: ok. 5 passed; 0 failed`.

- [ ] **Step 12: The tier tests for the leaf and for any later crate (failing)**

In `crates/gate/src/tier.rs`, at HEAD :282, replace:

```rust
        assert_eq!(tier_of(&["crates/manifold-pages/assets/start.html"], &[]), "TIER full");
    }
```

with:

```rust
        assert_eq!(tier_of(&["crates/manifold-pages/assets/start.html"], &[]), "TIER full");
        assert_eq!(tier_of(&["crates/manifold-hosts/src/lib.rs"], &[]), "TIER full");
        assert_eq!(tier_of(&["crates/manifold-hosts/tests/layout.rs"], &[]), "TIER full");
        // Every crate but the app is Full, so a crate added later needs no edit here.
        assert_eq!(tier_of(&["crates/some-later-crate/src/lib.rs"], &[]), "TIER full");
        // The app crate is classified by its own lists: a file outside its source folder is on none.
        assert_eq!(tier_of(&["crates/manifold/build.rs"], &[]), "TIER light");
    }
```

Today the three Full paths reach `light("path on no list")` (HEAD :151). `crates/manifold-hosts/Cargo.toml` is already Full through the `crates/*/Cargo.toml` arm (:107), so it is not a case here. The last assertion passes before and after: it pins the boundary the new arm must not cross.

- [ ] **Step 13: Run it to see it fail**

Run: `cargo test -j 8 -p gate the_gate_and_the_crate_manifests_are_full`

Expected: `test result: FAILED. 0 passed; 1 failed`. The panic is at the first new line (`crates/gate/src/tier.rs:283`), with `left: "TIER light"` and `right: "TIER full"`.

- [ ] **Step 14: The Full arm**

In `crates/gate/src/tier.rs`, at HEAD :107-113, replace:

```rust
        || (p.starts_with("crates/") && p.ends_with("/Cargo.toml"))
        || under("crates/manifold-view-protocol")
        || under("crates/manifold-pages")
        || (p.starts_with("crates/manifold-view-") && p["crates/".len()..].contains('/'))
    {
        return full("dependency, protocol, pages or engine crate");
    }
```

with:

```rust
        || (p.starts_with("crates/") && (p.ends_with("/Cargo.toml") || !under("crates/manifold")))
    {
        return full("dependency, or any crate but the app (the gate, protocol, pages, host table and engines)");
    }
```

Then delete the arm the new one makes dead (HEAD :122-124):

```rust
    if under("crates/gate") {
        return full("the gate's own code");
    }
```

`under` needs the `/` after its prefix, so `crates/manifold-hosts/...` is not under `crates/manifold`. The arm covers every path under `crates/` outside `crates/manifold`, not only crate directories. It moves no tracked path: every crate but the app (`gate`, `manifold-pages`, `manifold-view-*`) was Full already, `git ls-files crates` shows no other crate, and no tracked file sits directly in `crates/`.

Nothing else reads the old reason texts: before the edit, `git grep -n -F -e "pages or engine crate" -e "the gate's own code" -- crates` finds only the two lines this step removes.

- [ ] **Step 15: Run the tier tests**

Run: `cargo test -j 8 -p gate tier::tests`

Expected: `test result: ok. 6 passed; 0 failed` (plus the count of tests the filter left out).

- [ ] **Step 16: `-p manifold-hosts` in CI, the hook and CLAUDE.md**

In `.github/workflows/ci.yml`, at HEAD :16, replace:

```yaml
      - run: cargo clippy --locked -p manifold -p manifold-view-protocol -p manifold-pages -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings
```

with:

```yaml
      - run: cargo clippy --locked -p manifold -p manifold-hosts -p manifold-view-protocol -p manifold-pages -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings
```

In `.github/workflows/ci.yml`, at HEAD :17, replace:

```yaml
      - run: cargo test --locked -p manifold -p manifold-view-protocol -p manifold-pages -p manifold-view-webview2 -p gate
```

with:

```yaml
      - run: cargo test --locked -p manifold -p manifold-hosts -p manifold-view-protocol -p manifold-pages -p manifold-view-webview2 -p gate
```

In `.github/workflows/ci.yml`, at HEAD :24, replace:

```yaml
      - run: cargo test --locked -p manifold -p manifold-view-protocol # catches cfg(windows) leaks; no apt step: neither crate needs GTK
```

with:

```yaml
      - run: cargo test --locked -p manifold -p manifold-hosts -p manifold-view-protocol # catches cfg(windows) leaks; no apt step: none of the three needs GTK
```

In `.githooks/pre-commit`, at HEAD :7, replace:

```sh
cargo clippy -q -j 8 -p manifold -p manifold-view-protocol -p manifold-pages -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings || { echo "pre-commit: clippy failed" >&2; exit 1; }
```

with:

```sh
cargo clippy -q -j 8 -p manifold -p manifold-hosts -p manifold-view-protocol -p manifold-pages -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings || { echo "pre-commit: clippy failed" >&2; exit 1; }
```

In `CLAUDE.md`, at HEAD :11, replace:

```
`cargo clippy -j 8 -p manifold -p manifold-view-protocol -p manifold-pages
```

with:

```
`cargo clippy -j 8 -p manifold -p manifold-hosts -p manifold-view-protocol -p manifold-pages
```

Run: `grep -c -- "-p manifold -p manifold-hosts -p manifold-view-protocol" .github/workflows/ci.yml .githooks/pre-commit CLAUDE.md`

Expected: `.github/workflows/ci.yml:3`, `.githooks/pre-commit:1`, `CLAUDE.md:1`.

`ci.yml:17` still lists no `-p manifold-view-pdfium`. That gap was already there and is PARKED ("The `-p` crate lists are repeated"); leave it.

- [ ] **Step 17: README's crate map and CI line**

In `README.md`, at HEAD :17 (the last line of the Architecture crate map), replace:

```
manifold-pages            the `manifold://` pages (static HTML) and their router; every engine host serves them through its own scheme hook
```

with:

```
manifold-pages            the `manifold://` pages (static HTML) and their router; every engine host serves them through its own scheme hook
manifold-hosts            the host table: one row per engine host (exe, build folders, delivery, runtime); std only, and only manifold and gate may depend on it
```

In `README.md`, at HEAD :35 (the **CI** paragraph), replace:

```
on Linux, `cargo test -p manifold -p manifold-view-protocol`.
```

with:

```
on Linux, `cargo test -p manifold -p manifold-hosts -p manifold-view-protocol`.
```

Run: `grep -n -E '^manifold-hosts +|-p manifold-hosts -p manifold-view-protocol' README.md`

Expected: two lines. `18:manifold-hosts            the host table: ...` sits inside the crate-map code block, and `36:**CI** ...` contains the Linux test line.

- [ ] **Step 18: Format, test and lint everything this task touched**

```bash
cargo fmt --all
cargo test --locked -j 8 -p manifold-hosts
cargo test --locked -j 8 -p gate tier::tests
cargo clippy -j 8 -p manifold -p manifold-hosts -p manifold-view-protocol -p manifold-pages -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings
git diff --stat
git status --short
```

Expected:
- `cargo fmt --all` may rewrap lines in `crates/manifold-hosts`; that is fine.
- The tests give `7 passed` (lib), `5 passed` (layout), `0` doc tests and `6 passed` (tier). `--locked` passes because Step 4 already wrote `Cargo.lock`.
- clippy finishes with no clippy finding; if cargo reports the ``unused workspace dependency `manifold-hosts` `` warning from Step 2, it is expected, here and in the tests above. With a cold cache it can take several minutes; give it a 600000 ms timeout.
- `git diff --stat` lists exactly `.githooks/pre-commit`, `.github/workflows/ci.yml`, `CLAUDE.md`, `Cargo.lock`, `Cargo.toml`, `README.md` and `crates/gate/src/tier.rs`.
- `git status --short` also shows `?? crates/manifold-hosts/`.

A clippy finding in the new crate is fixed in the code. It is never silenced with an `#[allow]` that gives no reason (CLAUDE.md "Formatting and lints"). If the fix changes a public signature from the Interfaces block above, stop and report it, because Tasks 2-4 call it as written there.

- [ ] **Step 19: The same tests on Linux (WSL)**

The golden test claims one test binary checks both OSes and compares paths built with `Path::join`. A Linux run proves that, and so does the CI Linux job (`ci.yml:24`), but that job runs only on a push, which this plan never makes.

Run: `wsl.exe -d Ubuntu -- bash -lc 'pgrep -a cargo; rustup toolchain list'`

Expected:
- no `cargo` process (another session's WebKit build in `target-linux` would block on the lock);
- a toolchain line starting `nightly-2026-09-23`.

If the nightly is missing, skip the rest of this step and say so in the task report ("Linux run left to CI"). Do not let rustup download a toolchain here. If a `cargo` process is listed, wait for it to exit.

Run (timeout 600000 ms):

```bash
wsl.exe -d Ubuntu -- bash -lc 'cd "/mnt/c/Users/Winston Li/Documents/GitHub/manifold" && CARGO_TARGET_DIR=target-linux cargo test --locked -j 8 -p manifold-hosts'
```

Expected: `7 passed` and `5 passed`, as on Windows. `target-linux` is the repo's WSL build folder (README "Building", the WebKit command), and it is git-ignored. If WSL's cargo has not resolved this workspace before, it first fetches the locked sources it needs to load the workspace; that is cargo's own fetch, and it adds no package to the lock (`--locked`).

- [ ] **Step 20: Commit**

```bash
cargo fmt --all
git diff --stat
git add Cargo.toml Cargo.lock crates/manifold-hosts crates/gate/src/tier.rs .github/workflows/ci.yml .githooks/pre-commit CLAUDE.md README.md
git status --short
git commit -F - <<'EOF'
hosts: one table per host, in a zero-dependency leaf crate

crates/manifold-hosts holds a row per engine host (Servo, CEF, WebKit,
WebView2, PDFium): its exe, dev build folder, delivery, sidecars and
runtime, and the file names and paths derived from them for the OS
manifold runs on. Nothing reads it yet; the app, its build script and
the gate move onto it next, and runtime::Os stays until the app takes
the leaf's.

A golden test writes every row's derived values out by hand for Windows
and Linux. tests/layout.rs ties the table to the host crates' names,
.gitignore and release.yml, and keeps the leaf free of dependencies and
off every crate but manifold and gate. The gate's tier sends every
crate but the app to full; CI, the pre-commit hook and CLAUDE.md
lint and test it.

<attribution>
EOF
git log -1 --format=%B | grep -v '^$' | tail -n 1
```

Expected:
- `git diff --stat` matches Step 18's.
- `git status --short` after the add shows nothing untracked.
- The pre-commit hook runs fmt and clippy (Rust files are staged) and passes.
- The last line printed is the attribution line.
- Nothing is pushed.

### Task 2: The app reads the table

#### Part A: ViewDesc, Os, in-place lookup, install, pins

Part A moves the app crate onto `manifold_hosts`:
- `ViewDesc` reads its host row.
- `Os` comes from the leaf.
- The in-place lookup reads `Host::in_place`.
- The install reads `Host::file_name`, `pin_platform` and `runtime_lib`.
- `host_pin::parse` holds each manifest entry to `Host::pinned_under`.

Part B follows in the same working tree, with no commit in between. It covers:
- the embedded codegen (`build.rs`, `embedded_host.rs`);
- `prepare_webview2` and `pdf_view_exe`;
- the names guard, `deleted_names.rs` and `pdf_render.rs:140`.

Part B ends with the task's only commit.

**The tree does not build between the end of this part and the end of Part B.** At the end of Part A:
- `prepare_webview2` still names `HostSource` and `kind.desc().source`.
- `pdf_view_exe` still calls `sibling_binary` and `embedded_host::Embedded`.

So this part runs **no cargo command of any kind**. Its last step is a `git diff --stat` check. Part B runs `cargo fmt`, clippy and the tests over both halves, then commits.

How to work:
- Make every edit with the Edit tool. The old text is exact.
- Line hints ("at HEAD :1011") are line numbers at `10444ae5`. Task 1 changes none of the files below.
- Edits within a file go top to bottom, and each file is finished before the next starts. After an earlier edit in the same file, a hint is approximate; the old text is what to match.
- Run no Python.

**Files:**
- Modify: `crates/manifold/Cargo.toml`: `manifold-hosts` in `[dependencies]` (:18) and `[build-dependencies]` (:42-43).
- Modify: `crates/manifold/src/process_manager.rs`:
  - the `manifold_hosts` import (:20);
  - `HostSource` and `CEF_RUNTIME_DIR` deleted (:36-49);
  - the `ViewDesc` doc and fields (:51-78);
  - the four rows (:121-132, :148-159, :172-185, :204-215);
  - `from_name` (:238) and the serializer (:256);
  - `shipped_cef_host` (:641-648);
  - `spawn_tab`'s filter (:1054);
  - the `availability` import lists (:1179-1187) and the `runtime` import (:1203).
- Modify: `crates/manifold/src/session_sync/tests/misc.rs` (:201-204), `crates/manifold/src/debug_control.rs` (:452) and `crates/manifold/src/debug_export.rs` (:420).
- Modify: `crates/manifold/src/process_manager/runtime.rs`:
  - the module doc (:3-6) and the imports (:8-9);
  - `Os` deleted (:14-62);
  - `for_os` (:89-94);
  - the copied test deleted (:198-211);
  - `a_wsl_host_speaks_tcp` (:233).
- Modify: `crates/manifold/src/settings.rs` (:189, :392).
- Modify: `crates/manifold/src/process_manager/availability.rs`:
  - `:4`-`:66`: `sibling_binary`, `host_file_name`, `beside_exe` and `in_place_paths` deleted; `run_in_place` and `run_in_place_from` rewritten;
  - `:76`-`:121`: `view_binary_path`, `view_hosts_dir`, `cef_sibling_exe` and `sibling_host`;
  - `:129`-`:147`: `pick_webkit_host` and `view_runtime_dir`;
  - `:227`: `read_on_disk`.
- Test: `crates/manifold/src/process_manager/tests.rs`:
  - the import (:3-4);
  - the three in-place tests rewritten (:549-636);
  - `each_engine_has_its_own_host_row_and_only_pdfium_has_no_engine` added after them;
  - `:1091`.
- Modify: `crates/manifold/src/view_install.rs`:
  - the imports (:13-15);
  - `downloads` and `has_runtime` (:132-141);
  - `host_platform` (:168-172);
  - `host_marker_path` (:251);
  - `validate_custom` (:317-318, :326);
  - `host_file_names` (:842-846);
  - `host_leftovers` (:860).
- Test: `crates/manifold/src/view_install/tests.rs`: `lib()` (:153-156), and `host_file_names_name_each_host_as_the_os_it_is_built_for_does` added after `webkit_installs_its_elf_under_its_own_marker` (:1403).
- Modify: `crates/manifold/src/settings_page.rs` (:472).
- Modify: `crates/manifold/src/cef_platform.rs`:
  - deleted: `runtime_lib` and `exe_suffix` (:11-14), `host_exe` (:28-31), their two values in each `PLATFORM` (:79-80, :94-95) and `host_exe_adds_the_platforms_suffix` (:118-122);
  - the struct doc (:7) is reworded.
- Modify and test: `crates/manifold/src/host_pin.rs`:
  - the module doc (:12-13), the import (:15) and the `parse` doc (:86-87);
  - the set check after the files loop (:114);
  - tests: the import (:124), two tests rewritten (:145-177) and one added.

**Interfaces:**
- Consumes from Task 1 (the leaf, its spec "The crate" API, exactly):
  - the workspace dependency `manifold-hosts = { path = "crates/manifold-hosts" }` in the root `[workspace.dependencies]`;
  - `HOSTS_DIR` and `CEF_RUNTIME_DIR`;
  - `Os`, with `ALL` in the order `[Windows, Linux, Mac, Other]` and `here`, `name`, `pin_platform`;
  - `Delivery`;
  - `Host`'s fields `id`, `os`, `dev_dir`, `delivery` and `runtime`;
  - `Host`'s methods `file_name`, `pin_platform`, `beside`, `in_place`, `installed_in`, `runtime_in`, `runtime_lib`, `pinned_files` and `pinned_under`;
  - the statics `SERVO`, `CEF`, `WEBKIT`, `WEBVIEW2`, `PDFIUM` and `ALL`.
- Consumes from Part B: the rewrites that make the tree build again:
  - `prepare_webview2` calls `embedded_host::ensure(kind.desc().host, &dir)` and no longer names `HostSource`;
  - `pdf_view_exe` calls `run_in_place(&PDFIUM)`, else `embedded_host::ensure(&PDFIUM, &dir)`;
  - `embedded_host.rs`, `build.rs`, the names guard and `deleted_names.rs`;
  - `cargo fmt`, the test and clippy runs, `Cargo.lock`, and the commit.
- Produces:

```rust
// process_manager.rs
use manifold_hosts::{Host, Os, CEF};            // availability.rs reaches all three through its `use super::*`
pub(crate) struct ViewDesc {
    pub(crate) code: u64,
    pub(crate) host: &'static Host,             // replaces name, binary, source, host_os, runtime_dir, dev_dir
    pub(crate) label: &'static str,
    pub(crate) tags: TagSet,
    pub(crate) link: runtime::HostLink,
    // starting_default .. picker_rank unchanged
}
use availability::{cef_sibling_exe, platform_unavailable_reason, run_in_place, sibling_host, view_binary_path,
    view_on_disk_fresh, webkit_host_path};      // run_in_place joins the plain list, for Part B's pdf_view_exe

// process_manager/availability.rs
pub(super) fn run_in_place(host: &Host) -> Option<PathBuf>;            // None under cfg(test), as before
pub(super) fn run_in_place_from(host: &Host, exe_dir: &Path, dev_folders: bool) -> Option<PathBuf>;

// view_install.rs
use manifold_hosts::{Delivery, Os, CEF, CEF_RUNTIME_DIR};

// host_pin.rs: parse refuses an entry unless kind.desc().host.pinned_under(platform) is Some and equal to its files
// as a set; the errors read "<id>/<platform>: the host table pins no files there" and
// "<id>/<platform>: lists {..}, the host table pins {..}"
```
- Deletes:
  - `process_manager::HostSource` and `process_manager::CEF_RUNTIME_DIR`;
  - `ViewDesc.{name, binary, source, host_os, runtime_dir, dev_dir}`;
  - `runtime::Os` and its test `an_os_name_from_the_standard_library_reads_as_one_of_four`, which Task 1 copied into the leaf (Task 2 deletes the originals);
  - `availability::{sibling_binary, host_file_name, beside_exe, in_place_paths}` and the `"engine-hosts"` literal;
  - `CefPlatform.{runtime_lib, exe_suffix}`, `CefPlatform::host_exe` and `host_exe_adds_the_platforms_suffix`.
- Unchanged signatures, which now read `kind.desc().host`:
  - `view_binary_path(ViewKind)`, `sibling_host(ViewKind)`, `cef_sibling_exe()`, `pick_webkit_host(&Path, Option<&Path>, bool)`, `view_runtime_dir(ViewKind)` and `view_hosts_dir()`;
  - `downloads`, `has_runtime` and `host_platform`;
  - `HostRuntime::for_os` and `unavailable_reason`;
  - `platform_default_view_kind(Os)`, whose `Os` is now the leaf's;
  - `host_pin::parse(&str, &str)`.

- [ ] **Step 1: The app depends on the leaf, as a crate and as a build script dependency**

In `crates/manifold/Cargo.toml`, at HEAD :18, replace:

```toml
manifold-pages.workspace = true
```

with:

```toml
manifold-hosts.workspace = true
manifold-pages.workspace = true
```

and at HEAD :42-43 replace:

```toml
[build-dependencies]
sha2.workspace = true
```

with:

```toml
[build-dependencies]
manifold-hosts.workspace = true
sha2.workspace = true
```

`build.rs` starts using the build dependency in Part B. `Cargo.lock` gains the entry on Part B's first cargo run, and Part B commits it.

- [ ] **Step 2: `process_manager.rs`: import the leaf; delete `HostSource` and `CEF_RUNTIME_DIR`; `ViewDesc` gets `host`**

At HEAD :20, replace:

```rust
use manifold_view_protocol::host_env;
```

with:

```rust
use manifold_hosts::{Host, Os, CEF};
use manifold_view_protocol::host_env;
```

At HEAD :36-78, replace this block, from the `HostSource` doc through the `dev_dir` field:

```rust
/// Where an engine's host executable comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostSource {
    /// Carried inside `manifold.exe` when the build embeds it (`MANIFOLD_WEBVIEW2_HOST`, see `embedded_host`) and
    /// extracted at spawn when no host is in place (`run_in_place`). A build that embeds nothing finds it there like
    /// any other.
    Embedded(embedded_host::Embedded),
    /// Pinned in `host_pin` and installed on demand into `engine-hosts/` (`view_install`), unless a host is already in
    /// place (a dev tree's build, or a Linux package's).
    Download,
}

/// CEF's runtime archive unpacked beside its host: `<hosts dir>/cef-runtime/`, and in a dev build next to the exe.
pub(crate) const CEF_RUNTIME_DIR: &str = "cef-runtime";

/// What each engine is: the one place everything that differs between engines lives -- name, store code, binary,
/// label, pixel order, jar use, which OS its host is built for and how it reaches manifold, how it is installed, and
/// how the picker shows it.
/// `desc` is an exhaustive `match`, so a new `ViewKind` fails to compile until it has a row here.
pub(crate) struct ViewDesc {
    /// The string `settings.json`, session exports and the `view_kind` store field hold.
    pub(crate) name: &'static str,
    /// The store's `view_kind` field code: `store::model::ENGINES[code - 1]` is `name`.
    pub(crate) code: u64,
    pub(crate) binary: &'static str,
    pub(crate) label: &'static str,
    /// What the host is and can do (`Tag`: its rendering-engine family, frame format, jar, DevTools and find). A live
    /// tab inherits them (`App::card_kind`).
    pub(crate) tags: TagSet,
    pub(crate) source: HostSource,
    /// The OS the host is built for (`runtime::HostRuntime::between` says where that runs); `None`: whichever OS
    /// manifold runs on.
    pub(crate) host_os: Option<runtime::Os>,
    /// How the host reaches manifold.
    pub(crate) link: runtime::HostLink,
    /// The directory name (`CEF_RUNTIME_DIR`) of a runtime archive it needs besides its host exe, which also takes a
    /// bring-your-own runtime directory: only CEF's, the one component that is an archive (`Pin`, `unpack`) rather
    /// than single pinned files.
    pub(crate) runtime_dir: Option<&'static str>,
    /// The folder a dev tree builds this host in, beside `target/` (`<dev_dir>/debug/<host>`), when that is not
    /// `target/` itself: Servo's `target-servo` (README "Building Servo on its own") and WebKit's `target-linux` (built
    /// inside WSL). manifold runs a host found there in place (`availability::run_in_place`).
    pub(crate) dev_dir: Option<&'static str>,
```

with:

```rust
/// What each engine is: the one place everything that differs between engines lives -- name (its host's id), store
/// code, host, label, pixel order, jar use, how its host reaches manifold, and how the picker shows it. The host's own
/// layout (its exe, the OS it is built for, its dev folder, how it is delivered, its runtime) is its row in the host
/// table (`manifold_hosts`), which `build.rs` and the gate read too.
/// `desc` is an exhaustive `match`, so a new `ViewKind` fails to compile until it has a row here.
pub(crate) struct ViewDesc {
    /// The store's `view_kind` field code: `store::model::VIEW_KINDS[code - 1]` is `host.id`.
    pub(crate) code: u64,
    /// The host's row: its `id` is the string `settings.json`, session exports and the `view_kind` store field hold,
    /// and its `os` the OS it is built for (`runtime::HostRuntime::between` says where that runs).
    pub(crate) host: &'static Host,
    pub(crate) label: &'static str,
    /// What the host is and can do (`Tag`: its rendering-engine family, frame format, jar, DevTools and find). A live
    /// tab inherits them (`App::card_kind`).
    pub(crate) tags: TagSet,
    /// How the host reaches manifold.
    pub(crate) link: runtime::HostLink,
```

The deleted docs are not lost:
- The runtime-directory fact ("also takes a bring-your-own runtime directory ...") moves to `has_runtime` (Step 15).
- The dev-folder fact moves to `run_in_place_from` (Step 9).
- `use crate::embedded_host;` (:15) stays. `spawn_tab` uses it (`embedded_host::spawn_fresh`, :1109), and so does Part B's code.

- [ ] **Step 3: `process_manager.rs`: each row names its host**

At HEAD :121-132, replace:

```rust
            ViewKind::Servo => ViewDesc {
                name: "servo",
                code: 1,
                binary: "manifold-view-servo",
                label: "Servo",
                tags: TagSet::of(&[Tag::Servo]),
                source: HostSource::Download,
                host_os: None,
                link: HostLink::PipesShm,
                runtime_dir: None,
                dev_dir: Some("target-servo"),
                starting_default: true,
```

with:

```rust
            ViewKind::Servo => ViewDesc {
                code: 1,
                host: &manifold_hosts::SERVO,
                label: "Servo",
                tags: TagSet::of(&[Tag::Servo]),
                link: HostLink::PipesShm,
                starting_default: true,
```

At HEAD :148-159, replace:

```rust
            ViewKind::Cef => ViewDesc {
                name: "cef",
                code: 2,
                binary: "manifold-view-cef",
                label: "Blink (CEF)",
                tags: TagSet::of(&[Tag::Blink, Tag::Bgra, Tag::Jars, Tag::DevTools, Tag::Find]),
                source: HostSource::Download,
                host_os: None,
                link: HostLink::PipesShm,
                runtime_dir: Some(CEF_RUNTIME_DIR),
                dev_dir: None,
                starting_default: false,
```

with:

```rust
            ViewKind::Cef => ViewDesc {
                code: 2,
                host: &manifold_hosts::CEF,
                label: "Blink (CEF)",
                tags: TagSet::of(&[Tag::Blink, Tag::Bgra, Tag::Jars, Tag::DevTools, Tag::Find]),
                link: HostLink::PipesShm,
                starting_default: false,
```

At HEAD :172-185, replace:

```rust
            ViewKind::Webkit => ViewDesc {
                name: "webkit",
                code: 3,
                binary: "manifold-view-webkitgtk",
                label: "WebKit",
                tags: TagSet::of(&[Tag::WebKit, Tag::Find]),
                source: HostSource::Download,
                // A Linux program: native on Linux, through WSL on Windows (pinned under linux-x64; README "Engines by OS").
                // TCP because nothing else crosses the WSL boundary.
                host_os: Some(Os::Linux),
                link: HostLink::Tcp,
                runtime_dir: None,
                dev_dir: Some("target-linux"),
                starting_default: false,
```

with:

```rust
            ViewKind::Webkit => ViewDesc {
                code: 3,
                // A Linux program (`WEBKIT.os`): native on Linux, through WSL on Windows (pinned under linux-x64;
                // README "Engines by OS").
                host: &manifold_hosts::WEBKIT,
                label: "WebKit",
                tags: TagSet::of(&[Tag::WebKit, Tag::Find]),
                // TCP because nothing else crosses the WSL boundary.
                link: HostLink::Tcp,
                starting_default: false,
```

At HEAD :204-215, replace:

```rust
            ViewKind::Webview2 => ViewDesc {
                name: "webview2",
                code: 4,
                binary: "manifold-view-webview2",
                label: "WebView2",
                tags: TagSet::of(&[Tag::Blink, Tag::Bgra, Tag::Jars, Tag::DevTools, Tag::Find]),
                source: HostSource::Embedded(embedded_host::Embedded::Webview2),
                host_os: Some(Os::Windows),
                link: HostLink::PipesShm,
                runtime_dir: None,
                dev_dir: None,
                starting_default: true,
```

with:

```rust
            ViewKind::Webview2 => ViewDesc {
                code: 4,
                host: &manifold_hosts::WEBVIEW2,
                label: "WebView2",
                tags: TagSet::of(&[Tag::Blink, Tag::Bgra, Tag::Jars, Tag::DevTools, Tag::Find]),
                link: HostLink::PipesShm,
                starting_default: true,
```

Each removed value is the row's value in the leaf:
- the names `servo`, `cef`, `webkit`, `webview2` are the ids;
- the binaries are the `exe`s;
- `host_os` is `os`: WebKit `Some(Linux)`, WebView2 `Some(Windows)`, the others `None`;
- `runtime_dir: Some(CEF_RUNTIME_DIR)` is `CEF.runtime`;
- `dev_dir` is `dev_dir`: `target-servo` and `target-linux`;
- `source` is `delivery`: WebView2 Embedded, the others Download.

Task 1's golden test pins those values.

- [ ] **Step 4: `process_manager.rs`: identity, the CEF host fallback and the custom-folder filter read the row**

At HEAD :238, replace:

```rust
        Self::ALL.into_iter().find(|k| k.desc().name.eq_ignore_ascii_case(name))
```

with:

```rust
        Self::ALL.into_iter().find(|k| k.desc().host.id.eq_ignore_ascii_case(name))
```

At HEAD :256, replace:

```rust
        kind.desc().name.serialize(s)
```

with:

```rust
        kind.desc().host.id.serialize(s)
```

At HEAD :641-648, replace:

```rust
fn shipped_cef_host() -> PathBuf {
    sibling_host(ViewKind::Cef)
        .or_else(|| {
            let file = crate::cef_platform::PLATFORM.host_exe(ViewKind::Cef.desc().binary);
            view_hosts_dir().map(|dir| dir.join(file)).filter(|path| path.exists())
        })
        .unwrap_or_else(cef_sibling_exe)
}
```

with:

```rust
fn shipped_cef_host() -> PathBuf {
    sibling_host(ViewKind::Cef)
        .or_else(|| view_hosts_dir().map(|dir| CEF.installed_in(Os::here(), &dir)).filter(|path| path.exists()))
        .unwrap_or_else(cef_sibling_exe)
}
```

At HEAD :1054 (in `spawn_tab`), replace:

```rust
    let custom_cef = custom_cef.filter(|_| desc.runtime_dir.is_some());
```

with:

```rust
    let custom_cef = custom_cef.filter(|_| desc.host.runtime.is_some());
```

Leave `prepare_webview2` (:1011-1025) alone; Part B rewrites it.

- [ ] **Step 5: `process_manager.rs`: the import lists**

At HEAD :1179-1187, replace:

```rust
#[cfg(test)]
use availability::{
    availability_with, host_file_name, on_disk_cached, pick_webkit_host, read_on_disk_at, run_in_place_from,
    stamp_cached, OnDiskCache, StampCache, ON_DISK_CHECK_TTL,
};
use availability::{
    cef_sibling_exe, platform_unavailable_reason, sibling_binary, sibling_host, view_binary_path, view_on_disk_fresh,
    webkit_host_path,
};
```

with:

```rust
#[cfg(test)]
use availability::{
    availability_with, on_disk_cached, pick_webkit_host, read_on_disk_at, run_in_place_from, stamp_cached, OnDiskCache,
    StampCache, ON_DISK_CHECK_TTL,
};
use availability::{
    cef_sibling_exe, platform_unavailable_reason, run_in_place, sibling_host, view_binary_path, view_on_disk_fresh,
    webkit_host_path,
};
```

At HEAD :1203, replace:

```rust
use runtime::{HostLink, HostRuntime, Os};
```

with:

```rust
use runtime::{HostLink, HostRuntime};
```

`Os` now comes from Step 2's `use manifold_hosts::{Host, Os, CEF};`. It is used by `unsupported_on` (:680), `spawn_tab` (:1051) and `shipped_cef_host`. Through `use super::*` it also reaches `availability.rs` and `tests.rs`.

Leave `pdf_view_exe` (:1210-1221) alone. Part B rewrites it over `run_in_place(&PDFIUM)`. If Part B extends this file's `manifold_hosts` import with `PDFIUM`, `tests.rs`'s explicit `PDFIUM` (Step 12) then duplicates a glob-imported name. Rust lets an explicit import shadow a glob one, so it compiles either way.

- [ ] **Step 6: The id readers: the store-name test, `VIEWSTATE` and the debug export**

In `crates/manifold/src/session_sync/tests/misc.rs`, at HEAD :201-204 (in `view_table_matches_the_store_and_round_trips`), replace:

```rust
        assert_eq!(crate::store::model::VIEW_KINDS[i], desc.name, "the store names the same engine");
        assert_eq!(serde_json::to_value(Wrap(kind)).unwrap(), serde_json::json!(desc.name));
        assert_eq!(view_kind_of(view_code(kind)), kind);
        assert_eq!(ViewKind::from_name(&desc.name.to_uppercase()), Some(kind));
```

with:

```rust
        assert_eq!(crate::store::model::VIEW_KINDS[i], desc.host.id, "the store names the same engine");
        assert_eq!(serde_json::to_value(Wrap(kind)).unwrap(), serde_json::json!(desc.host.id));
        assert_eq!(view_kind_of(view_code(kind)), kind);
        assert_eq!(ViewKind::from_name(&desc.host.id.to_uppercase()), Some(kind));
```

In `crates/manifold/src/debug_control.rs`, at HEAD :451-452, replace:

```rust
                "view-state {}: {} [{}]",
                kind.desc().name,
```

with:

```rust
                "view-state {}: {} [{}]",
                kind.desc().host.id,
```

In `crates/manifold/src/debug_export.rs`, at HEAD :420, replace:

```rust
            (kind.desc().name.to_string(), serde_json::json!({"state": state, "disk": disk}))
```

with:

```rust
            (kind.desc().host.id.to_string(), serde_json::json!({"state": state, "disk": disk}))
```

The store-name test is the spec's "existing store-name test". It keeps the ids equal to `store::model::VIEW_KINDS`, and it now reads them from the table.

- [ ] **Step 7: `runtime.rs`: `Os` is the leaf's**

In `crates/manifold/src/process_manager/runtime.rs`, at HEAD :3-6, replace:

```rust
//! `Os` is the operating systems an engine host can target, and `Other` for every one without a host of its own. `HostRuntime` says how a host built for one OS runs
//! on another: natively on its own, or a Linux host through WSL on Windows. `HostLink` is how a host reaches
//! manifold, a property of the host, not of the runtime. A fourth compatibility layer (Wine) would be one more
//! `HostRuntime` and one more arm of `HostRuntime::between`.
```

with:

```rust
//! `Os` (the host table's, `manifold_hosts::Os`) is the operating systems an engine host can target, and `Other` for
//! every one without a host of its own; a host's row says which it is built for (`Host::os`). `HostRuntime` says how
//! a host built for one OS runs on another: natively on its own, or a Linux host through WSL on Windows. `HostLink` is
//! how a host reaches manifold, a property of the host, not of the runtime. A fourth compatibility layer (Wine) would
//! be one more `HostRuntime` and one more arm of `HostRuntime::between`.
```

At HEAD :8-9, replace:

```rust
use super::ViewKind;
use std::path::Path;
```

with:

```rust
use super::ViewKind;
use manifold_hosts::Os;
use std::path::Path;
```

At HEAD :14-64, delete `Os` and its `impl`. Replace this block, up to and including the next item's doc line:

```rust
/// An operating system a host is built for, or manifold runs on. `Other` is any not named here (FreeBSD and the like):
/// only the engines built for whichever OS manifold runs on (Servo, CEF) can run there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Os {
    Windows,
    Linux,
    Mac,
    Other,
}

impl Os {
    /// In the order a refusal lists them ("needs Windows or Linux").
    pub(crate) const ALL: [Os; 4] = [Os::Windows, Os::Linux, Os::Mac, Os::Other];

    /// The OS of a `std::env::consts::OS` value.
    pub(crate) fn from_consts(os: &str) -> Os {
        match os {
            "windows" => Os::Windows,
            "linux" => Os::Linux,
            "macos" => Os::Mac,
            _ => Os::Other,
        }
    }

    /// The OS manifold is running on.
    pub(crate) fn here() -> Os {
        Os::from_consts(std::env::consts::OS)
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Os::Windows => "Windows",
            Os::Linux => "Linux",
            Os::Mac => "macOS",
            Os::Other => "another OS",
        }
    }

    /// `host_pin`'s name for the platform a host built for this OS is pinned under.
    pub(crate) fn pin_platform(self) -> &'static str {
        match self {
            Os::Windows => "windows-x64",
            Os::Linux => "linux-x64",
            Os::Mac => "macos-x64",
            // No host is pinned for it, so a lookup finds nothing.
            Os::Other => "other-x64",
        }
    }
}

/// How a host built for one OS runs on another.
```

with:

```rust
/// How a host built for one OS runs on another.
```

At HEAD :89-90, replace:

```rust
    fn for_os(kind: ViewKind, here: Os) -> Option<HostRuntime> {
        match kind.desc().host_os {
```

with:

```rust
    fn for_os(kind: ViewKind, here: Os) -> Option<HostRuntime> {
        match kind.desc().host.os {
```

At HEAD :198-211, delete the test that Task 1 copied into the leaf. Replace:

```rust
    #[test]
    fn an_os_name_from_the_standard_library_reads_as_one_of_four() {
        assert_eq!(Os::from_consts("windows"), Os::Windows);
        assert_eq!(Os::from_consts("linux"), Os::Linux);
        assert_eq!(Os::from_consts("macos"), Os::Mac);
        for other in ["freebsd", "openbsd"] {
            assert_eq!(Os::from_consts(other), Os::Other, "{other}");
        }
        assert_eq!(Os::Windows.pin_platform(), "windows-x64");
        assert_eq!(Os::Linux.pin_platform(), "linux-x64");
    }

    #[test]
    fn the_reason_an_engine_is_unavailable_names_the_oses_that_can_run_it() {
```

with:

```rust
    #[test]
    fn the_reason_an_engine_is_unavailable_names_the_oses_that_can_run_it() {
```

At HEAD :233 (in `a_wsl_host_speaks_tcp`), replace:

```rust
                let Some(host) = kind.desc().host_os else { continue };
```

with:

```rust
                let Some(host) = kind.desc().host.os else { continue };
```

The other tests in this file stay as written:
- `a_host_runs_natively_on_its_own_os_and_in_wsl_for_linux_on_windows` uses `use Os::{Linux, Mac, Windows};`. That resolves through the module's `use super::*` to the leaf's `Os`.
- `the_reason_an_engine_is_unavailable_names_the_oses_that_can_run_it` keeps its literal "needs Windows or Linux". That literal checks the leaf's `Os::ALL` order.

`for_os` reads `host.os` as the spec says. `Host::built_for` would give the same answers (see the decisions).

- [ ] **Step 8: `settings.rs`: the two full paths**

In `crates/manifold/src/settings.rs`, at HEAD :188-190, replace:

```rust
            default_starting_view_kind: crate::process_manager::platform_default_view_kind(
                crate::process_manager::runtime::Os::here(),
            ),
```

with:

```rust
            default_starting_view_kind: crate::process_manager::platform_default_view_kind(manifold_hosts::Os::here()),
```

At HEAD :392, replace:

```rust
            crate::process_manager::platform_default_view_kind(crate::process_manager::runtime::Os::here())
```

with:

```rust
            crate::process_manager::platform_default_view_kind(manifold_hosts::Os::here())
```

- [ ] **Step 9: `availability.rs`: the in-place lookup is `Host::in_place`**

In `crates/manifold/src/process_manager/availability.rs`, at HEAD :4-66, replace this block. It runs from the `use` line through the end of `run_in_place_from`, deleting `sibling_binary`, `host_file_name`, `beside_exe` and `in_place_paths`:

```rust
use super::*;

/// A file beside `manifold.exe` named as this platform names an exe: the PDF renderer (`pdf_view_exe`), and CEF's
/// flat host (`sibling_host`).
pub(super) fn sibling_binary(name: &str) -> PathBuf {
    let mut path = std::env::current_exe().expect("current_exe");
    path.set_file_name(crate::cef_platform::PLATFORM.host_exe(name));
    path
}

/// The file name of `kind`'s host: as this platform names an exe (`PLATFORM.host_exe`), except a host built for another
/// OS than this one, which keeps its bare name (WebKit's ELF, run in WSL from Windows).
pub(super) fn host_file_name(kind: ViewKind) -> String {
    let desc = kind.desc();
    if desc.host_os.is_some_and(|os| os != Os::here()) {
        desc.binary.to_owned()
    } else {
        crate::cef_platform::PLATFORM.host_exe(desc.binary)
    }
}

/// Where `kind`'s host goes beside `manifold.exe` in `exe_dir`, whether or not it is there: flat, except CEF's, which
/// lives in its runtime folder (`cef-runtime/`). CEF ships its own `libEGL.dll`/`libGLESv2.dll` (ANGLE), and Windows
/// resolves an exe's implicit DLL dependencies from the exe's own folder first, so a CEF host in one folder with
/// another ANGLE user would load whichever copy was put there last. The other hosts are a lone exe beside their own
/// files (WebView2's loader is statically linked by the `webview2-com` crate), so they need no folder of their own.
fn beside_exe(kind: ViewKind, exe_dir: &Path) -> PathBuf {
    let file = host_file_name(kind);
    match kind.desc().runtime_dir {
        Some(runtime) => exe_dir.join(runtime).join(file),
        None => exe_dir.join(file),
    }
}

/// Every place `kind`'s host can run in place from, given `manifold.exe`'s folder `exe_dir`, in the order they are
/// tried: with `dev_folders`, its dev build folder (`ViewDesc::dev_dir`, then `debug/`) in `exe_dir` or any of its four
/// parents, nearest first; then beside `manifold.exe` (`beside_exe`). The dev folder comes first: it is where a dev tree
/// builds the host, and a stray copy left beside the exe must not shadow a fresh build.
fn in_place_paths(kind: ViewKind, exe_dir: &Path, dev_folders: bool) -> Vec<PathBuf> {
    let file = host_file_name(kind);
    let mut paths: Vec<PathBuf> = match kind.desc().dev_dir.filter(|_| dev_folders) {
        Some(dev) => exe_dir.ancestors().take(5).map(|dir| dir.join(dev).join("debug").join(&file)).collect(),
        None => Vec::new(),
    };
    paths.push(beside_exe(kind, exe_dir));
    paths
}

/// The host of `kind` that a spawn runs as it stands, when one is there (`in_place_paths`). It is never anything under
/// `view_hosts_dir()` (a download, hash-checked by its install, or the embedded WebView2 host), and for CEF only its
/// `cef-runtime/` copy, the one a tab runs. This is the exe whose protocol number `read_on_disk` checks. `None` under
/// `cfg(test)`, so no unit test reads a real host from disk (a dev tree's hosts are reachable from the test exe).
fn run_in_place(kind: ViewKind) -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    run_in_place_from(kind, &current_exe_dir(), walks_dev_folders())
}

/// `run_in_place` for a `manifold.exe` in `exe_dir`; `dev_folders`: whether to look in dev build folders.
pub(super) fn run_in_place_from(kind: ViewKind, exe_dir: &Path, dev_folders: bool) -> Option<PathBuf> {
    in_place_paths(kind, exe_dir, dev_folders).into_iter().find(|path| path.is_file())
}
```

with:

```rust
use super::*;
use manifold_hosts::{HOSTS_DIR, WEBKIT};

/// The copy of `host` that a spawn runs as it stands, when one is there: the first of its in-place paths
/// (`Host::in_place`) that is a file. It is never anything under `view_hosts_dir()` (a download, hash-checked by its
/// install, or an extracted embedded host), and for CEF only its `cef-runtime/` copy, the one a tab runs. This is the
/// exe whose protocol number `read_on_disk` checks, and the PDF view `pdf_view_exe` runs. `None` under `cfg(test)`, so
/// no unit test reads a real host from disk (a dev tree's hosts are reachable from the test exe).
pub(super) fn run_in_place(host: &Host) -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    run_in_place_from(host, &current_exe_dir(), walks_dev_folders())
}

/// `run_in_place` for a `manifold.exe` in `exe_dir`; `dev_folders`: whether to look in dev build folders
/// (`Host::dev_dir`: Servo's `target-servo`, README "Building Servo on its own"; WebKit's `target-linux`, built inside
/// WSL). The dev folder comes before the place beside the exe: it is where a dev tree builds the host, and a stray copy
/// left beside the exe must not shadow a fresh build.
pub(super) fn run_in_place_from(host: &Host, exe_dir: &Path, dev_folders: bool) -> Option<PathBuf> {
    host.in_place(Os::here(), exe_dir, dev_folders).into_iter().find(|path| path.is_file())
}
```

`Host` and `Os` reach this module through `use super::*` (Step 2's import). Only `HOSTS_DIR` and `WEBKIT` are imported here, so that nothing is imported twice. `walks_dev_folders` (:68-74) is unchanged.

- [ ] **Step 10: `availability.rs`: the install path, the hosts folder and the shipped hosts read the row**

At HEAD :76-121, replace this block, from the `view_binary_path` doc through the end of `sibling_host`:

```rust
/// Where to find each view-host's executable: the host that runs in place (`run_in_place`), else the copy
/// `view_install` puts in `view_hosts_dir()` (under `profile::view_data_dir()`), whether or not it is there yet: flat
/// for a downloaded host, in a folder of its own for the embedded one. (A downloaded CEF host is never run from there:
/// `prepare_cef` copies it beside the runtime's `libcef.dll`.) Without a data directory, the place beside
/// `manifold.exe` (`beside_exe`).
///
/// TODO: a dev tree's runtime files are put beside its hosts by `gate sync-runtime-deps` (README "Runtime-file
/// placement"), not by cargo; a real install or packaging step should do that.
pub(super) fn view_binary_path(kind: ViewKind) -> PathBuf {
    let desc = kind.desc();
    assert!(desc.link == HostLink::PipesShm, "{} is reached over TCP, not by a local path", desc.label);
    if let Some(exe) = run_in_place(kind) {
        return exe;
    }
    let Some(mut fallback) = view_hosts_dir() else { return beside_exe(kind, &current_exe_dir()) };
    if let HostSource::Embedded(host) = desc.source {
        // Where `spawn_tab` extracts the embedded host (`Embedded::ensure`), in a folder of its own.
        return host.host_path(&fallback);
    }
    fallback.push(host_file_name(kind));
    fallback
}

pub(crate) fn view_hosts_dir() -> Option<PathBuf> {
    crate::profile::view_data_dir().map(|mut dir| {
        dir.push("engine-hosts");
        dir
    })
}

/// `cef-runtime/<exe>` next to `manifold.exe`: where `gate sync-runtime-deps` puts it for dev builds.
pub(super) fn cef_sibling_exe() -> PathBuf {
    beside_exe(ViewKind::Cef, &current_exe_dir())
}

/// The host exe of `kind` that ships in place (a dev tree's, or a packaged one's), when it is there: the one that runs
/// in place (`run_in_place`), except CEF's, which is its flat exe beside `manifold.exe` first, then its `cef-runtime/`
/// copy (`shipped_cef_host` copies the flat one into a runtime that has no host). A host in place makes the host part
/// Ready whatever the pin table says.
pub(super) fn sibling_host(kind: ViewKind) -> Option<PathBuf> {
    if kind.desc().runtime_dir.is_none() {
        return run_in_place(kind);
    }
    let flat = Some(sibling_binary(kind.desc().binary)).filter(|path| path.exists());
    flat.or_else(|| Some(cef_sibling_exe()).filter(|path| path.exists()))
}
```

with:

```rust
/// Where to find each view-host's executable: the host that runs in place (`run_in_place`), else where it is installed
/// under `view_hosts_dir()` (under `profile::view_data_dir()`; `Host::installed_in`), whether or not it is there yet:
/// flat for a downloaded host, in a folder of its own for an embedded one (where `embedded_host` extracts it). (A
/// downloaded CEF host is never run from there: `prepare_cef` copies it beside the runtime's `libcef.dll`.) Without a
/// data directory, the place beside `manifold.exe` (`Host::beside`).
///
/// TODO: a dev tree's runtime files are put beside its hosts by `gate sync-runtime-deps` (README "Runtime-file
/// placement"), not by cargo; a real install or packaging step should do that.
pub(super) fn view_binary_path(kind: ViewKind) -> PathBuf {
    let desc = kind.desc();
    assert!(desc.link == HostLink::PipesShm, "{} is reached over TCP, not by a local path", desc.label);
    if let Some(exe) = run_in_place(desc.host) {
        return exe;
    }
    match view_hosts_dir() {
        Some(dir) => desc.host.installed_in(Os::here(), &dir),
        None => desc.host.beside(Os::here(), &current_exe_dir()),
    }
}

pub(crate) fn view_hosts_dir() -> Option<PathBuf> {
    crate::profile::view_data_dir().map(|mut dir| {
        dir.push(HOSTS_DIR);
        dir
    })
}

/// `cef-runtime/<exe>` next to `manifold.exe` (`Host::beside`): where `gate sync-runtime-deps` puts it for dev builds.
/// CEF's host lives in its runtime folder rather than flat: CEF ships its own `libEGL.dll`/`libGLESv2.dll` (ANGLE),
/// and Windows resolves an exe's implicit DLL dependencies from the exe's own folder first, so a CEF host in one folder
/// with another ANGLE user would load whichever copy was put there last. The other hosts are a lone exe beside their
/// own files (WebView2's loader is statically linked by the `webview2-com` crate), so they need no folder of their own.
pub(super) fn cef_sibling_exe() -> PathBuf {
    CEF.beside(Os::here(), &current_exe_dir())
}

/// The host exe of `kind` that ships in place (a dev tree's, or a packaged one's), when it is there: the one that runs
/// in place (`run_in_place`), except for a host with a runtime folder (CEF), which is its flat exe beside
/// `manifold.exe` first, then its copy in the runtime folder (`Host::beside`; `shipped_cef_host` copies the flat one
/// into a runtime that has no host). A host in place makes the host part Ready whatever the pin table says.
pub(super) fn sibling_host(kind: ViewKind) -> Option<PathBuf> {
    let host = kind.desc().host;
    if host.runtime.is_none() {
        return run_in_place(host);
    }
    let exe_dir = current_exe_dir();
    let flat = Some(exe_dir.join(host.file_name(Os::here()))).filter(|path| path.exists());
    flat.or_else(|| Some(host.beside(Os::here(), &exe_dir)).filter(|path| path.exists()))
}
```

What this changes, so the reviewer can check it:
- `view_binary_path` ends in one `installed_in` call. For a Download row that is `<hosts>/<file>`, as `fallback.push(host_file_name(kind))` was. For WebView2, the one Embedded row with a `ViewKind`, it is `<hosts>/webview2/<file>`, which was `Embedded::Webview2.host_path(..)`'s answer.
- The rationale for CEF's own folder moves from the deleted `beside_exe` doc to `cef_sibling_exe`.
- `sibling_host`'s flat path is `current_exe_dir().join(file_name)`, which is what `sibling_binary`'s `set_file_name` built.

- [ ] **Step 11: `availability.rs`: WebKit's fallback, the runtime folder and the on-disk read**

At HEAD :129-134, replace:

```rust
/// Where WebKit's host is: the one that runs in place (`run_in_place_from`), else the copy `view_install` puts in
/// `hosts_dir` (`engine-hosts/`), whether or not it is there yet. `None` without a data directory.
pub(super) fn pick_webkit_host(exe_dir: &Path, hosts_dir: Option<&Path>, dev_folders: bool) -> Option<PathBuf> {
    run_in_place_from(ViewKind::Webkit, exe_dir, dev_folders)
        .or_else(|| hosts_dir.map(|dir| dir.join(host_file_name(ViewKind::Webkit))))
}
```

with:

```rust
/// Where WebKit's host is: the one that runs in place (`run_in_place_from`), else where `view_install` installs it
/// (`Host::installed_in`) in `hosts_dir` (`engine-hosts/`), whether or not it is there yet. `None` without a data
/// directory.
pub(super) fn pick_webkit_host(exe_dir: &Path, hosts_dir: Option<&Path>, dev_folders: bool) -> Option<PathBuf> {
    run_in_place_from(&WEBKIT, exe_dir, dev_folders)
        .or_else(|| hosts_dir.map(|dir| WEBKIT.installed_in(Os::here(), dir)))
}
```

At HEAD :144-147, replace:

```rust
pub(crate) fn view_runtime_dir(kind: ViewKind) -> Option<PathBuf> {
    let name = kind.desc().runtime_dir?;
    view_hosts_dir().map(|dir| dir.join(name))
}
```

with:

```rust
pub(crate) fn view_runtime_dir(kind: ViewKind) -> Option<PathBuf> {
    kind.desc().host.runtime_in(&view_hosts_dir()?)
}
```

At HEAD :227 (in `read_on_disk`), replace:

```rust
    read_on_disk_at(kind, run_in_place(kind).as_deref(), &STAMPS)
```

with:

```rust
    read_on_disk_at(kind, run_in_place(kind.desc().host).as_deref(), &STAMPS)
```

After this step, this file has no `HostSource`, no `"engine-hosts"` and no `cef_platform` reference. Check with the Grep tool:
- pattern `HostSource|engine-hosts"|cef_platform|desc\(\)\.(runtime_dir|dev_dir|binary|source|name)`;
- path `crates/manifold/src/process_manager/availability.rs`.

Expected: no matches. (`engine-hosts/` in doc comments has no closing quote, so it does not match.)

- [ ] **Step 12: `process_manager/tests.rs`: the leaf's names, and the extracted WebView2 path**

In `crates/manifold/src/process_manager/tests.rs`, at HEAD :3-4, replace:

```rust
use crate::view_install::{no_download_reason, NO_PIN_REASON};
use std::path::Path;
```

with:

```rust
use crate::view_install::{no_download_reason, NO_PIN_REASON};
use manifold_hosts::{CEF_RUNTIME_DIR, HOSTS_DIR, PDFIUM, SERVO, WEBKIT, WEBVIEW2};
use std::path::Path;
```

Where the other names come from:
- `CEF`, `Os` and `Host` come through `use super::*` from Step 2's import.
- `CEF_RUNTIME_DIR` used to come from the deleted `process_manager::CEF_RUNTIME_DIR`.

At HEAD :1091 (in `an_extracted_webview2_host_keeps_its_user_data_folder_in_its_own_engine_hosts_folder`), replace:

```rust
    let host = crate::embedded_host::Embedded::Webview2.host_path(hosts);
```

with:

```rust
    let host = WEBVIEW2.installed_in(Os::here(), hosts);
```

The test's two assertions are unchanged (`hosts.join("webview2").join("manifold-view-webview2.exe")` and the profile folder under it). They hold on Linux too: WebView2 is built for Windows, so its file name carries `.exe` on every OS.

- [ ] **Step 13: `process_manager/tests.rs`: the lookup test over host rows, with a PDFium case**

At HEAD :549-594, replace the whole test `each_host_is_found_in_its_dev_folder_then_beside_the_exe`, with its doc comment:

```rust
/// Where each host runs in place from, given `manifold.exe`'s folder (`run_in_place_from`): its dev build folder found
/// by walking up (`ViewDesc::dev_dir`), then beside the exe; CEF only from its `cef-runtime/` copy, the one a tab runs.
/// WebKit falls back to its downloaded copy in `engine-hosts/`, whether or not it is there yet.
#[test]
fn each_host_is_found_in_its_dev_folder_then_beside_the_exe() {
    let root = crate::store::testutil::temp_dir("host-lookup");
    // Deep enough that the walk up from the exe (its folder and four parents) stays inside `root`.
    let repo = root.join("work").join("repo");
    let exe_dir = repo.join("target").join("debug");
    std::fs::create_dir_all(&exe_dir).unwrap();
    for kind in ViewKind::ALL {
        assert_eq!(run_in_place_from(kind, &exe_dir, true), None, "{kind:?}: nothing built");
    }
    let hosts = root.join("engine-hosts");
    assert_eq!(pick_webkit_host(&exe_dir, Some(&hosts), true), Some(hosts.join("manifold-view-webkitgtk")));
    assert_eq!(pick_webkit_host(&exe_dir, None, true), None);
    for (kind, dev) in [(ViewKind::Servo, "target-servo"), (ViewKind::Webkit, "target-linux")] {
        assert_eq!(kind.desc().dev_dir, Some(dev));
        let dev_dir = repo.join(dev).join("debug");
        std::fs::create_dir_all(&dev_dir).unwrap();
        assert_eq!(run_in_place_from(kind, &exe_dir, true), None, "{kind:?}: the folder alone is not a host");
        let flat = exe_dir.join(host_file_name(kind));
        std::fs::write(&flat, b"host").unwrap();
        assert_eq!(run_in_place_from(kind, &exe_dir, true), Some(flat), "{kind:?}: a packaged host beside the exe");
        let built = dev_dir.join(host_file_name(kind));
        std::fs::write(&built, b"host").unwrap();
        assert_eq!(run_in_place_from(kind, &exe_dir, true), Some(built), "{kind:?}: the dev build wins");
    }
    assert_eq!(
        pick_webkit_host(&exe_dir, Some(&hosts), true),
        Some(repo.join("target-linux").join("debug").join("manifold-view-webkitgtk"))
    );
    // CEF runs only from its runtime folder: a flat exe alone is not in place, so its stamp is never read for a copy
    // the tab would not run.
    assert_eq!(ViewKind::Cef.desc().dev_dir, None);
    std::fs::write(exe_dir.join(host_file_name(ViewKind::Cef)), b"host").unwrap();
    assert_eq!(run_in_place_from(ViewKind::Cef, &exe_dir, true), None, "a flat CEF exe alone");
    let copy = exe_dir.join(CEF_RUNTIME_DIR).join(host_file_name(ViewKind::Cef));
    std::fs::create_dir_all(copy.parent().unwrap()).unwrap();
    std::fs::write(&copy, b"host").unwrap();
    assert_eq!(run_in_place_from(ViewKind::Cef, &exe_dir, true), Some(copy));
    assert_eq!(ViewKind::Webview2.desc().dev_dir, None);
    let webview2 = exe_dir.join(host_file_name(ViewKind::Webview2));
    std::fs::write(&webview2, b"host").unwrap();
    assert_eq!(run_in_place_from(ViewKind::Webview2, &exe_dir, true), Some(webview2));
}
```

with:

```rust
/// Where each host runs in place from, given `manifold.exe`'s folder (`run_in_place_from`): its dev build folder found
/// by walking up (`Host::dev_dir`), then beside the exe; CEF only from its `cef-runtime/` copy, the one a tab runs;
/// WebView2 and the PDF view (no dev folder) only beside the exe. WebKit falls back to its downloaded copy in
/// `engine-hosts/`, whether or not it is there yet.
#[test]
fn each_host_is_found_in_its_dev_folder_then_beside_the_exe() {
    let root = crate::store::testutil::temp_dir("host-lookup");
    // Deep enough that the walk up from the exe (its folder and four parents) stays inside `root`.
    let repo = root.join("work").join("repo");
    let exe_dir = repo.join("target").join("debug");
    std::fs::create_dir_all(&exe_dir).unwrap();
    for host in manifold_hosts::ALL {
        assert_eq!(run_in_place_from(host, &exe_dir, true), None, "{}: nothing built", host.id);
    }
    let hosts = root.join(HOSTS_DIR);
    assert_eq!(pick_webkit_host(&exe_dir, Some(&hosts), true), Some(hosts.join("manifold-view-webkitgtk")));
    assert_eq!(pick_webkit_host(&exe_dir, None, true), None);
    for (host, dev) in [(&SERVO, "target-servo"), (&WEBKIT, "target-linux")] {
        assert_eq!(host.dev_dir, Some(dev));
        let dev_dir = repo.join(dev).join("debug");
        std::fs::create_dir_all(&dev_dir).unwrap();
        assert_eq!(run_in_place_from(host, &exe_dir, true), None, "{}: the folder alone is not a host", host.id);
        let flat = exe_dir.join(host.file_name(Os::here()));
        std::fs::write(&flat, b"host").unwrap();
        assert_eq!(run_in_place_from(host, &exe_dir, true), Some(flat), "{}: a packaged host beside the exe", host.id);
        let built = dev_dir.join(host.file_name(Os::here()));
        std::fs::write(&built, b"host").unwrap();
        assert_eq!(run_in_place_from(host, &exe_dir, true), Some(built), "{}: the dev build wins", host.id);
    }
    assert_eq!(
        pick_webkit_host(&exe_dir, Some(&hosts), true),
        Some(repo.join("target-linux").join("debug").join("manifold-view-webkitgtk"))
    );
    // CEF runs only from its runtime folder: a flat exe alone is not in place, so its stamp is never read for a copy
    // the tab would not run.
    assert_eq!(CEF.dev_dir, None);
    std::fs::write(exe_dir.join(CEF.file_name(Os::here())), b"host").unwrap();
    assert_eq!(run_in_place_from(&CEF, &exe_dir, true), None, "a flat CEF exe alone");
    let copy = CEF.beside(Os::here(), &exe_dir);
    assert_eq!(copy, exe_dir.join(CEF_RUNTIME_DIR).join(CEF.file_name(Os::here())));
    std::fs::create_dir_all(copy.parent().unwrap()).unwrap();
    std::fs::write(&copy, b"host").unwrap();
    assert_eq!(run_in_place_from(&CEF, &exe_dir, true), Some(copy));
    for host in [&WEBVIEW2, &PDFIUM] {
        assert_eq!(host.dev_dir, None);
        let beside = exe_dir.join(host.file_name(Os::here()));
        assert_eq!(run_in_place_from(host, &exe_dir, true), None, "{}: not built", host.id);
        std::fs::write(&beside, b"host").unwrap();
        assert_eq!(run_in_place_from(host, &exe_dir, true), Some(beside), "{}: beside the exe", host.id);
    }
}
```

What this changes, so the reviewer can check it:
- Every old assertion is kept:
  - nothing is found before anything is built;
  - the dev folder alone is not a host;
  - a packaged host is found beside the exe, and then the dev build wins;
  - WebKit falls back to `engine-hosts/`, and that fallback is `None` without a data directory;
  - CEF is found only from `cef-runtime/`;
  - WebView2 is found beside the exe.
- The "nothing built" loop now covers all five rows, PDFium included.
- New: the PDFium case, not found before its file is beside the exe and found after (spec "Tests", App).
- New: the assertion that `CEF.beside` is the `cef-runtime/<file>` path. The old test built that path by hand.
- `file_name(Os::here())` replaces `host_file_name(kind)`, and gives the same name for every `ViewKind` on the OS it runs on. The one exception is WebView2 on Linux, which gains `.exe` (built for Windows). The test reads the name from the same call, so the test agrees with the lookup on both OSes.
- `manifold_hosts::ALL` is a `static` array of `&Host`, which is `Copy`, so the loop copies it.

- [ ] **Step 14: `process_manager/tests.rs`: the walk and pinned-build tests over host rows; each engine reaches its own row**

At HEAD :596-636, replace these two tests, with their doc comments:

```rust
/// The dev folder is looked for in the exe's folder and its four parents (as WebKit's lookup walked): a host in
/// the fourth parent's dev folder is found, one in the fifth's is not.
#[test]
fn the_dev_folder_is_searched_four_parents_up_and_no_further() {
    let root = crate::store::testutil::temp_dir("host-walk");
    let exe_dir = root.join("p5").join("p4").join("p3").join("p2").join("p1").join("exe");
    std::fs::create_dir_all(&exe_dir).unwrap();
    let put = |parent: &Path| {
        let built = parent.join("target-servo").join("debug").join(host_file_name(ViewKind::Servo));
        std::fs::create_dir_all(built.parent().unwrap()).unwrap();
        std::fs::write(&built, b"host").unwrap();
        built
    };
    put(&root.join("p5"));
    assert_eq!(run_in_place_from(ViewKind::Servo, &exe_dir, true), None, "five parents up is out of reach");
    let reached = put(&root.join("p5").join("p4"));
    assert_eq!(run_in_place_from(ViewKind::Servo, &exe_dir, true), Some(reached), "four parents up is reached");
}

/// A build that pins its hosts (a release) never runs a host from a dev build folder: any user can create a folder in a
/// drive root, so a walk up from a release's folder could reach a host nobody pinned, and a host in place skips the hash
/// check. A host beside `manifold.exe` still runs in place.
#[test]
fn a_pinned_build_runs_no_host_from_a_dev_folder() {
    let root = crate::store::testutil::temp_dir("host-pinned");
    let exe_dir = root.join("repo").join("target").join("debug");
    for (kind, dev) in [(ViewKind::Servo, "target-servo"), (ViewKind::Webkit, "target-linux")] {
        let built = root.join("repo").join(dev).join("debug").join(host_file_name(kind));
        std::fs::create_dir_all(built.parent().unwrap()).unwrap();
        std::fs::write(&built, b"host").unwrap();
        assert_eq!(run_in_place_from(kind, &exe_dir, false), None, "{kind:?}: a dev build is not run");
        let hosts = root.join("engine-hosts");
        if kind == ViewKind::Webkit {
            assert_eq!(pick_webkit_host(&exe_dir, Some(&hosts), false), Some(hosts.join(host_file_name(kind))));
        }
        std::fs::create_dir_all(&exe_dir).unwrap();
        let flat = exe_dir.join(host_file_name(kind));
        std::fs::write(&flat, b"host").unwrap();
        assert_eq!(run_in_place_from(kind, &exe_dir, false), Some(flat), "{kind:?}: a host beside the exe runs");
    }
}
```

with:

```rust
/// The dev folder is looked for in the exe's folder and its four parents (as WebKit's lookup walked): a host in
/// the fourth parent's dev folder is found, one in the fifth's is not.
#[test]
fn the_dev_folder_is_searched_four_parents_up_and_no_further() {
    let root = crate::store::testutil::temp_dir("host-walk");
    let exe_dir = root.join("p5").join("p4").join("p3").join("p2").join("p1").join("exe");
    std::fs::create_dir_all(&exe_dir).unwrap();
    let put = |parent: &Path| {
        let built = parent.join("target-servo").join("debug").join(SERVO.file_name(Os::here()));
        std::fs::create_dir_all(built.parent().unwrap()).unwrap();
        std::fs::write(&built, b"host").unwrap();
        built
    };
    put(&root.join("p5"));
    assert_eq!(run_in_place_from(&SERVO, &exe_dir, true), None, "five parents up is out of reach");
    let reached = put(&root.join("p5").join("p4"));
    assert_eq!(run_in_place_from(&SERVO, &exe_dir, true), Some(reached), "four parents up is reached");
}

/// A build that pins its hosts (a release) never runs a host from a dev build folder: any user can create a folder in a
/// drive root, so a walk up from a release's folder could reach a host nobody pinned, and a host in place skips the hash
/// check. A host beside `manifold.exe` still runs in place.
#[test]
fn a_pinned_build_runs_no_host_from_a_dev_folder() {
    let root = crate::store::testutil::temp_dir("host-pinned");
    let exe_dir = root.join("repo").join("target").join("debug");
    for (host, dev) in [(&SERVO, "target-servo"), (&WEBKIT, "target-linux")] {
        let built = root.join("repo").join(dev).join("debug").join(host.file_name(Os::here()));
        std::fs::create_dir_all(built.parent().unwrap()).unwrap();
        std::fs::write(&built, b"host").unwrap();
        assert_eq!(run_in_place_from(host, &exe_dir, false), None, "{}: a dev build is not run", host.id);
        let hosts = root.join(HOSTS_DIR);
        if host.id == WEBKIT.id {
            let downloaded = hosts.join(host.file_name(Os::here()));
            assert_eq!(pick_webkit_host(&exe_dir, Some(&hosts), false), Some(downloaded));
        }
        std::fs::create_dir_all(&exe_dir).unwrap();
        let flat = exe_dir.join(host.file_name(Os::here()));
        std::fs::write(&flat, b"host").unwrap();
        assert_eq!(run_in_place_from(host, &exe_dir, false), Some(flat), "{}: a host beside the exe runs", host.id);
    }
}

/// Each engine reads its own row of the host table (`ViewDesc::host`), and the one row no engine reaches is the PDF
/// view's, which is not an engine.
#[test]
fn each_engine_has_its_own_host_row_and_only_pdfium_has_no_engine() {
    let ids = ViewKind::ALL.map(|kind| kind.desc().host.id);
    let mut distinct = ids.to_vec();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(distinct.len(), ids.len(), "two engines share a row: {ids:?}");
    let unreached: Vec<&str> = manifold_hosts::ALL.iter().map(|host| host.id).filter(|id| !ids.contains(id)).collect();
    assert_eq!(unreached, [PDFIUM.id]);
}
```

Notes for the reviewer:
- Rows are compared by `id` (`host.id == WEBKIT.id`), because `Host` has no `PartialEq` (spec "The crate").
- The file name is computed at each use rather than held in a variable. A by-value last use of a `String` variable would draw clippy's `needless_borrows_for_generic_args` on a `&file`.
- `the_lists_read_the_same_host_the_spawn_looks_for` (:726-733) and `only_cef_has_a_downloaded_runtime_dir` (:1033) stay as they are. Their `ViewKind` signatures are unchanged.

- [ ] **Step 15: `view_install.rs`: the imports, `downloads`, `has_runtime`, `host_platform` and the marker read the row**

In `crates/manifold/src/view_install.rs`, at HEAD :13-17, replace:

```rust
use crate::host_pin::Component as HostFile;
use crate::process_manager::runtime::Os;
use crate::process_manager::{HostSource, ViewKind, CEF_RUNTIME_DIR};

use manifold_view_protocol::transport::to_hex;
```

with:

```rust
use crate::host_pin::Component as HostFile;
use crate::process_manager::ViewKind;

use manifold_hosts::{Delivery, Os, CEF, CEF_RUNTIME_DIR};
use manifold_view_protocol::transport::to_hex;
```

At HEAD :132-141, replace:

```rust
/// Whether `kind` is installed by downloading (`HostSource::Download`), rather than run in place (from its dev folder
/// or beside `manifold.exe`) or carried inside it.
pub(crate) fn downloads(kind: ViewKind) -> bool {
    kind.desc().source == HostSource::Download
}

/// Whether `kind` also needs a runtime archive besides its host exe (`ViewDesc::runtime`).
pub(crate) fn has_runtime(kind: ViewKind) -> bool {
    kind.desc().runtime_dir.is_some()
}
```

with:

```rust
/// Whether `kind` is installed by downloading (its host row's `Delivery::Download`), rather than run in place (from its
/// dev folder or beside `manifold.exe`) or carried inside it.
pub(crate) fn downloads(kind: ViewKind) -> bool {
    kind.desc().host.delivery == Delivery::Download
}

/// Whether `kind` also needs a runtime archive besides its host exe (`Host::runtime`), which also takes a
/// bring-your-own runtime directory: only CEF's, the one component that is an archive (`Pin`, `unpack`) rather than
/// single pinned files.
pub(crate) fn has_runtime(kind: ViewKind) -> bool {
    kind.desc().host.runtime.is_some()
}
```

At HEAD :168-172, replace:

```rust
/// `host_pin`'s name for the platform `kind`'s host is built for: this build's own OS, except an engine built for
/// another (WebKit's is a Linux ELF even when manifold runs on Windows, through WSL).
pub(crate) fn host_platform(kind: ViewKind) -> &'static str {
    kind.desc().host_os.unwrap_or_else(Os::here).pin_platform()
}
```

with:

```rust
/// `host_pin`'s name for the platform `kind`'s host is built for (`Host::pin_platform`): this build's own OS, except an
/// engine built for another (WebKit's is a Linux ELF even when manifold runs on Windows, through WSL).
pub(crate) fn host_platform(kind: ViewKind) -> &'static str {
    kind.desc().host.pin_platform(Os::here())
}
```

At HEAD :251 (in `host_marker_path`), replace:

```rust
    dir.join(format!("{}.host", kind.desc().name))
```

with:

```rust
    dir.join(format!("{}.host", kind.desc().host.id))
```

The `CEF_RUNTIME_DIR` uses (:719-721, :839, :886, :921) keep their text. They now read the leaf's constant, which has the same value, `"cef-runtime"`.

`has_runtime`'s doc now carries the bring-your-own fact that the deleted `ViewDesc::runtime_dir` doc held. The spec flags the old doc (`ViewDesc::runtime`, which names no field) as a by-hand fix that no needle catches.

- [ ] **Step 16: `view_install.rs`: the custom-folder check and the removal's file names read the row**

At HEAD :317-318 (the `validate_custom` doc), replace:

```rust
/// Whether `dir` is a CEF runtime this host runs on: it holds the platform's runtime library
/// (`CefPlatform::runtime_lib`; nothing else is required, e.g. CEF 154's archive has no `libEGL.dll`)
```

with:

```rust
/// Whether `dir` is a CEF runtime this host runs on: it holds the runtime library of the OS CEF's host is built for
/// (`Host::runtime_lib`; nothing else is required, e.g. CEF 154's archive has no `libEGL.dll`)
```

At HEAD :326, replace:

```rust
    let lib = crate::cef_platform::PLATFORM.runtime_lib;
```

with:

```rust
    let lib = CEF.runtime_lib(Os::here()).expect("CEF has a runtime");
```

At HEAD :842-846, replace:

```rust
/// The file names in the hosts directory that are `kind`'s host: the pinned `files`, those its marker lists (an
/// install of another release) and the host exe's own name.
fn host_file_names(dir: &Path, kind: ViewKind, files: &[HostFile]) -> Vec<String> {
    let mut names: Vec<String> = files.iter().map(|f| f.file.to_string()).collect();
    names.push(crate::cef_platform::PLATFORM.host_exe(kind.desc().binary));
```

with:

```rust
/// The file names in the hosts directory that are `kind`'s host: the pinned `files`, those its marker lists (an
/// install of another release) and the host exe's own name (`Host::file_name`: as the OS it is built for names it, so
/// WebKit's has no `.exe` on Windows).
fn host_file_names(dir: &Path, kind: ViewKind, files: &[HostFile]) -> Vec<String> {
    let mut names: Vec<String> = files.iter().map(|f| f.file.to_string()).collect();
    names.push(kind.desc().host.file_name(Os::here()));
```

At HEAD :860 (in `host_leftovers`), replace:

```rust
    let marker_tmp = format!("{}.host.tmp", kind.desc().name);
```

with:

```rust
    let marker_tmp = format!("{}.host.tmp", kind.desc().host.id);
```

`host_file_names` changes behaviour on Windows, and only for WebKit:
- At HEAD it appended `.exe` to every host (`PLATFORM.host_exe`), so on Windows it named `manifold-view-webkitgtk.exe`. That is a file no install writes.
- A removal (`installed_paths_in`) or a leftover sweep (`host_leftovers`) in a build with no pins and no marker therefore missed WebKit's ELF.
- Now the host's own name is what the table gives (spec "The app reads the table", Install and pins).
- Step 17's new test pins this. On Windows at HEAD it fails on the first assertion: `["manifold-view-webkitgtk.exe"]` against `["manifold-view-webkitgtk"]`.

The CEF `.expect` is the spec's choice for all three CEF-only readers (`validate_custom`, `settings_page.rs:472` and the test helper `lib()`). CEF is the one row with a `runtime`, and the leaf's tests pin that.

- [ ] **Step 17: `view_install/tests.rs`: the runtime library from the row; a host's own name as its OS gives it**

In `crates/manifold/src/view_install/tests.rs`, at HEAD :153-156, replace:

```rust
/// The runtime library's file name on this platform.
fn lib() -> &'static str {
    crate::cef_platform::PLATFORM.runtime_lib
}
```

with:

```rust
/// The runtime library's file name on this platform (`Host::runtime_lib`).
fn lib() -> &'static str {
    CEF.runtime_lib(Os::here()).expect("CEF has a runtime")
}
```

`CEF` and `Os` come through the module's `use super::*` (Step 15's import).

At HEAD :1402-1406, insert a test between `webkit_installs_its_elf_under_its_own_marker` and `missing_files_names_only_what_the_folder_lacks`. Replace:

```rust
    assert_eq!(host_state(&dir, ViewKind::Webkit, f), RuntimeOnDisk::Missing, "the marker without the ELF");
}

#[test]
fn missing_files_names_only_what_the_folder_lacks() {
```

with:

```rust
    assert_eq!(host_state(&dir, ViewKind::Webkit, f), RuntimeOnDisk::Missing, "the marker without the ELF");
}

/// A host's own file name is the one the OS it is built for gives it (`Host::file_name`): WebKit's Linux ELF has no
/// `.exe` even when manifold runs on Windows, so a removal in a build that pins nothing finds it with no marker.
#[test]
fn host_file_names_name_each_host_as_the_os_it_is_built_for_does() {
    let dir = temp_dir("host-names");
    assert_eq!(host_file_names(&dir, ViewKind::Webkit, &[]), ["manifold-view-webkitgtk"]);
    assert_eq!(host_file_names(&dir, ViewKind::Cef, &[]), [CEF.file_name(Os::here())]);
    std::fs::write(dir.join("manifold-view-webkitgtk"), b"elf").unwrap();
    assert_eq!(installed_paths_in(&dir, ViewKind::Webkit, &[]), [dir.join("manifold-view-webkitgtk")]);
}

#[test]
fn missing_files_names_only_what_the_folder_lacks() {
```

Why the test is written this way:
- The WebKit literal `manifold-view-webkitgtk` is deliberate. It is the file name `webkit_installs_its_elf_under_its_own_marker` (just above) installs and reads, so the test checks the table against an independent fixture rather than against itself.
- The CEF case reads the table. It pins only that a host built for whichever OS manifold runs on keeps that OS's suffix.
- Red at HEAD, argued and not run: on Windows, HEAD's `host_file_names` returns `["manifold-view-webkitgtk.exe"]`, so the first assertion fails. On Linux it passes at HEAD and after the change.

- [ ] **Step 18: `settings_page.rs`: the folder hint reads the row**

In `crates/manifold/src/settings_page.rs`, at HEAD :472 (in `settings_cef_directory_row`), replace:

```rust
        let hint = format!("Folder with {}", crate::cef_platform::PLATFORM.runtime_lib);
```

with:

```rust
        let lib = manifold_hosts::CEF.runtime_lib(manifold_hosts::Os::here()).expect("CEF has a runtime");
        let hint = format!("Folder with {lib}");
```

No other `lib` binding is in scope in `settings_cef_directory_row`. The file has no `manifold_hosts` import, and full paths keep it that way for one use.

- [ ] **Step 19: `cef_platform.rs`: the host's file name and the runtime library leave**

In `crates/manifold/src/cef_platform.rs`, at HEAD :1, replace:

```rust
//! What differs per OS about installing and launching CEF, in one place (spec "Platforms",
```

with:

```rust
//! What differs per OS about CEF's runtime archive and the libraries its host loads (spec "Platforms",
```

The old wording no longer holds: the host's file name and the runtime library now live in the host table.

At HEAD :7-15, replace:

```rust
/// The per-platform facts the install and the host launch read.
pub(crate) struct CefPlatform {
    /// The CEF CDN's platform id: the `_<id>_` in `cef_binary_<version>_<id>_minimal.tar.bz2`.
    pub(crate) id: &'static str,
    /// The runtime library whose presence marks a CEF directory.
    pub(crate) runtime_lib: &'static str,
    /// Appended to an engine host's binary name (`.exe` on Windows).
    pub(crate) exe_suffix: &'static str,
    /// The archive's subfolders (under its one top folder) that hold the runtime; flattened on unpack.
```

with:

```rust
/// The per-platform facts of CEF's runtime archive that the install reads, and the system libraries its host loads. The
/// host's file name and the runtime's marker library are its row in the host table (`manifold_hosts::CEF`).
pub(crate) struct CefPlatform {
    /// The CEF CDN's platform id: the `_<id>_` in `cef_binary_<version>_<id>_minimal.tar.bz2`.
    pub(crate) id: &'static str,
    /// The archive's subfolders (under its one top folder) that hold the runtime; flattened on unpack.
```

At HEAD :27-33, replace:

```rust
impl CefPlatform {
    /// `binary`'s file name on this platform.
    pub(crate) fn host_exe(&self, binary: &str) -> String {
        format!("{binary}{}", self.exe_suffix)
    }

    /// The `system_libs` the system does not list (`ldconfig -p`), so the download can warn before a host that
```

with:

```rust
impl CefPlatform {
    /// The `system_libs` the system does not list (`ldconfig -p`), so the download can warn before a host that
```

At HEAD :78-81, replace:

```rust
    id: "windows64",
    runtime_lib: "libcef.dll",
    exe_suffix: ".exe",
    runtime_dirs: ["Release", "Resources"],
```

with:

```rust
    id: "windows64",
    runtime_dirs: ["Release", "Resources"],
```

At HEAD :93-96, replace:

```rust
    id: "linux64",
    runtime_lib: "libcef.so",
    exe_suffix: "",
    runtime_dirs: ["Release", "Resources"],
```

with:

```rust
    id: "linux64",
    runtime_dirs: ["Release", "Resources"],
```

At HEAD :118-125, delete the test `host_exe_adds_the_platforms_suffix`. Replace:

```rust
    #[test]
    fn host_exe_adds_the_platforms_suffix() {
        assert_eq!(PLATFORM.host_exe("manifold-view-cef"), format!("manifold-view-cef{}", PLATFORM.exe_suffix));
        assert_eq!(PLATFORM.host_exe("x"), if cfg!(windows) { "x.exe" } else { "x" });
    }

    #[test]
    fn the_archive_name_carries_the_platform_id() {
```

with:

```rust
    #[test]
    fn the_archive_name_carries_the_platform_id() {
```

What stays in `CefPlatform` (spec "The app reads the table"): `id`, `runtime_dirs`, `skip_dirs`, `skip_files`, `skip_suffixes` and `system_libs`, with `missing_system_libs` and the test-only `archive_name`.

Where the deleted facts live now:
- The suffix is `Os::exe_suffix`, read through `Host::file_name`.
- `libcef.dll` (Windows) and `libcef.so` (every other OS) are `CEF.runtime.lib`. The leaf's golden test pins them.
- The deleted test's claim is covered by the leaf's `file_name` cases, which Task 1 adds.

Check that no reader is left. Use the Grep tool with:
- pattern `runtime_lib|exe_suffix|host_exe\(`
- path `crates/manifold/src`
- output mode `content`

Expected: exactly 5 lines, each a `CEF.runtime_lib(` call or a `Host::runtime_lib` doc mention, never a `PLATFORM.` read:
- `settings_page.rs`: Step 18's `CEF.runtime_lib` call;
- `view_install.rs`: Step 16's `validate_custom` doc (`Host::runtime_lib`) and its `CEF.runtime_lib` call;
- `view_install/tests.rs`: Step 17's `lib()` doc (`Host::runtime_lib`) and its body.

- [ ] **Step 20: `host_pin.rs`: the tests first: names from the table, and a manifest must list exactly what the table pins**

This is the one behaviour Part A adds: `parse` refuses an entry whose file set is not the table's. The test goes in first.

In `crates/manifold/src/host_pin.rs`, at HEAD :123-124, replace:

```rust
mod tests {
    use super::*;
```

with:

```rust
mod tests {
    use super::*;
    use manifold_hosts::{Os, CEF, SERVO, WEBKIT, WEBVIEW2};
```

At HEAD :145-177, replace the two tests `a_host_is_fetched_from_its_own_releases_download` and `a_bad_manifest_is_refused_whole`. They run through the end of the `for bad` loop and its closing brace:

```rust
    #[test]
    fn a_host_is_fetched_from_its_own_releases_download() {
        let entries = parse(&manifest("manifold-view-cef.exe", 740_000, HASH), "1.2.3").unwrap();
        let table: &'static [Entry] = Box::leak(entries.into_boxed_slice());
        let c = &lookup(table, ViewKind::Cef, "windows-x64")[0];
        assert_eq!(c.file, "manifold-view-cef.exe");
        assert_eq!(
            c.url,
            "https://github.com/winstonyli/manifold/releases/download/v1.2.3/manifold-view-cef.exe-windows-x64"
        );
        assert_eq!((c.size, c.sha256), (740_000, HASH));
        assert!(lookup(table, ViewKind::Cef, "linux-x64").is_empty());
        assert!(lookup(table, ViewKind::Servo, "windows-x64").is_empty());
    }

    #[test]
    fn a_bad_manifest_is_refused_whole() {
        for bad in [
            manifest("manifold-view-cef.exe", 1, ""),
            manifest("manifold-view-cef.exe", 1, &HASH.to_uppercase()),
            manifest("manifold-view-cef.exe", 1, &HASH[1..]),
            manifest("manifold-view-cef.exe", 1, &HASH.replace('d', "g")),
            manifest("manifold-view-cef.exe", 0, HASH),
            manifest("", 1, HASH),
            manifest("../x.exe", 1, HASH),
            manifest("a\\\\b.exe", 1, HASH), // JSON `a\\b.exe`, the name a\b.exe
            manifest("manifold-view-cef.exe", 1, HASH).replace("cef", "nosuch"),
            "[]".to_owned(),
            "{".to_owned(),
        ] {
            assert!(parse(&bad, "1.2.3").is_err(), "accepted {bad}");
        }
    }
```

with:

```rust
    #[test]
    fn a_host_is_fetched_from_its_own_releases_download() {
        let exe = CEF.file_name(Os::Windows);
        let entries = parse(&manifest(&exe, 740_000, HASH), "1.2.3").unwrap();
        let table: &'static [Entry] = Box::leak(entries.into_boxed_slice());
        let c = &lookup(table, ViewKind::Cef, "windows-x64")[0];
        assert_eq!(c.file, exe);
        assert_eq!(c.url, format!("https://github.com/winstonyli/manifold/releases/download/v1.2.3/{exe}-windows-x64"));
        assert_eq!((c.size, c.sha256), (740_000, HASH));
        assert!(lookup(table, ViewKind::Cef, "linux-x64").is_empty());
        assert!(lookup(table, ViewKind::Servo, "windows-x64").is_empty());
    }

    #[test]
    fn a_bad_manifest_is_refused_whole() {
        let exe = CEF.file_name(Os::Windows);
        assert!(parse(&manifest(&exe, 1, HASH), "1.2.3").is_ok(), "each case below is this one manifest, broken once");
        for bad in [
            manifest(&exe, 1, ""),
            manifest(&exe, 1, &HASH.to_uppercase()),
            manifest(&exe, 1, &HASH[1..]),
            manifest(&exe, 1, &HASH.replace('d', "g")),
            manifest(&exe, 0, HASH),
            manifest("", 1, HASH),
            manifest("../x.exe", 1, HASH),
            manifest("a\\\\b.exe", 1, HASH), // JSON `a\\b.exe`, the name a\b.exe
            manifest(&exe, 1, HASH).replace("cef", "nosuch"),
            "[]".to_owned(),
            "{".to_owned(),
        ] {
            assert!(parse(&bad, "1.2.3").is_err(), "accepted {bad}");
        }
    }

    /// Each entry lists exactly the files the host table pins for that host under that platform (`Host::pinned_under`),
    /// whatever OS parses it: a host without one of its sidecars, a host that is not downloaded, a platform no host is
    /// pinned for and a host under another OS's platform each refuse the whole manifest.
    #[test]
    fn a_manifest_must_list_exactly_what_the_host_table_pins() {
        let files = |names: &[String]| -> serde_json::Value {
            names.iter().map(|file| serde_json::json!({"file": file, "size": 1, "sha256": HASH})).collect()
        };
        let servo = SERVO.pinned_under("windows-x64").expect("Servo is pinned for Windows");
        let no_egl: Vec<String> = servo.iter().filter(|file| file.as_str() != "libEGL.dll").cloned().collect();
        assert_eq!(no_egl.len() + 1, servo.len(), "Servo's Windows host has libEGL.dll beside it: {servo:?}");
        let without_egl = serde_json::json!({"servo": {"windows-x64": files(&no_egl)}});
        let err = parse(&without_egl.to_string(), "1.2.3").unwrap_err();
        let listed: std::collections::BTreeSet<&str> = no_egl.iter().map(String::as_str).collect();
        let pinned: std::collections::BTreeSet<&str> = servo.iter().map(String::as_str).collect();
        assert_eq!(err, format!("servo/windows-x64: lists {listed:?}, the host table pins {pinned:?}"));
        let refused = [
            ("webview2", "windows-x64", files(&[WEBVIEW2.file_name(Os::Windows)])),
            ("cef", "plan9-x64", files(&CEF.pinned_files(Os::Windows))),
            ("webkit", "windows-x64", files(&WEBKIT.pinned_files(Os::Linux))),
        ];
        for (id, platform, listed) in refused {
            let err = parse(&serde_json::json!({ id: { platform: listed } }).to_string(), "1.2.3").unwrap_err();
            assert_eq!(err, format!("{id}/{platform}: the host table pins no files there"));
        }
        let cef = files(&CEF.pinned_files(Os::Windows));
        let webkit = files(&WEBKIT.pinned_files(Os::Linux));
        let both = serde_json::json!({"cef": {"windows-x64": cef}, "webkit": {"linux-x64": webkit}});
        let entries = parse(&both.to_string(), "1.2.3").unwrap();
        let got: Vec<(ViewKind, &str)> = entries.iter().map(|e| (e.kind, e.platform)).collect();
        assert_eq!(got, [(ViewKind::Cef, "windows-x64"), (ViewKind::Webkit, "linux-x64")]);
    }
```

Why the tests are written this way (spec "Tests", Manifests):
- File names come from `CEF.file_name(Os::Windows)` and the like, never `Os::here()`. The fixtures say `windows-x64` and `linux-x64`, and the Linux CI job runs these tests.
- The wire keys (`"servo"`, `"cef"`, `"webkit"`, `"webview2"`) stay literal: they are the manifest format, which `release.yml` writes as literals too.
- The new assertion that the unbroken manifest parses keeps every `a_bad_manifest_is_refused_whole` case failing for its own reason. The set check runs after the per-file loop, so a bad hash or bad name is still refused by that loop. The baseline is accepted because CEF has no sidecars, so `{exe}` alone is its Windows set.
- The four refusals are the spec's list:
  - Servo without `libEGL.dll`: its error names the host and both file sets.
  - A host that is not downloaded: WebView2 is `Embedded`.
  - A platform no host is pinned for: `plan9-x64`.
  - A host under another OS's platform: `webkit/windows-x64`.
- The accepted pair is the spec's `cef`/`windows-x64` with `webkit`/`linux-x64`. Entry order is the manifest's (`serde_json` has `preserve_order` in this crate), and alphabetical order would give the same answer.

Red, argued and not run (the crate does not build until Part B ends): at HEAD, `parse` accepts any file list for a known engine. So `parse(&without_egl.to_string(), ..).unwrap_err()` panics with "called `Result::unwrap_err()` on an `Ok` value", and so would each `refused` case. The two rewritten tests assert the same behaviour before and after the change (they are re-expressed over the rows, so they only compile once the dependency from Step 1 is in); they are not new behaviour.

- [ ] **Step 21: `host_pin.rs`: each entry must be the table's file set**

At HEAD :12-13, replace:

```rust
//! The manifest is `{"cef": {"windows-x64": [{"file": "manifold-view-cef.exe", "size": 123, "sha256": "..."}]}}`
//! with the engine's store name (`ViewKind::desc().name`) as the first key.
```

with:

```rust
//! The manifest is `{"cef": {"windows-x64": [{"file": "manifold-view-cef.exe", "size": 123, "sha256": "..."}]}}`
//! with the host's id (`manifold_hosts::Host::id`, the engine's store name) as the first key, and under each platform
//! exactly the files the host table pins there (`Host::pinned_under`).
```

At HEAD :15, replace:

```rust
use std::sync::OnceLock;
```

with:

```rust
use std::collections::BTreeSet;
use std::sync::OnceLock;
```

At HEAD :86-87, replace:

```rust
/// The entries `manifest` lists, with each URL under `v<version>`'s release. Blank text is an empty table; a
/// malformed hash, a zero size or a path in a file name is an error (the whole manifest is refused).
```

with:

```rust
/// The entries `manifest` lists, with each URL under `v<version>`'s release. Blank text is an empty table; a
/// malformed hash, a zero size or a path in a file name is an error (the whole manifest is refused), and so is an entry
/// whose files are not, as a set, the ones the host table pins for that host under that platform (`Host::pinned_under`:
/// none for a host that is not downloaded, a platform no host is pinned for, or another OS's platform). The check reads
/// the platform string, not the OS this runs on, so a manifest parses the same on Windows and Linux.
```

At HEAD :113-115, replace:

```rust
                components.push(Component { file: leak(file), url, size, sha256: leak(sha256) });
            }
            let components = Box::leak(components.into_boxed_slice());
```

with:

```rust
                components.push(Component { file: leak(file), url, size, sha256: leak(sha256) });
            }
            let Some(pinned) = kind.desc().host.pinned_under(platform) else {
                return Err(format!("{who}: the host table pins no files there"));
            };
            let want: BTreeSet<&str> = pinned.iter().map(String::as_str).collect();
            let got: BTreeSet<&str> = components.iter().map(|c| c.file).collect();
            if got != want {
                return Err(format!("{who}: lists {got:?}, the host table pins {want:?}"));
            }
            let components = Box::leak(components.into_boxed_slice());
```

What this does, so the reviewer can check it:
- `kind` comes from `ViewKind::from_name(name)` (:95, kept with its "unknown engine" error). So the id and the host row are one lookup and cannot disagree (spec H12).
- `platform` is the manifest's key, a `&String`, which deref-coerces to `&str`. The check never asks which OS it runs on.
- The check runs after the per-file loop, as the spec requires, so the existing refusals keep their own messages.
- A refused entry refuses the whole manifest. That is the existing policy: `table()` prints "host pins ignored: <error>" and pins nothing.
- Equality is by set, as the spec says, so a file listed twice is not refused (see the decisions).
- The `got`/`want` `Debug` output is a sorted set, so the Servo message reads `servo/windows-x64: lists {"libGLESv2.dll", "manifold-view-servo.exe"}, the host table pins {"libEGL.dll", "libGLESv2.dll", "manifold-view-servo.exe"}`.

Real manifests still pass:
- `release.yml` (:85-87) at HEAD writes `cef/windows-x64` = the exe, `servo/windows-x64` = the exe, `libEGL.dll` and `libGLESv2.dll`, and `webkit/linux-x64` = the ELF. Each is its row's `pinned_under` set.
- The gate's shots fixture (`crates/gate/src/shots.rs:1645`, file `FAKE_CEF_HOST` = `manifold-view-cef.exe`) is `cef/windows-x64` with the exe alone, which also passes.

- [ ] **Step 22: Grep for what Part A must have removed, and what it leaves for Part B**

Run, from the repo root:

```bash
grep -rnF -e HostSource -e "Embedded::" -e "PLATFORM.host_exe" -e "PLATFORM.exe_suffix" -e "PLATFORM.runtime_lib" -e "CefPlatform::runtime_lib" -e beside_exe -e in_place_paths -e sibling_binary -e "host_file_name(" -e "ViewDesc::dev_dir" -e MANIFOLD_WEBVIEW2_SHA256 -e MANIFOLD_PDFIUM_SHA256 crates/manifold/src | grep -v "^crates/manifold/src/embedded_host.rs:"
```

Expected: exactly 3 lines, all in `crates/manifold/src/process_manager.rs`, all Part B's:
- the `(Some(dir), HostSource::Embedded(host)) if sibling_host(kind).is_none() => host.ensure(&dir)?,` line of `prepare_webview2`;
- `let sibling = sibling_binary("manifold-view-pdfium");` in `pdf_view_exe`;
- `Some(dir) => embedded_host::Embedded::Pdfium.ensure(&dir),` in `pdf_view_exe`.

`embedded_host.rs` is Part B's file. Part B adds these needles to `deleted_names.rs` once both halves have cleared them.

Run:

```bash
grep -rnE "desc\(\)\.(name|binary|source|host_os|runtime_dir|dev_dir)\b|desc\.(name|binary|source|host_os|runtime_dir|dev_dir)\b|runtime::Os|process_manager::CEF_RUNTIME_DIR|cef_platform::PLATFORM\.(runtime_lib|exe_suffix|host_exe)" crates/manifold/src
```

Expected: exactly 3 lines, all Part B's:
- `crates/manifold/src/names/tests.rs:470` (a doc comment naming `desc().name`);
- `crates/manifold/src/names/tests.rs:495` (`name_reads("engine.desc().name")`);
- the `let extracted = match (view_hosts_dir(), kind.desc().source) {` line of `prepare_webview2` in `process_manager.rs`.

Any other hit is an edit this part missed. Fix it before Step 23, using the step that names the file.

- [ ] **Step 23: Check the change set; no build, no commit**

Run:

```bash
git diff --stat
git status --short --untracked-files=all -- crates
```

Expected from `git diff --stat`: exactly these 14 files, and nothing under `crates/manifold-hosts/`, `crates/gate/` or the root:
- `crates/manifold/Cargo.toml`
- `crates/manifold/src/cef_platform.rs`
- `crates/manifold/src/debug_control.rs`
- `crates/manifold/src/debug_export.rs`
- `crates/manifold/src/host_pin.rs`
- `crates/manifold/src/process_manager.rs`
- `crates/manifold/src/process_manager/availability.rs`
- `crates/manifold/src/process_manager/runtime.rs`
- `crates/manifold/src/process_manager/tests.rs`
- `crates/manifold/src/session_sync/tests/misc.rs`
- `crates/manifold/src/settings.rs`
- `crates/manifold/src/settings_page.rs`
- `crates/manifold/src/view_install.rs`
- `crates/manifold/src/view_install/tests.rs`

Expected from `git status`: the same 14 files as ` M`, and no `??` line under `crates`. `Cargo.lock` is unchanged, because no cargo command ran. Part B's first cargo run adds `manifold-hosts` to the `manifold` package's dependencies there; Task 1 already locked the leaf package itself.

**Part A ends here: no cargo command, no commit.**
- The tree does not build yet. `prepare_webview2` still names `HostSource` and `kind.desc().source`, and `pdf_view_exe` still calls `sibling_binary` and `embedded_host::Embedded`, all deleted or rewritten above.
- Part B continues in this same working tree, with Step B1. It rewrites those two functions, `embedded_host.rs` and `build.rs`, and the names guard, `deleted_names.rs` and `pdf_render.rs:140`.
- Then it runs `cargo fmt --all`, the tests and clippy over both halves, and makes Task 2's one commit, `Cargo.lock` included.

Part B's test run, `cargo test -j 8 -p manifold --bin manifold -- --test-threads 8` (the app has no lib target; Step B15), must include these tests from Part A:
- `process_manager::tests`:
  - `each_host_is_found_in_its_dev_folder_then_beside_the_exe`
  - `the_dev_folder_is_searched_four_parents_up_and_no_further`
  - `a_pinned_build_runs_no_host_from_a_dev_folder`
  - `each_engine_has_its_own_host_row_and_only_pdfium_has_no_engine`
  - `an_extracted_webview2_host_keeps_its_user_data_folder_in_its_own_engine_hosts_folder`
  - `the_lists_read_the_same_host_the_spawn_looks_for` and `only_cef_has_a_downloaded_runtime_dir` (unchanged, re-run)
- `process_manager::runtime::tests::{a_wsl_host_speaks_tcp, the_reason_an_engine_is_unavailable_names_the_oses_that_can_run_it, a_host_runs_natively_on_its_own_os_and_in_wsl_for_linux_on_windows}`
- `view_install::tests::host_file_names_name_each_host_as_the_os_it_is_built_for_does`, and the whole `view_install::tests` module, since `lib()` now reads the row
- `host_pin::tests::{a_manifest_must_list_exactly_what_the_host_table_pins, a_host_is_fetched_from_its_own_releases_download, a_bad_manifest_is_refused_whole, the_embedded_manifest_is_well_formed}`
- `cef_platform::tests` (one test fewer)
- `session_sync::tests::misc::view_table_matches_the_store_and_round_trips`

#### Part B: embedded hosts, the names guard, and the single commit

Part B finishes Task 2 in the working tree Part A left, with no commit in between. It covers:
- `build.rs` generates the embedded-host table from the `Delivery::Embedded` rows (H7);
- `embedded_host.rs` reads it through `embedded(host)` and extracts to `Host::installed_in`;
- `prepare_webview2` and `pdf_view_exe` call `embedded_host::ensure` with a row;
- `pdf_render.rs:140`'s hint reads `PDFIUM.exe`;
- the names guard loses its `desc().name` exemption (H3), and `deleted_names.rs` guards the deleted host names;
- the first cargo runs over both parts, `Cargo.lock`, and Task 2's only commit.

How to work:
- Make every edit with the Edit tool. The old text is exact.
- Line hints ("at HEAD :140") are line numbers at `10444ae5`. Task 1 changes none of these files. Part A's edits shift the lines of `process_manager.rs`, so there the old text is what to match.
- Within a file, edits go in the order given. After an earlier edit in the same file, a hint is approximate.
- The steps are numbered B1 to B18, so they never collide with Part A's numbers.
- Run no Python.

**Files:**
- Modify: `crates/manifold/src/deleted_names.rs`: a second test between `no_source_names_a_deleted_enum` (ends :22) and `scan` (:24).
- Modify and test: `crates/manifold/src/names/tests.rs`:
  - `name_reads`'s doc (:469-470) and filter (:475-477);
  - the `desc().name` assertion (:495) is deleted.
- Modify: `crates/manifold/build.rs`: the whole file (:1-49).
- Modify and test: `crates/manifold/src/embedded_host.rs`:
  - the module doc (:1-4) and the imports (:6);
  - `Embedded`, its statics and its `impl` (:12-62), replaced by the generated table, `embedded` and `ensure`;
  - in `ensure_with`: its doc (:68), signature (:75), destination (:79) and three messages (:101, :106, :118);
  - tests: the imports (:156-158), the `files` helper (:171-173) and the two host loops (:185-187, :310-322);
  - tests: the `Embedded::Webview2` and `Embedded::Pdfium` call sites (:182-276, :328-329), `:228` and `:333`;
  - one new test.
- Modify: `crates/manifold/src/process_manager.rs`:
  - `prepare_webview2`'s match (at HEAD :1012-1015);
  - `pdf_view_exe` and its doc (at HEAD :1210-1221).
- Modify: `crates/manifold/src/pdf_render.rs:140`.
- Modify: `Cargo.lock`. cargo writes it: the `manifold` package gains `manifold-hosts`.
- Commit: Parts A and B together, Task 2's only commit.

**Interfaces:**
- Consumes from Task 1 (`manifold_hosts`, the spec's "The crate" API):
  - the statics `ALL: [&Host; 5]`, `WEBVIEW2` and `PDFIUM`;
  - `Delivery::Embedded` (`Delivery` derives `PartialEq`);
  - `Host`'s fields `id`, `exe` and `delivery`;
  - `Host::file_name(&self, running: Os) -> String`;
  - `Host::installed_in(&self, running: Os, hosts_dir: &Path) -> PathBuf`, which is `<hosts_dir>/<id>/<file>` for an Embedded row;
  - `Host::embed_var(&self) -> String`, which is `MANIFOLD_<ID>_HOST`;
  - `Os::here()`;
  - `-p manifold-hosts` in CLAUDE.md's clippy command and in `.githooks/pre-commit`.
- Consumes from Part A:
  - `manifold-hosts.workspace = true` under `[dependencies]` and under `[build-dependencies]` in `crates/manifold/Cargo.toml` (Part A's Step 1);
  - `use manifold_hosts::{Host, Os, CEF};` in `process_manager.rs`, which reaches its child modules through `use super::*`;
  - `ViewDesc.host: &'static Host`;
  - `pub(super) fn run_in_place(host: &Host) -> Option<PathBuf>` in `availability.rs`, `None` under `cfg(test)`;
  - `run_in_place` in `process_manager.rs`'s plain `use availability::{..}` list;
  - `HostSource`, `sibling_binary` and `host_file_name` deleted, and `sibling_host(ViewKind)` and `view_hosts_dir()` unchanged;
  - every `desc().name` read as `desc().host.id`;
  - `process_manager/tests.rs:1091` reads `WEBVIEW2.installed_in(Os::here(), hosts)`;
  - no name from Step B2's list left in any file Part A touched.
- Produces:

```rust
// crates/manifold/src/embedded_host.rs
include!(concat!(env!("OUT_DIR"), "/embedded.rs")); // static EMBEDDED: &[(&str, &[u8], &str)], (id, bytes, sha256)
fn embedded(host: &Host) -> (&'static [u8], &'static str); // (&[], "") for a host with no entry or an empty one
pub(crate) fn ensure(host: &Host, hosts_dir: &Path) -> io::Result<Option<PathBuf>>;
pub(crate) fn ensure_with(host: &Host, hosts_dir: &Path, bytes: &[u8], sha256: &str) -> io::Result<Option<PathBuf>>;
// unchanged: retry_busy, spawn_fresh
```

- `build.rs`:
  - It reads `Host::embed_var()` for each Embedded row (`MANIFOLD_WEBVIEW2_HOST`, `MANIFOLD_PDFIUM_HOST`); these are the names Task 5's `gate release-embedded` prints. It also reads `MANIFOLD_HOST_PINS`.
  - It writes `OUT_DIR/<id>-host.bin`, `OUT_DIR/embedded.rs` and `OUT_DIR/host-pins.json`.
  - It emits no `cargo:rustc-env`.
- Tests: `deleted_names::no_source_names_a_deleted_host_layout_item` and `embedded_host::tests::the_generated_table_holds_the_embedded_rows_in_order`.
- Deletes:
  - `embedded_host::Embedded` and its `folder`, `file`, `label`, `bytes`, `host_path` and `ensure`;
  - `embedded_host`'s `WEBVIEW2` and `PDFIUM` statics;
  - `build.rs`'s `HOSTS`;
  - the rustc-env values `MANIFOLD_WEBVIEW2_SHA256` and `MANIFOLD_PDFIUM_SHA256`;
  - the names guard's `desc().name` exemption.

- [ ] **Step B1: Check the precondition**

Part A is applied; cargo build is expected to fail until this part is applied. Run no cargo command before Step B14.

From the repo root, run:

```bash
git log -1 --format=%s
git status --short
grep -nE 'pub fn (here|file_name|installed_in|embed_var)\b|pub static (WEBVIEW2|PDFIUM|ALL)\b|pub enum Delivery\b' crates/manifold-hosts/src/lib.rs
grep -nE '^manifold-hosts\.workspace = true$|^\[(dependencies|dev-dependencies|build-dependencies)\]$' crates/manifold/Cargo.toml
grep -nE "^use manifold_hosts::|pub\(crate\) host: &'static Host,|^    cef_sibling_exe, platform_unavailable_reason, run_in_place, " crates/manifold/src/process_manager.rs
grep -n 'pub(super) fn run_in_place(host: &Host) -> Option<PathBuf>' crates/manifold/src/process_manager/availability.rs
grep -n 'WEBVIEW2.installed_in(Os::here(), hosts)' crates/manifold/src/process_manager/tests.rs
```

Expected:
- `git log`: Task 1's commit, the leaf crate.
- `git status --short`:
  - only ` M` lines under `crates/manifold/`, which are Part A's files;
  - no `Cargo.lock`.
- The leaf: eight lines, one per name.
- `Cargo.toml`: five lines in this order: `[dependencies]`, `manifold-hosts.workspace = true`, `[dev-dependencies]`, `[build-dependencies]`, `manifold-hosts.workspace = true`.
- `process_manager.rs`: three lines:
  - `use manifold_hosts::{Host, Os, CEF};`;
  - the `host` field;
  - the first line of the plain `use availability::{` list.
- One line each from `availability.rs` and `tests.rs`.

Anything else means Part A is not fully applied: stop and report. Part A's files are Part A's to fix.

- [ ] **Step B2: See the deleted host names still in the tree (the failing run)**

This grep is the failing run of the guard Step B3 adds. It uses the same needles, over the guard's scope (`crates/manifold/src`) plus `build.rs`, which the guard does not scan:

```bash
grep -rnE 'HostSource|Embedded::|PLATFORM\.host_exe|PLATFORM\.exe_suffix|PLATFORM\.runtime_lib|CefPlatform::runtime_lib|beside_exe|in_place_paths|sibling_binary|host_file_name\(|ViewDesc::dev_dir|MANIFOLD_WEBVIEW2_SHA256|MANIFOLD_PDFIUM_SHA256' crates/manifold/src crates/manifold/build.rs | grep -v '/deleted_names\.rs:' | cut -d: -f1 | sort | uniq -c
```

Expected, exactly:

```
      2 crates/manifold/build.rs
     33 crates/manifold/src/embedded_host.rs
      3 crates/manifold/src/process_manager.rs
```

The three `process_manager.rs` lines are this part's:
- `prepare_webview2`'s `HostSource::Embedded(host)` arm;
- `pdf_view_exe`'s `sibling_binary("manifold-view-pdfium")`;
- `pdf_view_exe`'s `embedded_host::Embedded::Pdfium`.

Any other file, or more lines in one of these, is a Part A leftover: stop and report.

- [ ] **Step B3: Write the guard for the deleted host names**

In `crates/manifold/src/deleted_names.rs`, at HEAD :21-24, replace:

```rust
    assert_eq!(offenders, Vec::<String>::new(), "read the type through App::card_kind and Caps, see plan 2b-6");
}

fn scan(dir: &Path, needles: &[&str], offenders: &mut Vec<String>) {
```

with:

```rust
    assert_eq!(offenders, Vec::<String>::new(), "read the type through App::card_kind and Caps, see plan 2b-6");
}

/// Each host's layout is one row of `manifold_hosts`, read through `ViewDesc::host` (spec
/// 2026-10-29-host-table-design.md). The per-engine host fields, `HostSource`, the `Embedded` enum and its hash
/// variables, `CefPlatform`'s exe and runtime names and the in-place path helpers are gone. The live
/// `host_file_names` and `Host::runtime_lib` contain two of the old names, so those needles are `host_file_name(` and
/// `PLATFORM.runtime_lib`.
#[test]
fn no_source_names_a_deleted_host_layout_item() {
    let mut offenders = Vec::new();
    let needles = [
        "HostSource",
        "Embedded::",
        "PLATFORM.host_exe",
        "PLATFORM.exe_suffix",
        "PLATFORM.runtime_lib",
        "CefPlatform::runtime_lib",
        "beside_exe",
        "in_place_paths",
        "sibling_binary",
        "host_file_name(",
        "ViewDesc::dev_dir",
        "MANIFOLD_WEBVIEW2_SHA256",
        "MANIFOLD_PDFIUM_SHA256",
    ];
    scan(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &needles, &mut offenders);
    assert_eq!(offenders, Vec::<String>::new(), "read the host through `ViewDesc::host` and `manifold_hosts`");
}

fn scan(dir: &Path, needles: &[&str], offenders: &mut Vec<String>) {
```

The test runs once the tree builds again (Step B15). Its failing run is Step B2's grep. Step B13 repeats the grep and expects nothing.

- [ ] **Step B4: The names guard loses the `desc().name` exemption**

In `crates/manifold/src/names/tests.rs`, at HEAD :469-470, replace:

```rust
/// The `.name` reads on `line`: `.name` not followed by more of an identifier (`.name_format`), not assigned
/// (`.name = `), and not an engine descriptor's (`desc().name`).
```

with:

```rust
/// The `.name` reads on `line`: `.name` not followed by more of an identifier (`.name_format`) and not assigned
/// (`.name = `).
```

At HEAD :475-477, replace:

```rust
            !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_')
                && !rest.starts_with(" = ")
                && !line[..*i].ends_with("desc()")
```

with:

```rust
            !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_') && !rest.starts_with(" = ")
```

At HEAD :494-495 (in `name_reads_counts_reads_only`), replace:

```rust
    assert_eq!(name_reads("label.name_format(font)"), 0);
    assert_eq!(name_reads("engine.desc().name"), 0);
```

with:

```rust
    assert_eq!(name_reads("label.name_format(font)"), 0);
```

Then run `grep -n 'desc()' crates/manifold/src/names/tests.rs`. Expected: no output.

- [ ] **Step B5: `embedded_host` tests, part 1: imports, helpers and the two host loops**

In `crates/manifold/src/embedded_host.rs`, at HEAD :156-158, replace:

```rust
    use super::*;
    use crate::store::testutil::temp_dir;
    use sha2::{Digest, Sha256};
```

with:

```rust
    use super::*;
    use crate::store::testutil::temp_dir;
    use manifold_hosts::{Delivery, PDFIUM, WEBVIEW2};
    use sha2::{Digest, Sha256};
```

`Host` and `Os` come through `use super::*` (Step B8's import).

At HEAD :171-173, replace:

```rust
    /// The host folder's files other than the lock file.
    fn files(hosts: &Path) -> Vec<String> {
        names(Embedded::Webview2.host_path(hosts).parent().unwrap())
```

with:

```rust
    /// The `Delivery::Embedded` rows, in `manifold_hosts::ALL`'s order.
    fn embedded_rows() -> impl Iterator<Item = &'static Host> {
        manifold_hosts::ALL.into_iter().filter(|host| host.delivery == Delivery::Embedded)
    }

    /// The WebView2 host folder's files other than the lock file.
    fn files(hosts: &Path) -> Vec<String> {
        names(WEBVIEW2.installed_in(Os::here(), hosts).parent().unwrap())
```

At HEAD :185-187 (in `an_empty_embed_returns_none_and_touches_nothing`), replace:

```rust
        for host in [Embedded::Webview2, Embedded::Pdfium] {
            if host.bytes().0.is_empty() {
                assert_eq!(host.ensure(&hosts).unwrap(), None);
```

with:

```rust
        for host in embedded_rows() {
            if embedded(host).0.is_empty() {
                assert_eq!(ensure(host, &hosts).unwrap(), None);
```

At HEAD :310-322 (the body of `the_embedded_hosts_extract_to_their_pinned_hashes`), replace:

```rust
        for host in [Embedded::Webview2, Embedded::Pdfium] {
            let (embedded, embedded_sha) = host.bytes();
            if embedded.is_empty() {
                assert_eq!(embedded_sha, "");
                continue;
            }
            assert_eq!(hash(embedded), embedded_sha);
            let hosts = temp_dir("embed_real");
            let dest = host.ensure(&hosts).unwrap().unwrap();
            assert_eq!(dest, host.host_path(&hosts));
            assert_eq!(crate::view_install::sha256_file(&dest).unwrap(), embedded_sha);
            assert_eq!(std::fs::metadata(&dest).unwrap().len(), embedded.len() as u64);
        }
```

with:

```rust
        for host in embedded_rows() {
            let (bytes, sha256) = embedded(host);
            if bytes.is_empty() {
                assert_eq!(sha256, "");
                continue;
            }
            assert_eq!(hash(bytes), sha256);
            let hosts = temp_dir("embed_real");
            let dest = ensure(host, &hosts).unwrap().unwrap();
            assert_eq!(dest, host.installed_in(Os::here(), &hosts));
            assert_eq!(crate::view_install::sha256_file(&dest).unwrap(), sha256);
            assert_eq!(std::fs::metadata(&dest).unwrap().len(), bytes.len() as u64);
        }
```

The doc above that test (:307, "the dedicated run with `MANIFOLD_*_HOST` set at build time") stays: the variables keep their names.

- [ ] **Step B6: `embedded_host` tests, part 2: the call sites, the PDFium path and the table test**

In `crates/manifold/src/embedded_host.rs`, make these five Edit calls with `replace_all: true`. Each touches test code only, and the counts are at HEAD:

| old | new | hits |
|---|---|---|
| `ensure_with(Embedded::Webview2,` | `ensure_with(&WEBVIEW2,` | 11 (:182, 196, 205, 215, 225, 230, 233, 240, 253, 273, 328) |
| `Embedded::Webview2.host_path(&hosts)` | `WEBVIEW2.installed_in(Os::here(), &hosts)` | 6 (:197, 198, 222, 257, 259, 269) |
| `[Embedded::Webview2.file()]` | `[WEBVIEW2.file_name(Os::here())]` | 5 (:199, 216, 231, 260, 276) |
| `contains(Embedded::Webview2.file())` | `contains(WEBVIEW2.file_name(Os::here()).as_str())` | 2 (:241, 274) |
| `ensure_with(Embedded::Pdfium,` | `ensure_with(&PDFIUM,` | 1 (:329) |

At HEAD :228, replace:

```rust
        assert_eq!(files(&hosts), [Embedded::Webview2.file(), &format!("{}.old", Embedded::Webview2.file())]);
```

with:

```rust
        let file = WEBVIEW2.file_name(Os::here());
        assert_eq!(files(&hosts), [file.as_str(), &format!("{file}.old")]);
```

At HEAD :333-335, the end of `two_embedded_hosts_extract_side_by_side` and of the module, replace:

```rust
        assert!(b.ends_with("pdfium/manifold-view-pdfium.exe"), "{}", b.display());
    }
}
```

with:

```rust
        assert_eq!(b, PDFIUM.installed_in(Os::here(), &dir));
    }

    /// `build.rs` writes one entry per `Delivery::Embedded` row, in `manifold_hosts::ALL`'s order, so `embedded` finds
    /// each row's.
    #[test]
    fn the_generated_table_holds_the_embedded_rows_in_order() {
        let generated: Vec<&str> = EMBEDDED.iter().map(|entry| entry.0).collect();
        let rows: Vec<&str> = embedded_rows().map(|host| host.id).collect();
        assert!(!rows.is_empty(), "the table has embedded hosts");
        assert_eq!(generated, rows);
    }
}
```

The literal `pdfium/manifold-view-pdfium.exe` would fail on the Linux CI job, where the PDF view's file name has no `.exe`. The table test's non-empty assert keeps it from passing when both lists are empty.

Then run `grep -n 'Embedded::' crates/manifold/src/embedded_host.rs`. Expected: exactly two lines, `2:` (the module doc) and `68:` (`ensure_with`'s doc). Steps B8 and B9 rewrite both.

- [ ] **Step B7: `build.rs` generates the embedded table from the rows**

Replace the whole of `crates/manifold/build.rs` (HEAD :1-49) with:

```rust
//! Embeds each `Delivery::Embedded` row of `manifold_hosts::ALL` (the WebView2 host and the PDF view) in
//! `manifold.exe` when its variable (`Host::embed_var`: `MANIFOLD_WEBVIEW2_HOST`, `MANIFOLD_PDFIUM_HOST`) names a
//! file and the target is Windows (`release.yml` sets them; the extraction sets no execute bit, so the embed is a
//! Windows mechanism). Every other build embeds an empty stub, so dev builds, the gate and Linux CI use the hosts in
//! place. `embedded_host.rs` includes the generated `OUT_DIR/embedded.rs`: one `(id, bytes, sha256)` entry per row,
//! in `ALL`'s order, the bytes from `OUT_DIR/<id>-host.bin` and the lowercase hex SHA-256 as a literal (empty for the
//! stub). Only a row's `id`, `delivery` and `embed_var()` are read here, never a method taking an OS: `Os::here()`
//! would be the build machine, not the target. `MANIFOLD_HOST_PINS` names the manifest of the downloadable hosts a
//! release carries; it is copied to `OUT_DIR/host-pins.json` for `host_pin.rs` (empty without it).

use manifold_hosts::Delivery;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // A build script's `cfg(windows)` describes the build machine; the target is what decides whether a host exists.
    let windows = std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows");
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let mut table = String::from("// generated by build.rs: one entry per Embedded row, in ALL's order\n");
    table.push_str("static EMBEDDED: &[(&str, &[u8], &str)] = &[\n");
    for host in manifold_hosts::ALL.into_iter().filter(|host| host.delivery == Delivery::Embedded) {
        let var = host.embed_var();
        println!("cargo:rerun-if-env-changed={var}");
        let path = std::env::var_os(&var).filter(|path| !path.is_empty() && windows);
        let (bytes, sha256) = match path {
            Some(path) => {
                let path = PathBuf::from(path);
                println!("cargo:rerun-if-changed={}", path.display());
                let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{var} {}: {e}", path.display()));
                assert!(!bytes.is_empty(), "{var} {} is empty", path.display());
                let sha256 = Sha256::digest(&bytes).iter().map(|byte| format!("{byte:02x}")).collect::<String>();
                (bytes, sha256)
            }
            None => (Vec::new(), String::new()),
        };
        let bin = format!("{}-host.bin", host.id);
        let out = out_dir.join(&bin);
        std::fs::write(&out, bytes).unwrap_or_else(|e| panic!("{}: {e}", out.display()));
        table.push_str(&format!(
            "    ({:?}, include_bytes!(concat!(env!(\"OUT_DIR\"), \"/{bin}\")), {sha256:?}),\n",
            host.id
        ));
    }
    table.push_str("];\n");
    let out = out_dir.join("embedded.rs");
    std::fs::write(&out, table).unwrap_or_else(|e| panic!("{}: {e}", out.display()));
    println!("cargo:rerun-if-env-changed=MANIFOLD_HOST_PINS");
    let pins = match std::env::var_os("MANIFOLD_HOST_PINS").filter(|path| !path.is_empty()) {
        Some(path) => {
            println!("cargo:rerun-if-changed={}", PathBuf::from(&path).display());
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("MANIFOLD_HOST_PINS {path:?}: {e}"))
        }
        None => String::new(),
    };
    let out = out_dir.join("host-pins.json");
    std::fs::write(&out, pins).unwrap_or_else(|e| panic!("{}: {e}", out.display()));
}
```

What carries over from HEAD:
- The per-host body is HEAD's loop body (:22-36): the same variable check, panic texts, hash and blob file.
- The `MANIFOLD_HOST_PINS` block is HEAD's :39-48, reading the `out_dir` computed once.

What changes:
- The loop runs over the rows instead of `HOSTS`.
- `cargo:rustc-env` is gone. Each entry's hash is a literal in `embedded.rs`; `{:?}` writes `"webview2"` and `"<hex>"` or `""`.
- A change to a row reruns this script: the leaf is a build dependency, so cargo rebuilds the script and runs it again.

- [ ] **Step B8: `embedded_host` production, part 1: the doc, the import and the table**

In `crates/manifold/src/embedded_host.rs`, at HEAD :1-4, replace:

```rust
//! The hosts carried inside `manifold.exe`: `build.rs` embeds each when its
//! `MANIFOLD_*_HOST` variable names a file, and `Embedded::ensure` puts it at a stable path under `engine-hosts/` so a
//! bare `manifold.exe` works offline. A build without the variable embeds nothing: `ensure` returns `None` and the
//! sibling exe is used, as before.
```

with:

```rust
//! The hosts carried inside `manifold.exe`: `build.rs` embeds each `Delivery::Embedded` row of `manifold_hosts` when
//! its variable (`Host::embed_var`) names a file, and `ensure` puts it at a stable path under `engine-hosts/` so a
//! bare `manifold.exe` works offline. That path is `Host::installed_in`, a folder of its own per host (`<id>/`), so
//! the `.WebView2` user-data folder WebView2 keys on the exe path, the lock file and the `.old` leftover all stay out
//! of the flat host folder. A build without the variable embeds nothing for that host: `ensure` returns `None` and the
//! host in place is used, as before.
```

At HEAD :6, replace:

```rust
use std::io;
```

with:

```rust
use manifold_hosts::{Host, Os};
use std::io;
```

At HEAD :12-62, replace the whole block from the `Embedded` doc through the end of `impl Embedded`:

```rust
/// A host carried inside `manifold.exe`. Each has a folder of its own under `engine-hosts/`, so a host's lock file,
/// leftover `.old` and (WebView2) user-data folder stay apart.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Embedded {
    Webview2,
    Pdfium,
}

/// The host files' bytes (empty when this build embeds nothing) and their lowercase hex SHA-256 (`build.rs`).
static WEBVIEW2: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/webview2-host.bin"));
static PDFIUM: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/pdfium-host.bin"));

impl Embedded {
    fn folder(self) -> &'static str {
        match self {
            Self::Webview2 => "webview2",
            Self::Pdfium => "pdfium",
        }
    }
    /// Unchanged for WebView2: the exe name is how released builds and scripts know it.
    fn file(self) -> &'static str {
        match self {
            Self::Webview2 => "manifold-view-webview2.exe",
            Self::Pdfium => "manifold-view-pdfium.exe",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Webview2 => "the WebView2 host",
            Self::Pdfium => "the PDF view",
        }
    }
    fn bytes(self) -> (&'static [u8], &'static str) {
        match self {
            Self::Webview2 => (WEBVIEW2, env!("MANIFOLD_WEBVIEW2_SHA256")),
            Self::Pdfium => (PDFIUM, env!("MANIFOLD_PDFIUM_SHA256")),
        }
    }
    /// Where the extracted host lives under `hosts_dir` (`engine-hosts/`): its own folder, so the `.WebView2`
    /// user-data folder WebView2 keys on the exe path, the lock file and the `.old` leftover all stay out of the flat
    /// host folder.
    pub(crate) fn host_path(self, hosts_dir: &Path) -> PathBuf {
        hosts_dir.join(self.folder()).join(self.file())
    }
    /// Makes `host_path(hosts_dir)` hold the embedded host and returns it; `None` when this build embeds nothing (and
    /// nothing is touched).
    pub(crate) fn ensure(self, hosts_dir: &Path) -> io::Result<Option<PathBuf>> {
        let (bytes, sha256) = self.bytes();
        ensure_with(self, hosts_dir, bytes, sha256)
    }
}
```

with:

```rust
// `static EMBEDDED: &[(&str, &[u8], &str)]`, written by `build.rs`: per `Delivery::Embedded` row, in
// `manifold_hosts::ALL`'s order, its id, the host file's bytes and their lowercase hex SHA-256, both empty when this
// build embeds nothing for it.
include!(concat!(env!("OUT_DIR"), "/embedded.rs"));

/// The bytes and SHA-256 this build embeds for `host`: empty for a host with no entry (not `Delivery::Embedded`) or an
/// empty one.
fn embedded(host: &Host) -> (&'static [u8], &'static str) {
    match EMBEDDED.iter().find(|entry| entry.0 == host.id) {
        Some(&(_, bytes, sha256)) => (bytes, sha256),
        None => (&[], ""),
    }
}

/// Makes `host.installed_in(Os::here(), hosts_dir)` hold the host this build embeds and returns it; `None` when this
/// build embeds nothing for it (and nothing is touched).
pub(crate) fn ensure(host: &Host, hosts_dir: &Path) -> io::Result<Option<PathBuf>> {
    let (bytes, sha256) = embedded(host);
    ensure_with(host, hosts_dir, bytes, sha256)
}
```

The comment over `include!` is a `//` comment, not `///`: rustc's `unused_doc_comments` warns on a doc comment over a macro call. The old docs are kept elsewhere:
- the folder-of-its-own fact (old :12-13, :50-52) is now in the module doc;
- "the exe name is how released builds and scripts know it" (:31) is now the row's `exe`.

- [ ] **Step B9: `embedded_host` production, part 2: `ensure_with` takes the row**

In `crates/manifold/src/embedded_host.rs`, at HEAD :68, replace:

```rust
/// `Embedded::ensure` for the given `host` bytes and their SHA-256 `sha256`.
```

with:

```rust
/// `ensure` for `host`, with the given `bytes` and their SHA-256 `sha256`.
```

At HEAD :75-79, replace:

```rust
pub(crate) fn ensure_with(host: Embedded, hosts_dir: &Path, bytes: &[u8], sha256: &str) -> io::Result<Option<PathBuf>> {
    if bytes.is_empty() {
        return Ok(None);
    }
    let dest = host.host_path(hosts_dir);
```

with:

```rust
pub(crate) fn ensure_with(host: &Host, hosts_dir: &Path, bytes: &[u8], sha256: &str) -> io::Result<Option<PathBuf>> {
    if bytes.is_empty() {
        return Ok(None);
    }
    let dest = host.installed_in(Os::here(), hosts_dir);
    let file = host.file_name(Os::here());
```

Next, three messages name the host's file in place of the deleted labels (spec: "could not write manifold-view-pdfium.exe ..." in place of "the PDF view").

At HEAD :101, replace:

```rust
        std::fs::write(&temp, bytes).map_err(|e| wrap(&format!("could not write {} for", host.label()), e))?;
```

with:

```rust
        std::fs::write(&temp, bytes).map_err(|e| wrap(&format!("could not write {file} for"), e))?;
```

At HEAD :106, replace:

```rust
                &format!("{} written for", host.label()),
```

with:

```rust
                &format!("{file} written for"),
```

At HEAD :118, replace:

```rust
            return Err(wrap(&format!("could not move the new {} to", host.label()), e));
```

with:

```rust
            return Err(wrap(&format!("could not move the new {file} to"), e));
```

The rest of `ensure_with`, plus `EXTRACT`, `TEMP_COUNTER`, `sibling_with_suffix`, `retry_busy` and `spawn_fresh`, is unchanged. The two error tests (`a_bytes_hash_mismatch_writes_nothing`, `a_locked_file_that_differs_errors_naming_it_and_stays`) assert that the message contains the file name. Every message `wrap` builds (`"{what} <destination>: <error>"`) names the destination path, whose last component is that name, so both keep passing.

Then run:

```bash
grep -nE 'Embedded::|enum Embedded|impl Embedded|\.bytes\(\)|\.host_path\(|\.file\(\)|\.label\(\)|_SHA256' crates/manifold/src/embedded_host.rs
```

Expected: no output.

- [ ] **Step B10: `prepare_webview2` extracts the row's host**

In `crates/manifold/src/process_manager.rs`, in `prepare_webview2` (at HEAD :1012-1015), replace:

```rust
    let extracted = match (view_hosts_dir(), kind.desc().source) {
        (Some(dir), HostSource::Embedded(host)) if sibling_host(kind).is_none() => host.ensure(&dir)?,
        _ => None,
    };
```

with:

```rust
    let extracted = match view_hosts_dir() {
        Some(dir) if sibling_host(kind).is_none() => embedded_host::ensure(kind.desc().host, &dir)?,
        _ => None,
    };
```

`prepare_webview2` runs only for `ViewKind::Webview2`, so `kind.desc().host` is `WEBVIEW2`. A build that embeds nothing gets `None` from `ensure`, as before, and nothing is extracted. The doc above (at HEAD :1008-1010) stays.

- [ ] **Step B11: `pdf_view_exe` runs PDFium in place, else extracts it**

In `crates/manifold/src/process_manager.rs`, at HEAD :1210-1221, replace:

```rust
/// The PDF view exe: beside `manifold.exe` when built there (dev trees, the gate), else the copy embedded in this
/// build, extracted on first use. `None` when neither exists.
pub(crate) fn pdf_view_exe() -> std::io::Result<Option<std::path::PathBuf>> {
    let sibling = sibling_binary("manifold-view-pdfium");
    if sibling.exists() {
        return Ok(Some(sibling));
    }
    match view_hosts_dir() {
        Some(dir) => embedded_host::Embedded::Pdfium.ensure(&dir),
        None => Ok(None),
    }
}
```

with:

```rust
/// The PDF view exe: in place beside `manifold.exe` when built there (dev trees, the gate; `run_in_place`), else the
/// copy embedded in this build, extracted on first use (`embedded_host::ensure`). `None` when neither exists.
pub(crate) fn pdf_view_exe() -> std::io::Result<Option<std::path::PathBuf>> {
    if let Some(exe) = run_in_place(&manifold_hosts::PDFIUM) {
        return Ok(Some(exe));
    }
    match view_hosts_dir() {
        Some(dir) => embedded_host::ensure(&manifold_hosts::PDFIUM, &dir),
        None => Ok(None),
    }
}
```

- `run_in_place` reaches this function through Part A's plain `use availability::{..}` list (Step B1 checked it).
- `PDFIUM` is named by its full path. Part A's `use manifold_hosts::{Host, Os, CEF};` stays as it is, so `process_manager/tests.rs`'s explicit `PDFIUM` import never overlaps a glob-imported one.
- PDFium has no `dev_dir`, so `Host::in_place` gives only the place beside `manifold.exe`, which is HEAD's `sibling_binary` path.
- Under `cfg(test)`, `run_in_place` and `view_hosts_dir()` are both `None`, so `pdf_view_exe` returns `Ok(None)` and a unit test never runs a real PDF view. Its one caller is `pdf_render::render`. A test that queues a render through `PdfRenderer` gets "not installed", as at HEAD, where no PDF view sits beside the test exe in `target/debug/deps`.

- [ ] **Step B12: The PDF renderer's hint names the row's exe**

In `crates/manifold/src/pdf_render.rs`, at HEAD :140, replace:

```rust
        .ok_or_else(|| format!("{LABEL} is not installed: build manifold-view-pdfium"))?;
```

with:

```rust
        .ok_or_else(|| format!("{LABEL} is not installed: build {}", manifold_hosts::PDFIUM.exe))?;
```

The text is unchanged: `PDFIUM.exe` is `manifold-view-pdfium`. No test asserts it. The protocol crate's matching hints (`render.rs:239`, `tests/render_child.rs:50`, `:65`) stay, per the spec's "Not covered".

- [ ] **Step B13: See the deleted names gone (the guard's passing run, before cargo)**

Run:

```bash
grep -rnE 'HostSource|Embedded::|PLATFORM\.host_exe|PLATFORM\.exe_suffix|PLATFORM\.runtime_lib|CefPlatform::runtime_lib|beside_exe|in_place_paths|sibling_binary|host_file_name\(|ViewDesc::dev_dir|MANIFOLD_WEBVIEW2_SHA256|MANIFOLD_PDFIUM_SHA256' crates/manifold/src crates/manifold/build.rs | grep -v '/deleted_names\.rs:'
grep -rnE 'desc\(\)\.name\b|desc\.name\b' crates/manifold/src
grep -rnE 'HostSource|Embedded::|MANIFOLD_WEBVIEW2_SHA256|MANIFOLD_PDFIUM_SHA256' crates --include='*.rs' | grep -v '^crates/manifold/'
grep -rn 'manifold-view-pdfium' crates/manifold/src crates/manifold/build.rs | grep -vE '^[^:]+:[0-9]+:\s*//'
```

Expected: no output from any of the four.
- The first is Step B2's grep without the counting. It covers `build.rs`, which the new guard does not scan.
- The second checks that the names guard's exemption has nothing left to exempt.
- The third checks that no other crate names the deleted items. `crates/manifold-hosts/` is checked too: it does not match `^crates/manifold/`.
- The fourth checks that no app code outside comments spells the PDF view's exe name. At HEAD it found `process_manager.rs:1213`, `pdf_render.rs:140`, `embedded_host.rs:35` and `:333`.

Any hit is fixed in this task, then the grep is rerun.

- [ ] **Step B14: Format, build, and check `Cargo.lock`**

First, in PowerShell, run `Get-Process cargo, manifold -ErrorAction SilentlyContinue | Select-Object Id, ProcessName, Path`:
- Any `cargo` belongs to another session: wait for it to exit (one cargo per target dir).
- A `manifold` whose `Path` is this repo's `target\debug\manifold.exe` holds the exe the build replaces: ask the user to close it.

Then, from the repo root:

```bash
cargo fmt --all
cargo build -j 8 -p manifold
git diff -- Cargo.lock
```

Expected:
- The build ends in `Finished`, with no error and no warning. It is the first build of Parts A and B together.
- A compile error is fixed in this task, whichever part's file it is in, with the smallest change that keeps the spec's design. Report each such fix as a deviation.
- The `Cargo.lock` diff is one added line, ` "manifold-hosts",`, between ` "image",` and ` "manifold-pages",` in the `name = "manifold"` package's `dependencies` (at HEAD :5010-5011). Cargo lists normal and build dependencies once, together. Task 1 already added the `manifold-hosts` package itself.

- [ ] **Step B15: Run the app's tests**

`<scratchpad>` is this session's scratchpad directory (its 8.3 path has no space). Run:

```bash
cargo test -j 8 -p manifold --bin manifold -- --test-threads 8 2>&1 | tee "<scratchpad>/task2-tests.txt" | tail -n 5
grep -cE 'test (embedded_host::tests::the_generated_table_holds_the_embedded_rows_in_order|embedded_host::tests::an_empty_embed_returns_none_and_touches_nothing|deleted_names::no_source_names_a_deleted_host_layout_item|deleted_names::no_source_names_a_deleted_enum|names::tests::name_reads_counts_reads_only|names::tests::every_stored_name_reaches_the_screen_through_a_name_rule|process_manager::tests::an_extracted_webview2_host_keeps_its_user_data_folder_in_its_own_engine_hosts_folder) \.\.\. ok' "<scratchpad>/task2-tests.txt"
```

Expected:
- All pass: `test result: ok.` with `0 failed`.
- The count is `7`: this part's new and changed tests, plus Part A's `:1091` rewrite. Part A's own tests are in that run too.
- A failure is fixed in this task. Rerun until all pass.

- [ ] **Step B16: Check a real embed, then restore the stub**

This runs the path dev builds never take, where `build.rs` reads a host file, hashes it and writes a non-empty table. It takes two relinks of the test binary, a few minutes each. From the repo root:

```bash
cargo build -j 8 -p manifold-view-webview2 -p manifold-view-pdfium
MANIFOLD_WEBVIEW2_HOST="$(cygpath -w "$PWD/target/debug/manifold-view-webview2.exe")" MANIFOLD_PDFIUM_HOST="$(cygpath -w "$PWD/target/debug/manifold-view-pdfium.exe")" cargo test -j 8 -p manifold --bin manifold embedded_host::
cat "$(ls -t target/debug/build/manifold/*/out/embedded.rs | head -1)"
sha256sum target/debug/manifold-view-webview2.exe target/debug/manifold-view-pdfium.exe
```

Expected:
- `test result: ok. 11 passed; 0 failed`. With the hosts embedded, `the_embedded_hosts_extract_to_their_pinned_hashes` runs its body for both rows. It checks each hash, the extraction to `installed_in` and the size.
- `embedded.rs` is five lines:
  - the `// generated by build.rs` comment;
  - `static EMBEDDED: &[(&str, &[u8], &str)] = &[`;
  - a `("webview2", include_bytes!(...webview2-host.bin")), "<64 hex digits>"),` entry;
  - the same for `"pdfium"`;
  - `];`.
- The two hashes equal `sha256sum`'s, in the same order.
- The build script's output folder is `target/debug/build/manifold/<hash>/out/` with this cargo (observed 2026-10-29). If `ls` finds no `embedded.rs` there, find it with `find target/debug/build -name embedded.rs -newer crates/manifold/build.rs` before reading anything into it.

Then restore the stub build, so nothing after this step carries the embedded hosts:

```bash
cargo test -j 8 -p manifold --bin manifold embedded_host::
cat "$(ls -t target/debug/build/manifold/*/out/embedded.rs | head -1)"
```

Expected: `test result: ok. 11 passed; 0 failed`, and both entries end in `""`.

- [ ] **Step B17: Lint**

Run the clippy command from CLAUDE.md (Task 1 added `-p manifold-hosts` to it), then the format check:

```bash
cargo clippy -j 8 -p manifold -p manifold-hosts -p manifold-view-protocol -p manifold-pages -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings
cargo fmt --all --check
```

Expected: clippy exits 0 with no warning, and the format check prints nothing. A warning is fixed, not silenced. An `#[allow]` is used only with a `reason`, per CLAUDE.md.

- [ ] **Step B18: Commit Task 2 (Parts A and B)**

```bash
cargo fmt --all
git status --short
git diff --stat
git add -u -- crates/manifold Cargo.lock
git status --short
git commit -F - <<'EOF'
app: every host's layout comes from its row in manifold-hosts

ViewDesc holds host: &'static Host in place of name, binary, source,
host_os, runtime_dir and dev_dir, and an engine's store name is
desc().host.id. Deleted: HostSource, process_manager's CEF_RUNTIME_DIR
and runtime::Os (the leaf's are used), CefPlatform's runtime_lib,
exe_suffix and host_exe, and availability's sibling_binary,
host_file_name, beside_exe and in_place_paths. The in-place lookup reads
Host::in_place, the install reads the row, and host_pin::parse holds
each entry's files, as a set, to Host::pinned_under. WebKit's install
candidates on Windows hold no .exe name.

build.rs generates OUT_DIR/embedded.rs from the Embedded rows (WebView2,
PDFium), each hash a literal, so embedded_host::Embedded, its two
include_bytes! statics, build.rs's HOSTS and the MANIFOLD_<ID>_SHA256
rustc-env values are gone. embedded_host::ensure takes the row and
extracts to Host::installed_in; prepare_webview2 and pdf_view_exe call
it. The names guard loses its desc().name exemption, and deleted_names
guards the deleted host names.

Spec: docs/superpowers/specs/2026-10-29-host-table-design.md (H2, H3,
H7, H8, H11, H12; Order of work, step 2).

<attribution>
EOF
git log -1 --format=%B | grep -v '^$' | tail -n 1
```

Expected:
- The first `git status --short`:
  - ` M` lines under `crates/manifold/` and ` M Cargo.lock`;
  - nothing else. Any other path means a part touched a file outside its list: stop and report before staging.
- `git diff --stat` lists:
  - Part A's files;
  - `crates/manifold/build.rs`, `src/deleted_names.rs`, `src/embedded_host.rs`, `src/names/tests.rs`, `src/pdf_render.rs` and `src/process_manager.rs`;
  - `Cargo.lock` with `1 +`.
- The second `git status --short`: every path staged (`M` in the first column), and nothing else.
- The pre-commit hook runs `cargo fmt --all --check` and the clippy line, and both pass. Never use `--no-verify`. If the hook fails, fix the cause, stage again and make a new commit attempt.
- The last line printed is the attribution line.

No push, tag or version bump. This is Task 2's only commit. The tree builds and its tests pass again from here on.

### Task 3: `sync_host` and `run_in`

Spec step 3 ("The gate", `gate sync-runtime-deps`; Tests, Gate, the `sync_host` bullet; H9). `sync_servo` and `sync_cef` become one `sync_host` over the table's rows, and the run loops `manifold_hosts::ALL`.

**Files:**
- Modify: `crates/gate/Cargo.toml` (`[dependencies]`, HEAD :10-11): the dependency on `manifold-hosts`.
- Modify: `Cargo.lock` (cargo writes it on the first cargo command: one line in the `gate` package's `dependencies`).
- Modify: `crates/gate/src/sync_deps.rs` (345 lines at HEAD 10444ae5; Tasks 1 and 2 do not touch it): everything above `#[cfg(test)]` is rewritten (Step 6), and the test module is replaced (Step 4).
- Unchanged, checked: `crates/gate/src/main.rs:66`-`67` (`#[cfg(windows)] mod sync_deps;`, so the module and its tests compile only on Windows) and `:176` (the stage row `Run::Bare(crate::sync_deps::sync_runtime_deps)`, whose `pub fn sync_runtime_deps() -> ExitCode` keeps its signature). Nothing outside `sync_deps.rs` calls `run`, `sync_servo`, `sync_cef`, `find_newest`, `SERVO_DLLS` or `SERVO_EXE` (grep at 10444ae5: the only other hits are PARKED.md :61, :157, :158, whose entries Task 7 deletes).

**Interfaces:**
- Consumes (Task 1):
  - the root `[workspace.dependencies]` entry `manifold-hosts = { path = "crates/manifold-hosts" }`;
  - `manifold_hosts::{Host, BuildOutput, Os, ALL, SERVO, CEF, WEBKIT, WEBVIEW2, PDFIUM}`;
  - `Os::here() -> Os`;
  - `Host` fields `id: &'static str`, `exe: &'static str`, `dev_dir: Option<&'static str>`, `runtime: Option<Runtime>`; `Sidecar` fields `file: &'static str`, `from: BuildOutput`; `Runtime` fields `dir: &'static str`, `from: BuildOutput`; `BuildOutput` fields `krate: &'static str`, `tail: &'static str`;
  - `Host::file_name(&self, running: Os) -> String`, `Host::sidecars_for(&self, running: Os) -> impl Iterator<Item = &Sidecar>`, `Host::runtime_lib(&self, running: Os) -> Option<&'static str>`;
  - the rows' values the tests' fixtures spell: `SERVO` has `dev_dir` `Some("target-servo")` and, for `Os::Windows`, the sidecars `libEGL.dll` and `libGLESv2.dll`, each `from: BuildOutput { krate: "mozangle", tail: <the file> }`; `CEF` has `dev_dir` `None`, no sidecar, and `runtime` `{ dir: "cef-runtime", lib: libcef.dll on Windows, from: BuildOutput { krate: "cef-dll-sys", tail: "cef_windows_x86_64" } }`; `WEBKIT`, `WEBVIEW2` and `PDFIUM` have no sidecar for `Os::Windows` and no runtime; `ALL` is `[&SERVO, &CEF, &WEBKIT, &WEBVIEW2, &PDFIUM]`.
- Consumes (Start, Step S3): BEFORE, the `sync-before` run folder under `%LOCALAPPDATA%\claude-runs\manifold\` (`out.log`, `err.log`, `run.json`) holding `target/debug/gate.exe sync-runtime-deps` run on this tree at the plan's start.
- Produces (in `crates/gate/src/sync_deps.rs`):

```rust
/// The newest match in the first root that has one (both cargo layouts per root). Task 5 calls it on
/// `[root.join("target/release/build")]` for the sidecars of the hosts `release-pins` copies.
pub(crate) fn find_first(roots: &[PathBuf], from: &BuildOutput) -> Option<PathBuf>;
// private to the module:
#[derive(Debug, PartialEq, Eq)]
enum HostSync { NothingToSync, NoBuildFolder, Synced }
fn sync_host(root: &Path, host: &Host, outcome: &mut Outcome) -> HostSync;
fn run_in(root: &Path) -> Result<(), Vec<String>>;
fn copy_runtime(out: &Path, folder: &Path, dir: &str, outcome: &mut Outcome) -> io::Result<()>;
```

- Produces (the output Task 7's README "Runtime-file placement" text describes; each line starts `sync-runtime-deps: `):
  - unchanged: `recopied <label>`, `<label> already current`, `copied <dir>/<name>/`, `<label> is in use: close manifold and run again`, `everything already current, nothing to do`; changed only in stream: `no candidate found for <dir>/<exe file>, skipping` (stdout through `say()`, stderr at HEAD `:72`; Q-streams); the labels are `<dev_dir or target>/debug/<file>` for a sidecar and `<runtime.dir>/<name>` for a runtime file or the exe copy;
  - new skip and keep lines (stdout): `no <dev_dir or target>/debug, skipping <package>`, `no build output holds <file>, keeping <label> in place`, `no build output holds <file> and no <exe file> is built, skipping`, `no build output holds <package>'s runtime, keeping <dir>/<lib> in place`, `no build output holds <package>'s runtime and no <exe file> is built, skipping`;
  - errors (stderr, exit 1): `no build output holds <file>, which <exe file> needs: build <package> first`; `no build output holds the runtime <exe file> needs (<lib>): a CEF_PATH build leaves it outside OUT_DIR, so copy it into <dev_dir or target>/debug/<dir>/`; `no build folder for any host - build first`.
  - `<package>` is `Host.exe` (`manifold-view-servo`), `<exe file>` is `file_name(Os::here())` (`manifold-view-servo.exe`).
- Leaves stale for Task 7: README `:90` ("it takes the newest of either"), inside the "Runtime-file placement" section the spec's Docs list names; PARKED.md :61, :157, :158.

- [ ] **Step 1: Start**

- Run the start check (Global Constraints, "Start condition") and the "One cargo per target dir" check.
- `git log --oneline -1` shows Task 2's commit.
- `git diff --stat 10444ae5 HEAD -- crates/gate/src/sync_deps.rs crates/gate/Cargo.toml` prints nothing, so the HEAD line numbers below hold. If it prints anything, re-find every quoted anchor by its text before going on.
- `grep -n "manifold-hosts" Cargo.toml` shows Task 1's `[workspace.dependencies]` line `manifold-hosts = { path = "crates/manifold-hosts" }` (and the `members` entry). If it does not, stop: Task 1 is not in.

- [ ] **Step 2: Give the gate the dependency**

In `crates/gate/Cargo.toml` (HEAD :10-11), replace:

```toml
[dependencies]
serde_json = { workspace = true, features = ["preserve_order"] }
```

with:

```toml
[dependencies]
manifold-hosts.workspace = true
serde_json = { workspace = true, features = ["preserve_order"] }
```

- [ ] **Step 3: Add the new types as stubs, so the tests can compile and fail on behaviour**

In `crates/gate/src/sync_deps.rs`, edit 1 (HEAD :22-24). Replace:

```rust
use std::time::SystemTime;

/// Servo's whole runtime besides its exe (release.yml ships the same three files).
```

with:

```rust
use std::time::SystemTime;

use manifold_hosts::Host;

/// Servo's whole runtime besides its exe (release.yml ships the same three files).
```

Edit 2 (HEAD :176). Replace:

```rust
fn run() -> Result<(), Vec<String>> {
```

with:

```rust
/// What `sync_host` found, so `run_in` and the tests read it instead of captured output.
#[derive(Debug, PartialEq, Eq)]
enum HostSync {
    NothingToSync,
    NoBuildFolder,
    Synced,
}

/// Stub until Step 6: reports nothing to sync and touches nothing.
fn sync_host(_root: &Path, _host: &Host, _outcome: &mut Outcome) -> HostSync {
    HostSync::NothingToSync
}

/// Stub until Step 6: succeeds without syncing.
fn run_in(_root: &Path) -> Result<(), Vec<String>> {
    Ok(())
}

fn run() -> Result<(), Vec<String>> {
```

Run: `cargo test -j 8 -p gate sync_deps`
Expected: it compiles, with dead-code warnings naming `HostSync`, `sync_host` and `run_in` (nothing calls them yet); `test result: ok. 4 passed; 0 failed` (the four tests at HEAD). Then `git diff -- Cargo.lock` shows exactly one added line, ` "manifold-hosts",`, between ` "image",` and ` "serde_json",` in the `name = "gate"` package's `dependencies` (HEAD Cargo.lock :3092-3098).

- [ ] **Step 4: Write the failing tests**

In `crates/gate/src/sync_deps.rs`, replace the whole test module, from `#[cfg(test)]` (HEAD :213) to the end of the file (HEAD :345), with the module below. The module is too large and repeats too much to be an Edit `old_string`: Read the file, then Write it whole, keeping everything above `#[cfg(test)]` exactly as it is (Step 3's version) and putting the new module below it. It keeps `touch`, `scratch` and `a_held_file_says_to_close_manifold` as they are; splits `the_newest_in_either_build_folder_wins_and_sync_copies_only_on_change` (HEAD :235) into `each_part_comes_from_the_hosts_own_build_folder_first_and_the_newest_there` (the first root that has a match wins, not the newest across roots) and `sync_file_copies_only_on_change`; rewrites `servo_dlls_go_beside_its_host_and_a_failed_one_does_not_stop_the_rest` (HEAD :257) and `a_cef_file_that_cant_be_copied_is_listed_and_the_rest_still_copy` (HEAD :296) over `sync_host` with fixtures under `<root>/target-servo/debug` and `<root>/target/debug`, keeping their assertions (the error text is now "build manifold-view-servo first"); and adds four tests for the rest of the rule. They assert on `HostSync`, `Outcome` and the files, never on printed lines. The literals `target-servo/debug/libEGL.dll: ` and `cef-runtime/a.dll: ` pin the two label forms.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use manifold_hosts::{Os, CEF, PDFIUM, SERVO, WEBKIT, WEBVIEW2};

    fn touch(p: &Path, secs: u64, bytes: &[u8]) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
        let f = fs::OpenOptions::new().append(true).open(p).unwrap();
        f.set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs)).unwrap();
    }

    /// A fresh, empty folder for one test, under the system temp folder.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gate-sync-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The names in `folder`, sorted.
    fn names_in(folder: &Path) -> Vec<String> {
        let mut names: Vec<String> =
            fs::read_dir(folder).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        names.sort();
        names
    }

    /// A part comes from the host's own build folder when that holds a match, the newest written there across both
    /// cargo layouts, even though `target/`'s is newer still; a part only `target/` holds comes from there; another
    /// crate's output is never taken.
    #[test]
    fn each_part_comes_from_the_hosts_own_build_folder_first_and_the_newest_there() {
        let root = scratch("first");
        let (own, main) = (root.join("target-servo/debug/build"), root.join("target/debug/build"));
        touch(&own.join("mozangle-aaa/out/libEGL.dll"), 10, b"own, older");
        touch(&own.join("mozangle/bbb/out/libEGL.dll"), 20, b"own, newest");
        touch(&main.join("mozangle-ccc/out/libEGL.dll"), 30, b"target, newer still");
        touch(&own.join("other-ddd/out/libEGL.dll"), 40, b"another crate");
        touch(&main.join("mozangle/eee/out/libGLESv2.dll"), 10, b"target only");
        let mut outcome = Outcome::default();
        assert_eq!(sync_host(&root, &SERVO, &mut outcome), HostSync::Synced);
        assert!(outcome.changed && outcome.errors.is_empty(), "{:?}", outcome.errors);
        let debug = root.join("target-servo/debug");
        assert_eq!(fs::read(debug.join("libEGL.dll")).unwrap(), b"own, newest");
        assert_eq!(fs::read(debug.join("libGLESv2.dll")).unwrap(), b"target only");
        let _ = fs::remove_dir_all(&root);
    }

    /// A file is copied when its bytes differ from the copy's and not again once they agree; with no source it is
    /// skipped.
    #[test]
    fn sync_file_copies_only_on_change() {
        let dir = scratch("copy");
        let (src, dst) = (dir.join("src.dll"), dir.join("dst.dll"));
        fs::write(&src, b"one").unwrap();
        assert!(sync_file(Some(&src), &dst, "x").unwrap(), "first copy");
        assert!(!sync_file(Some(&src), &dst, "x").unwrap(), "already current");
        fs::write(&src, b"two").unwrap();
        assert!(sync_file(Some(&src), &dst, "x").unwrap(), "changed bytes copy again");
        assert_eq!(fs::read(&dst).unwrap(), b"two");
        assert!(!sync_file(None, &dst, "x").unwrap(), "no source: skipped");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Servo's DLLs go beside its host in `target-servo/debug/`. With no such folder the host is skipped; with the
    /// folder and neither a host nor a DLL in any build, nothing happens; a DLL that can't be copied is listed and the
    /// other still copies; a built host with no DLL to give it is an error for each.
    #[test]
    fn servo_dlls_go_beside_its_host_and_a_failed_one_does_not_stop_the_rest() {
        let dir = scratch("servo");
        for dll in ["libEGL.dll", "libGLESv2.dll"] {
            touch(&dir.join("target/debug/build/mozangle-aaa/out").join(dll), 10, dll.as_bytes());
        }
        let mut outcome = Outcome::default();
        assert_eq!(sync_host(&dir, &SERVO, &mut outcome), HostSync::NoBuildFolder);
        assert!(!outcome.changed && outcome.errors.is_empty(), "no target-servo/debug: skipped, {:?}", outcome.errors);

        let empty = dir.join("empty");
        fs::create_dir_all(empty.join("target-servo/debug")).unwrap();
        let mut outcome = Outcome::default();
        assert_eq!(sync_host(&empty, &SERVO, &mut outcome), HostSync::Synced);
        assert!(!outcome.changed && outcome.errors.is_empty(), "no host and no DLL: skipped, {:?}", outcome.errors);

        let servo_debug = dir.join("target-servo/debug");
        fs::create_dir_all(servo_debug.join("libEGL.dll")).unwrap();
        let mut outcome = Outcome::default();
        assert_eq!(sync_host(&dir, &SERVO, &mut outcome), HostSync::Synced);
        assert_eq!(outcome.errors.len(), 1, "{:?}", outcome.errors);
        assert!(outcome.errors[0].starts_with("target-servo/debug/libEGL.dll: "), "{:?}", outcome.errors);
        assert!(outcome.changed, "libGLESv2.dll still copied");
        assert_eq!(fs::read(servo_debug.join("libGLESv2.dll")).unwrap(), b"libGLESv2.dll");

        let bare = dir.join("bare");
        touch(&bare.join("target-servo/debug").join(SERVO.file_name(Os::here())), 10, b"host");
        let mut outcome = Outcome::default();
        assert_eq!(sync_host(&bare, &SERVO, &mut outcome), HostSync::Synced);
        assert_eq!(outcome.errors.len(), 2, "{:?}", outcome.errors);
        assert!(outcome.errors.iter().all(|e| e.contains("build manifold-view-servo first")), "{:?}", outcome.errors);
        let _ = fs::remove_dir_all(&dir);
    }

    /// CEF's runtime and its host go into `cef-runtime/`: a file that can't be copied is listed while every other
    /// file, the locales folder and the host exe still copy (a failed file does not stop the exe), and the sync then
    /// fails; once everything is current a rerun copies nothing and succeeds.
    #[test]
    fn a_cef_file_that_cant_be_copied_is_listed_and_the_rest_still_copy() {
        let root = scratch("cef");
        let td = root.join("target/debug");
        let out = td.join("build/cef-dll-sys-aaa/out/cef_windows_x86_64");
        for file in ["a.dll", "b.dll", "locales/en.pak"] {
            touch(&out.join(file), 10, file.as_bytes());
        }
        let exe = CEF.file_name(Os::here());
        touch(&td.join(&exe), 10, b"host");
        let runtime = td.join("cef-runtime");
        // A folder where a file should go: copying onto it fails on every run.
        fs::create_dir_all(runtime.join("a.dll")).unwrap();
        let mut outcome = Outcome::default();
        assert_eq!(sync_host(&root, &CEF, &mut outcome), HostSync::Synced);
        assert_eq!(outcome.errors.len(), 1, "{:?}", outcome.errors);
        assert!(outcome.errors[0].starts_with("cef-runtime/a.dll: "), "{:?}", outcome.errors);
        for file in ["b.dll", "locales/en.pak"] {
            assert_eq!(fs::read(runtime.join(file)).unwrap(), file.as_bytes(), "{file}");
        }
        assert_eq!(fs::read(runtime.join(&exe)).unwrap(), b"host");
        assert!(outcome.changed);
        let errors = outcome.errors.clone();
        assert_eq!(finish(outcome), Err(errors), "a failed step fails the sync");

        fs::remove_dir(runtime.join("a.dll")).unwrap();
        let mut outcome = Outcome::default();
        assert_eq!(sync_host(&root, &CEF, &mut outcome), HostSync::Synced);
        assert!(outcome.changed && outcome.errors.is_empty(), "a.dll now copies: {:?}", outcome.errors);
        let mut outcome = Outcome::default();
        assert_eq!(sync_host(&root, &CEF, &mut outcome), HostSync::Synced);
        assert!(!outcome.changed && outcome.errors.is_empty(), "all current: {:?}", outcome.errors);
        assert_eq!(finish(outcome), Ok(()));
        let _ = fs::remove_dir_all(&root);
    }

    /// A part no build output holds is kept when it is in place: a sidecar beside the exe, or a runtime folder
    /// holding its marker file. Neither is an error or copied over, and a runtime in place still gets the built exe.
    /// A built exe whose part is neither found nor in place is an error. A host with no runtime gets no copy of its
    /// exe.
    #[test]
    fn a_part_in_place_is_kept_and_the_exe_follows_a_runtime_in_place() {
        let root = scratch("kept");
        let servo_debug = root.join("target-servo/debug");
        let servo_exe = SERVO.file_name(Os::here());
        touch(&servo_debug.join(&servo_exe), 10, b"servo host");
        touch(&servo_debug.join("libEGL.dll"), 10, b"kept");
        let mut outcome = Outcome::default();
        assert_eq!(sync_host(&root, &SERVO, &mut outcome), HostSync::Synced);
        assert_eq!(outcome.errors.len(), 1, "only libGLESv2.dll is missing: {:?}", outcome.errors);
        assert!(outcome.errors[0].contains("libGLESv2.dll"), "{:?}", outcome.errors);
        assert!(!outcome.changed);
        assert_eq!(fs::read(servo_debug.join("libEGL.dll")).unwrap(), b"kept");
        assert_eq!(names_in(&servo_debug), ["libEGL.dll", servo_exe.as_str()], "no copy of a host without a runtime");

        let td = root.join("target/debug");
        let cef_exe = CEF.file_name(Os::here());
        touch(&td.join(&cef_exe), 10, b"cef host");
        touch(&td.join("cef-runtime/libcef.dll"), 10, b"runtime in place");
        let mut outcome = Outcome::default();
        assert_eq!(sync_host(&root, &CEF, &mut outcome), HostSync::Synced);
        assert!(outcome.changed && outcome.errors.is_empty(), "{:?}", outcome.errors);
        assert_eq!(fs::read(td.join("cef-runtime/libcef.dll")).unwrap(), b"runtime in place");
        assert_eq!(fs::read(td.join("cef-runtime").join(&cef_exe)).unwrap(), b"cef host", "the exe follows");
        let _ = fs::remove_dir_all(&root);
    }

    /// A CEF build with no runtime in any build output and none in place (a `CEF_PATH` build) is exactly one error,
    /// naming `CEF_PATH` and the folder to copy the runtime into; the exe copy is not attempted, so no folder is
    /// created. With no exe built either, it is skipped.
    #[test]
    fn a_cef_build_with_no_runtime_is_one_error_naming_cef_path_and_creates_no_folder() {
        let root = scratch("cef-path");
        let td = root.join("target/debug");
        fs::create_dir_all(&td).unwrap();
        let mut outcome = Outcome::default();
        assert_eq!(sync_host(&root, &CEF, &mut outcome), HostSync::Synced);
        assert!(!outcome.changed && outcome.errors.is_empty(), "no exe and no runtime: skipped, {:?}", outcome.errors);
        assert!(!td.join("cef-runtime").exists());

        touch(&td.join(CEF.file_name(Os::here())), 10, b"cef host");
        let mut outcome = Outcome::default();
        assert_eq!(sync_host(&root, &CEF, &mut outcome), HostSync::Synced);
        assert_eq!(outcome.errors.len(), 1, "{:?}", outcome.errors);
        assert!(outcome.errors[0].contains("CEF_PATH"), "{:?}", outcome.errors);
        assert!(outcome.errors[0].contains("target/debug/cef-runtime/"), "{:?}", outcome.errors);
        assert!(!outcome.changed);
        assert!(!td.join("cef-runtime").exists(), "no folder created");
        let _ = fs::remove_dir_all(&root);
    }

    /// Servo and CEF have parts to sync, so a tree without their build folders skips each. WebKit, WebView2 and
    /// PDFium have no sidecar on this OS and no runtime: nothing is looked for, even with build folders there.
    #[test]
    fn a_host_with_nothing_to_sync_is_not_looked_for() {
        let root = scratch("nothing");
        for host in [&SERVO, &CEF] {
            let mut outcome = Outcome::default();
            assert_eq!(sync_host(&root, host, &mut outcome), HostSync::NoBuildFolder, "{}", host.id);
            assert!(!outcome.changed && outcome.errors.is_empty(), "{}: {:?}", host.id, outcome.errors);
        }
        for dev in ["target", "target-linux"] {
            fs::create_dir_all(root.join(dev).join("debug")).unwrap();
        }
        for host in [&WEBKIT, &WEBVIEW2, &PDFIUM] {
            let mut outcome = Outcome::default();
            assert_eq!(sync_host(&root, host, &mut outcome), HostSync::NothingToSync, "{}", host.id);
            assert!(!outcome.changed && outcome.errors.is_empty(), "{}: {:?}", host.id, outcome.errors);
        }
        let _ = fs::remove_dir_all(&root);
    }

    /// The run fails only when no host with something to sync has its build folder: either host's folder is enough,
    /// and WebKit's, whose host has nothing to sync, does not count.
    #[test]
    fn the_run_fails_only_when_no_host_has_its_build_folder() {
        let none = Err(vec!["no build folder for any host - build first".to_string()]);
        let root = scratch("run");
        assert_eq!(run_in(&root), none, "an empty tree");
        fs::create_dir_all(root.join("target-linux/debug")).unwrap();
        assert_eq!(run_in(&root), none, "WebKit's folder alone");
        fs::create_dir_all(root.join("target-servo/debug")).unwrap();
        assert_eq!(run_in(&root), Ok(()), "Servo's folder: CEF's missing target/debug is skipped");
        let other = scratch("run-target");
        fs::create_dir_all(other.join("target/debug")).unwrap();
        assert_eq!(run_in(&other), Ok(()), "target/debug alone: Servo's missing folder is skipped");
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&other);
    }

    /// A file Windows holds open (a DLL a running host loaded) is named, with what to do about it.
    #[test]
    fn a_held_file_says_to_close_manifold() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = scratch("held");
        let (src, dst) = (dir.join("src.dll"), dir.join("dst.dll"));
        fs::write(&src, b"new").unwrap();
        fs::write(&dst, b"old").unwrap();
        // FILE_SHARE_READ, as a loaded DLL allows: comparing reads it, and the copy over it is the step that fails.
        let held = fs::OpenOptions::new().read(true).share_mode(1).open(&dst).unwrap();
        let mut outcome = Outcome::default();
        outcome.step("target-servo/debug/dst.dll", sync_file(Some(&src), &dst, "dst"));
        assert_eq!(outcome.errors, ["target-servo/debug/dst.dll is in use: close manifold and run again"]);
        drop(held);
        let _ = fs::remove_dir_all(&dir);
    }
}
```

- [ ] **Step 5: Run the tests to see them fail**

Run: `cargo test -j 8 -p gate sync_deps`
Expected: it compiles, and Step 3's dead-code warnings are gone (the tests call the stubs). `test result: FAILED. 2 passed; 7 failed`. Passing: `sync_deps::tests::sync_file_copies_only_on_change` and `sync_deps::tests::a_held_file_says_to_close_manifold` (`sync_file` and `Outcome` are unchanged). Failing, each at its first `assert_eq!` with `left: NothingToSync` (the stub's answer):
- `each_part_comes_from_the_hosts_own_build_folder_first_and_the_newest_there` (right: `Synced`);
- `servo_dlls_go_beside_its_host_and_a_failed_one_does_not_stop_the_rest` (right: `NoBuildFolder`);
- `a_cef_file_that_cant_be_copied_is_listed_and_the_rest_still_copy` (right: `Synced`);
- `a_part_in_place_is_kept_and_the_exe_follows_a_runtime_in_place` (right: `Synced`);
- `a_cef_build_with_no_runtime_is_one_error_naming_cef_path_and_creates_no_folder` (right: `Synced`);
- `a_host_with_nothing_to_sync_is_not_looked_for` (right: `NoBuildFolder`, message `servo`);
- and `the_run_fails_only_when_no_host_has_its_build_folder` with `left: Ok(())`, `right: Err(["no build folder for any host - build first"])` (the stub `run_in`'s answer).

A compile error instead means a name in the test module differs from Task 1's crate: compare it with the spec's "The crate" and fix the test, never the leaf.

- [ ] **Step 6: Write `sync_host`, `find_first` and `run_in`, and delete what they replace**

Replace everything in `crates/gate/src/sync_deps.rs` above `#[cfg(test)]` (the module doc through `sync_runtime_deps`, including Step 3's stubs) with the code below. As in Step 4, Read the file and Write it whole: the code below, then the test module exactly as Step 4 left it. It deletes `SERVO_DLLS`, `SERVO_EXE`, `find_newest`, `sync_servo`, `sync_cef` and `run`'s literal paths; `copy_dir`, `Outcome`, `step_error`, `finish` and `sync_runtime_deps` are copied unchanged, and `sync_file` differs from HEAD in one line (its no-candidate skip line goes through `say()`, on stdout: Q-streams); `copy_runtime` takes its label prefix and folder name from the row (`dir`, today the literal `cef-runtime` at HEAD :157). The repo root comes from the existing `crate::cef_pin::repo()` (`cef_pin.rs:20`-`22`, the same `CARGO_MANIFEST_DIR/../..` that HEAD :177 spelled). `run` is no longer canonicalized: every printed path is a relative label, and a missing `target/debug` is now one host's skip, not the run's failure (H9).

```rust
//! `gate sync-runtime-deps` (was scripts/sync-runtime-deps.sh): puts each engine host's runtime files beside the host
//! a dev tree runs (README "Runtime-file placement"). Cargo does not, and the fingerprint folder they are built into
//! can change on a rebuild that touched neither Cargo.toml nor Cargo.lock, silently staling an earlier copy.
//!
//! One rule for every row of `manifold_hosts::ALL` (`sync_host`). A host's build folder is `<dev_dir>/debug/`, or
//! `target/debug/` for a host with no `dev_dir`. Its sidecars go beside its exe there (Servo's ANGLE DLLs beside
//! `target-servo/debug/manifold-view-servo.exe`), and its runtime and a copy of its exe go into its runtime folder
//! there (CEF's into `target/debug/cef-runtime/`, the copy manifold runs). Each part comes from the host's own
//! `build/` folder when that holds one, else from `target/debug/build/` (`find_first`). A part found in neither is
//! kept when it is already in place, an error when the host exe is built, and skipped otherwise. A host whose build
//! folder is missing is skipped; the run fails only when no host with something to sync has one.
//!
//! Safe to run any time (a no-op when everything is current); run it after every build of manifold-view-servo or
//! manifold-view-cef. Every step runs even when one fails: the failures are listed at the end and the exit code is 1,
//! and a file a running host holds open is named with "close manifold and run again". Not a build.rs: a build script
//! sees only its own crate's OUT_DIR.
//!
//! The source is the most recently *written* match, not the newest fingerprint directory, whose mtime cargo touches
//! on every build. Newer cargo nightlies put build-script output under `build/<crate>/<hash>/`, older ones under
//! `build/<crate>-<hash>/`, and a target dir built by both has each.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::SystemTime;

use manifold_hosts::{BuildOutput, Host, Os};

use crate::cef_pin::repo;

fn say(msg: &str) {
    println!("sync-runtime-deps: {msg}");
}

fn mtime(p: &Path) -> Option<SystemTime> {
    fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// The newest match by mtime in the first of `roots` that has one. Each root is searched in both cargo layouts,
/// `<root>/<krate>-*/out/<tail>` and `<root>/<krate>/*/out/<tail>`; a root that isn't there is skipped.
pub(crate) fn find_first(roots: &[PathBuf], from: &BuildOutput) -> Option<PathBuf> {
    roots.iter().find_map(|root| {
        let mut found = Vec::new();
        for e in fs::read_dir(root).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with(&format!("{}-", from.krate)) {
                found.push(e.path().join("out").join(from.tail));
            } else if name == from.krate {
                for h in fs::read_dir(e.path()).into_iter().flatten().flatten() {
                    found.push(h.path().join("out").join(from.tail));
                }
            }
        }
        found.into_iter().filter_map(|p| Some((mtime(&p)?, p))).max().map(|(_, p)| p)
    })
}

fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for e in fs::read_dir(from)? {
        let e = e?;
        let dst = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_dir(&e.path(), &dst)?;
        } else {
            fs::copy(e.path(), dst)?;
        }
    }
    Ok(())
}

/// Copies `src` over `dst` unless their bytes already agree; true if it copied.
fn sync_file(src: Option<&Path>, dst: &Path, label: &str) -> io::Result<bool> {
    let Some(src) = src else {
        say(&format!("no candidate found for {label}, skipping"));
        return Ok(false);
    };
    if dst.is_file() && fs::read(src)? == fs::read(dst)? {
        say(&format!("{label} already current"));
        return Ok(false);
    }
    fs::copy(src, dst)?;
    say(&format!("recopied {label}"));
    Ok(true)
}

/// What the steps did: whether any copied, and a line for each that failed.
#[derive(Default)]
struct Outcome {
    changed: bool,
    errors: Vec<String>,
}

impl Outcome {
    /// Records one step's result; `dst` names the file it was writing.
    fn step(&mut self, dst: &str, result: io::Result<bool>) {
        match result {
            Ok(changed) => self.changed |= changed,
            Err(e) => self.errors.push(step_error(dst, &e)),
        }
    }
}

/// The line for a step that failed. A file Windows holds open (a DLL loaded by a running host: os error 32, 33 or
/// 1224) says to close manifold, the usual holder.
fn step_error(dst: &str, e: &io::Error) -> String {
    if matches!(e.raw_os_error(), Some(32 | 33 | 1224)) {
        format!("{dst} is in use: close manifold and run again")
    } else {
        format!("{dst}: {e}")
    }
}

/// What `sync_host` found, so `run_in` and the tests read it instead of captured output.
#[derive(Debug, PartialEq, Eq)]
enum HostSync {
    /// No sidecar for this OS and no runtime: nothing is looked for or printed, and the run does not count it.
    NothingToSync,
    /// Its build folder is missing: skipped, with one line.
    NoBuildFolder,
    /// Its build folder is there, and each part was copied, kept, skipped or listed as a failure.
    Synced,
}

/// One host's parts in its build folder, `root/<dev_dir or target>/debug/`: each sidecar for this OS beside the exe,
/// then, for a host with a runtime, the runtime and a copy of the exe in `<runtime.dir>/`. A part comes from
/// `find_first` over the folder's own `build/`, then `target/debug/build/`. One found in neither is kept when it is
/// in place (the sidecar beside the exe; the runtime folder holding its marker file), an error when the exe is built,
/// and skipped otherwise. The exe copy is always compared (it is rebuilt with its crate), and is attempted only once
/// the runtime was copied or is in place: otherwise the line above already says why, and the copy would fail on a
/// missing folder as a second error.
fn sync_host(root: &Path, host: &Host, outcome: &mut Outcome) -> HostSync {
    let here = Os::here();
    if host.sidecars_for(here).next().is_none() && host.runtime.is_none() {
        return HostSync::NothingToSync;
    }
    let (dev, package) = (host.dev_dir.unwrap_or("target"), host.exe);
    let folder = root.join(dev).join("debug");
    if !folder.is_dir() {
        say(&format!("no {dev}/debug, skipping {package}"));
        return HostSync::NoBuildFolder;
    }
    let exe = host.file_name(here);
    let exe_there = folder.join(&exe).is_file();
    let mut roots = vec![folder.join("build")];
    if host.dev_dir.is_some() {
        roots.push(root.join("target").join("debug").join("build"));
    }
    for sidecar in host.sidecars_for(here) {
        let file = sidecar.file;
        let label = format!("{dev}/debug/{file}");
        let dst = folder.join(file);
        match find_first(&roots, &sidecar.from) {
            Some(src) => outcome.step(&label, sync_file(Some(&src), &dst, &label)),
            None if dst.is_file() => say(&format!("no build output holds {file}, keeping {label} in place")),
            None if exe_there => outcome
                .errors
                .push(format!("no build output holds {file}, which {exe} needs: build {package} first")),
            None => say(&format!("no build output holds {file} and no {exe} is built, skipping")),
        }
    }
    if let (Some(runtime), Some(lib)) = (&host.runtime, host.runtime_lib(here)) {
        let dir = runtime.dir;
        let dst = folder.join(dir);
        let runtime_ready = match find_first(&roots, &runtime.from) {
            Some(out) => match copy_runtime(&out, &folder, dir, outcome) {
                Ok(()) => true,
                Err(e) => {
                    outcome.errors.push(step_error(dir, &e));
                    false
                }
            },
            None if dst.join(lib).is_file() => {
                say(&format!("no build output holds {package}'s runtime, keeping {dir}/{lib} in place"));
                true
            }
            None if exe_there => {
                outcome.errors.push(format!(
                    "no build output holds the runtime {exe} needs ({lib}): a CEF_PATH build leaves it outside \
                     OUT_DIR, so copy it into {dev}/debug/{dir}/"
                ));
                false
            }
            None => {
                say(&format!("no build output holds {package}'s runtime and no {exe} is built, skipping"));
                false
            }
        };
        if runtime_ready {
            let label = format!("{dir}/{exe}");
            let built = folder.join(&exe);
            outcome.step(&label, sync_file(exe_there.then_some(built.as_path()), &dst.join(&exe), &label));
        }
    }
    HostSync::Synced
}

/// Each file of a runtime's build output `out` into `folder/<dir>/`, recopied when it differs, and each directory
/// (CEF's locales/) only when missing or empty, since those don't change between rebuilds. A file or directory that
/// fails is listed and the rest still copy; only a folder that can't be created or listed stops it.
fn copy_runtime(out: &Path, folder: &Path, dir: &str, outcome: &mut Outcome) -> io::Result<()> {
    let runtime = folder.join(dir);
    fs::create_dir_all(&runtime)?;
    for e in fs::read_dir(out)? {
        let e = e?;
        let name = e.file_name().to_string_lossy().into_owned();
        let label = format!("{dir}/{name}");
        let dst = runtime.join(&name);
        if e.file_type()?.is_dir() {
            if fs::read_dir(&dst).map_or(true, |mut d| d.next().is_none()) {
                match copy_dir(&e.path(), &dst) {
                    Ok(()) => {
                        say(&format!("copied {label}/"));
                        outcome.changed = true;
                    }
                    Err(err) => outcome.errors.push(step_error(&label, &err)),
                }
            }
        } else {
            outcome.step(&label, sync_file(Some(&e.path()), &dst, &label));
        }
    }
    Ok(())
}

/// Syncs every host into the dev tree at `root`. Fails with each failed step, or when no host with something to
/// sync has its build folder.
fn run_in(root: &Path) -> Result<(), Vec<String>> {
    let mut outcome = Outcome::default();
    let mut synced = false;
    for host in manifold_hosts::ALL {
        synced |= sync_host(root, host, &mut outcome) == HostSync::Synced;
    }
    if !synced {
        return Err(vec!["no build folder for any host - build first".to_string()]);
    }
    finish(outcome)
}

fn run() -> Result<(), Vec<String>> {
    run_in(&repo())
}

/// What the sync reports once every step ran: each failure (so the exit code is 1), else success, saying so when
/// nothing needed copying.
fn finish(outcome: Outcome) -> Result<(), Vec<String>> {
    if !outcome.errors.is_empty() {
        return Err(outcome.errors);
    }
    if !outcome.changed {
        say("everything already current, nothing to do");
    }
    Ok(())
}

pub fn sync_runtime_deps() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(errors) => {
            for e in errors {
                eprintln!("sync-runtime-deps: {e}");
            }
            ExitCode::from(1)
        }
    }
}
```

rustfmt may rewrap the `None if exe_there => outcome.errors.push(...)` arm and the `outcome.step(&label, sync_file(...))` line; that is fine. Notes on types, for a reader checking the code against Task 1's crate: `for host in manifold_hosts::ALL` copies the `static` array (`[&'static Host; 5]` is `Copy`); `(&host.runtime, host.runtime_lib(here))` binds `runtime: &Runtime` and `lib: &'static str`, so no `expect` is needed for a host with a runtime; `sidecar.file`, `runtime.dir` and `host.exe` are `&'static str`, bound to locals so every `format!` argument is inlined (clippy's `uninlined_format_args`).

- [ ] **Step 7: Run the tests to see them pass, then clippy and the greps**

Run: `cargo test -j 8 -p gate -- --test-threads 8`
Expected: `test result: ok.`, with the nine `sync_deps::tests::` tests among the passed (the seven of Step 5 now pass); no warning.

Run the clippy line (Global Constraints; with Task 1's `-p manifold-hosts`): `cargo clippy -j 8 -p manifold -p manifold-hosts -p manifold-view-protocol -p manifold-pages -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings`
Expected: no warning, exit 0.

Then from bash:

```bash
grep -rn "find_newest\|sync_servo\|sync_cef\|SERVO_DLLS\|SERVO_EXE" crates
sed -n '1,/^#\[cfg(test)\]/p' crates/gate/src/sync_deps.rs | grep -v '^//!' | grep -n 'mozangle\|cef-runtime\|cef-dll-sys\|cef_windows\|manifold-view-\|libEGL\|libGLESv2\|libcef\|target-servo'
```

Expected: both print nothing. The first proves no caller of a deleted item is left in code (PARKED.md's three hits are Task 7's); the second that the module's code above the tests spells no host's layout, only the table's (the `//!` module doc names Servo and CEF as examples).

- [ ] **Step 8: Run the sync on this tree and compare it with the BEFORE capture**

In PowerShell, run `Get-Process cargo, gate, manifold, manifold-view-cef, manifold-view-servo -ErrorAction SilentlyContinue | Select-Object Id, ProcessName, Path`. Any `cargo` or `gate` is another session's: wait for it to exit. A `manifold` or host whose `Path` is in this repo's `target\debug` or `target-servo\debug` holds files the sync may replace: ask the user to close it. Then from bash (`<scratchpad>` is your session's scratchpad directory):

```bash
cargo build -j 8 -p gate
BEFORE=$(ls -dt "$LOCALAPPDATA"/claude-runs/manifold/????-??-??-sync-before* | head -1); echo "$BEFORE"
target/debug/gate.exe sync-runtime-deps > "<scratchpad>/sync-after.out" 2> "<scratchpad>/sync-after.err"; echo "exit $?"
diff "$BEFORE/out.log" "<scratchpad>/sync-after.out"; echo "diff exit $?"
cat "<scratchpad>/sync-after.err"
```

`$BEFORE` is the run folder Step S3 recorded (`out.log`, `err.log`, `run.json`). This is an unrecorded smoke run: it writes no `docs/runs.md` row, and Task 8 Step 10 makes the recorded comparison.

Expected: `exit 0`, `diff exit 0` with no diff output, and an empty `sync-after.err` (as `$BEFORE/err.log` is).

Both runs start from a synced tree (Step S3 ran the sync once before recording), so no `recopied` or `copied` line is expected in either. The one exception is `sync-runtime-deps: recopied cef-runtime/manifold-view-cef.exe` in AFTER (and no `everything already current` line), when something rebuilt `target/debug/manifold-view-cef.exe` after the capture: `stat -c '%y' target/debug/manifold-view-cef.exe "$BEFORE/out.log"` then shows the exe newer than the BEFORE log. No task up to this one builds that host (no host crate depends on `manifold-hosts`, H1), so expect it only if another session built it; say so in the report.

Why nothing else differs (the spec's "What changes for this machine's tree"):
- Servo's two lines read `sync-runtime-deps: target-servo/debug/libEGL.dll already current` (and `libGLESv2.dll`) in both. The source moves from the newest across both trees (`target/debug/build/mozangle/ad274970f12e1e29/out/`, mtime 1792731909) to `target-servo/debug/build/mozangle/ad274970f12e1e29/out/`, and the source is never printed. Both trees' outputs and the copies in `target-servo/debug/` are byte-identical: sha256 `63f9174985fd…` for every `libEGL.dll` and `2cf899204b24…` for every `libGLESv2.dll` (checked 2026-10-29).
- CEF has no `dev_dir`, so its one root is `target/debug/build`, as at HEAD. Its one match is `cef-dll-sys/1350bf2d9e1a185f/out/cef_windows_x86_64` (the other three hash folders hold no `cef_windows_x86_64`). Its labels (`cef-runtime/<name>`, `cef-runtime/manifold-view-cef.exe`) and their order (`read_dir`, then the exe) are unchanged.
- The rows run in `ALL`'s order (Servo, CEF, WebKit, WebView2, PDFium): Servo, then CEF, as at HEAD. The last three have nothing to sync and print nothing. Both build folders exist, so no skip, keep or error line is new.

Any other difference stops the task: report `$BEFORE/out.log`, `sync-after.out` and `sync-after.err`. That includes a line naming another file, a new skip, keep or error line, or a non-zero `exit`. A `<label> is in use: close manifold and run again` line in AFTER only means a running manifold holds the file: ask the user to close it, then run the sync and the `diff` again.

- [ ] **Step 9: Commit**

```bash
cargo fmt --all
git diff --stat
git add Cargo.lock crates/gate/Cargo.toml crates/gate/src/sync_deps.rs
git commit -F - <<'EOF'
gate: sync-runtime-deps reads the host table, one rule for every host

sync_servo and sync_cef become sync_host over manifold_hosts::ALL, and
run becomes run_in(root). A host's own build folder is searched before
target/'s (find_first), so Servo's DLLs come from target-servo's
mozangle output first. A host whose build folder is missing is skipped,
and the run fails only when no host with something to sync has one. A
part found in no build output is kept when it is in place, an error
only when the host exe is built. The CEF exe is copied into cef-runtime/
only once the runtime was copied or is in place, so a CEF_PATH build
with no runtime gets one error naming CEF_PATH instead of a bare
path-not-found. Every skip line now goes to stdout through say(); two
were on stderr. SERVO_DLLS, SERVO_EXE, find_newest, sync_servo and
sync_cef are deleted. The gate depends on manifold-hosts (Cargo.lock).

<attribution>
EOF
git log -1 --format=%B | grep -v '^$' | tail -n 1
```

Expected: `git diff --stat` lists exactly `Cargo.lock` (1 insertion), `crates/gate/Cargo.toml` (1 insertion) and `crates/gate/src/sync_deps.rs`. The pre-commit hook passes (fmt and clippy). The last line printed is your session's `Co-Authored-By:` trailer, not `<attribution>`.

### Task 4: The other gate readers

The gate's remaining readers of a host's layout take it from `manifold-hosts` (spec "Other gate readers", except `release_verify`, which is Task 6): `fresh.rs`'s runtime-copy check and dep-info list, `package.rs`'s install-check files, engine checks, platform and extracted-host checks, the fixtures and probes (`jar.rs`, `probes.rs`, `shots.rs`, `stress.rs`), `sleep.rs`'s `kill_hosts` and `all.rs`'s `SITE_HOST` rule. On Windows every name read from the table is the one spelled today. What a run shows differently: `gate package`'s two extracted-host checks are labelled by the host's file (`manifold-view-webview2.exe was extracted under the redirected APPDATA`) and come after the PDF render check, its Servo state check comes before CEF's (the table's order), and `kill_hosts` looks for WebKit's host as `manifold-view-webkitgtk` (it was `manifold-view-webkit.exe`, which no host is called; neither is a Windows process, see Step 13).

Sites deliberately left as they are (spec "Not covered" and H13): the build lines and `-p` lists (`main.rs:8`, `:246`, `all.rs:20`, `:289`-`301`, `sleep.rs:20`, `shots.rs:50`, the module docs of `click_count.rs`, `iframe_typing.rs`, `page_text.rs`, `search.rs`; PARKED "The `-p` crate lists are repeated"), the profile prefix `manifold/data` (`package.rs:168`, `:226`, `shots.rs:1590`: only their `engine-hosts` component comes from the table), `all.rs:283`'s comment (it says the page server's address is for the WebKit stages, still true), `package.rs:26`'s PDF fixture path, `shots.rs:1633`'s `ready` marker, the `"webview2"` and `"cef"` scenario strings, and test literals that pin today's values on purpose (`fresh.rs:153`-`166`, `package.rs:263`, `:266`). `sync_deps.rs` is Task 3's, `release_verify.rs` Task 6's, `tier.rs` Task 1's.

**Files:**
- Modify: `crates/gate/src/fresh.rs`: the imports (:5-6), a new `DEP_INFO_HOSTS` static, `require_fresh_exe` (:53-90), the test module (:93-171).
- Modify: `crates/gate/src/package.rs`: the imports (:19-21), `newest_release` (:30-43), `pins` (:45-49), `install_check` (:105-116), `installed_files` and `install` (:129-139), `install`'s hosts folder (:168), `run`'s engine loops and extracted-host checks (:208-210, :219-240), a new test (Step 6).
- Modify: `crates/gate/src/main.rs`: a new test in `windows_main::tests` (:344-359).
- Modify: `crates/gate/src/jar.rs`: the imports (:35), `ten` (:481), `jar` (:744).
- Modify: `crates/gate/src/probes.rs`: the imports (:22), `failed_spawn` (:559-562).
- Modify: `crates/gate/src/shots.rs`: the module doc (:37), the imports (:58), `FAKE_CEF_HOST` (:73-75), `engine_app` (:1590), `fake_host` (:1625), `fake_runtime` (:1630-1632), `engine_states` (:1641-1653, :1704), the test (:1801-1802).
- Modify: `crates/gate/src/stress.rs`: the imports (:15), `suspend_engines` (:36-46).
- Modify: `crates/gate/src/sleep.rs`: the imports (:24-26), `kill_hosts` (:82-87).
- Modify: `crates/gate/src/all.rs`: the imports (:36), the `SITE_HOST` case (:318-326).

**Interfaces:**
- Consumes (Task 1, the leaf, exactly as the spec's "The crate" declares it): `manifold_hosts::{Os, Host, Delivery, ALL, CEF, WEBVIEW2, PDFIUM, HOSTS_DIR, CEF_RUNTIME_DIR, by_id}`; `Os::here()`, `Os::pin_platform(self) -> &'static str`; the fields `Host.id`, `Host.exe`, `Host.delivery`, `Host.runtime: Option<Runtime>`, `Runtime.dir`; the methods `Host::built_for(&self, Os) -> Os`, `file_name(&self, Os) -> String`, `beside(&self, Os, &Path) -> PathBuf`, `installed_in(&self, Os, &Path) -> PathBuf`, `installed_files(&self, Os) -> Vec<String>`, `runtime_lib(&self, Os) -> Option<&'static str>`.
- Consumes (Task 3): `crates/gate/Cargo.toml`'s `manifold-hosts` dependency and its `Cargo.lock` entry.
- Produces:

```rust
// crates/gate/src/package.rs
/// The Download hosts built for Os::here(), in ALL's order: servo, then cef.
pub(crate) fn installable() -> impl Iterator<Item = &'static manifold_hosts::Host>;
fn installed_files(engine: &str) -> Option<Vec<String>>; // was Option<Vec<&'static str>>
fn install(engine: &str, files: &[String], exe_src: &Path, work: &Path) -> io::Result<()>; // was files: &[&str]
// crates/gate/src/fresh.rs
static DEP_INFO_HOSTS: [&Host; 2]; // [&WEBVIEW2, &PDFIUM]
// crates/gate/src/shots.rs
fn fake_cef_host() -> (String, u64, &'static str); // replaces const FAKE_CEF_HOST
```

- Produces (behaviour): `gate install-check`'s two messages list `installable`'s ids sorted, so they read as today and as `main.rs`'s usage literal: with no arguments `usage: gate install-check cef|servo EXE`, with an engine `installable` does not yield `install-check knows cef and servo, not "<engine>"`, both exiting 2. `gate all` passes `SITE_HOST` to a step whose engine's host is built for another OS than the gate's, and without it logs `GATE <step> not run: WITH_ENGINES has <engine> but SITE_HOST is not set`.

- [ ] **Step 1: Start**

- `git status --short -- crates README.md PARKED.md docs` prints nothing: Task 3 is committed.
- `grep -n "manifold-hosts" crates/gate/Cargo.toml` prints the dependency line Task 3 added. If it prints nothing, stop: Task 3 is not in.
- `git diff --stat 10444ae5 HEAD -- crates/gate/src/fresh.rs crates/gate/src/package.rs crates/gate/src/main.rs crates/gate/src/jar.rs crates/gate/src/probes.rs crates/gate/src/shots.rs crates/gate/src/stress.rs crates/gate/src/sleep.rs crates/gate/src/all.rs` prints nothing. 10444ae5 is the commit this task was written against, so every old text below is as quoted and at the line numbers given. If a file is listed, find each old text in it by its content before editing it.
- `Get-Process cargo, gate -ErrorAction SilentlyContinue | Select-Object Id, ProcessName, Path` (PowerShell) prints nothing. A `cargo` or `gate` is another session's: wait for it to exit, never stop it (a second cargo on `target/` blocks on its lock, and Step 15's build cannot replace a running `gate.exe`).

- [ ] **Step 2: Write the failing `fresh.rs` tests**

These tests pin the runtime-copy check and build the fixture from the rows. The check exists today with literal names, so the new test pins it across the rewrite. It fails now only because the fixture reads `DEP_INFO_HOSTS` and `Os`, which Step 4 adds.

In `crates/gate/src/fresh.rs`, replace the head of the test module (at HEAD :93-97):

```rust
mod tests {
    use super::*;

    #[test]
    fn dep_info_unescapes_spaces() {
```

with:

```rust
mod tests {
    use manifold_hosts::CEF;

    use super::*;

    #[test]
    fn dep_info_unescapes_spaces() {
```

(The house order, as in `session.rs`'s tests: external crates, a blank line, then `use super::*;`.)

Replace the fixture's doc, struct, `new` and `touch` (at HEAD :103-129):

```rust
    /// A folder with manifold.exe, the WebView2 host and the PDF view, each with a `.d` naming `src.rs`; `src.rs` and
    /// every exe get the mtimes (seconds since the epoch) the test sets.
    struct Bin(PathBuf);

    impl Bin {
        fn new(name: &str) -> Bin {
            let dir = std::env::temp_dir().join(format!("gate-fresh-test-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let bin = Bin(dir);
            bin.touch("src.rs", 1);
            for n in ["manifold", "manifold-view-webview2", "manifold-view-pdfium"] {
                let rule = format!("{}: {}", bin.0.join(format!("{n}.exe")).display(), bin.0.join("src.rs").display());
                std::fs::write(
                    bin.0.join(format!("{n}.d")),
                    rule.replace(' ', "\\ ").replacen(".exe:\\ ", ".exe: ", 1),
                )
                .unwrap();
                bin.touch(&format!("{n}.exe"), 2);
            }
            bin
        }

        fn touch(&self, file: &str, secs: u64) {
            let f = std::fs::OpenOptions::new().create(true).append(true).open(self.0.join(file)).unwrap();
            f.set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs)).unwrap();
        }
```

with:

```rust
    /// A folder with manifold.exe and the hosts in `DEP_INFO_HOSTS`, each with a `.d` naming `src.rs`, and CEF's built
    /// exe with its copy in its runtime folder (`cef_exe_and_copy`); `src.rs` and every exe get the mtimes (seconds
    /// since the epoch) the test sets.
    struct Bin(PathBuf);

    impl Bin {
        fn new(name: &str) -> Bin {
            let dir = std::env::temp_dir().join(format!("gate-fresh-test-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let bin = Bin(dir);
            bin.touch("src.rs", 1);
            // `manifold` is the app's own exe, not a host row.
            let exes = std::iter::once(("manifold", "manifold.exe".to_owned()))
                .chain(DEP_INFO_HOSTS.iter().map(|h| (h.exe, h.file_name(Os::here()))));
            for (stem, file) in exes {
                let rule = format!("{}: {}", bin.0.join(&file).display(), bin.0.join("src.rs").display());
                std::fs::write(
                    bin.0.join(format!("{stem}.d")),
                    rule.replace(' ', "\\ ").replacen(".exe:\\ ", ".exe: ", 1),
                )
                .unwrap();
                bin.touch(&file, 2);
            }
            let (cef, copy) = cef_exe_and_copy();
            bin.touch(&cef, 2);
            bin.touch(&copy, 2);
            bin
        }

        /// Sets `file`'s mtime, creating the file and its folder if they are missing.
        fn touch(&self, file: &str, secs: u64) {
            let path = self.0.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let f = std::fs::OpenOptions::new().create(true).append(true).open(&path).unwrap();
            f.set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs)).unwrap();
        }
```

At the end of the module, replace the close of `fresh_exes_pass_and_each_stale_one_is_named` and of the module (at HEAD :169-172):

```rust
        bin.touch("manifold.exe", 2);
        assert!(bin.check().unwrap_err().contains("manifold.exe is older"), "only manifold.exe stale");
    }
}
```

with:

```rust
        bin.touch("manifold.exe", 2);
        assert!(bin.check().unwrap_err().contains("manifold.exe is older"), "only manifold.exe stale");
    }

    /// CEF's built exe and its copy in its runtime folder, relative to a `Bin`'s folder.
    fn cef_exe_and_copy() -> (String, String) {
        let file = CEF.file_name(Os::here());
        let runtime = CEF.runtime.as_ref().expect("CEF has a runtime");
        let copy = format!("{}/{file}", runtime.dir);
        (file, copy)
    }

    /// A host's copy in its runtime folder (CEF's `cef-runtime/`) older than the built exe is refused, naming the sync
    /// that refreshes it and `ALLOW_STALE`; with no copy there is nothing to compare.
    #[test]
    fn a_stale_runtime_copy_is_refused_and_an_absent_one_skipped() {
        if std::env::var_os("ALLOW_STALE").is_some() {
            return;
        }
        let bin = Bin::new("runtime");
        let (cef, copy) = cef_exe_and_copy();
        assert_eq!(bin.check(), Ok(()), "the copy is as new as the built exe");

        bin.touch(&cef, 3);
        let err = bin.check().unwrap_err();
        assert!(err.contains(&format!("{copy} is older than the built one")), "{err}");
        assert!(err.contains("sync-runtime-deps") && err.contains("ALLOW_STALE"), "{err}");

        std::fs::remove_file(bin.0.join(&copy)).unwrap();
        assert_eq!(bin.check(), Ok(()), "an absent copy is skipped");
    }
}
```

The literals in `fresh_exes_pass_and_each_stale_one_is_named` (at HEAD :150-167) stay: they pin today's names and today's build hint on purpose.

- [ ] **Step 3: Run them to see them fail**

Run: `cargo test -j 8 -p gate fresh::`
Expected: a compile error in the test module: `error[E0425]: cannot find value `DEP_INFO_HOSTS` in this scope` and `error[E0433]: failed to resolve: use of undeclared type `Os``.

- [ ] **Step 4: Check every host with a runtime, and name the dep-info hosts by row**

In `crates/gate/src/fresh.rs`, replace the imports and the doc line after them (at HEAD :5-8):

```rust
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The sources a dep-info file lists: one make rule, `<exe>: <src> <src> ...`, with a space in a path escaped as
```

with:

```rust
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use manifold_hosts::{Host, Os, PDFIUM, WEBVIEW2};

/// The hosts the gate's build line builds beside manifold.exe: separate crates that `-p manifold` does not build, so
/// each is judged by its own dep-info, and the build hint names them (one list of these is PARKED "The `-p` crate
/// lists are repeated").
static DEP_INFO_HOSTS: [&Host; 2] = [&WEBVIEW2, &PDFIUM];

/// The sources a dep-info file lists: one make rule, `<exe>: <src> <src> ...`, with a space in a path escaped as
```

In `require_fresh_exe`, replace (at HEAD :53-70):

```rust
    let dir = exe.parent().unwrap_or(Path::new("."));
    // CEF runs from an isolated copy that only `gate sync-runtime-deps` refreshes. The app itself refuses a copy whose
    // protocol number differs from its own (Unavailable, naming which side to rebuild); this catches a copy that is
    // older but speaks the same protocol, which would run old code silently.
    let cef = dir.join("manifold-view-cef.exe");
    let cef_copy = dir.join("cef-runtime").join("manifold-view-cef.exe");
    if let (Some(built), Some(copy)) = (mtime(&cef), mtime(&cef_copy)) {
        if built > copy {
            return Err(
                "cef-runtime/manifold-view-cef.exe is older than the built one: run `target/debug/gate.exe sync-runtime-deps`, or set ALLOW_STALE=1."
                    .into(),
            );
        }
    }
    // The WebView2 host and the PDF view are separate crates that `-p manifold` does not build; judge each by its own
    // dep-info, and skip one that is absent.
    let hosts = [dir.join("manifold-view-webview2.exe"), dir.join("manifold-view-pdfium.exe")];
    let mut report = String::new();
```

with:

```rust
    let dir = exe.parent().unwrap_or(Path::new("."));
    // A host with a runtime (CEF) runs from an isolated copy in its runtime folder that only `gate sync-runtime-deps`
    // refreshes. The app itself refuses a copy whose protocol number differs from its own (Unavailable, naming which
    // side to rebuild); this catches a copy that is older but speaks the same protocol, which would run old code
    // silently.
    for host in manifold_hosts::ALL {
        let Some(runtime) = &host.runtime else { continue };
        let file = host.file_name(Os::here());
        if let (Some(built), Some(copy)) = (mtime(&dir.join(&file)), mtime(&host.beside(Os::here(), dir))) {
            if built > copy {
                return Err(format!(
                    "{}/{file} is older than the built one: run `target/debug/gate.exe sync-runtime-deps`, or set ALLOW_STALE=1.",
                    runtime.dir
                ));
            }
        }
    }
    // The hosts in `DEP_INFO_HOSTS`, each judged by its own dep-info; one that is absent is skipped.
    let hosts: Vec<PathBuf> = DEP_INFO_HOSTS.iter().map(|h| dir.join(h.file_name(Os::here()))).collect();
    let mut report = String::new();
```

The loop after it (`for e in std::iter::once(exe).chain(hosts.iter().map(PathBuf::as_path))`) is unchanged: a `Vec<PathBuf>`'s `iter()` yields the same `&PathBuf` items as the array's did.

Replace the end of the function (at HEAD :83-89):

```rust
    if report.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{report}Build first (cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium), or set ALLOW_STALE=1."
        ))
    }
```

with:

```rust
    if report.is_empty() {
        Ok(())
    } else {
        let crates: String = DEP_INFO_HOSTS.iter().map(|h| format!(" -p {}", h.exe)).collect();
        Err(format!("{report}Build first (cargo build -j 8 -p manifold{crates}), or set ALLOW_STALE=1."))
    }
```

The hint reads as today: `cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium`. `host.beside(Os::here(), dir)` is `dir/cef-runtime/manifold-view-cef.exe` for CEF (the leaf's rule: inside the runtime folder for a host with one), the path the literals spelled.

- [ ] **Step 5: Run the `fresh.rs` tests to see them pass**

Run: `cargo test -j 8 -p gate fresh::`
Expected: `test result: ok. 3 passed; 0 failed` (`dep_info_unescapes_spaces`, `fresh_exes_pass_and_each_stale_one_is_named`, `a_stale_runtime_copy_is_refused_and_an_absent_one_skipped`).

- [ ] **Step 6: Pin install-check's file lists before they move to the table**

`installed_files` moves from two literal lists to `Host::installed_files`. This test pins today's lists. Its comparison compiles against both the old `Vec<&str>` and the new `Vec<String>` (each `join`s to a `String`), so it runs before and after.

In `crates/gate/src/package.rs`'s test module, replace the end of `the_manifest_pins_by_engine_and_platform` and of the module (at HEAD :266-269):

```rust
        assert!(!pins(r#"{"cef": {"linux-x64": [{}]}}"#, "cef"));
        assert!(!pins("", "cef"));
    }
}
```

with:

```rust
        assert!(!pins(r#"{"cef": {"linux-x64": [{}]}}"#, "cef"));
        assert!(!pins("", "cef"));
    }

    /// install-check installs the downloaded hosts built for the gate's OS, and finds on disk the files they had before
    /// the lists came from the host table. WebKit (built for Linux), the embedded hosts and an id spelled in another
    /// case are not installable.
    #[test]
    fn install_check_knows_cef_and_servo_and_their_files() {
        let files = |engine: &str| installed_files(engine).map(|f| f.join(" "));
        assert_eq!(files("cef").as_deref(), Some("manifold-view-cef.exe cef-runtime/libcef.dll"));
        assert_eq!(files("servo").as_deref(), Some("manifold-view-servo.exe libEGL.dll libGLESv2.dll"));
        for other in ["webkit", "webview2", "pdfium", "CEF", "chrome"] {
            assert_eq!(files(other), None, "{other}");
        }
    }
}
```

Run: `cargo test -j 8 -p gate package::tests`
Expected: `test result: ok. 3 passed; 0 failed`. The new test passes against today's literal lists.

- [ ] **Step 7: Write the failing install-check usage test**

`install-check`'s usage stays the literal `cef|servo EXE` in `STAGES` (a `const` table of `&'static str`s). This test ties the literal to the table's filter.

In `crates/gate/src/main.rs`, replace the end of `windows_main::tests` (at HEAD :356-360):

```rust
            assert!(!usage.contains("gate hold"), "the test aid is not listed");
            assert!(is_stage("trail") && !is_stage("sleep-history"));
        }
    }
}
```

with:

```rust
            assert!(!usage.contains("gate hold"), "the test aid is not listed");
            assert!(is_stage("trail") && !is_stage("sleep-history"));
        }

        /// `install-check`'s usage names exactly the engines it installs, as a set: the downloaded hosts built for this
        /// OS. `STAGES` holds only literals, so the literal is checked here against the host table.
        #[test]
        fn install_checks_usage_lists_the_installable_hosts() {
            let usage = STAGES.iter().find(|s| s.name == "install-check").and_then(|s| s.usage).unwrap_or("");
            let listed: std::collections::BTreeSet<&str> =
                usage.split_whitespace().next().unwrap_or("").split('|').collect();
            let hosts: std::collections::BTreeSet<&str> = crate::package::installable().map(|h| h.id).collect();
            assert_eq!(listed, hosts, "gate install-check {usage}");
        }
    }
}
```

Run: `cargo test -j 8 -p gate windows_main::tests`
Expected: a compile error: `error[E0425]: cannot find function `installable` in module `crate::package``.

- [ ] **Step 8: Take install-check's hosts and files from one filter**

All edits in `crates/gate/src/package.rs`.

Add the leaf's group between the std and `crate::` imports (at HEAD :19-21):

```rust
use std::time::{Duration, Instant};

use crate::check::{check, Section};
```

becomes:

```rust
use std::time::{Duration, Instant};

use manifold_hosts::{Delivery, Host, Os};

use crate::check::{check, Section};
```

Replace the head of `install_check` (at HEAD :105-116):

```rust
/// `gate install-check ENGINE EXE`: the exe alone in a temp folder with APPDATA redirected, asked to install the
/// engine (`cef` or `servo`) from the network, as a user's Download click does. For a published release only: a draft's asset URLs are not public. Waits up
/// to 10 minutes for `VIEWSTATE ENGINE` to read Installed or Failed.
pub(crate) fn install_check(args: &[String]) -> ExitCode {
    let [engine, exe_src] = args else {
        eprintln!("usage: gate install-check cef|servo EXE");
        return ExitCode::from(2);
    };
    let Some(files) = installed_files(engine) else {
        eprintln!("install-check knows cef and servo, not {engine:?}");
        return ExitCode::from(2);
    };
```

with:

```rust
/// `gate install-check ENGINE EXE`: the exe alone in a temp folder with APPDATA redirected, asked to install the
/// engine (one `installable` yields: `cef` or `servo`) from the network, as a user's Download click does. For a
/// published release only: a draft's asset URLs are not public. Waits up to 10 minutes for `VIEWSTATE ENGINE` to read
/// Installed or Failed.
pub(crate) fn install_check(args: &[String]) -> ExitCode {
    // Sorted, so the messages read as the stage's usage literal in `main.rs` (`cef|servo EXE`).
    let mut ids: Vec<&str> = installable().map(|h| h.id).collect();
    ids.sort_unstable();
    let [engine, exe_src] = args else {
        eprintln!("usage: gate install-check {} EXE", ids.join("|"));
        return ExitCode::from(2);
    };
    let Some(files) = installed_files(engine) else {
        eprintln!("install-check knows {}, not {engine:?}", ids.join(" and "));
        return ExitCode::from(2);
    };
```

Replace `installed_files` and `install`'s signature (at HEAD :129-139):

```rust
/// The files, relative to `engine-hosts/`, an installed `engine` leaves on disk; None for an engine that is not
/// downloaded. The Servo list is the manifest's three files and is unconfirmed until a published release carries them.
fn installed_files(engine: &str) -> Option<Vec<&'static str>> {
    match engine {
        "cef" => Some(vec!["manifold-view-cef.exe", "cef-runtime/libcef.dll"]),
        "servo" => Some(vec!["manifold-view-servo.exe", "libEGL.dll", "libGLESv2.dll"]),
        _ => None,
    }
}

fn install(engine: &str, files: &[&str], exe_src: &Path, work: &Path) -> io::Result<()> {
```

with:

```rust
/// The hosts `gate install-check` installs and `gate package` reads the state of, in `ALL`'s order: the downloaded
/// hosts built for this OS (Servo and CEF on Windows). WebKit's is pinned too, but runs only through WSL, which the
/// release runner and a bare `gate package` run do not have.
pub(crate) fn installable() -> impl Iterator<Item = &'static Host> {
    let here = Os::here();
    manifold_hosts::ALL.into_iter().filter(move |h| h.delivery == Delivery::Download && h.built_for(here) == here)
}

/// The files, relative to `HOSTS_DIR`, an installed `engine` leaves on disk (`Host::installed_files`); None for an
/// engine `installable` does not yield (the id is matched exactly, as the app's state lines spell it). Servo's were
/// confirmed on disk from a published release by run `2026-10-23-install-check-v080`.
fn installed_files(engine: &str) -> Option<Vec<String>> {
    installable().find(|h| h.id == engine).map(|h| h.installed_files(Os::here()))
}

fn install(engine: &str, files: &[String], exe_src: &Path, work: &Path) -> io::Result<()> {
```

`install_check`'s call `install(engine, &files, ...)` (at HEAD :125) is unchanged: `&Vec<String>` coerces to `&[String]`, and `install`'s `hosts.join(file)` (at HEAD :170) takes the `&String`.

- [ ] **Step 9: Run the package and usage tests to see them pass**

Run: `cargo test -j 8 -p gate -- package::tests windows_main::tests`
Expected: `test result: ok. 5 passed; 0 failed` (`package::tests`: `engine_state_lines`, `the_manifest_pins_by_engine_and_platform`, `install_check_knows_cef_and_servo_and_their_files`; `windows_main::tests`: `stage_names_are_unique_and_the_usage_lists_the_public_ones`, `install_checks_usage_lists_the_installable_hosts`). The characterization test of Step 6 passes unchanged against the table's lists.

- [ ] **Step 10: `gate package`'s platform, hosts folder, engines and extracted hosts from the table**

All edits in `crates/gate/src/package.rs`.

Replace the leaf's import line Step 8 added:

```rust
use manifold_hosts::{Delivery, Host, Os};
```

with:

```rust
use manifold_hosts::{Delivery, Host, Os, HOSTS_DIR};
```

Replace `newest_release`'s doc and its filter (at HEAD :30-38):

```rust
/// The newest `dist/manifold-v*-windows-x64.exe` under `repo`.
fn newest_release(repo: &Path) -> Option<PathBuf> {
    let mut found: Vec<_> = fs::read_dir(repo.join("dist"))
        .ok()?
        .flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.starts_with("manifold-v") && n.ends_with("-windows-x64.exe")
        })
```

with:

```rust
/// The newest `dist/manifold-v*-<platform>.exe` under `repo`, `<platform>` being this OS's pin platform (`windows-x64`
/// on Windows).
fn newest_release(repo: &Path) -> Option<PathBuf> {
    let suffix = format!("-{}.exe", Os::here().pin_platform());
    let mut found: Vec<_> = fs::read_dir(repo.join("dist"))
        .ok()?
        .flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.starts_with("manifold-v") && n.ends_with(&suffix)
        })
```

Replace `pins` (at HEAD :45-49):

```rust
/// Whether the pin manifest has a `windows-x64` entry with files for `kind` (an engine's store name such as `cef`).
fn pins(manifest: &str, kind: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(manifest)
        .is_ok_and(|m| m[kind]["windows-x64"].as_array().is_some_and(|files| !files.is_empty()))
}
```

with:

```rust
/// Whether the pin manifest has an entry with files for `kind` (an engine's store name such as `cef`) under this OS's
/// pin platform (`windows-x64` on Windows).
fn pins(manifest: &str, kind: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(manifest)
        .is_ok_and(|m| m[kind][Os::here().pin_platform()].as_array().is_some_and(|files| !files.is_empty()))
}
```

In `install`, replace (at HEAD :168):

```rust
    let hosts = profile.join("manifold").join("data").join("engine-hosts");
    for file in files {
```

with:

```rust
    let hosts = profile.join("manifold").join("data").join(HOSTS_DIR);
    for file in files {
```

In `run`, replace (at HEAD :208-210):

```rust
    for line in ["VIEWSTATE cef", "VIEWSTATE servo"] {
        inst.send(line)?;
    }
```

with:

```rust
    for host in installable() {
        inst.send(&format!("VIEWSTATE {}", host.id))?;
    }
```

Replace the state loop's head (at HEAD :219):

```rust
    for engine in ["cef", "servo"] {
```

with:

```rust
    for engine in installable().map(|h| h.id) {
```

Replace the two extracted-host checks around the PDF render check (at HEAD :226-240):

```rust
    let hosts = profile.join("manifold").join("data").join("engine-hosts");
    check(
        "the WebView2 host was extracted under the redirected APPDATA",
        hosts.join("webview2").join("manifold-view-webview2.exe").is_file(),
    );
    let rendered = log.lines().any(|l| l.starts_with("pdf: rendered ") && l.contains("fixture.pdf: 1000x500, 1 page"));
    if !check("the PDF view rendered the fixture (1000x500, 1 page)", rendered) {
        for l in log.lines().filter(|l| l.starts_with("pdf: ")) {
            println!("    log: {l}");
        }
    }
    check(
        "the PDF view was extracted under the redirected APPDATA",
        hosts.join("pdfium").join("manifold-view-pdfium.exe").is_file(),
    );
```

with:

```rust
    let rendered = log.lines().any(|l| l.starts_with("pdf: rendered ") && l.contains("fixture.pdf: 1000x500, 1 page"));
    if !check("the PDF view rendered the fixture (1000x500, 1 page)", rendered) {
        for l in log.lines().filter(|l| l.starts_with("pdf: ")) {
            println!("    log: {l}");
        }
    }
    let hosts = profile.join("manifold").join("data").join(HOSTS_DIR);
    for host in manifold_hosts::ALL.into_iter().filter(|h| h.delivery == Delivery::Embedded) {
        check(
            &format!("{} was extracted under the redirected APPDATA", host.file_name(Os::here())),
            host.installed_in(Os::here(), &hosts).is_file(),
        );
    }
```

`installed_in` puts an `Embedded` host at `<hosts>/<id>/<file>`: `webview2/manifold-view-webview2.exe` and `pdfium/manifold-view-pdfium.exe`, the paths the literals spelled. `pins` is only called with an id `installable` yields, a host built for this OS, so `Os::here().pin_platform()` is that host's own platform.

Run: `cargo test -j 8 -p gate -- package::tests windows_main::tests`
Expected: `test result: ok. 5 passed; 0 failed`, no warning (`the_manifest_pins_by_engine_and_platform` pins `pins` under the Windows platform).

- [ ] **Step 11: The fixtures and probes in `jar.rs` and `probes.rs`**

In `crates/gate/src/jar.rs`, replace (at HEAD :35):

```rust
use serde_json::{json, Value};
```

with:

```rust
use manifold_hosts::{Os, CEF_RUNTIME_DIR, WEBVIEW2};
use serde_json::{json, Value};
```

In `ten`, replace (at HEAD :481):

```rust
        let local_state = self.exe.with_file_name("manifold-view-webview2.exe.WebView2/EBWebView/Local State");
```

with:

```rust
        let webview2 = WEBVIEW2.file_name(Os::here());
        let local_state = self.exe.with_file_name(format!("{webview2}.WebView2/EBWebView/Local State"));
```

In `jar`, replace (at HEAD :744):

```rust
    let cef = exe.with_file_name("cef-runtime").display().to_string().replace('\\', "/");
```

with:

```rust
    let cef = exe.with_file_name(CEF_RUNTIME_DIR).display().to_string().replace('\\', "/");
```

In `crates/gate/src/probes.rs`, replace (at HEAD :22):

```rust
use serde_json::{json, Value};
```

with:

```rust
use manifold_hosts::{Os, WEBVIEW2};
use serde_json::{json, Value};
```

In `failed_spawn`, replace (at HEAD :559-562):

```rust
        fs::write(
            bin.join("manifold-view-webview2.exe"),
            unloadable_copy(&exe.with_file_name("manifold-view-webview2.exe"))?,
        )?;
```

with:

```rust
        let webview2 = WEBVIEW2.file_name(Os::here());
        fs::write(bin.join(&webview2), unloadable_copy(&exe.with_file_name(&webview2))?)?;
```

Run: `cargo build -j 8 -p gate`
Expected: it compiles with no warning.

- [ ] **Step 12: The CEF fixture in `shots.rs`**

All edits in `crates/gate/src/shots.rs`.

In the module doc, replace (at HEAD :37):

```rust
//! manifest with a made-up CEF host (`FAKE_CEF_HOST`) and needs no real download.
```

with:

```rust
//! manifest with a made-up CEF host (`fake_cef_host`) and needs no real download.
```

Replace (at HEAD :58):

```rust
use serde_json::{json, Value};
```

with:

```rust
use manifold_hosts::{Os, CEF, CEF_RUNTIME_DIR, HOSTS_DIR, WEBVIEW2};
use serde_json::{json, Value};
```

Replace the const (at HEAD :73-75), which cannot call `file_name`:

```rust
/// The CEF host the fixture manifest pins: (file, size, SHA-256). Nothing downloads it; the files on disk are sparse.
const FAKE_CEF_HOST: (&str, u64, &str) =
    ("manifold-view-cef.exe", 740_000, "db3e0751979c3cb3068f732ed4f0ef12ade69d183d98ceee120ad08c8d00f74c");
```

with:

```rust
/// The CEF host the fixture manifest pins: (file, size, SHA-256). Nothing downloads it; the files on disk are sparse.
fn fake_cef_host() -> (String, u64, &'static str) {
    (CEF.file_name(Os::here()), 740_000, "db3e0751979c3cb3068f732ed4f0ef12ade69d183d98ceee120ad08c8d00f74c")
}
```

In `engine_app`, replace (at HEAD :1590):

```rust
        self.eng_data = profile.join("manifold").join("data").join("engine-hosts");
```

with:

```rust
        self.eng_data = profile.join("manifold").join("data").join(HOSTS_DIR);
```

In `fake_host`, replace (at HEAD :1625):

```rust
        fs::write(self.eng_data.join("cef.host"), marker)
```

with:

```rust
        fs::write(self.eng_data.join(format!("{}.host", CEF.id)), marker)
```

In `fake_runtime`, replace (at HEAD :1630-1632):

```rust
        let dir = self.eng_data.join("cef-runtime");
        fs::create_dir_all(&dir)?;
        fs::File::create(dir.join("libcef.dll"))?.set_len(423 * 1024 * 1024)?;
```

with:

```rust
        let dir = self.eng_data.join(CEF_RUNTIME_DIR);
        fs::create_dir_all(&dir)?;
        let lib = CEF.runtime_lib(Os::here()).expect("CEF has a runtime");
        fs::File::create(dir.join(lib))?.set_len(423 * 1024 * 1024)?;
```

The `ready` marker on the next line stays a literal (the app's marker, not a layout fact).

In `engine_states`, replace (at HEAD :1641-1646):

```rust
        let (file, size, sha256) = FAKE_CEF_HOST;
        let pins = vec![(file.to_string(), size, sha256.to_string())];
        fs::write(
            self.work.join("host-pins.json"),
            json!({"cef": {"windows-x64": [{"file": file, "size": size, "sha256": sha256}]}}).to_string(),
        )?;
```

with:

```rust
        let (file, size, sha256) = fake_cef_host();
        let pins = vec![(file.clone(), size, sha256.to_string())];
        // The manifest's keys: the host's id, then the platform it is pinned under (the app's parser reads both).
        let (id, platform) = (CEF.id, Os::here().pin_platform());
        fs::write(
            self.work.join("host-pins.json"),
            json!({id: {platform: [{"file": file, "size": size, "sha256": sha256}]}}).to_string(),
        )?;
```

(`json!` takes an identifier key as the expression it names, `id.into()`, and takes values by reference, so `file` is still owned after it.)

Replace (at HEAD :1652-1653):

```rust
        let host = exe.with_file_name("manifold-view-webview2.exe");
        fs::copy(&host, eng.join("manifold-view-webview2.exe"))?;
```

with:

```rust
        let webview2 = WEBVIEW2.file_name(Os::here());
        fs::copy(exe.with_file_name(&webview2), eng.join(&webview2))?;
```

(`host` is not read after :1653.)

Replace (at HEAD :1704):

```rust
            let dir = content_dir(&std::env::temp_dir(), &[("libcef.dll", b""), ("archive.json", archive.as_bytes())])?;
```

with:

```rust
            let lib = CEF.runtime_lib(Os::here()).expect("CEF has a runtime");
            let dir = content_dir(&std::env::temp_dir(), &[(lib, b""), ("archive.json", archive.as_bytes())])?;
```

In the test module, replace (at HEAD :1801-1802):

```rust
        let (file, size, sha) = FAKE_CEF_HOST;
        assert!(file.ends_with(".exe") && size > 0 && sha.len() == 64);
```

with:

```rust
        let (file, size, sha) = fake_cef_host();
        assert!(file == CEF.file_name(Os::here()) && size > 0 && sha.len() == 64);
```

(The test module's `use super::*;` brings in the module's imports, `CEF` and `Os` among them.)

Run: `cargo test -j 8 -p gate shots::`
Expected: `test result: ok.` with 0 failed; `shots::tests::the_fixture_host_and_the_repos_cef_pin_parse` among the passed.

- [ ] **Step 13: The host names in `stress.rs` and `sleep.rs`**

In `crates/gate/src/stress.rs`, replace (at HEAD :15):

```rust
use serde_json::{json, Value};
```

with:

```rust
use manifold_hosts::{Os, WEBVIEW2};
use serde_json::{json, Value};
```

Replace `suspend_engines`' script head and call (at HEAD :38-45), so the script takes the exe name as a parameter, as `sleep.rs`'s does:

```rust
    const SCRIPT: &str = "param([int]$Parent)
Add-Type -Namespace P -Name N -MemberDefinition '[DllImport(\"ntdll.dll\")] public static extern int NtSuspendProcess(IntPtr h);'
Get-CimInstance Win32_Process -Filter \"ParentProcessId=$Parent AND Name='manifold-view-webview2.exe'\" | ForEach-Object {
    $null = [P.N]::NtSuspendProcess((Get-Process -Id $_.ProcessId).Handle)
    $_.ProcessId
}
";
    powershell(work, "suspend", SCRIPT, &["-Parent", &parent.to_string()])
```

with:

```rust
    const SCRIPT: &str = "param([int]$Parent, [string]$Name)
Add-Type -Namespace P -Name N -MemberDefinition '[DllImport(\"ntdll.dll\")] public static extern int NtSuspendProcess(IntPtr h);'
Get-CimInstance Win32_Process -Filter \"ParentProcessId=$Parent AND Name='$Name'\" | ForEach-Object {
    $null = [P.N]::NtSuspendProcess((Get-Process -Id $_.ProcessId).Handle)
    $_.ProcessId
}
";
    let name = WEBVIEW2.file_name(Os::here());
    powershell(work, "suspend", SCRIPT, &["-Parent", &parent.to_string(), "-Name", &name])
```

(`Add-Type`'s own `-Name N` is that cmdlet's parameter, not the script's `$Name`.)

In `crates/gate/src/sleep.rs`, add the leaf's group between the std and `crate::` imports (at HEAD :24-26):

```rust
use std::process::{Command, ExitCode, Stdio};

use crate::check::{check, check_eq, Section};
```

becomes:

```rust
use std::process::{Command, ExitCode, Stdio};

use manifold_hosts::Os;

use crate::check::{check, check_eq, Section};
```

Replace `kill_hosts`' doc and name (at HEAD :82-87):

```rust
/// Kills the engine hosts that are children of `parent` (only this instance's own: other manifolds may be running).
fn kill_hosts(work: &Path, parent: u32, view: &str) -> io::Result<()> {
    const SCRIPT: &str = "param([int]$Parent, [string]$Name)
Get-CimInstance Win32_Process -Filter \"ParentProcessId=$Parent AND Name='$Name'\" | ForEach-Object { $_.ProcessId }
";
    let name = format!("manifold-view-{view}.exe");
```

with:

```rust
/// Kills the engine hosts that are children of `parent` (only this instance's own: other manifolds may be running).
/// WebKit's host is no Windows process (manifold's child there is the `wsl.exe` relay), so for WebKit this kills
/// nothing. A `view` that names no host (`VIEWS` is free text) is an `InvalidInput` error.
fn kill_hosts(work: &Path, parent: u32, view: &str) -> io::Result<()> {
    const SCRIPT: &str = "param([int]$Parent, [string]$Name)
Get-CimInstance Win32_Process -Filter \"ParentProcessId=$Parent AND Name='$Name'\" | ForEach-Object { $_.ProcessId }
";
    let host = manifold_hosts::by_id(view)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, format!("no engine host is named {view:?}")))?;
    let name = host.file_name(Os::here());
```

The loop after it (`for pid in powershell(work, "hosts", SCRIPT, &["-Parent", &parent.to_string(), "-Name", &name])? {`) is unchanged. Its one caller (`:220`, the `crash` case) already returns `io::Result` through `?`, so the error ends that case with its message, as any other probe error does.

Run: `cargo build -j 8 -p gate`
Expected: it compiles with no warning.

- [ ] **Step 14: Key `gate all`'s `SITE_HOST` on the host's OS**

In `crates/gate/src/all.rs`, replace (at HEAD :36):

```rust
use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
```

with:

```rust
use manifold_hosts::{by_id, Os};
use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
```

Replace (at HEAD :318-326):

```rust
        if step.engine == Some("webkit") {
            // The page server's address as WSL sees it; `gate()` removed it for the other steps.
            let Ok(ip) = std::env::var("SITE_HOST") else {
                append(&log, &format!("GATE {} not run: WITH_ENGINES has webkit but SITE_HOST is not set", step.name));
                g.failed = true;
                continue;
            };
            c.env("SITE_HOST", ip);
        }
```

with:

```rust
        // A host built for another OS than the gate's runs in a VM (WebKit's, in WSL), which reaches the page server
        // at the address `SITE_HOST` names; `gate()` removed it for the other steps.
        let vm_engine = step.engine.filter(|e| by_id(e).expect("known engine").built_for(Os::here()) != Os::here());
        if let Some(engine) = vm_engine {
            let Ok(ip) = std::env::var("SITE_HOST") else {
                append(
                    &log,
                    &format!("GATE {} not run: WITH_ENGINES has {engine} but SITE_HOST is not set", step.name),
                );
                g.failed = true;
                continue;
            };
            c.env("SITE_HOST", ip);
        }
```

The `expect` cannot fire: `every_step_runs_a_real_stage_under_its_own_log_name` (at HEAD :354) limits a step's engine to `servo` and `webkit`, both rows. For those two the predicate is false for Servo (built for whichever OS runs it) and true for WebKit (built for Linux), so the same steps get `SITE_HOST` as before.

Run: `cargo build -j 8 -p gate`
Expected: it compiles with no warning.

- [ ] **Step 15: Check that no layout literal is left, then build, run and lint**

From bash:

```bash
grep -cE 'manifold-view-[a-z0-9]+\.exe|"(cef-runtime|engine-hosts|libcef\.dll|cef\.host)"|"-?windows-x64|manifold-view-\{' crates/gate/src/fresh.rs crates/gate/src/package.rs crates/gate/src/main.rs crates/gate/src/jar.rs crates/gate/src/probes.rs crates/gate/src/shots.rs crates/gate/src/stress.rs crates/gate/src/sleep.rs crates/gate/src/all.rs
grep -nE 'FAKE_CEF_HOST|== Some\("webkit"\)' crates/gate/src/shots.rs crates/gate/src/all.rs
```

Expected from the first: `crates/gate/src/fresh.rs:8`, `crates/gate/src/package.rs:3`, and `:0` for the other seven files. At HEAD 10444ae5 it counts 36 hits over seven files (fresh 12, package 9, jar 2, probes 2, shots 9, stress 1, sleep 1). The 11 left are tests that pin today's values on purpose: in `fresh.rs`, the eight literals of `fresh_exes_pass_and_each_stale_one_is_named` (`bin.touch("manifold-view-pdfium.exe", 4)` and the like); in `package.rs`, the manifest in `the_manifest_pins_by_engine_and_platform` and the two lists of `install_check_knows_cef_and_servo_and_their_files`. Any other hit is a site this task missed: replace it the same way. The second command prints nothing.

Then, in PowerShell, re-run the Step 1 process check (`Get-Process cargo, gate ...`) and wait for any other session's to exit. From bash:

```bash
cargo build -j 8 -p gate
target/debug/gate.exe install-check; echo "exit $?"
target/debug/gate.exe install-check webkit nothing.exe; echo "exit $?"
```

Expected, the same text as at HEAD, now built from the table (neither reads a profile, a file or the network):

```
usage: gate install-check cef|servo EXE
exit 2
install-check knows cef and servo, not "webkit"
exit 2
```

Run: `cargo test -j 8 -p gate -- --test-threads 8`
Expected: `test result: ok.` with 0 failed; among the passed, the three `fresh::tests`, the three `package::tests` and the two `windows_main::tests`.

Run the clippy line (Global Constraints; with Task 1's `-p manifold-hosts`): `cargo clippy -j 8 -p manifold -p manifold-hosts -p manifold-view-protocol -p manifold-pages -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings`
Expected: no warning, exit 0.

- [ ] **Step 16: Commit**

```bash
cargo fmt --all
git diff --stat
git add crates/gate/src/fresh.rs crates/gate/src/package.rs crates/gate/src/main.rs crates/gate/src/jar.rs crates/gate/src/probes.rs crates/gate/src/shots.rs crates/gate/src/stress.rs crates/gate/src/sleep.rs crates/gate/src/all.rs
git commit -F - <<'EOF'
gate: the other readers of a host's layout read manifold-hosts

fresh checks every host with a runtime for a copy in its runtime folder
older than the built exe, and names the dep-info hosts (WebView2, the
PDF view) by row, the build hint built from the same list. install-check
and gate package take the engines they install and check from one
filter, installable(): the downloaded hosts built for this OS. A test
holds install-check's usage literal to it. installed_files is the
table's, and the platform, the hosts folder and the extracted-host
checks come from the table too. The fixtures and probes (jar, probes,
shots, stress) take file names, the runtime folder and its library from
the rows; FAKE_CEF_HOST becomes fake_cef_host(). kill_hosts takes the
file name from by_id (an InvalidInput error for a view that names no
host), and gate all passes SITE_HOST to a step whose host
is built for another OS than the gate's, not to the one named webkit.
On Windows every file name is the one spelled before, except the WebKit
name kill_hosts looks for (manifold-view-webkit.exe named no host).

<attribution>
EOF
git log -1 --format=%B | grep -v '^$' | tail -n 1
```

Expected: `git diff --stat` lists exactly the nine files above (`cargo fmt --all` may have re-wrapped a line of this task's; nothing else changes). The pre-commit hook passes (fmt and clippy). The last line printed is your session's `Co-Authored-By:` trailer, not `<attribution>`.

### Task 5: release-pins, release-embedded and embedded_record

Spec step 5 ("The gate", `gate release-pins vX.Y.Z DIST FOREIGN` and `gate release-embedded vX.Y.Z DIST`; Tests, Gate, the "Step 5" bullet; H5, H6; Risks R2). Two new stages in a new `cfg(windows)` module, `crates/gate/src/release_hosts.rs`, write a release's host files from the table's `Download` and `Embedded` rows. Each stage is a thin wrapper over a core that takes the repo root and returns the lines the stage prints, so the tests run the cores on temp trees and assert on returned values. Nothing calls the stages yet: `release.yml` moves to them in Task 6, in the same commit as the renamed records' upload and reader (H6), so this commit changes no release behaviour.

**Files:**
- Create: `crates/gate/src/release_hosts.rs`
- Modify: `crates/gate/src/main.rs`: the module list (HEAD :46-49) and one `STAGES` row (HEAD :172). Task 4 only appends a test at the end of the file, so both sites are as at HEAD.
- Unchanged, checked: `crates/gate/Cargo.toml`. `serde_json` (with `preserve_order`, so a `Map` keeps insertion order; HEAD :11) and `sha2` (HEAD :13, used through `cef_pin::hash_file`) are already gate dependencies, and Task 3 added `manifold-hosts`. No new dependency, so `Cargo.lock` does not change.
- Unchanged: `.github/workflows/release.yml` and `crates/gate/src/release_verify.rs` (both Task 6).

**Interfaces:**
- Consumes (Task 1, `manifold_hosts`):
  - `Os` (`Copy`, `PartialEq`), `Os::here() -> Os`, `Os::pin_platform(self) -> &'static str` (`"windows-x64"` on Windows);
  - `Delivery::{Download, Embedded}` (`PartialEq`); `Host` fields `id: &'static str`, `delivery: Delivery`; `Sidecar` fields `file: &'static str`, `from: BuildOutput`; `BuildOutput` fields `krate: &'static str`, `tail: &'static str`;
  - `Host::built_for(&self, Os) -> Os`, `Host::file_name(&self, Os) -> String`, `Host::pin_platform(&self, Os) -> &'static str`, `Host::sidecars_for(&self, Os) -> impl Iterator<Item = &Sidecar>`, `Host::pinned_files(&self, Os) -> Vec<String>`, `Host::embed_var(&self) -> String`;
  - `ALL: [&Host; 5]` in the order `SERVO`, `CEF`, `WEBKIT`, `WEBVIEW2`, `PDFIUM`; the statics `WEBVIEW2` and `PDFIUM`;
  - the row values the tests' literal fixture spells: `servo` (`manifold-view-servo`, `os` None, `Download`, Windows sidecars `libEGL.dll` and `libGLESv2.dll`, each `from: BuildOutput { krate: "mozangle", tail: <the file> }`), `cef` (`manifold-view-cef`, `os` None, `Download`, no sidecar), `webkit` (`manifold-view-webkitgtk`, `os` `Some(Linux)`, `Download`, no sidecar), `webview2` (`manifold-view-webview2`, `os` `Some(Windows)`, `Embedded`), `pdfium` (`manifold-view-pdfium`, `os` None, `Embedded`); `embed_var()` is `MANIFOLD_<ID>_HOST` (`MANIFOLD_WEBVIEW2_HOST`, `MANIFOLD_PDFIUM_HOST`).
- Consumes (Task 3): `crate::sync_deps::find_first(roots: &[PathBuf], from: &BuildOutput) -> Option<PathBuf>` (the newest match in the first root that has one, both cargo layouts per root; `pub(crate)`), and the gate's dependency on `manifold-hosts`.
- Consumes (HEAD): `crate::cef_pin::repo() -> PathBuf` (`CARGO_MANIFEST_DIR/../..`, `cef_pin.rs:20`, the root the spec's wrapper passes) and `crate::cef_pin::hash_file(&Path) -> Result<(String, String, u64), String>` (lowercase SHA-256 hex, SHA-1 hex, byte count; `cef_pin.rs:32`).
- Produces (in `crates/gate/src/release_hosts.rs`, compiled only on Windows):

```rust
/// `<stem>.embedded-<id>.sha256`; `stem` is `manifold-vX.Y.Z-<platform>`.
pub(crate) fn embedded_record(stem: &str, host: &Host) -> String;
/// Returns `["MANIFOLD_HOST_PINS=<absolute manifest path>"]`.
pub(crate) fn release_pins_in(root: &Path, tag: &str, dist: &Path, foreign: &Path) -> Result<Vec<String>, String>;
/// Returns one `<embed_var>=<absolute copy>` line per `Embedded` row, in `ALL`'s order.
pub(crate) fn release_embedded_in(root: &Path, tag: &str, dist: &Path) -> Result<Vec<String>, String>;
pub fn release_pins(args: &[String]) -> ExitCode;     // STAGES "release-pins", usage "vX.Y.Z DIST FOREIGN"
pub fn release_embedded(args: &[String]) -> ExitCode; // STAGES "release-embedded", usage "vX.Y.Z DIST"
```

- Produces (the behaviour Task 6's YAML relies on; DIST and FOREIGN are joined to the repo root, an absolute one stays as it is):
  - `gate release-pins vX.Y.Z DIST FOREIGN`: creates DIST if missing; copies each `Download` row's `pinned_files` to `DIST/<file>-<platform>` (a host built for this OS from `target/release/<file>`, its sidecars from `find_first([target/release/build], ..)`; a host built for another OS from `FOREIGN/<file>`); writes `DIST/manifold-vX.Y.Z-<platform>.host-pins.json` (pretty JSON, trailing newline, keys in `ALL`'s order: `servo`, `cef`, `webkit`; each platform a list of `{"file", "size", "sha256"}`); stdout is exactly one line, `MANIFOLD_HOST_PINS=<absolute path>`, and stderr carries the manifest's text; exit 0. A missing file: nothing on stdout, stderr `release-pins: <id>/<platform> <file>: not found (looked for <where>)`, exit 1.
  - `gate release-embedded vX.Y.Z DIST`: creates `DIST/embedded`; copies each `Embedded` row's `target/release/<file>` to `DIST/embedded/<file>` and writes `DIST/<embedded_record(stem, host)>` holding `<sha256>  <file>` and a newline; stdout `MANIFOLD_WEBVIEW2_HOST=<absolute copy>` then `MANIFOLD_PDFIUM_HOST=<absolute copy>`; exit 0. A missing file: stderr `release-embedded: <id> <file>: not found (looked for <path>)`, exit 1.
  - A wrong argument count prints the usage line on stderr and exits 2. No printed path carries a `\\?\` prefix (`std::path::absolute`, not `canonicalize`).

- [ ] **Step 1: Start**

- `git status --short -- crates .github README.md` prints nothing: Task 4 is committed.
- `grep -n "manifold-hosts\|serde_json\|sha2" crates/gate/Cargo.toml` prints three lines: Task 3's `manifold-hosts` dependency, `serde_json = { workspace = true, features = ["preserve_order"] }` and `sha2.workspace = true`. If the `manifold-hosts` line is missing, stop: Task 3 is not in.
- `grep -n "pub(crate) fn find_first" crates/gate/src/sync_deps.rs` prints `pub(crate) fn find_first(roots: &[PathBuf], from: &BuildOutput) -> Option<PathBuf> {`. If the signature differs, stop and report: Step 5 calls it with exactly these types.
- `grep -c "pub fn built_for\|pub fn file_name\|pub fn sidecars_for\|pub fn pinned_files\|pub fn embed_var\|pub fn here\|pub fn pin_platform" crates/manifold-hosts/src/lib.rs` prints `7` or more (`pin_platform` is both `Os`'s and `Host`'s). If it prints less, find which is missing and stop: Task 1 produced a different interface.
- `git diff --stat 10444ae5 HEAD -- .github/workflows/release.yml crates/gate/src/release_verify.rs crates/gate/src/cef_pin.rs` prints nothing.
- `grep -n "^mod release_verify;\|\"release-verify\"" crates/gate/src/main.rs` prints line 49 (`mod release_verify;`) and line 172 (the `Stage::args("release-verify", ...)` row). Other numbers mean an earlier task moved them: find the sites by their text below.
- In PowerShell, `Get-Process cargo -ErrorAction SilentlyContinue` prints nothing. A `cargo` listed is another session's on this target dir: wait for it to exit.

- [ ] **Step 2: Write the failing tests**

Create `crates/gate/src/release_hosts.rs` with the module doc, the imports the code of Step 5 uses, and the tests (spec Tests, Gate, "Step 5"). The fixture spells every host file as a literal path, so the tests are a witness of the table's rows that does not come from the table. Each file holds its own name, so a copy can be told from its source and from a decoy.

```rust
//! `gate release-pins vX.Y.Z DIST FOREIGN` and `gate release-embedded vX.Y.Z DIST`: a release's host files, from the
//! host table (`crates/manifold-hosts`). The release's own platform is `Os::here()`, since the gate runs on the release
//! runner.
//! - `release-pins` copies every downloaded host (`Delivery::Download`) and its sidecars into DIST as `FILE-PLATFORM`
//!   and writes their pin manifest, `DIST/manifold-vX.Y.Z-<platform>.host-pins.json`, which manifold.exe is built with.
//!   A host built for this OS comes from `target/release/`, its sidecars from the newest build output under
//!   `target/release/build/` (`sync_deps::find_first`); a host built for another OS comes from FOREIGN (the
//!   `webkit-host` job's artifact).
//! - `release-embedded` copies every embedded host (`Delivery::Embedded`) into `DIST/embedded/` and writes its hash
//!   record, `DIST/<stem>.embedded-<id>.sha256` (`embedded_record`), which `release-verify` checks.
//!
//! Both print `NAME=path` lines on stdout, the build variables the job's later `cargo build -p manifold` reads, so the
//! release job appends them to `$GITHUB_ENV`. DIST and FOREIGN are relative to the repo root (the runner's working
//! directory); the printed paths are absolute. Each stage is a thin wrapper over a core that takes the root and returns
//! the lines, so the tests run the cores on temp trees.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use manifold_hosts::{Delivery, Host, Os};
use serde_json::{json, Map, Value};

use crate::cef_pin::{hash_file, repo};
use crate::sync_deps::find_first;

#[cfg(test)]
mod tests {
    use super::*;

    const TAG: &str = "vX.Y.Z";

    /// A repo root with every host's release build under `target/release/` (Servo's ANGLE DLLs in mozangle's
    /// `build/mozangle/<hash>/out/`) and the Linux job's artifact under `webkit-host/`, each file holding its own name.
    /// No `dist/` yet.
    fn release_tree(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("gate-release-{name}-{}", std::process::id()));
        let root = std::path::absolute(root).unwrap();
        let _ = fs::remove_dir_all(&root);
        for file in [
            "target/release/manifold-view-servo.exe",
            "target/release/build/mozangle/aaa/out/libEGL.dll",
            "target/release/build/mozangle/aaa/out/libGLESv2.dll",
            "target/release/manifold-view-cef.exe",
            "target/release/manifold-view-webview2.exe",
            "target/release/manifold-view-pdfium.exe",
            "webkit-host/manifold-view-webkitgtk",
        ] {
            let path = root.join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, file.rsplit('/').next().unwrap()).unwrap();
        }
        root
    }

    #[test]
    fn release_pins_copies_each_downloaded_host_and_writes_its_manifest() {
        let root = release_tree("pins");
        let dist = root.join("dist");
        assert!(!dist.exists());
        let lines = release_pins_in(&root, TAG, &dist, &root.join("webkit-host")).unwrap();
        let manifest_path = dist.join("manifold-vX.Y.Z-windows-x64.host-pins.json");
        assert_eq!(lines, [format!("MANIFOLD_HOST_PINS={}", manifest_path.display())]);
        assert!(!lines[0].contains(r"\\?\"));
        let manifest: Value = serde_json::from_str(&fs::read_to_string(manifest_path).unwrap()).unwrap();
        let ids: Vec<&str> = manifest.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(ids, ["servo", "cef", "webkit"]);
        let pinned = [
            ("servo", "windows-x64", "manifold-view-servo.exe"),
            ("servo", "windows-x64", "libEGL.dll"),
            ("servo", "windows-x64", "libGLESv2.dll"),
            ("cef", "windows-x64", "manifold-view-cef.exe"),
            ("webkit", "linux-x64", "manifold-view-webkitgtk"),
        ];
        for (id, platform, file) in pinned {
            let copy = dist.join(format!("{file}-{platform}"));
            assert_eq!(fs::read_to_string(&copy).unwrap(), file, "{}", copy.display());
            let (sha256, _, size) = hash_file(&copy).unwrap();
            assert_eq!(size, file.len() as u64);
            let entry = manifest[id][platform].as_array().unwrap().iter().find(|e| e["file"] == file);
            let entry = entry.unwrap_or_else(|| panic!("{id}/{platform} lists no {file}"));
            assert_eq!(entry["size"], json!(size), "{id}/{platform} {file}");
            assert_eq!(entry["sha256"], json!(sha256), "{id}/{platform} {file}");
        }
        let platforms = manifest.as_object().unwrap().values().flat_map(|p| p.as_object().unwrap().values());
        assert_eq!(platforms.map(|files| files.as_array().unwrap().len()).sum::<usize>(), pinned.len());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_host_file_is_an_error_naming_it_and_where_it_was_looked_for() {
        let root = release_tree("missing");
        let (dist, foreign) = (root.join("dist"), root.join("webkit-host"));
        let release = root.join("target").join("release");
        fs::remove_file(release.join("build/mozangle/aaa/out/libGLESv2.dll")).unwrap();
        let looked = format!("mozangle's out/libGLESv2.dll under {}", release.join("build").display());
        let want = format!("servo/windows-x64 libGLESv2.dll: not found (looked for {looked})");
        assert_eq!(release_pins_in(&root, TAG, &dist, &foreign), Err(want));
        fs::remove_file(release.join("manifold-view-servo.exe")).unwrap();
        let looked = release.join("manifold-view-servo.exe").display().to_string();
        let want = format!("servo/windows-x64 manifold-view-servo.exe: not found (looked for {looked})");
        assert_eq!(release_pins_in(&root, TAG, &dist, &foreign), Err(want));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_host_built_for_another_os_comes_from_foreign() {
        let root = release_tree("foreign");
        let (dist, foreign) = (root.join("dist"), root.join("webkit-host"));
        // A file of the same name in this OS's target/release is not the Linux job's build.
        fs::write(root.join("target").join("release").join("manifold-view-webkitgtk"), "not this one").unwrap();
        release_pins_in(&root, TAG, &dist, &foreign).unwrap();
        let copied = fs::read_to_string(dist.join("manifold-view-webkitgtk-linux-x64")).unwrap();
        assert_eq!(copied, "manifold-view-webkitgtk");
        let elf = foreign.join("manifold-view-webkitgtk");
        fs::remove_file(&elf).unwrap();
        let want = format!("webkit/linux-x64 manifold-view-webkitgtk: not found (looked for {})", elf.display());
        assert_eq!(release_pins_in(&root, TAG, &dist, &foreign), Err(want));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn release_embedded_copies_each_embedded_host_and_records_its_hash() {
        let root = release_tree("embedded");
        let dist = root.join("dist");
        let lines = release_embedded_in(&root, TAG, &dist).unwrap();
        let webview2 = dist.join("embedded").join("manifold-view-webview2.exe");
        let pdfium = dist.join("embedded").join("manifold-view-pdfium.exe");
        let want = [
            format!("MANIFOLD_WEBVIEW2_HOST={}", webview2.display()),
            format!("MANIFOLD_PDFIUM_HOST={}", pdfium.display()),
        ];
        assert_eq!(lines, want);
        assert!(lines.iter().all(|line| !line.contains(r"\\?\")));
        let records = [
            (&webview2, "manifold-vX.Y.Z-windows-x64.embedded-webview2.sha256"),
            (&pdfium, "manifold-vX.Y.Z-windows-x64.embedded-pdfium.sha256"),
        ];
        for (copy, record) in records {
            let file = copy.file_name().unwrap().to_str().unwrap();
            assert_eq!(fs::read_to_string(copy).unwrap(), file);
            let (sha256, ..) = hash_file(copy).unwrap();
            assert_eq!(fs::read_to_string(dist.join(record)).unwrap(), format!("{sha256}  {file}\n"));
        }
        let source = root.join("target").join("release").join("manifold-view-pdfium.exe");
        fs::remove_file(&source).unwrap();
        let want = format!("pdfium manifold-view-pdfium.exe: not found (looked for {})", source.display());
        assert_eq!(release_embedded_in(&root, TAG, &dist), Err(want));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn embedded_record_names_the_files_the_release_uploads() {
        // Literal: release-embedded and release-verify both call embedded_record, so neither can catch a wrong name.
        let stem = "manifold-vX.Y.Z-windows-x64";
        let webview2 = embedded_record(stem, &manifold_hosts::WEBVIEW2);
        assert_eq!(webview2, "manifold-vX.Y.Z-windows-x64.embedded-webview2.sha256");
        let pdfium = embedded_record(stem, &manifold_hosts::PDFIUM);
        assert_eq!(pdfium, "manifold-vX.Y.Z-windows-x64.embedded-pdfium.sha256");
    }
}
```

What each test proves (the spec's Step 5 list): files copied and named `<file>-<platform>`, the manifest's keys, sizes and hashes, and nothing else listed (the first); a missing file an error naming it and where it was looked for, both for a sidecar and for an exe (the second); a foreign host taken from FOREIGN and not from this OS's `target/release`, and its absence named (the third); the records in the `<hash>  <file>` format and the returned `NAME=VALUE` lines with absolute paths and no `\\?\` (the first and fourth); `embedded_record`'s literal names (the fifth).

- [ ] **Step 3: Register the module**

In `crates/gate/src/main.rs` (at HEAD :46-49), replace:

```rust
#[cfg(windows)]
mod release_check;
#[cfg(windows)]
mod release_verify;
```

with:

```rust
#[cfg(windows)]
mod release_check;
#[cfg(windows)]
mod release_hosts;
#[cfg(windows)]
mod release_verify;
```

- [ ] **Step 4: Run the tests to see them fail**

Run: `cargo test -j 8 -p gate release_hosts`
Expected: the test build fails with `error[E0425]: cannot find function` for `release_pins_in`, `release_embedded_in` and `embedded_record`, plus unused-import warnings; no test runs. An `E0432`/`E0433` naming `find_first` or a `manifold_hosts` item instead means Task 3 or Task 1 produced a different interface: stop and report it.

- [ ] **Step 5: Write the cores and the stages**

In `crates/gate/src/release_hosts.rs`, replace:

```rust
use crate::sync_deps::find_first;

#[cfg(test)]
```

with:

```rust
use crate::sync_deps::find_first;

/// The hash record of an embedded host in a release's DIST, `<stem>.embedded-<id>.sha256`, where `stem` is
/// `manifold-vX.Y.Z-<platform>`. `release-embedded` writes it, the draft release uploads it, `release-verify` reads it.
pub(crate) fn embedded_record(stem: &str, host: &Host) -> String {
    format!("{stem}.embedded-{}.sha256", host.id)
}

/// `path` made absolute without `canonicalize`'s `\\?\` prefix, like the `Resolve-Path` values the job used before.
fn absolute(path: &Path) -> Result<PathBuf, String> {
    std::path::absolute(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Each file a release pins for `host`, in `pinned_files`' order, with where it comes from: the file, or where it was
/// looked for. A host built for another OS comes whole from `foreign`; one built for `running` from `release`, its
/// sidecars from the newest build output under `release/build`.
fn sources(host: &Host, running: Os, release: &Path, foreign: &Path) -> Vec<(String, Result<PathBuf, String>)> {
    let at = |path: PathBuf| if path.is_file() { Ok(path) } else { Err(path.display().to_string()) };
    if host.built_for(running) != running {
        return host.pinned_files(running).into_iter().map(|file| (file.clone(), at(foreign.join(file)))).collect();
    }
    let file = host.file_name(running);
    let exe = release.join(&file);
    let mut out = vec![(file, at(exe))];
    let builds = [release.join("build")];
    for sidecar in host.sidecars_for(running) {
        let found = find_first(&builds, &sidecar.from)
            .ok_or_else(|| format!("{}'s out/{} under {}", sidecar.from.krate, sidecar.from.tail, builds[0].display()));
        out.push((sidecar.file.to_owned(), found));
    }
    out
}

/// `gate release-pins`' core: copies each downloaded host's files into `dist` as `<file>-<platform>` and writes their
/// pin manifest there, `manifold-<tag>-<platform>.host-pins.json`; returns the one line `MANIFOLD_HOST_PINS=<path>`.
/// `dist` and `foreign` are joined to `root` (an absolute one stays as it is). The first file not found is the error.
pub(crate) fn release_pins_in(root: &Path, tag: &str, dist: &Path, foreign: &Path) -> Result<Vec<String>, String> {
    let running = Os::here();
    let root = absolute(root)?;
    let (dist, foreign, release) = (root.join(dist), root.join(foreign), root.join("target").join("release"));
    fs::create_dir_all(&dist).map_err(|e| format!("{}: {e}", dist.display()))?;
    let mut manifest = Map::new();
    for host in manifold_hosts::ALL.iter().filter(|host| host.delivery == Delivery::Download) {
        let platform = host.pin_platform(running);
        let mut files = Vec::new();
        for (file, source) in sources(host, running, &release, &foreign) {
            let source =
                source.map_err(|looked| format!("{}/{platform} {file}: not found (looked for {looked})", host.id))?;
            let copy = dist.join(format!("{file}-{platform}"));
            fs::copy(&source, &copy).map_err(|e| format!("{} -> {}: {e}", source.display(), copy.display()))?;
            let (sha256, _, size) = hash_file(&copy)?;
            files.push(json!({ "file": file, "size": size, "sha256": sha256 }));
        }
        let mut platforms = Map::new();
        platforms.insert(platform.to_owned(), Value::Array(files));
        manifest.insert(host.id.to_owned(), Value::Object(platforms));
    }
    let path = dist.join(format!("manifold-{tag}-{}.host-pins.json", running.pin_platform()));
    let text = serde_json::to_string_pretty(&Value::Object(manifest)).map_err(|e| e.to_string())?;
    fs::write(&path, text + "\n").map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(vec![format!("MANIFOLD_HOST_PINS={}", absolute(&path)?.display())])
}

/// `gate release-embedded`'s core: copies each embedded host from `target/release` into `dist/embedded/` and writes its
/// hash record in `dist` (`embedded_record`; `<sha256>  <file>`); returns one `<embed_var>=<copy>` line per host.
pub(crate) fn release_embedded_in(root: &Path, tag: &str, dist: &Path) -> Result<Vec<String>, String> {
    let running = Os::here();
    let root = absolute(root)?;
    let (dist, release) = (root.join(dist), root.join("target").join("release"));
    let embedded = dist.join("embedded");
    fs::create_dir_all(&embedded).map_err(|e| format!("{}: {e}", embedded.display()))?;
    let stem = format!("manifold-{tag}-{}", running.pin_platform());
    let mut lines = Vec::new();
    for host in manifold_hosts::ALL.iter().filter(|host| host.delivery == Delivery::Embedded) {
        let file = host.file_name(running);
        let source = release.join(&file);
        if !source.is_file() {
            return Err(format!("{} {file}: not found (looked for {})", host.id, source.display()));
        }
        let copy = embedded.join(&file);
        fs::copy(&source, &copy).map_err(|e| format!("{} -> {}: {e}", source.display(), copy.display()))?;
        let (sha256, ..) = hash_file(&copy)?;
        let record = dist.join(embedded_record(&stem, host));
        fs::write(&record, format!("{sha256}  {file}\n")).map_err(|e| format!("{}: {e}", record.display()))?;
        lines.push(format!("{}={}", host.embed_var(), absolute(&copy)?.display()));
    }
    Ok(lines)
}

/// Prints a core's lines on stdout (the release job appends them to `$GITHUB_ENV`), or its error on stderr.
fn print_lines(stage: &str, result: Result<Vec<String>, String>) -> ExitCode {
    match result {
        Ok(lines) => {
            for line in lines {
                println!("{line}");
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{stage}: {e}");
            ExitCode::from(1)
        }
    }
}

/// `gate release-pins vX.Y.Z DIST FOREIGN`: prints `MANIFOLD_HOST_PINS=<manifest>`, and the manifest itself on stderr
/// for the job's log.
pub fn release_pins(args: &[String]) -> ExitCode {
    let [tag, dist, foreign] = args else {
        eprintln!("usage: gate release-pins vX.Y.Z DIST FOREIGN");
        return ExitCode::from(2);
    };
    let result = release_pins_in(&repo(), tag, Path::new(dist), Path::new(foreign));
    for path in result.iter().flatten().filter_map(|line| line.strip_prefix("MANIFOLD_HOST_PINS=")) {
        match fs::read_to_string(path) {
            Ok(text) => eprint!("{text}"),
            Err(e) => eprintln!("{path}: {e}"),
        }
    }
    print_lines("release-pins", result)
}

/// `gate release-embedded vX.Y.Z DIST`: prints `<embed_var>=<copy>` per embedded host.
pub fn release_embedded(args: &[String]) -> ExitCode {
    let [tag, dist] = args else {
        eprintln!("usage: gate release-embedded vX.Y.Z DIST");
        return ExitCode::from(2);
    };
    print_lines("release-embedded", release_embedded_in(&repo(), tag, Path::new(dist)))
}

#[cfg(test)]
```

Notes for the reader of the code:
- `root` is made absolute once, so every path the cores return or name in an error is absolute; `std::path::absolute` on Windows resolves the `..` in `repo()`'s `crates/gate/../..` and adds no `\\?\`. A test root (already absolute) is unchanged by it, which is why the tests compare against `root.join(..)`.
- `sources` lists the exe first, then `sidecars_for(running)`: the order `pinned_files` defines, so the manifest lists files in the table's order.
- The manifest's text reaches the log through the wrapper (stderr), as `release.yml`'s `Get-Content` put it there before; the core returns no text, so no test depends on captured output.

- [ ] **Step 6: Add the two stages to `STAGES`**

In `crates/gate/src/main.rs` (at HEAD :172), replace:

```rust
        Stage::args("release-verify", "vX.Y.Z [DIST]", crate::release_verify::release_verify),
```

with:

```rust
        Stage::args("release-pins", "vX.Y.Z DIST FOREIGN", crate::release_hosts::release_pins),
        Stage::args("release-embedded", "vX.Y.Z DIST", crate::release_hosts::release_embedded),
        Stage::args("release-verify", "vX.Y.Z [DIST]", crate::release_verify::release_verify),
```

The rows sit in the order the release job runs them. `windows_main::tests::stage_names_are_unique_and_the_usage_lists_the_public_ones` (main.rs, HEAD :349) covers both: unique names, and `gate release-pins` and `gate release-embedded` in the usage text.

- [ ] **Step 7: Run the tests to see them pass, then clippy**

Run: `cargo test -j 8 -p gate release_hosts`
Expected: `test result: ok. 5 passed`, the five `release_hosts::tests::` tests of Step 2; no warning.

Run: `cargo test -j 8 -p gate -- --test-threads 8`
Expected: `test result: ok.`, including `windows_main::tests::stage_names_are_unique_and_the_usage_lists_the_public_ones`.

Run the clippy line (CLAUDE.md, with Task 1's `-p manifold-hosts`): `cargo clippy -j 8 -p manifold -p manifold-hosts -p manifold-view-protocol -p manifold-pages -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings`
Expected: no warning, exit 0.

- [ ] **Step 8: Run `release-embedded` on this machine's release build (spec R2)**

The one piece of the new release path that runs locally: this machine's `target/release` holds `manifold-view-webview2.exe` and `manifold-view-pdfium.exe` (checked 2026-10-29; Servo and CEF release builds are not there, so `release-pins` is covered by the unit tests only). `<scratchpad>` below is your session's scratchpad directory; the folder is a throwaway. In PowerShell, from the repo root:

```powershell
cargo build -j 8 -p gate
$dist = "<scratchpad>\release-embedded-check"
& target/debug/gate.exe release-embedded v0.0.0-local $dist; "exit $LASTEXITCODE"
Get-ChildItem -Recurse -File $dist | ForEach-Object Name
Get-Content "$dist\manifold-v0.0.0-local-windows-x64.embedded-webview2.sha256"
(Get-FileHash target\release\manifold-view-webview2.exe -Algorithm SHA256).Hash.ToLower()
Remove-Item -Recurse -Force $dist
```

Expected: two `MANIFOLD_<ID>_HOST=<absolute path>` lines, `exit 0`, and a record whose `<hash>` equals the `Get-FileHash` line. This is a smoke run only; Task 8 Step 7 holds the full list of expectations and is the one home for them.

If either exe is missing from `target/release`, the gate instead prints `release-embedded: <id> <file>: not found (looked for <repo>\target\release\<file>)` on stderr and `exit 1`: that is the error path working, not a failure of this task; say so in the report and go on. Do not build the release hosts for this check.

- [ ] **Step 9: Commit**

```bash
cargo fmt --all
git status --short
git add crates/gate/src/release_hosts.rs crates/gate/src/main.rs
git diff --cached --stat
git commit -F - <<'EOF'
gate: release-pins and release-embedded write a release's host files from the host table

Two new stages, with their cores in release_hosts.rs over the table's
Download and Embedded rows. release-pins copies each downloaded host
and its sidecars into DIST as FILE-PLATFORM (a host built for another
OS from FOREIGN, the Linux job's artifact) and writes the pin
manifest. release-embedded copies each embedded host into
DIST/embedded and writes its hash record, named by embedded_record
(<stem>.embedded-<id>.sha256). Both print the build variables as
NAME=path lines for the release job's $GITHUB_ENV. Nothing calls them
yet: release.yml moves to them in the next commit, together with the
renamed records' upload and reader.

<attribution>
EOF
git log -1 --format=%B | grep -v '^$' | tail -n 1
```

Expected: `git status --short` lists ` M crates/gate/src/main.rs` and `?? crates/gate/src/release_hosts.rs`. `git diff --cached --stat` lists exactly `crates/gate/src/main.rs` (4 insertions) and `crates/gate/src/release_hosts.rs` (new). The pre-commit hook passes (fmt and clippy). The last line printed is your session's `Co-Authored-By:` trailer, not `<attribution>`. No gate run here: Task 8 runs the full gate on the finished plan.

### Task 6: release_verify, manifest_gaps and the release.yml change

Spec step 6, the last commit with gate or release code (Task 7 follows with docs and two ENGINES comment edits) ("Other gate readers", the `release_verify.rs` bullet; the `release.yml` block after "`gate release-pins` ... and `gate release-embedded`"; Tests, Gate, the "Step 6" bullet; H5, H6, H14; Risks R2, R4, R5, R6; Deleted, the `release.yml` and `release_verify.rs` lines; Docs, README "Releases"). One commit, because the renamed hash records have a writer (`gate release-embedded`, called from the YAML), an uploader (the draft-release line) and a reader (`release_verify`), and all three change together (H6). After it, `release.yml` holds no host table, no source search and no embedded file path: it builds, calls the two Task 5 stages, and forwards their `NAME=path` lines to `$GITHUB_ENV`.

**Files:**
- Modify: `crates/gate/src/release_verify.rs` (124 lines at HEAD 10444ae5; Tasks 1-5 do not touch it): module doc and imports (HEAD :1-12), `Pinned` (:24-32), `manifest_files`' signature (:34-35), `verify` (:56-85), and the test module's end (:121-124).
- Modify: `crates/gate/src/release_hosts.rs` (Task 5's file): its test module's imports and end.
- Modify: `.github/workflows/release.yml` (145 lines at HEAD; Tasks 1-5 do not touch it): the two steps at HEAD :68-123 become three, and the draft-release line at HEAD :144.
- Modify: `README.md`: the Releases bullets (HEAD :387-390; one line lower after Task 1's crate-map line, so :388-391).

**Interfaces:**
- Consumes (Task 5, `crate::release_hosts`): `embedded_record(stem: &str, host: &Host) -> String`, `release_pins_in(root, tag, dist, foreign) -> Result<Vec<String>, String>`, the stages `release-pins vX.Y.Z DIST FOREIGN` and `release-embedded vX.Y.Z DIST` with their stdout of `NAME=path` lines, and in `release_hosts::tests` the fixture `release_tree(name: &str) -> PathBuf` and `const TAG: &str = "vX.Y.Z"`.
- Consumes (Task 1, `manifold_hosts`): `Os::here()`, `Os::pin_platform(self)`, `Os::exe_suffix(self)`; `Delivery`; `Host::pinned_files`, `Host::pin_platform`, `Host::file_name`, `Host::pinned_under(&self, platform: &str) -> Option<Vec<String>>`; `ALL`, `CEF`, `by_id(&str) -> Option<&'static Host>`; and the layout tests in `crates/manifold-hosts/tests/layout.rs` that read `release.yml` (every row's `exe` after a `-p `, `target/release/manifold-view-webkitgtk` in the Linux job, the `-Match '-(windows|linux)-x64$'` alternation on the `gh release create` line). This task keeps all three true.
- Produces (in `crates/gate/src/release_verify.rs`, `cfg(windows)`):

```rust
#[derive(Debug, PartialEq)]
pub(crate) struct Pinned {
    pub(crate) kind: String,
    pub(crate) platform: String,
    pub(crate) file: String,
    pub(crate) size: u64,
    pub(crate) sha256: String,
}
pub(crate) fn manifest_files(manifest: &str) -> Result<Vec<Pinned>, String>;
/// Each expected (id, platform, file) the manifest lacks ("<id>/<platform> <file>: missing from the manifest"), then
/// each listed one not expected ("<id>/<platform> <file>: not pinned by the host table").
pub(crate) fn manifest_gaps(entries: &[Pinned], running: Os) -> Vec<String>;
```

- Produces (`gate release-verify vX.Y.Z [DIST]`, usage unchanged): the stem `manifold-vX.Y.Z-<Os::here().pin_platform()>`; a new check `the manifest pins exactly the host table's files for this release's platform` (on failure `FAIL: ...: got ["<gap>", ...], wanted []`); the per-file re-hash unchanged; `<stem>.exe matches its .sha256`; one check per `Embedded` row, `embedded/<file> matches <stem>.embedded-<id>.sha256`.
- Produces (`release.yml`): the Windows job's steps "Downloadable hosts and their pin manifest", "Embedded hosts" and "manifold.exe" exactly as the spec's block; the draft release uploads `(Get-ChildItem dist -File -Filter "$stem.embedded-*.sha256" | ForEach-Object FullName)` where it named the two records. No `embedded-host`, `embedded-pdf-view`, `MANIFOLD_`, `embedded/`, `mozangle` or `libEGL` left in the file.
- Produces (README "Releases"): the bullets name `gate release-pins`, `gate release-embedded` (three times), the records `.embedded-pdfium.sha256` and `.embedded-webview2.sha256`, and no `hosts.yml`. Task 7 consumes this as "Task 6, README Releases".

- [ ] **Step 1: Start**

- `git status --short -- crates .github README.md` prints nothing, and `git log -1 --format=%s` prints Task 5's subject, `gate: release-pins and release-embedded write a release's host files from the host table`.
- `git diff --stat 10444ae5 HEAD -- .github/workflows/release.yml crates/gate/src/release_verify.rs` prints nothing, so the old texts below are as quoted.
- Each of these prints `1` (the README anchors of Step 9, unchanged since HEAD):

```bash
grep -c -F '(`dist/manifold-vX.Y.Z-windows-x64.host-pins.json`), the embedded WebView2 host and PDF view, `manifold.exe` built with' README.md
grep -c -F '`gate release-verify` (the host files in `dist/` match the manifest)' README.md
grep -c -F 'set by `release.yml` and `hosts.yml` only' README.md
grep -c -F '`release.yml` records its hash in `dist/manifold-<tag>-windows-x64.embedded-pdf-view.sha256`, a second file next to the WebView2 host'"'"'s; `gate release-verify` checks both' README.md
grep -c -F '(set by `release.yml`)' README.md
```

- In PowerShell, `Get-Process cargo -ErrorAction SilentlyContinue` prints nothing.

- [ ] **Step 2: Write the failing `manifest_gaps` test**

In `crates/gate/src/release_verify.rs` (at HEAD :121-124, the end of the file), replace:

```rust
        assert!(manifest_files(r#"{"cef": {"windows-x64": [{"file": "a.exe"}]}}"#).is_err());
        assert!(manifest_files("[]").is_err());
    }
}
```

with:

```rust
        assert!(manifest_files(r#"{"cef": {"windows-x64": [{"file": "a.exe"}]}}"#).is_err());
        assert!(manifest_files("[]").is_err());
    }

    fn pinned(kind: &str, platform: &str, file: &str) -> Pinned {
        Pinned { kind: kind.into(), platform: platform.into(), file: file.into(), size: 1, sha256: "00".into() }
    }

    /// What a Windows release pins, per the host table: Servo with its two ANGLE DLLs, CEF, and WebKit's Linux ELF.
    fn windows_release() -> Vec<Pinned> {
        vec![
            pinned("servo", "windows-x64", "manifold-view-servo.exe"),
            pinned("servo", "windows-x64", "libEGL.dll"),
            pinned("servo", "windows-x64", "libGLESv2.dll"),
            pinned("cef", "windows-x64", "manifold-view-cef.exe"),
            pinned("webkit", "linux-x64", "manifold-view-webkitgtk"),
        ]
    }

    #[test]
    fn a_manifest_that_differs_from_the_host_table_has_a_gap_per_file() {
        let windows = Os::Windows;
        assert_eq!(manifest_gaps(&windows_release(), windows), Vec::<String>::new());
        let without = |file: &str| windows_release().into_iter().filter(|e| e.file != file).collect::<Vec<_>>();
        let missing_host = manifest_gaps(&without("manifold-view-cef.exe"), windows);
        assert_eq!(missing_host, ["cef/windows-x64 manifold-view-cef.exe: missing from the manifest"]);
        let missing_sidecar = manifest_gaps(&without("libEGL.dll"), windows);
        assert_eq!(missing_sidecar, ["servo/windows-x64 libEGL.dll: missing from the manifest"]);
        let mut extra = windows_release();
        extra.push(pinned("servo", "windows-x64", "extra.dll"));
        assert_eq!(manifest_gaps(&extra, windows), ["servo/windows-x64 extra.dll: not pinned by the host table"]);
        let mut moved = without("manifold-view-webkitgtk");
        moved.push(pinned("webkit", "windows-x64", "manifold-view-webkitgtk"));
        let want = [
            "webkit/linux-x64 manifold-view-webkitgtk: missing from the manifest",
            "webkit/windows-x64 manifold-view-webkitgtk: not pinned by the host table",
        ];
        assert_eq!(manifest_gaps(&moved, windows), want);
        // CEF is built for whichever OS runs it, so pinned_under alone (host_pin::parse's check) accepts it under
        // linux-x64; a Windows release does not pin it there.
        assert_eq!(manifold_hosts::CEF.pinned_under("linux-x64"), Some(vec!["manifold-view-cef".to_owned()]));
        let mut linux_cef = windows_release();
        linux_cef.push(pinned("cef", "linux-x64", "manifold-view-cef"));
        let linux_gaps = manifest_gaps(&linux_cef, windows);
        assert_eq!(linux_gaps, ["cef/linux-x64 manifold-view-cef: not pinned by the host table"]);
    }
}
```

The cases are the spec's list (Tests, "Manifests"): a missing host, a missing sidecar, an extra file, a host under the wrong platform, and `cef` under `linux-x64` in a Windows release, which `pinned_under` alone accepts. They pass `Os::Windows`, not `Os::here()`, so the expected set is fixed.

- [ ] **Step 3: Write the failing round-trip, agreement and upload-glob tests**

In `crates/gate/src/release_hosts.rs`, replace:

```rust
mod tests {
    use super::*;

    const TAG: &str = "vX.Y.Z";
```

with:

```rust
mod tests {
    use super::*;
    use crate::release_verify::{manifest_files, manifest_gaps, Pinned};

    const TAG: &str = "vX.Y.Z";
```

(rustfmt keeps `super::*` before `crate::` in one group, as in `manifold-view-protocol/src/frame_buffer.rs:220`-`221`.)

Then replace the end of the file (the end of Task 5's last test):

```rust
        let pdfium = embedded_record(stem, &manifold_hosts::PDFIUM);
        assert_eq!(pdfium, "manifold-vX.Y.Z-windows-x64.embedded-pdfium.sha256");
    }
}
```

with:

```rust
        let pdfium = embedded_record(stem, &manifold_hosts::PDFIUM);
        assert_eq!(pdfium, "manifold-vX.Y.Z-windows-x64.embedded-pdfium.sha256");
    }

    /// The manifest `release_pins_in` writes for a `release_tree`, parsed as `release-verify` parses it.
    fn written_manifest(name: &str) -> Vec<Pinned> {
        let root = release_tree(name);
        let lines = release_pins_in(&root, TAG, &root.join("dist"), &root.join("webkit-host")).unwrap();
        let path = lines[0].strip_prefix("MANIFOLD_HOST_PINS=").unwrap();
        let entries = manifest_files(&fs::read_to_string(path).unwrap()).unwrap();
        let _ = fs::remove_dir_all(&root);
        entries
    }

    fn sorted(mut files: Vec<String>) -> Vec<String> {
        files.sort_unstable();
        files
    }

    #[test]
    fn the_written_manifest_has_no_gap() {
        // The round trip: what release-pins writes is what release-verify accepts, so the two cannot drift.
        let entries = written_manifest("round-trip");
        assert_eq!(entries.len(), 5);
        assert_eq!(manifest_gaps(&entries, Os::here()), Vec::<String>::new());
    }

    #[test]
    fn every_entry_of_the_written_manifest_is_one_the_app_accepts() {
        // Spec R6: a manifest manifest_gaps passes also passes host_pin::parse, whose check is Host::pinned_under.
        let entries = written_manifest("agreement");
        assert_eq!(entries.len(), 5);
        for e in &entries {
            let host = manifold_hosts::by_id(&e.kind).unwrap_or_else(|| panic!("no row {}", e.kind));
            let same = entries.iter().filter(|o| o.kind == e.kind && o.platform == e.platform);
            let want = sorted(same.map(|o| o.file.clone()).collect());
            assert_eq!(host.pinned_under(&e.platform).map(sorted), Some(want), "{}/{}", e.kind, e.platform);
        }
    }

    #[test]
    fn the_draft_release_uploads_every_embedded_record() {
        // The upload line first runs on a real tag (spec H14), so its glob is tied to embedded_record here.
        let yml = fs::read_to_string(repo().join(".github").join("workflows").join("release.yml")).unwrap();
        let line = yml.lines().find(|line| line.contains("gh release create")).expect("no gh release create line");
        let stem = "manifold-vX.Y.Z-windows-x64";
        let glob = line
            .split_once("-Filter \"")
            .and_then(|(_, rest)| rest.split_once('"'))
            .map(|(glob, _)| glob.replace("$stem", stem))
            .unwrap_or_else(|| panic!("no -Filter \"...\" on: {line}"));
        let (left, right) = glob.split_once('*').unwrap_or_else(|| panic!("no * in the -Filter glob {glob}"));
        let matches = |name: &str| name.strip_prefix(left).is_some_and(|rest| rest.ends_with(right));
        for host in manifold_hosts::ALL.iter().filter(|host| host.delivery == Delivery::Embedded) {
            let record = embedded_record(stem, host);
            assert!(matches(record.as_str()), "{glob} does not match {record}");
        }
        assert!(!matches("manifold-vX.Y.Z-windows-x64.exe.sha256"), "{glob} matches the exe's record too");
    }
}
```

The last test is the spec's upload-glob test: it takes the text between `-Filter "` and the next `"` on the `gh release create` line, substitutes the literal `$stem`, and checks every `Embedded` row's record against the halves around `*`; the exe's record must not match, so it cannot pass vacuously.

- [ ] **Step 4: Run the tests to see them fail**

Run: `cargo test -j 8 -p gate -- release_hosts release_verify`
Expected: the test build fails, no test runs. The errors, among others, name `manifest_gaps` (`error[E0432]: unresolved import` in `release_hosts.rs`, `error[E0425]: cannot find function` in `release_verify.rs`'s tests), `Os` (`error[E0433]: failed to resolve: use of undeclared type` in `release_verify.rs`'s tests, until Step 5 imports it) and the private `manifest_files` and `Pinned` (`error[E0603]: ... is private`).

- [ ] **Step 5: `release_verify` reads the table**

Four edits in `crates/gate/src/release_verify.rs`.

(a) The module doc and imports (at HEAD :1-12). Replace:

```rust
//! `gate release-verify vX.Y.Z [DIST]`: re-checks a built release (which commit it was built from is `release-check`'s job). Every host the tag's
//! pin manifest (`DIST/manifold-vX.Y.Z-windows-x64.host-pins.json`, the file the exe was built with) lists must sit in
//! DIST as `FILE-PLATFORM` with the listed size and SHA-256; the exe in DIST (default dist/) must match its .sha256;
//! and the embedded WebView2 host and PDF view copies in DIST/embedded must match the recorded hashes. That the exe
//! really contains those bytes and that manifest is not checked here (build.rs embeds them; `gate package` reads the
//! engine states the manifest produces).

use std::path::Path;
use std::process::ExitCode;

use crate::cef_pin::{hash_file, repo};
use crate::check::{check, exit_code, note_failure};
```

with:

```rust
//! `gate release-verify vX.Y.Z [DIST]`: re-checks a built release (which commit it was built from is
//! `release-check`'s job). The release's platform is `Os::here()`, since the gate runs on the release runner, and its
//! files are named from the stem `manifold-vX.Y.Z-<platform>`. The pin manifest (`DIST/<stem>.host-pins.json`, the
//! file the exe was built with) must pin exactly the host table's files for that platform (`manifest_gaps`), and every
//! file it lists must sit in DIST (default dist/) as `FILE-PLATFORM` with the listed size and SHA-256; the exe in DIST
//! must match its .sha256; and each embedded host's copy in DIST/embedded must match its hash record
//! (`release_hosts::embedded_record`). That the exe really contains those bytes and that manifest is not checked here
//! (build.rs embeds them; `gate package` reads the engine states the manifest produces).

use std::path::Path;
use std::process::ExitCode;

use manifold_hosts::{Delivery, Os};

use crate::cef_pin::{hash_file, repo};
use crate::check::{check, check_is, exit_code, note_failure};
use crate::release_hosts::embedded_record;
```

(b) `Pinned` (at HEAD :24-32). Replace:

```rust
/// One file the pin manifest lists.
#[derive(Debug, PartialEq)]
struct Pinned {
    kind: String,
    platform: String,
    file: String,
    size: u64,
    sha256: String,
}
```

with:

```rust
/// One file the pin manifest lists.
#[derive(Debug, PartialEq)]
pub(crate) struct Pinned {
    pub(crate) kind: String,
    pub(crate) platform: String,
    pub(crate) file: String,
    pub(crate) size: u64,
    pub(crate) sha256: String,
}
```

(c) `manifest_files`' signature (at HEAD :34-35; its body is unchanged). Replace:

```rust
/// Every file the manifest lists.
fn manifest_files(manifest: &str) -> Result<Vec<Pinned>, String> {
```

with:

```rust
/// Every file the manifest lists.
pub(crate) fn manifest_files(manifest: &str) -> Result<Vec<Pinned>, String> {
```

(d) `verify` (at HEAD :56-85), with `manifest_gaps` added before it. Replace:

```rust
fn verify(tag: &str, dist: &Path) -> Result<(), String> {
    let stem = format!("manifold-{tag}-windows-x64");
    let manifest = std::fs::read_to_string(dist.join(format!("{stem}.host-pins.json")))
        .map_err(|e| format!("{stem}.host-pins.json: {e}"))?;
    for Pinned { kind, platform, file, size, sha256 } in manifest_files(&manifest)? {
        let path = dist.join(format!("{file}-{platform}"));
        let ok = hash_file(&path).is_ok_and(|(got, _, n)| got == sha256 && n == size);
        check(&format!("{kind}/{platform} {file} sits in dist with the pinned size and SHA-256"), ok);
    }
    let exe_name = format!("manifold-{tag}-windows-x64.exe");
    check(
        &format!("{exe_name} matches its .sha256"),
        matches_record(&dist.join(&exe_name), &dist.join(format!("{exe_name}.sha256"))),
    );
    check(
        "the embedded WebView2 host copy matches the recorded hash",
        matches_record(
            &dist.join("embedded/manifold-view-webview2.exe"),
            &dist.join(format!("{stem}.embedded-host.sha256")),
        ),
    );
    check(
        "the embedded PDF view copy matches the recorded hash",
        matches_record(
            &dist.join("embedded/manifold-view-pdfium.exe"),
            &dist.join(format!("{stem}.embedded-pdf-view.sha256")),
        ),
    );
    Ok(())
}
```

with:

```rust
/// What keeps `entries` (a parsed manifest) from pinning exactly the host table's files for a release built on
/// `running`: each file a `Download` row pins under its platform that the manifest lacks, then each file the manifest
/// lists that no row pins there. Stricter than `Host::pinned_under`, the check `host_pin::parse` makes: a host under
/// another OS's platform (`cef` under `linux-x64` in a Windows release) is a gap here.
pub(crate) fn manifest_gaps(entries: &[Pinned], running: Os) -> Vec<String> {
    let mut expected = Vec::new();
    for host in manifold_hosts::ALL.iter().filter(|host| host.delivery == Delivery::Download) {
        for file in host.pinned_files(running) {
            expected.push((host.id, host.pin_platform(running), file));
        }
    }
    let mut gaps = Vec::new();
    for (id, platform, file) in &expected {
        if !entries.iter().any(|e| e.kind == *id && e.platform == *platform && e.file == *file) {
            gaps.push(format!("{id}/{platform} {file}: missing from the manifest"));
        }
    }
    for e in entries {
        if !expected.iter().any(|(id, platform, file)| e.kind == *id && e.platform == *platform && e.file == *file) {
            gaps.push(format!("{}/{} {}: not pinned by the host table", e.kind, e.platform, e.file));
        }
    }
    gaps
}

fn verify(tag: &str, dist: &Path) -> Result<(), String> {
    let running = Os::here();
    let stem = format!("manifold-{tag}-{}", running.pin_platform());
    let manifest = std::fs::read_to_string(dist.join(format!("{stem}.host-pins.json")))
        .map_err(|e| format!("{stem}.host-pins.json: {e}"))?;
    let entries = manifest_files(&manifest)?;
    check_is(
        "the manifest pins exactly the host table's files for this release's platform",
        Vec::<String>::new(),
        manifest_gaps(&entries, running),
    );
    for Pinned { kind, platform, file, size, sha256 } in entries {
        let path = dist.join(format!("{file}-{platform}"));
        let ok = hash_file(&path).is_ok_and(|(got, _, n)| got == sha256 && n == size);
        check(&format!("{kind}/{platform} {file} sits in dist with the pinned size and SHA-256"), ok);
    }
    let exe_name = format!("{stem}{}", running.exe_suffix());
    check(
        &format!("{exe_name} matches its .sha256"),
        matches_record(&dist.join(&exe_name), &dist.join(format!("{exe_name}.sha256"))),
    );
    for host in manifold_hosts::ALL.iter().filter(|host| host.delivery == Delivery::Embedded) {
        let (file, record) = (host.file_name(running), embedded_record(&stem, host));
        let label = format!("embedded/{file} matches {record}");
        check(&label, matches_record(&dist.join("embedded").join(file), &dist.join(record)));
    }
    Ok(())
}
```

`release_verify` (the `pub fn` wrapper, HEAD :87-101), `recorded` and `matches_record` are unchanged. The re-hash of each listed file (`:58`-`63`) still reads only files, so a wrong row cannot make it agree (Risks R1). On Windows the stem, exe name and record names are the ones the release has always used: `manifold-vX.Y.Z-windows-x64`, `manifold-vX.Y.Z-windows-x64.exe`, and the new `.embedded-webview2.sha256` and `.embedded-pdfium.sha256` (H6).

- [ ] **Step 6: Run the tests: only the upload glob fails**

Run: `cargo test -j 8 -p gate -- release_hosts release_verify`
Expected: `test result: FAILED. 9 passed; 1 failed`. The one failure is `release_hosts::tests::the_draft_release_uploads_every_embedded_record`, panicking with `no -Filter "..." on:` followed by HEAD's `gh release create` line (it still names the two records). The `release_verify::tests` (2) and the other seven `release_hosts::tests` pass.

- [ ] **Step 7: Change `release.yml`**

Two edits in `.github/workflows/release.yml`. They replace the PowerShell host table, source map and manifest loop (HEAD :74-101) with `gate release-pins`, and the embedded copies, `Resolve-Path` variables and records (HEAD :107-112, :117-123) with `gate release-embedded` (H5). Because a `$GITHUB_ENV` line takes effect from the next step, `cargo build -p manifold` moves into a step of its own. The Servo step (HEAD :63-67), the package and verify step (HEAD :124-130) and the Linux job (H10) are unchanged.

(a) At HEAD :68-123, replace:

```yaml
      - name: Downloadable hosts and their pin manifest
        run: |
          # Each host is attached to this release; manifold.exe is built with the manifest of their sizes and hashes.
          $env:RUSTFLAGS = '-C target-feature=+crt-static'
          cargo build --release --locked -p manifold-view-cef
          if ($LASTEXITCODE -ne 0) { exit 1 }
          New-Item -ItemType Directory -Force dist | Out-Null
          # Extra host files: ANGLE's DLLs come from mozangle's build output (the newest copy is the one this build
          # made); WebKit's host comes from the Linux job.
          $sources = @{ 'manifold-view-webkitgtk' = 'webkit-host/manifold-view-webkitgtk' }
          if (-not (Test-Path $sources['manifold-view-webkitgtk'])) { throw 'no WebKit host from the Linux job' }
          foreach ($dll in 'libEGL.dll', 'libGLESv2.dll') {
            $sources[$dll] = Get-ChildItem target/release/build -Recurse -Filter $dll | Where-Object FullName -Match 'mozangle' | Sort-Object LastWriteTime -Descending | Select-Object -First 1 -ExpandProperty FullName
            if (-not $sources[$dll]) { throw "no $dll under target/release/build" }
          }
          function Source($name) { if ($sources.ContainsKey($name)) { $sources[$name] } else { "target/release/$name" } }
          $hosts = [ordered]@{
            cef = [ordered]@{ 'windows-x64' = @('manifold-view-cef.exe') }
            servo = [ordered]@{ 'windows-x64' = @('manifold-view-servo.exe', 'libEGL.dll', 'libGLESv2.dll') }
            webkit = [ordered]@{ 'linux-x64' = @('manifold-view-webkitgtk') }
          }
          $manifest = [ordered]@{}
          foreach ($kind in $hosts.Keys) {
            $manifest[$kind] = [ordered]@{}
            foreach ($platform in $hosts[$kind].Keys) {
              $files = foreach ($name in $hosts[$kind][$platform]) {
                Copy-Item (Source $name) "dist/$name-$platform"
                [ordered]@{ file = $name; size = (Get-Item "dist/$name-$platform").Length; sha256 = (Get-FileHash "dist/$name-$platform" -Algorithm SHA256).Hash.ToLower() }
              }
              $manifest[$kind][$platform] = @($files)
            }
          }
          $manifest | ConvertTo-Json -Depth 6 | Set-Content -Encoding ascii "dist/manifold-$env:TAG-windows-x64.host-pins.json"
          Get-Content "dist/manifold-$env:TAG-windows-x64.host-pins.json"
      - name: Embedded hosts and manifold.exe
        run: |
          $env:RUSTFLAGS = '-C target-feature=+crt-static'
          cargo build --release --locked -p manifold-view-webview2 -p manifold-view-pdfium
          if ($LASTEXITCODE -ne 0) { exit 1 }
          $env:MANIFOLD_HOST_PINS = (Resolve-Path "dist/manifold-$env:TAG-windows-x64.host-pins.json").Path
          New-Item -ItemType Directory -Force dist/embedded | Out-Null
          Copy-Item target/release/manifold-view-webview2.exe dist/embedded/
          Copy-Item target/release/manifold-view-pdfium.exe dist/embedded/
          $env:MANIFOLD_WEBVIEW2_HOST = (Resolve-Path dist/embedded/manifold-view-webview2.exe).Path
          $env:MANIFOLD_PDFIUM_HOST = (Resolve-Path dist/embedded/manifold-view-pdfium.exe).Path
          cargo build --release --locked -p manifold
          if ($LASTEXITCODE -ne 0) { exit 1 }
          $stem = "manifold-$env:TAG-windows-x64"
          Copy-Item target/release/manifold.exe "dist/$stem.exe"
          function Record($path, $out) {
            $h = (Get-FileHash $path -Algorithm SHA256).Hash.ToLower()
            "$h  $(Split-Path $path -Leaf)" | Set-Content -Encoding ascii $out
          }
          Record "dist/$stem.exe" "dist/$stem.exe.sha256"
          Record "dist/embedded/manifold-view-webview2.exe" "dist/$stem.embedded-host.sha256"
          Record "dist/embedded/manifold-view-pdfium.exe" "dist/$stem.embedded-pdf-view.sha256"
```

with (the spec's block, verbatim):

```yaml
      - name: Downloadable hosts and their pin manifest
        run: |
          $env:RUSTFLAGS = '-C target-feature=+crt-static'
          cargo build --release --locked -p manifold-view-cef
          if ($LASTEXITCODE -ne 0) { exit 1 }
          & target/debug/gate.exe release-pins $env:TAG dist webkit-host | Add-Content $env:GITHUB_ENV
          if ($LASTEXITCODE -ne 0) { exit 1 }
      - name: Embedded hosts
        run: |
          $env:RUSTFLAGS = '-C target-feature=+crt-static'
          cargo build --release --locked -p manifold-view-webview2 -p manifold-view-pdfium
          if ($LASTEXITCODE -ne 0) { exit 1 }
          & target/debug/gate.exe release-embedded $env:TAG dist | Add-Content $env:GITHUB_ENV
          if ($LASTEXITCODE -ne 0) { exit 1 }
      - name: manifold.exe
        run: |
          $env:RUSTFLAGS = '-C target-feature=+crt-static'
          cargo build --release --locked -p manifold
          if ($LASTEXITCODE -ne 0) { exit 1 }
          $stem = "manifold-$env:TAG-windows-x64"
          Copy-Item target/release/manifold.exe "dist/$stem.exe"
          $h = (Get-FileHash "dist/$stem.exe" -Algorithm SHA256).Hash.ToLower()
          "$h  $stem.exe" | Set-Content -Encoding ascii "dist/$stem.exe.sha256"
```

Why each piece holds (spec notes on the block): `target/debug/gate.exe` is built earlier in the job (HEAD :57, `cargo build --locked -p gate`) and runs in the repo root, which is the root `repo()` names, so `dist` and `webkit-host` resolve as before; the job's shell is `pwsh` (HEAD :36); `Add-Content $env:GITHUB_ENV` is how the job already writes it (HEAD :48); a native exe piped into a cmdlet with `$LASTEXITCODE` checked after is how it already runs `gate release-notes` (HEAD :142-143). The manifest's text still reaches the log (the gate writes it to stderr). The comment at HEAD :70 goes with the block: the spec's block has no comment, and the step now builds the hosts and writes the manifest only, no longer `manifold.exe`.

(b) The draft-release upload line (HEAD :144). Replace:

```yaml
          gh release create $env:TAG --draft --verify-tag --title $env:TAG --notes-file notes.md "dist/$stem.exe" "dist/$stem.exe.sha256" "dist/$stem.embedded-host.sha256" "dist/$stem.embedded-pdf-view.sha256" "dist/$stem.host-pins.json" (Get-ChildItem dist -File | Where-Object Name -Match '-(windows|linux)-x64$' | ForEach-Object FullName)
```

with:

```yaml
          gh release create $env:TAG --draft --verify-tag --title $env:TAG --notes-file notes.md "dist/$stem.exe" "dist/$stem.exe.sha256" (Get-ChildItem dist -File -Filter "$stem.embedded-*.sha256" | ForEach-Object FullName) "dist/$stem.host-pins.json" (Get-ChildItem dist -File | Where-Object Name -Match '-(windows|linux)-x64$' | ForEach-Object FullName)
```

The records keep their place in the upload list; the `-Match` alternation that Task 1's layout test reads is unchanged.

Run: `wc -l .github/workflows/release.yml` and `grep -n "gh release create" .github/workflows/release.yml`
Expected: `112 .github/workflows/release.yml` (145, less the 56 lines of (a), plus its 23), and the upload line at :111.

- [ ] **Step 8: Run the tests to see them pass, then clippy**

Run: `cargo test -j 8 -p gate -- release_hosts release_verify`
Expected: `test result: ok. 10 passed`.

Run: `cargo test -j 8 -p gate -- --test-threads 8`
Expected: `test result: ok.`; no warning.

Run: `cargo test -j 8 -p manifold-hosts`
Expected: `test result: ok.` for the unit tests and for `tests/layout.rs`, whose `release.yml` checks (the `-p` lines, `target/release/manifold-view-webkitgtk`, the `-Match` alternation) read the changed file.

Run the clippy line: `cargo clippy -j 8 -p manifold -p manifold-hosts -p manifold-view-protocol -p manifold-pages -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings`
Expected: no warning, exit 0.

- [ ] **Step 9: README "Releases"**

Five edits in `README.md`, all in the Releases bullets under `## Branches` (HEAD :387-390, one lower after Task 1). Each old text is unique (Step 1 counted it).

(a) In the "Releases run on GitHub Actions" bullet, replace:

```markdown
(`dist/manifold-vX.Y.Z-windows-x64.host-pins.json`), the embedded WebView2 host and PDF view, `manifold.exe` built with
```

with:

```markdown
(`dist/manifold-vX.Y.Z-windows-x64.host-pins.json`, written by `gate release-pins`, which copies into `dist/` each host the host table in `crates/manifold-hosts` marks as downloaded, a host built for another OS from the `webkit-host` job's artifact), the embedded WebView2 host and PDF view (`gate release-embedded` copies them into `dist/embedded/` and writes their hash records; both subcommands print the build variables as `NAME=path` lines, which `release.yml` appends to `$GITHUB_ENV` for the step that builds `manifold.exe`), `manifold.exe` built with
```

(b) In the same bullet, replace:

```markdown
`gate release-verify` (the host files in `dist/` match the manifest)
```

with:

```markdown
`gate release-verify` (the manifest pins exactly the host table's files for the release's platform, the host files in `dist/` match it, and each embedded host's copy matches its hash record)
```

(c) In the "Packaging" bullet (`hosts.yml` no longer exists), replace:

```markdown
set by `release.yml` and `hosts.yml` only
```

with:

```markdown
set by `release.yml` only
```

(d) In the "Embedded PDF view" bullet (the record names, H6), replace:

```markdown
`release.yml` records its hash in `dist/manifold-<tag>-windows-x64.embedded-pdf-view.sha256`, a second file next to the WebView2 host's; `gate release-verify` checks both
```

with:

```markdown
`gate release-embedded` records its hash in `dist/manifold-<tag>-windows-x64.embedded-pdfium.sha256`, next to the WebView2 host's `.embedded-webview2.sha256`; `gate release-verify` checks both
```

(e) In the "Embedded WebView2 host" bullet, replace:

```markdown
(set by `release.yml`)
```

with:

```markdown
(set by `release.yml` from `gate release-embedded`'s output)
```

Run:

```bash
grep -c -F 'gate release-pins' README.md
grep -c -F 'gate release-embedded' README.md
grep -c -F 'hosts.yml' README.md
grep -c -F '.embedded-pdfium.sha256' README.md
```

Expected: `1`, `3`, `0`, `1` (the second counts lines: (a), (d) and (e) are on three bullets).

- [ ] **Step 10: Grep for the deleted asset names and the YAML's old host logic**

From bash:

```bash
git grep -n -e embedded-host -e embedded-pdf-view -- .github crates README.md
git grep -n -e 'MANIFOLD_' -e 'embedded/' -e 'Resolve-Path' -e mozangle -e libEGL -e 'webkit-host/' -- .github/workflows/release.yml
```

Expected: both print nothing (exit 1). At HEAD the first printed `release.yml:122`, `:123`, `:144`, `README.md:389` and `release_verify.rs:74`, `:81`, the only places the old record names lived (spec R4; old plans and specs keep them, as history). The second proves the YAML names no app variable and no embedded file path (spec, the notes on the block) and holds no host table or source search of its own.

- [ ] **Step 11: Leave the release dry run to the user**

Do not push. The changed `release.yml` first runs in the workflow's manual dry run (`workflow_dispatch` on a pushed branch: it builds, writes `dist/`, runs `gate package` and `gate release-verify`, and uploads `dist/` as an artifact), which needs this branch pushed: that is the user's call (spec H14). The draft-release line (b) runs only on a real tag; until then the upload-glob test of Step 3 and Task 1's layout test are its only checks (spec R2). Say this in the task report, together with what a dry run would show: the manifest keys in `ALL`'s order (`servo`, `cef`, `webkit`, spec R5, which both parsers ignore), the two records named `.embedded-webview2.sha256` and `.embedded-pdfium.sha256`, and `release-verify`'s new line `PASS: the manifest pins exactly the host table's files for this release's platform`.

- [ ] **Step 12: Commit**

```bash
cargo fmt --all
git diff --stat
git add .github/workflows/release.yml README.md crates/gate/src/release_hosts.rs crates/gate/src/release_verify.rs
git commit -F - <<'EOF'
release: the gate writes and checks the release's host files from the host table

release.yml's PowerShell host table, source search, manifest loop and
embedded copies give way to gate release-pins and gate
release-embedded, whose NAME=path lines go to $GITHUB_ENV, so building
manifold.exe moves into a step of its own. The embedded hash records
are renamed <stem>.embedded-<id>.sha256 (were embedded-host and
embedded-pdf-view): writer, draft-release upload (now a -Filter glob)
and release-verify change together. release-verify takes the stem
from Os::here(), checks the embedded hosts in one loop over the
Embedded rows, and fails a manifest that does not pin exactly the
host table's files for the release's platform (manifest_gaps). Tests
tie the written manifest to manifest_gaps and to Host::pinned_under,
and the upload glob to embedded_record. README "Releases" follows.

<attribution>
EOF
git log -1 --format=%B | grep -v '^$' | tail -n 1
```

Expected: `git diff --stat` lists exactly `.github/workflows/release.yml`, `README.md`, `crates/gate/src/release_hosts.rs` and `crates/gate/src/release_verify.rs`. The pre-commit hook passes (fmt and clippy). The last line printed is your session's `Co-Authored-By:` trailer, not `<attribution>`. No gate run here: the change reaches `crates/gate/` (tier `full`), and Task 8 runs the full gate on the finished plan.

### Task 7: The remaining docs, PARKED and decision records

Spec step 7, with the spec's "Docs" and "Deleted" (the PARKED entries) sections. Docs and comments only: no behaviour
changes, no CHANGELOG entry (it is written at the bump), and the spec's `status:` is not touched here.

**Files:**
- Modify: `README.md` ("Engines by OS" and "Runtime-file placement"; Task 1 already edited the crate map and the CI
  line, Task 6 the Releases bullets)
- Modify: `PARKED.md`
- Modify: `docs/decisions/2026-10-23-host-runtimes.md`
- Modify: `docs/decisions/2026-10-18-hosts-in-the-release.md`
- Modify: `crates/manifold/src/session_sync.rs:121`, `crates/manifold/src/store/model.rs:227` (doc comments only)
- Modify only where Step 12 or Step 15 finds a stale name in a comment or doc that no other task rewrites (comments and
  docs only; at plan time none is known)

The doc comments the spec lists on items Tasks 2-6 delete or rewrite are rewritten by those tasks, with their code
(spec "Docs": "Code comments that name a deleted item change with the code"). This task only checks that they are gone
(Step 13) and writes no second version of them.

**Interfaces:**
- Consumes (Tasks 1-6, all committed before this task starts):
  - Task 1: `crates/manifold-hosts/src/lib.rs` with `pub fn in_place(`, `Host.os`, `Host::built_for`, `Host::beside`,
    `Host::installed_in`, `Host::file_name`, `Host::runtime_lib`, `Host::installed_files`, `Host::embed_var`,
    `Host.dev_dir`, `Delivery`, `CEF_RUNTIME_DIR`, `ALL`, `by_id`; README's `manifold-hosts` crate-map line and
    `-p manifold-hosts` on README's CI line; `-p manifold-hosts` in `ci.yml`, the pre-commit hook and CLAUDE.md.
  - Task 2: `ViewDesc.host: &'static Host` (a field line containing `host: &'static`); `HostSource`, `Embedded`,
    `CEF_RUNTIME_DIR` in `process_manager.rs` and `sibling_binary`, `host_file_name`, `beside_exe`, `in_place_paths`
    deleted; `run_in_place(`, `view_binary_path(` and its `TODO` kept in `availability.rs`; `runtime::Os` moved to
    the leaf; the second test in `crates/manifold/src/deleted_names.rs`; the spec's listed doc comments in
    `process_manager.rs`, `host_pin.rs`, `view_install.rs`, `availability.rs`, `runtime.rs`, `embedded_host.rs`,
    `build.rs` and `process_manager/tests.rs` rewritten, and the names guard's `desc().name` exemption dropped (H3).
  - Task 3: `sync_host`, `HostSync`, `find_first`, `run_in` in `crates/gate/src/sync_deps.rs`, with its module doc
    rewritten; `find_newest`, `sync_servo`, `sync_cef`, `SERVO_DLLS`, `SERVO_EXE` deleted.
  - Task 4: `fresh.rs`'s host list of rows (`static DEP_INFO_HOSTS`, `[&WEBVIEW2, &PDFIUM]`) and
    `require_fresh_exe`; `package.rs`'s `installed_files` from `Host::installed_files`, with its doc rewritten;
    `sleep.rs`'s `kill_hosts` through `by_id`.
  - Task 5: `gate release-pins`, `gate release-embedded`, `embedded_record`.
  - Task 6: `release_verify.rs` (`manifest_gaps`, the `embedded-<id>` record names, its module doc), `release.yml`,
    README "Releases".
- Produces: README, PARKED and the two decision records describing the host table; PARKED without GATE-CEF-DIR, "One
  table per host" and "Servo's DLLs come from the newest mozangle output in either target folder"; no comment in the
  tree naming a deleted item; one commit.

**Line numbers** below are HEAD's (`10444ae5`) and are hints only. Tasks 1-6 move them: Task 1 adds one README line in
the crate map, so every README line cited here after `:18` sits one lower (`:22` is `:23`, `:90` is `:91`). Find every
site by its quoted text. Evidence lines in PARKED are written from Step 6's grep output, never from these hints.

- [ ] **Step 1: Check Task 1's README edits and that README's in-place paragraph still holds**

Task 1 owns the crate-map line and the CI line; this step only checks them. README's "Which host runs, and whether it
speaks this build's protocol" paragraph (HEAD `:33`) and the "Runtime-file placement" opening (HEAD `:82`) name
functions in `availability.rs` that must survive Task 2.

Run:
```bash
grep -n -E '^manifold-hosts +' README.md
grep -n -F 'on Linux, `cargo test' README.md
git grep -n -F -e 'fn run_in_place(' -e 'pub(super) fn view_binary_path(' -e 'fn walks_dev_folders(' -e "TODO: a dev tree's runtime files are put beside its hosts by" -- crates/manifold/src/process_manager/availability.rs
git grep -n -F 'pub fn beside(' -- crates/manifold-hosts/src/lib.rs
```
Expected: one line starting `manifold-hosts`, inside the Architecture section's crate-map code block; the CI line
contains `-p manifold-hosts`;
four lines from `availability.rs` (one per needle); one line from the leaf. If the crate-map line or the CI line's
`-p manifold-hosts` is missing, stop and report it as a Task 1 deviation (do not add it here). If an `availability.rs`
needle is missing, stop and report it as a Task 2 deviation: README `:33` and `:82` name those items.

- [ ] **Step 2: README "Engines by OS" names the host table's `Host.os`**

In `README.md` (HEAD `:22`), replace:
```text
**Engines by OS** (2026-10-23; the app-side switch is `ViewDesc::host_os` read through `process_manager/runtime.rs`, the pick-up rules are in `process_manager/availability.rs`).
```
with:
```text
**Engines by OS** (2026-10-23; the app-side switch is `Host.os` in the host table (`crates/manifold-hosts`), the OS each host is built for, which `Host::built_for` resolves and `process_manager/runtime.rs` reads; the pick-up rules are in `process_manager/availability.rs`).
```
The rest of that line (`The principle is each OS's own system webview plus CEF and Servo; ...`) is unchanged.

Run: `grep -c -F 'ViewDesc::host_os' README.md`
Expected: `0`

- [ ] **Step 3: README "Runtime-file placement" describes the one sync rule**

In `README.md` under `### Runtime-file placement (required after every build)`, replace the whole paragraph (one
line, HEAD `:90`) that starts `It finds Servo's ANGLE DLLs under` and ends `simpler and more honest than fighting
Cargo's model.` with:
```text
It reads each host's layout from the host table (`manifold_hosts::ALL`, `crates/manifold-hosts/src/lib.rs`) and puts its runtime files beside the host manifold runs: Servo's ANGLE DLLs beside `target-servo/debug/manifold-view-servo.exe`, and CEF's runtime, with the CEF host itself, in `target/debug/cef-runtime/`. Each file comes from the build-script output in the host's own build folder first (`target-servo/debug/build/` for Servo) and from `target/debug/build/` only when that holds none, under `<crate>-<hash>/` or `<crate>/<hash>/` (where newer cargo nightlies put build-script output; `mozangle` for Servo's DLLs, `cef-dll-sys` for CEF's runtime); within one build folder it takes the newest match. A host whose build folder does not exist is skipped with one line (a tree with no `target-servo/` syncs CEF alone); the run fails only when no host with files to place has its build folder. A file found in no build output is kept, and said so, when it is already in place; otherwise it is an error when its host exe is built, and skipped when it is not. So a CEF build that left no runtime in `target/debug/build/` (a `CEF_PATH` build resolves it outside `OUT_DIR`) and has none in `target/debug/cef-runtime/` yet gets an error saying to copy the runtime there, and the CEF host is copied into that folder only once the runtime is in it. It `cmp`s each against the copy already in place and only recopies what's actually stale (safe to run any time — a no-op if nothing changed). Every step runs even when one fails; the failures are listed at the end, and a file a running host holds open is named with "close manifold and run again". Not a build.rs: a build script only sees its own crate's `OUT_DIR`, not a sibling crate's, and there's no reliable cross-crate coordination here without those upstream crates cooperating (they don't) — a plain script is simpler and more honest than fighting Cargo's model.
```
The paragraphs before it (`Cargo doesn't do this itself`, the WebView2 note, the `gate sync-runtime-deps` block) and
after it (`Why this needs a script at all`, which describes the newest-file pick within a build folder, and the prune
paragraph) stay: both still hold under the one rule (spec H9, "`gate sync-runtime-deps`").

Run: `grep -c -F -e 'takes the newest of either' -e '{target-servo,target}/debug/build' -e "It finds Servo's ANGLE DLLs under" README.md`
Expected: `0`

Run: `grep -c -F "fighting Cargo's model." README.md`
Expected: `1` (the new paragraph keeps the old one's closing sentence; this proves the whole old line, not part of it, was replaced)

- [ ] **Step 4: Amend `docs/decisions/2026-10-23-host-runtimes.md` (spec H2, H11)**

The record keeps its original text; each amendment is a dated note pointing at the spec.

Replace `:3`:
```text
Plan: `docs/superpowers/plans/2026-10-23-host-runtimes.md`. Code: `crates/manifold/src/process_manager/runtime.rs`.
```
with:
```text
Plan: `docs/superpowers/plans/2026-10-23-host-runtimes.md`. Code: `crates/manifold/src/process_manager/runtime.rs`; `Os`
has lived in `crates/manifold-hosts/src/lib.rs` since the host table (2026-10-29, spec
`docs/superpowers/specs/2026-10-29-host-table-design.md`, H11).
```

Replace `:9`:
```text
- `host_os: Option<Os>`, the OS its host is built for (`None`: whichever OS manifold runs on, as for Servo and CEF).
```
with:
```text
- `host_os: Option<Os>`, the OS its host is built for (`None`: whichever OS manifold runs on, as for Servo and CEF).
  Amended 2026-10-29 (host table spec, H2): this fact is now the host's row, `Host.os` in `crates/manifold-hosts`,
  which `ViewDesc.host` points to; `Host::built_for` resolves `None`.
```

After ruling A's second line (`:19`, which ends `so only Servo and CEF run there.`), insert these two lines (a
continuation of ruling A's list item) before `- **B. README matrix hand-written**`:
```text
  Amended 2026-10-29 (host table spec, H11): `Os` moved, with its tests, to `crates/manifold-hosts`; the row that holds
  it is the host's (`Host.os`).
```

Replace `:68`-`69`:
```text
- **Tags carry classification only.** `binary`, `prepare`, `source`, `link`, `host_os`, `runtime_dir`, `color`, copy stay
  plain host data. No family "card" above the host.
```
with:
```text
- **Tags carry classification only.** `binary`, `prepare`, `source`, `link`, `host_os`, `runtime_dir`, `color`, copy stay
  plain host data. No family "card" above the host. Amended 2026-10-29 (host table spec, H2): `binary`, `source`,
  `host_os` and `runtime_dir` are still plain data, in a table of their own: `ViewDesc.host` points to the host's row
  (`manifold_hosts::Host`: `exe`, `delivery`, `os`, `runtime`); `prepare`, `link`, `color` and copy stay on `ViewDesc`.
```

Run: `grep -c 'Amended 2026-10-29' docs/decisions/2026-10-23-host-runtimes.md`
Expected: `3`

Run: `grep -c -F 'docs/superpowers/specs/2026-10-29-host-table-design.md' docs/decisions/2026-10-23-host-runtimes.md`
Expected: `1`

- [ ] **Step 5: One line in `docs/decisions/2026-10-18-hosts-in-the-release.md` (spec H5)**

The decision (a release carries its hosts and a manifest the exe is built with) is unchanged; only the writer moved.
Replace:
```text
Spotify CDN) is not our file and keeps its own pin, `cef_pin.rs`, set by `gate cef-pin`.

A build with no manifest
```
with:
```text
Spotify CDN) is not our file and keeps its own pin, `cef_pin.rs`, set by `gate cef-pin`.

Since the host table (2026-10-29, `docs/superpowers/specs/2026-10-29-host-table-design.md`, H5), `gate release-pins`
writes the manifest from the table's downloadable hosts and `release.yml` calls it; the decision itself is unchanged.

A build with no manifest
```

Run: `grep -c -F 'gate release-pins' docs/decisions/2026-10-18-hosts-in-the-release.md`
Expected: `1`

- [ ] **Step 6: Collect the evidence lines for PARKED**

Steps 8-10 write PARKED evidence with markers (`LINE-...`, `DATE-TODAY`); each is replaced, while writing, by the value
found here. The markers never reach the file (Step 11 checks).

Run (Git Bash, repo root):
```bash
git grep -n -F -e 'fn ensure_host_copy(' -e 'fn shipped_cef_host(' -e 'fn prepare_cef(' -e '("VIEW_CACHE_DIR",' -e '("VIEW_WEBVIEW2_PROFILE",' -e 'let custom_cef = custom_cef.filter(' -e '.env("VIEW_TAB_ID",' -e "host: &'static" -- crates/manifold/src/process_manager.rs
git grep -n -F -e 'fn run_in_place(' -e 'fn runtime_on_disk(' -- crates/manifold/src/process_manager/availability.rs
git grep -n -F 'fn has_runtime(' -- crates/manifold/src/view_install.rs
git grep -n -F 'pub fn in_place(' -- crates/manifold-hosts/src/lib.rs
git grep -n -F '"VIEW_INITIAL_URL_HEX", "VIEW_TCP_PORT", "VIEW_TOKEN"]' -- crates/manifold/src/process_manager/tests.rs
git grep -n -e 'cargo clippy' -e 'cargo test' -- .github/workflows/ci.yml .githooks/pre-commit
git grep -n -F '**CI** (' -- README.md
git grep -n -F -e '-p manifold-view-pdfium -p gate' -- crates/gate/src/main.rs crates/gate/src/all.rs
git grep -n -F 'cargo build -p manifold -p' -- crates/gate/src/sleep.rs crates/gate/src/click_count.rs crates/gate/src/iframe_typing.rs crates/gate/src/page_text.rs crates/gate/src/search.rs crates/gate/src/shots.rs
git grep -n -A 15 -F 'if mode == Mode::Ui {' -- crates/gate/src/all.rs
git grep -n -F -e 'let with_engines' -e 'let mut c = gate(step.stage);' -- crates/gate/src/all.rs
git grep -n -F -e 'pub fn require_fresh_exe(' -e 'static DEP_INFO_HOSTS' -- crates/gate/src/fresh.rs
date +%F
```
Expected: every grep prints at least one line. Fill the table (the HEAD column is for orientation only; Tasks 2-4
shift most of these):

| Marker | Where | The line that holds | HEAD |
|---|---|---|---|
| `LINE-ENSURE-COPY` | `process_manager.rs` | `fn ensure_host_copy(` | 604 |
| `LINE-SHIPPED` | `process_manager.rs` | `fn shipped_cef_host(` | 641 |
| `LINE-PREPARE-CEF` | `process_manager.rs` | `fn prepare_cef(` | 997 |
| `LINE-CACHE-DIR` | `process_manager.rs` | `("VIEW_CACHE_DIR",` | 1004 |
| `LINE-WV2-PROFILE` | `process_manager.rs` | `("VIEW_WEBVIEW2_PROFILE",` | 1019 |
| `LINE-CUSTOM-FILTER` | `process_manager.rs` | `let custom_cef = custom_cef.filter(` | 1054 |
| `LINE-TAB-ID` | `process_manager.rs` | `.env("VIEW_TAB_ID",` | 1094 |
| `LINE-VIEWDESC-HOST` | `process_manager.rs` | the `host: &'static ...` field inside `pub(crate) struct ViewDesc` (if the grep prints more than one `host: &'static` line, take the one in the struct) | new |
| `LINE-RUN-IN-PLACE` | `availability.rs` | `fn run_in_place(` | 56 |
| `LINE-RUNTIME-ON-DISK` | `availability.rs` | `fn runtime_on_disk(` | 163 |
| `LINE-HAS-RUNTIME` | `view_install.rs` | `fn has_runtime(` | 139 |
| `LINE-IN-PLACE` | `crates/manifold-hosts/src/lib.rs` | `pub fn in_place(` | new |
| `LINE-WEBKIT-NAMES` | `process_manager/tests.rs` | the `assert_eq!(names, [...])` of WebKit's names | 44 |
| `LINE-CI-CLIPPY` | `ci.yml` | the first line (`cargo clippy --locked`) | 16 |
| `LINE-CI-TEST` | `ci.yml` | the first `cargo test --locked` line (Windows) | 17 |
| `LINE-CI-LINUX` | `ci.yml` | the second `cargo test --locked` line (`# catches cfg(windows) leaks`) | 24 |
| `LINE-HOOK` | `.githooks/pre-commit` | `cargo clippy` | 7 |
| `LINE-README-CI` | `README.md` | `**CI** (` | 35 (36 after Task 1) |
| `LINE-ALL-DOC` | `all.rs` | the `//! Build first (` module-doc line | 20 |
| `LINE-MAIN-DOC` | `main.rs` | the `//!` module-doc line | 8 |
| `LINE-MAIN-BUILD` | `main.rs` | the `not found: cargo build` message line | 246 |
| `LINE-SLEEP` | `sleep.rs` | its `Build first:` line | 20 |
| `LINE-CLICK` | `click_count.rs` | its build line | 4 |
| `LINE-IFRAME` | `iframe_typing.rs` | its build line | 4 |
| `LINE-PAGE-TEXT` | `page_text.rs` | its build line | 4 |
| `LINE-SEARCH` | `search.rs` | its build line | 4 |
| `LINE-SHOTS` | `shots.rs` | its `Build first:` line | 50 |
| `LINE-UNIT-START` | `all.rs` | the `c.args([` line in the `-A 15` output | 289 |
| `LINE-UNIT-END` | `all.rs` | the `]);` that closes it | 301 |
| `LINE-WITH-ENGINES` | `all.rs` | `let with_engines` | 307 |
| `LINE-STEP-GATE` | `all.rs` | `let mut c = gate(step.stage);` | 316 |
| `LINE-FRESH-FN` | `fresh.rs` | `pub fn require_fresh_exe(` | 49 |
| `LINE-FRESH-HOSTS` | `fresh.rs` | `static DEP_INFO_HOSTS` (the host list, Task 4) | new |
| `DATE-TODAY` | | the `date +%F` output | |

Also confirm the facts Steps 9-10 keep:
- the Windows `cargo test --locked` line (`LINE-CI-TEST`) and the `-A 15` unit-stage output name no
  `manifold-view-pdfium` ("some not visibly" in the `-p` entry);
- the unit stage still sits under `if mode == Mode::Ui {` and does not list `manifold-hosts` (spec "Tests": it runs
  only at tier `ui`);
- the cited lines below, in files no task touches (no host crate depends on the leaf, H1), are unchanged:

```bash
git grep -n -F -e 'std::env::var("VIEW_CACHE_DIR")' -e 'std::env::var("VIEW_WEBVIEW2_PROFILE")' -e '"MANIFOLD_CEF_VERBOSE"' -- crates/manifold-view-cef/src crates/manifold-view-webview2/src
git grep -n -F -e '("VIEW_INITIAL_URL_HEX"' -e '("VIEW_TCP_PORT"' -e '("VIEW_TOKEN"' -e 'VIEW_HOST_IP=$(' -e '.env("VIEW_HOST_IP"' -- crates/manifold/src/process_manager/webkit.rs
git grep -n -F 'pub const ENV_' -- crates/manifold-view-protocol/src/host_env.rs
git grep -n -F 'std::env::var("VIEW_' -- crates/manifold-view-webkitgtk/src/main.rs
```
Expected: `manifold-view-cef/src/app.rs:45`, `manifold-view-cef/src/main.rs:150`, `manifold-view-webview2/src/main.rs:689`;
`webkit.rs:47`, `:48`, `:49`, `:68`, `:120`; `host_env.rs:13`, `:15`, `:17`, `:20`; `webkitgtk/src/main.rs:38`, `:43`,
`:52`, `:62`. If any differs, use the new number in Step 10's entry in place of the one written there.

- [ ] **Step 7: Delete the three PARKED entries the host table closes**

Delete these whole lines from `PARKED.md` (each is one list item; the items around them stay):
- the line starting ``- **GATE-CEF-DIR.** The `gate` crate spells `cef-runtime` in five files`` (HEAD `:61`, section
  "Architecture survey 2026-10-19"; it ends `*Status:* held for "One table per host".`);
- the line starting `- **One table per host.** *What:* each host's exe name, runtime files` (HEAD `:157`, section "Host
  protocol stamp (2026-10-29)");
- the line starting `- **Servo's DLLs come from the newest mozangle output in either target folder.**` (HEAD `:158`, same
  section).

None of the three is referred to from anywhere else (checked at HEAD: only each other and the spec name them), and
GATE-CEF-DIR's survey section intro names no item by id.

Run: `grep -c -F -e 'GATE-CEF-DIR' -e 'One table per host' -e "Servo's DLLs come from the newest mozangle output" PARKED.md`
Expected: `0`

Run: `git grep -n -F -e 'GATE-CEF-DIR' -e 'One table per host' -- ':!docs/superpowers'`
Expected: no output (nothing else refers to the deleted entries).

- [ ] **Step 8: PARKED's family-query entry reads against `ViewDesc.host`**

The split the entry anticipated has partly happened (`ViewDesc.host` points to one host-table row); a macOS spike is
still its trigger, so it stays, reworded. Replace the whole line (HEAD `:87`) starting
`- **One "WebKit" choice across OSes (family query).**` with (markers from Step 6):
```markdown
- **One "WebKit" choice across OSes (family query).** A tab stores a host key (`webkit`); a macOS WKWebView host would need `ViewKind::Webkit` to resolve a host per OS (the `WebKit` family tag, `HostRuntime::between`), so its `ViewDesc.host`, today the one row `manifold_hosts::WEBKIT`, would become a choice among host-table rows by the OS manifold runs on. *Why parked:* no second WebKit host exists and the shape of that choice is guesswork before a macOS spike. *Trigger:* starting a macOS host spike. *Evidence:* `docs/decisions/2026-10-23-host-runtimes.md` "Host tags", read 2026-10-23; `crates/manifold/src/process_manager.rs:LINE-VIEWDESC-HOST` (`ViewDesc.host`), verified DATE-TODAY. *Status:* parked.
```

Run: ``grep -c -F 'split `ViewDesc` into engine and host parts' PARKED.md``
Expected: `0`

- [ ] **Step 9: Re-cite "Hosts outside the in-place lookup are not stamp-checked"**

`in_place_paths` is deleted; the lookup is the leaf's `Host::in_place`, read through `run_in_place`. In the line
starting `- **Hosts outside the in-place lookup are not stamp-checked.**` (HEAD `:156`), replace:
```text
*Evidence:* `crates/manifold/src/process_manager.rs:997` (`prepare_cef`), `:641` (`shipped_cef_host`) and `:604` (`ensure_host_copy`), and `crates/manifold/src/process_manager/availability.rs:42` (`in_place_paths`, the lookup the check reads), verified 2026-10-29. *Status:* decided against.
```
with:
```text
*Evidence:* `crates/manifold/src/process_manager.rs:LINE-PREPARE-CEF` (`prepare_cef`), `:LINE-SHIPPED` (`shipped_cef_host`) and `:LINE-ENSURE-COPY` (`ensure_host_copy`), and `crates/manifold-hosts/src/lib.rs:LINE-IN-PLACE` (`Host::in_place`, the lookup the check reads, through `run_in_place` at `crates/manifold/src/process_manager/availability.rs:LINE-RUN-IN-PLACE`), verified DATE-TODAY. *Status:* decided against.
```
The entry's *What*, *Why parked*, *Trigger* and *Fix* still hold after Task 2 (the app reads the stamp only from the host
`run_in_place` finds; `prepare_cef` still copies `shipped_cef_host()` into a custom folder); read them once against
the code at the lines just cited before writing `verified DATE-TODAY`. If one no longer holds, stop and report it.

Other PARKED entries cite files this branch changes (Tasks 2, 3 and 6 move their lines), and several of the numbers were already stale at HEAD. Locate each by its parenthetical label, never by the old number: the old number can point at the wrong code (HEAD `process_manager.rs:355` is `set_visible` and `:1028` is a doc comment; the labelled code is at `:364` and `:1041`). Per entry, run the Grep, read the line it finds, and replace only the number in the entry's `file:NNN` with that line:
- `- **Two hosts can name the same file.**` (HEAD `:129`): `process_manager.rs:1028` (`spawn_tab`): `grep -n 'pub fn spawn_tab(' crates/manifold/src/process_manager.rs`.
- `- **The Settings page reads the disk on the UI thread.**` (HEAD `:133`): two citations. `view_install.rs:1020` (`cef_dir_verdict`): `grep -n 'fn cef_dir_verdict(' crates/manifold/src/view_install.rs`. `settings_page.rs:805` (`cached`; Task 2 Step 18 adds a line above it, and the old number pointed at the doc comment): `grep -n 'fn cached<' crates/manifold/src/settings_page.rs`, and write the `fn` line.
- `` - **WSL drives are assumed at `/mnt`.** `` (HEAD `:135`): `runtime.rs:99` (`host_path`) and `:116` (`app_path`): `grep -n 'fn host_path(\|fn app_path(' crates/manifold/src/process_manager/runtime.rs`.
- `- **A settings message refused by a full send queue counts as sent.**` (HEAD `:137`): `process_manager.rs:355` (`set_settings`): `grep -n 'pub fn set_settings(' crates/manifold/src/process_manager.rs`; its `view_io.rs:134` (`push`) is untouched, so leave it unless `grep -n 'fn push(' crates/manifold/src/process_manager/view_io.rs` finds it elsewhere.
- `- **A renderer that writes a bad frame and then hangs holds a worker.**` (HEAD `:154`): `pdf_render.rs:18` (`WORKERS = 2`): `grep -n 'const WORKERS' crates/manifold/src/pdf_render.rs`.
- `- **Opt-in chrome-shots views reach a card by a fixed Tab count, and the counts are stale.**` (HEAD `:147`): its `shots.rs` citations (for example `shots.rs:1017`, `note-key`): Task 4 edits `crates/gate/src/shots.rs` at the top and from `:1590`, so re-run `grep -n 'self.view("note-key"' crates/gate/src/shots.rs` and fix any number that moved. The entry cites eleven `shots.rs` lines (`:1042`, `:1048`, `:942`, `:953`, `:986`, `:1214`, `:460`, `:593`, `:594`, `:610` and `:1017`). Task 4 Step 12 adds one line at HEAD `:58` and one at `:73`-`75`, both above `:460`, and edits nothing between `:460` and `:1214`, so every one of the eleven moves by exactly +2 (`:1017` to `:1019`, `:460` to `:462`, and so on). Check the +2 against the labelled code, not only `note-key`.

Read each line the Grep finds against the entry's label before writing the number. If a labelled item is gone, stop and report it.

Run: `grep -c -F 'in_place_paths' PARKED.md`
Expected: `0`

- [ ] **Step 10: Check and refresh the "Host table (2026-10-29)" section**

The section's intro line (`Left out of the host table by decision (spec ..., H13 and "Weighed and not taken").`)
stays. Each of its four entries is replaced as a whole line (found by its start), with Step 6's values for the markers.
What changes besides the numbers:
- "The app's runtime code is CEF's": the final `Runtime` also names where a dev build's runtime comes from
  (`Runtime.from`, which the gate's `sync_host` reads for any host); the app code behind `runtime.is_some()` is
  still CEF's.
- "The `-p` crate lists are repeated": the spec's added evidence (README's CI line, `main.rs:8` and `:246`, `sleep.rs:20`
  and the stage module docs), `fresh.rs`'s hint now built from a list of rows (Task 4), and the claim that the leaf was
  added "to each" list corrected: the gate's unit stage runs only at tier `ui` and does not list it (spec "Tests").
- The other two: line numbers only.

Replace the line starting `- **The app's runtime code is CEF's.**` (HEAD `:165`) with:
```markdown
- **The app's runtime code is CEF's.** *What:* the table's `Runtime` names a host's runtime folder, its marker file and where a dev build's runtime comes from, for any host, but the app's code behind `runtime.is_some()` is CEF's: the custom ("Use your own") folder and the host copied into it (`prepare_cef`, `host_home`, `ensure_host_copy`, `shipped_cef_host`), the runtime's download state read from CEF's pin (`runtime_on_disk` reads `cef_pin::pin_for_platform()`), and the archive layout (`cef_platform`). *Why parked:* by decision: only CEF has a runtime, and generalizing that code for one host would be speculative. *Trigger:* a second host that needs a runtime folder. *Fix:* the pin, the archive layout and the custom-folder copy keyed on the row's `Runtime`, with CEF's facts as its data. *Evidence:* `crates/manifold/src/process_manager.rs:LINE-PREPARE-CEF` (`prepare_cef`), `:LINE-ENSURE-COPY` (`ensure_host_copy`), `:LINE-SHIPPED` (`shipped_cef_host`), `:LINE-CUSTOM-FILTER` (`spawn_tab`'s custom-folder filter), `crates/manifold/src/process_manager/availability.rs:LINE-RUNTIME-ON-DISK` (`runtime_on_disk`), `crates/manifold/src/view_install.rs:LINE-HAS-RUNTIME` (`has_runtime`), verified DATE-TODAY. *Status:* parked.
```

Replace the line starting ``- **The `-p` crate lists are repeated.**`` (HEAD `:166`) with:
```markdown
- **The `-p` crate lists are repeated.** *What:* the crates that build on Windows without the CEF download or Servo are listed by hand in `.github/workflows/ci.yml:LINE-CI-CLIPPY`, `:LINE-CI-TEST` and (the Linux subset) `:LINE-CI-LINUX`, `.githooks/pre-commit:LINE-HOOK`, CLAUDE.md "Formatting and lints", README's CI paragraph (`README.md:LINE-README-CI`, the Linux subset again) and `gate all`'s unit stage (`crates/gate/src/all.rs:LINE-UNIT-START`-`LINE-UNIT-END`); the gate's build line (`-p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate`) is spelled in `all.rs:LINE-ALL-DOC`, the gate's own module doc and not-found message (`crates/gate/src/main.rs:LINE-MAIN-DOC`, `:LINE-MAIN-BUILD`), CLAUDE.md "Gate" and the README, and its hosts are a list of host-table rows in `fresh.rs:LINE-FRESH-HOSTS`, from which the stale-build hint is built; other build lines sit in stage module docs (`sleep.rs:LINE-SLEEP`, `click_count.rs:LINE-CLICK`, `iframe_typing.rs:LINE-IFRAME`, `page_text.rs:LINE-PAGE-TEXT`, `search.rs:LINE-SEARCH`, `shots.rs:LINE-SHOTS`). The lists differ, some on purpose (Linux builds neither Windows host) and some not visibly (`ci.yml:LINE-CI-TEST` and the unit stage test no `manifold-view-pdfium`). *Why parked:* by decision: these are tool invocations, not host layout, so outside the host table, whose crate `manifold-hosts` was added to them by hand (`ci.yml`'s three lines, the hook, CLAUDE.md's clippy command, README's Linux line); the gate's unit stage runs only at tier `ui`, which a change to the table never is, so it does not list it. *Trigger:* the next crate added or removed, or a list found to have drifted. *Fix:* one name per set, such as cargo aliases in `.cargo/config.toml` (`[alias]`) that CI, the hook and the gate call. *Evidence:* the lines above, verified DATE-TODAY. *Status:* parked.
```

Replace the line starting `- **Per-host environment variable names are spelled on both sides.**` (HEAD `:167`) with
(the host-crate and `webkit.rs` numbers are the ones Step 6 confirmed):
```markdown
- **Per-host environment variable names are spelled on both sides.** *What:* the variables shared by every host live in `manifold-view-protocol::host_env` (`crates/manifold-view-protocol/src/host_env.rs:13`-`20`), but each host's own are string literals in the app and again in the host: `VIEW_CACHE_DIR` (`crates/manifold/src/process_manager.rs:LINE-CACHE-DIR`, `crates/manifold-view-cef/src/main.rs:150`), `VIEW_WEBVIEW2_PROFILE` (`process_manager.rs:LINE-WV2-PROFILE`, `crates/manifold-view-webview2/src/main.rs:689`), and WebKit's `VIEW_INITIAL_URL_HEX`, `VIEW_TCP_PORT`, `VIEW_TOKEN` and `VIEW_HOST_IP` (`crates/manifold/src/process_manager/webkit.rs:47`-`49`, `:68`, `:120`; `crates/manifold-view-webkitgtk/src/main.rs:38`-`62`). `VIEW_TAB_ID` (`process_manager.rs:LINE-TAB-ID`) is set and read by no host. `MANIFOLD_CEF_VERBOSE` (`crates/manifold-view-cef/src/app.rs:45`) is left out: a developer switch only the host reads. *Why parked:* by decision: not host layout; a test pins the app's WebKit names (`process_manager/tests.rs:LINE-WEBKIT-NAMES`). *Trigger:* a host failing on a renamed variable, or a new per-host variable. *Fix:* constants in `host_env` beside the shared ones, read by both sides; `VIEW_TAB_ID` deleted or given a reader. *Evidence:* the lines above, verified DATE-TODAY. *Status:* parked.
```

Replace the line starting `- **Fresh-host check for opt-in engines.**` (HEAD `:168`) with:
```markdown
- **Fresh-host check for opt-in engines.** *What:* `gate all` refuses a stale `manifold.exe`, WebView2 host, PDF view or CEF runtime copy (`fresh.rs`), but not a Servo or WebKit host older than its sources, so a `WITH_ENGINES=servo` run can test an old Servo host silently (the protocol stamp catches only a protocol change). *Why parked:* by decision: checking every built host would fail every gate run after a `manifold-pages` or protocol edit until Servo is rebuilt (about 33 minutes), though most runs start no Servo tab. *Trigger:* a `WITH_ENGINES` run found to have passed on a stale host. *Fix:* `gate all` checks the dep-info of each engine in `WITH_ENGINES` (`by_id(engine)`, the `<exe>.d` cargo writes beside the exe in its dev folder, e.g. `target-servo/debug/manifold-view-servo.d`) before running its steps. *Evidence:* `crates/gate/src/fresh.rs:LINE-FRESH-FN` (`require_fresh_exe`), `crates/gate/src/all.rs:LINE-WITH-ENGINES`-`LINE-STEP-GATE`, verified DATE-TODAY. *Status:* parked.
```
Before writing it, confirm the *What* still holds after Task 4: `require_fresh_exe` checks `manifold.exe`, the hosts in
`DEP_INFO_HOSTS` (WebView2, PDFium) and each runtime copy (CEF's), and no Servo or WebKit dep-info.

- [ ] **Step 11: Check PARKED's edits**

Run:
```bash
grep -c -e 'LINE-' -e 'DATE-TODAY' PARKED.md
git show HEAD:PARKED.md | grep -c '^- \*\*'
grep -c '^- \*\*' PARKED.md
git diff --stat -- PARKED.md
```
Expected: the first command prints `0` (no marker left); the third prints the second minus 3 (three entries deleted,
none added); `--stat` lists one file. Then open each `LINE-` value just written for the entries of Steps 8-10 against Step 6's table (the
numbers in the file must equal the table's, entry by entry).

- [ ] **Step 12: The deleted-identifier grep (spec "Docs", the manual half)**

`deleted_names.rs`'s second test (Task 2) already fails on any app-crate needle anywhere in `crates/manifold/src`,
comments included. This grep covers the rest of the tree: the gate, the workflows, `crates/manifold/build.rs` and the
docs, with the gate's needles (`find_newest`, `sync_servo`, `sync_cef`, `embedded-host`, `embedded-pdf-view`,
`manifold-view-{` for `sleep.rs:87`) added.

Run:
```bash
git grep -n -F -e HostSource -e 'Embedded::' -e PLATFORM.host_exe -e PLATFORM.exe_suffix -e PLATFORM.runtime_lib -e CefPlatform::runtime_lib -e beside_exe -e in_place_paths -e sibling_binary -e 'host_file_name(' -e ViewDesc::dev_dir -e MANIFOLD_WEBVIEW2_SHA256 -e MANIFOLD_PDFIUM_SHA256 -e find_newest -e sync_servo -e sync_cef -e embedded-host -e embedded-pdf-view -e 'manifold-view-{' -- . ':!docs/superpowers' ':!CHANGELOG.md' ':!crates/manifold/src/deleted_names.rs'
```
Expected: no output. (`CHANGELOG.md` is history; `docs/superpowers` holds the plans and specs that name the old items
on purpose; `.superpowers/` is untracked, so `git grep` skips it.)

On a hit:
- A hit under `crates/manifold/src/` is Task 2's: for an app needle (all but the last six) it means Task 2's guard is
  not passing. Stop and report it. (At HEAD the last six occur only in the gate, `release.yml`, README "Releases" and
  the three PARKED entries Step 7 deletes.)
- A hit in code (a line that does not start with `//` after its indentation, in a non-`.md` file) is an item its task
  should have deleted: stop and report it to that task's owner; do not fix code in this task. Owners:
  `crates/manifold/**` Task 2; `crates/gate/src/sync_deps.rs` Task 3; `crates/gate/src/{fresh,package,sleep,all,jar,probes,shots,stress}.rs`
  Task 4; `crates/gate/src/release_hosts.rs` Task 5; `crates/gate/src/release_verify.rs`, `.github/workflows/release.yml`
  Task 6.
- A hit in README's "Releases" bullets (`- Packaging:`, `- Embedded PDF view:`, `- Embedded WebView2 host:`) is
  Task 6's: stop and report it.
- A hit in a comment at a site in Step 13's table is its owner's (that table names it): stop and report it.
- Any other hit in a comment or a doc is fixed here, replacing only the stale name by its successor (this table also
  serves Step 15):

| Stale name | Successor |
|---|---|
| `HostSource::Download` / `HostSource::Embedded` | `Delivery::Download` / `Delivery::Embedded` (the row's `delivery`) |
| `Embedded::ensure` | `embedded_host::ensure` |
| `Embedded::Webview2` / `Embedded::Pdfium` | the row: `manifold_hosts::WEBVIEW2` / `PDFIUM` |
| `PLATFORM.host_exe` | `Host::file_name` |
| `PLATFORM.exe_suffix` | `Os::exe_suffix` |
| `PLATFORM.runtime_lib`, `CefPlatform::runtime_lib` | `CEF.runtime_lib(Os::here())` (`Host::runtime_lib`) |
| `beside_exe` | `Host::beside` |
| `in_place_paths` | `Host::in_place` |
| `sibling_binary` | `run_in_place` (the PDF view) or `sibling_host` (CEF's flat host) |
| `host_file_name(` | `Host::file_name(` |
| `ViewDesc::dev_dir` | `Host::dev_dir` |
| `MANIFOLD_WEBVIEW2_SHA256`, `MANIFOLD_PDFIUM_SHA256` | the hash literal in the generated `OUT_DIR/embedded.rs` |
| `find_newest` | `find_first` |
| `sync_servo`, `sync_cef` | `sync_host` |
| `embedded-host` | `embedded-webview2` |
| `embedded-pdf-view` | `embedded-pdfium` |
| `manifold-view-{` | the row's `file_name(Os::here())` through `by_id` |
| `ViewDesc::host_os`, `desc().host_os` | `Host.os`, `desc().host.os` (resolved by `Host::built_for`) |
| `ViewDesc::runtime`, `desc().runtime_dir`, `desc.runtime_dir` | the row's `runtime`: `desc().host.runtime`, `desc.host.runtime` |
| `runtime::Os` | `manifold_hosts::Os` |
| `desc().name` | `desc().host.id` |
| `desc().binary` | `desc().host.exe` |
| `desc().source` | `desc().host.delivery` |
| `SERVO_DLLS`, `SERVO_EXE` | `manifold_hosts::SERVO`'s `sidecars` and `exe` |

Re-run the grep after any fix. Expected: no output.

- [ ] **Step 13: The spec's known hits are gone**

The spec ("Docs") lists the doc comments on items this work deletes or rewrites. Each is rewritten by the task that
changes its item, in that task's commit; this step only checks that none of HEAD's text is left. Those under
`crates/manifold/src/` that contain an app needle are also forced by Task 2's guard; the rest contain no needle and are
found here by their HEAD text.

Run:
```bash
git grep -n -F -e 'everything that differs between engines lives -- name, store code, binary,' -e 'ENGINES[code - 1]' -e 'desc().name' -e 'ViewDesc::runtime' -e '`Os` is the operating systems an engine host can target' -e 'MANIFOLD_<NAME>_SHA256' -e 'embedded WebView2 host and PDF view copies in DIST/embedded' -e "The Servo list is the manifest's three files" -e 'newest mozangle output' -- crates
git grep -n -E 'pub\(crate\) (name|binary|source|host_os|runtime_dir|dev_dir):' -- crates/manifold/src/process_manager.rs
git grep -n -F -e 'enum Embedded' -e 'fn host_path(' -- crates/manifold/src/embedded_host.rs
git grep -n -F -e 'pub(crate) const CEF_RUNTIME_DIR' -e 'enum HostSource' -- crates/manifold/src/process_manager.rs
```
Expected: no output from any of the four. A hit is a site its owner left: stop and report it, with the line, to the
owner the table names. Do not rewrite it here: the owner's step holds that site's new text, and a second version
written here would drift from it.

| Site (HEAD) | HEAD text | Owner (the step that rewrites it) |
|---|---|---|
| `process_manager.rs:36`-`49` | `HostSource`, `CEF_RUNTIME_DIR` and their docs | Task 2, Part A: deleting `HostSource` and `CEF_RUNTIME_DIR`, `ViewDesc` gets `host` |
| `process_manager.rs:51`-`53`, `:58`, `:71`, `:76` | the `ViewDesc` doc ("name, store code, binary, ..."), `code`'s doc (`ENGINES[code - 1]`), the `runtime_dir` and `dev_dir` field docs | Task 2, Part A: the same step |
| `process_manager.rs:1213`, `:1218` | `pdf_view_exe`'s `sibling_binary` and `Embedded::Pdfium` | Task 2, Part B: `pdf_view_exe` |
| `host_pin.rs:12`-`13` | `ViewKind::desc().name` | Task 2, Part A: `host_pin.rs` |
| `view_install.rs:132`-`133`, `:138`, `:318` | `HostSource::Download`, `ViewDesc::runtime`, `CefPlatform::runtime_lib` | Task 2, Part A: `downloads`, `has_runtime`, `validate_custom` |
| `availability.rs:6`-`55` | the docs of `sibling_binary`, `host_file_name`, `beside_exe`, `in_place_paths` and `run_in_place` | Task 2, Part A: the in-place lookup is `Host::in_place` |
| `availability.rs:76`-`80`, `:92` | `view_binary_path`'s doc, and the `Embedded::ensure` comment in its body | Task 2, Part A: the install path reads the row |
| `runtime.rs:3`-`6` | "`Os` is the operating systems an engine host can target" (not in the spec's list; `Os` moves, H11) | Task 2, Part A: `Os` is the leaf's |
| `names/tests.rs:470`, `:477`, `:495` | the names guard's `desc().name` exemption (H3) | Task 2, Part B: the names guard |
| `embedded_host.rs:1`-`4`, `:12`-`13`, `:50`-`52`, `:68` | the module doc, the docs of `enum Embedded` and `host_path`, `ensure_with`'s `Embedded::ensure` | Task 2, Part B: the embedded codegen |
| `crates/manifold/build.rs:1`-`6` | `MANIFOLD_<NAME>_SHA256` | Task 2, Part B: the embedded codegen (H7) |
| `process_manager/tests.rs:550` | `ViewDesc::dev_dir` | Task 2, Part A: the lookup test over host rows |
| `crates/gate/src/sync_deps.rs:1`-`16` | "newest mozangle output" | Task 3: writing `sync_host`, `find_first` and `run_in` |
| `crates/gate/src/package.rs:129`-`130` | "The Servo list is the manifest's three files" | Task 4: install-check's hosts and files from one filter |
| `crates/gate/src/release_verify.rs:2`-`6` | "embedded WebView2 host and PDF view copies in DIST/embedded" | Task 6: `release_verify` |

A `desc().name` hit in code anywhere under `crates/manifold/` (a read of the deleted field) is Task 2's too.

- [ ] **Step 14: The two other docs that name the nonexistent `store::model::ENGINES`**

Task 2 renames `ENGINES` to `VIEW_KINDS` in `code`'s doc (spec "The app"); the same stale name, left by the
engine-to-view rename (`37bfb906`), sits in two more doc comments the spec does not list and no other task touches.
They are fixed here so one grep finds none (`-w` keeps out `WITH_ENGINES`; at HEAD the grep prints exactly these two
lines and `code`'s doc).

In `crates/manifold/src/session_sync.rs` (HEAD `:121`, `view_code`'s doc), replace:
```rust
/// `store::model::ENGINES` order: code `n` is `ENGINES[n - 1]`.
```
with:
```rust
/// `store::model::VIEW_KINDS` order: code `n` is `VIEW_KINDS[n - 1]`.
```

In `crates/manifold/src/store/model.rs` (HEAD `:227`, `VIEW_KINDS`' own doc), replace:
```rust
/// Engine codes for the `view_kind` field: code `n` is `ENGINES[n - 1]`,
```
with:
```rust
/// Engine codes for the `view_kind` field: code `n` is `VIEW_KINDS[n - 1]`,
```

Run: `git grep -n -w -F 'ENGINES' -- crates`
Expected: no output.

- [ ] **Step 15: The docs check for stale claims**

Every claim this work changed (where `Os` lives, the `ViewDesc` fields, the sync's search order, the record names, the
three closed PARKED entries), grepped across the docs and code comments.

Run:
```bash
git grep -n -F -e 'ViewDesc::host_os' -e 'ViewDesc::runtime' -e 'runtime::Os' -e 'desc().name' -e 'desc().binary' -e 'desc().source' -e 'desc().host_os' -e 'desc().runtime_dir' -e 'desc.runtime_dir' -e 'SERVO_DLLS' -e 'SERVO_EXE' -e 'newest mozangle' -e 'takes the newest of either' -e 'hosts.yml' -e 'GATE-CEF-DIR' -e 'One table per host' -- ':!docs/superpowers' ':!CHANGELOG.md' ':!crates/manifold/src/deleted_names.rs'
git grep -n -F 'const CEF_RUNTIME_DIR' -- ':!docs/superpowers'
git grep -n -F -e 'ViewDesc' -e 'sync-runtime-deps' -e 'HostSource' -e 'embedded-host' -- ROADMAP.md CLAUDE.md docs/superpowers/specs/2026-10-24-properties-design.md docs/superpowers/plans/2026-10-28-download-folder-setting.md
```
Expected: the first prints nothing; the second prints exactly one line, in `crates/manifold-hosts/src/lib.rs`; the third
prints nothing (ROADMAP, CLAUDE.md and the other two `draft`/`active` documents name none of these; `NOTES.md` does
not exist). For the first grep's hits, the rules are Step 12's: code, or a comment at a site in Step 13's table, is its
owner's (`SERVO_DLLS` and `SERVO_EXE` are Task 3's; the `desc()` and `runtime::Os` needles under `crates/manifold/`
are Task 2's), and a hit in README "Releases" (`hosts.yml`) is Task 6's: stop and report it. Any other hit in a
comment or a doc: replace the stale name by its successor in Step 12's table.

Read once, for the close-out's docs check (no edit expected): README "Which host runs" (HEAD `:33`), the opening of
"Runtime-file placement" (`:82`), "Why this needs a script at all" (`:92`), the prune paragraph (`:94`), "Engine setup
for the gate" (HEAD `:500`-`501`) and the Servo build block (`:66`-`70`). Each still holds: the in-place order and the
names in `availability.rs` are unchanged (Step 1), the newest-file pick still applies within a build folder, and the
sync still puts Servo's DLLs beside `target-servo/debug/manifold-view-servo.exe` and CEF's host and runtime in
`target/debug/cef-runtime/`. If one does not, stop and report it.

- [ ] **Step 16: Check that only comments changed in Rust files**

Run:
```bash
git diff --name-only -- '*.rs'
git diff -U0 -- '*.rs' | grep -E '^[-+]' | grep -v -E '^(\+\+\+|---) ' | grep -v -E '^[-+][[:space:]]*//'
cargo fmt --all --check
```
Expected: the first lists `crates/manifold/src/session_sync.rs`, `crates/manifold/src/store/model.rs` and any file
Step 12 or Step 15 fixed; the second prints nothing (every changed line is a comment); the third prints nothing and
exits 0.

- [ ] **Step 17: Commit**

Run:
```bash
cargo fmt --all
git diff --stat
```
Expected: `README.md`, `PARKED.md`, `docs/decisions/2026-10-23-host-runtimes.md`,
`docs/decisions/2026-10-18-hosts-in-the-release.md`, `crates/manifold/src/session_sync.rs`,
`crates/manifold/src/store/model.rs`, and only the files Step 12 or Step 15 fixed; no `Cargo.lock`, no `CHANGELOG.md`,
no spec or plan file.

```bash
git add README.md PARKED.md docs/decisions/2026-10-23-host-runtimes.md docs/decisions/2026-10-18-hosts-in-the-release.md
git add crates/manifold/src/session_sync.rs crates/manifold/src/store/model.rs
git status --short
```
Add each other file Step 12 or Step 15 fixed by name too (the `git diff --stat` above lists them). Expected: every changed file staged (`M` in the first column), nothing in the second column of a tracked file. A `??`
line is a file this task did not create (it creates none): stop and report it.

```bash
git commit -F - <<'EOF'
docs: host table: README sync rule and engine switch, PARKED, decision records

README "Runtime-file placement" describes the one sync rule (the host's own build folder first, a missing folder
skips its host, a part already in place is kept, a CEF_PATH build without a runtime is an error) and "Engines by OS"
names Host.os. PARKED loses GATE-CEF-DIR, "One table per host" and "Servo's DLLs come from the newest mozangle output
in either target folder", re-cites the in-place lookup to Host::in_place and refreshes the Host table section's
evidence. The 2026-10-23 and 2026-10-18 decision records note where Os and the manifest writer live now. Two doc
comments name store::model::VIEW_KINDS in place of the nonexistent ENGINES.

<attribution>
EOF
```
If Step 12 or Step 15 fixed a comment or doc, add one sentence before `<attribution>` naming each file and the stale
name replaced.
The pre-commit hook runs `cargo fmt --all --check` and the clippy line when Rust files are staged; never pass
`--no-verify`. If it fails, fix the cause, re-stage and commit again.

Run: `git log -1 --format=%B | grep -v '^$' | tail -n 1`
Expected: the last attribution line.

Run: `git status --short --untracked-files=no`
Expected: no output (the tree is clean apart from untracked files that predate this task). Nothing is pushed, tagged
or bumped.

### Task 8: Final checks and the full gate

**Files:**
- Modify: `docs/runs.md` (the `sync-after` and `gate-host-table` rows, and the `sleep-history-rerun-table` row if Step 12 runs it).
- Modify: `docs/superpowers/specs/2026-10-29-host-table-design.md` and `docs/superpowers/plans/2026-10-29-host-table.md` (front matter `status: done`).
- Fix commits from Step 9's review touch whichever files the findings name.

**Interfaces:**
- Consumes:
  - B and the `sync-before` run (Start, Steps S2 and S3).
  - From Task 1: `-p manifold-hosts` in CLAUDE.md's clippy command, `.githooks/pre-commit:7`, `ci.yml:16`, `:17` and `:24`.
  - From Task 2:
    - `deleted_names.rs`'s second `#[test]`;
    - `build.rs` writing `OUT_DIR/<id>-host.bin` for each `Embedded` row from `Host::embed_var()`;
    - `the_embedded_hosts_extract_to_their_pinned_hashes` looping the `Embedded` rows.
  - From Task 3: `gate sync-runtime-deps` over `run_in`.
  - From Task 5: `gate release-embedded vX.Y.Z DIST`, which:
    - prints one `<embed_var>=<absolute path>` line per `Embedded` row;
    - copies each host to `DIST/embedded/<file>`;
    - writes `DIST/manifold-<TAG>-<platform>.embedded-<id>.sha256` (`<hash>  <file>`).
  - From Tasks 6 and 7: the README, PARKED and decision-record edits.
- Produces:
  - the runs `sync-after` and `gate-host-table`;
  - the review outcome;
  - the spec and this plan marked `done`.

Order matters here:
- The final review and its fixes (Step 9) come before the sync and the gate, so the gate runs once on the final code.
- The sync comes before the gate, because the gate's start-up freshness check (`fresh.rs`) reads `cef-runtime/`'s copy of the CEF host.

- [ ] **Step 1: Check the start condition, cargo and the disk**

Run the start-condition `git status` line, `Get-Process cargo -ErrorAction SilentlyContinue` and `df -h /c | tail -n 1`.
Expected: no output from the first two, and at least 2 GB free on C:.

- [ ] **Step 2: Run the local checks**

Run: `cargo fmt --all --check`
Expected: no output, exit 0.

Run the clippy line (Global Constraints, "Commands").
Expected: `Finished`, and no `warning:` or `error` line.

Run CI's Windows test line (`ci.yml:17`, which holds `-p manifold-hosts` right after `-p manifold` once Task 1 lands; the `-p` order below differs, which changes nothing), with `-j 8` and the thread cap:

```bash
cargo test --locked -j 8 -p manifold -p manifold-view-protocol -p manifold-pages -p manifold-view-webview2 -p gate -p manifold-hosts -- --test-threads 8 2>&1 | grep -E '^test result|FAILED|panicked|^error'; echo "exit ${PIPESTATUS[0]}"
```

Expected:
- `exit 0`, and every `test result:` line reads `ok.`. The three crates this plan touches are `manifold-hosts`, `manifold` and `gate`; the other three are CI's own line.
- `--locked` finds no stale `Cargo.lock`. If it says the lock file needs to be updated:
  1. run the clippy line once (it has no `--locked`, so cargo updates the lock file);
  2. check that `git diff Cargo.lock` touches only the workspace crates' entries;
  3. commit it alone, as `build: Cargo.lock for manifold-hosts`, with the `<attribution>` trailer.

- [ ] **Step 3: Check that the `-p` lists agree**

```bash
for f in CLAUDE.md .githooks/pre-commit .github/workflows/ci.yml; do grep -m1 'cargo clippy' "$f" | grep -oE -- '-p [a-z0-9-]+' | sort | tr '\n' ' '; echo "<- $f"; done
grep -n 'cargo test' .github/workflows/ci.yml
```

Expected:
- The three lists before `<-` are identical and each holds `-p manifold-hosts`.
- Both `cargo test` lines in `ci.yml` (`:17` and `:24`) hold `-p manifold-hosts`.

The spec's Tooling section requires the three to change together, so they never disagree.

- [ ] **Step 4: Run the leaf's tests on Linux**

Check that `wsl.exe -d Ubuntu -e pgrep -af "[c]argo"` prints nothing. Then run the command under "Leaf tests on Linux (WSL)" with a 600000 ms timeout.

Expected:
- every `test result:` line reads `ok.`: the unit tests in `src/lib.rs` (the golden table and `Os`), `tests/layout.rs`, and the doc-tests;
- the layout test reads this checkout's `release.yml`, `.gitignore` and manifests through `/mnt/c`.

- [ ] **Step 5: Grep for deleted names**

These are the spec's "Docs" needles, with `manifold-view-{` for `sleep.rs:87`, over the whole tree: the same grep, with the same pathspec, that Task 7 Step 12 ran before this task. That run owned each hit and stopped; here no owner is left, so a hit is fixed where it stands (below). Excluded:
- `docs/superpowers` (old plans and specs; `.superpowers/` is untracked, so `git grep` skips it) and `CHANGELOG.md` (history);
- `crates/manifold/src/deleted_names.rs`, which lists the app-crate needles in its own test and skips itself when scanning.

```bash
git grep -n -F -e HostSource -e 'Embedded::' -e PLATFORM.host_exe -e PLATFORM.exe_suffix -e PLATFORM.runtime_lib -e CefPlatform::runtime_lib -e beside_exe -e in_place_paths -e sibling_binary -e 'host_file_name(' -e ViewDesc::dev_dir -e MANIFOLD_WEBVIEW2_SHA256 -e MANIFOLD_PDFIUM_SHA256 -e find_newest -e sync_servo -e sync_cef -e embedded-host -e embedded-pdf-view -e 'manifold-view-{' -- . ':!docs/superpowers' ':!CHANGELOG.md' ':!crates/manifold/src/deleted_names.rs'
```

Expected: no output (exit 1). At 10444ae5 it listed 118 matching lines in 15 files.

The second grep covers reads of the deleted fields and constants (H2, H11 and the spec's "Deleted" list) in the two crates that used them:

```bash
git grep -n -F -e 'desc().name' -e 'desc().binary' -e 'desc().source' -e 'desc().runtime_dir' -e 'desc().dev_dir' -e 'desc().host_os' -e 'runtime::Os' -e 'const CEF_RUNTIME_DIR' -e SERVO_DLLS -e SERVO_EXE -e '"engine-hosts"' -- crates/manifold crates/gate
```

Expected: no output (exit 1). The needles and what each catches:
- `runtime::Os` would catch a re-export left behind;
- `const CEF_RUNTIME_DIR` would catch the app's copy (the leaf's lives in `crates/manifold-hosts`);
- `"engine-hosts"` would catch a literal where `HOSTS_DIR` belongs. The gate's fixtures (`package.rs:168`, `:226`, `shots.rs:1590`) read `HOSTS_DIR` per the spec. `"data/engine-hosts"` (`process_manager/tests.rs:1090`) does not match.

A hit in either grep is fixed in the code or doc it names, and the fix is committed as `fix: host table: stale <needle>`. Then the grep runs again.

- [ ] **Step 6: Build with real embeds and run the embed tests**

Dev builds and the gate embed nothing, and the embed test skips a host with an empty blob. Only this run exercises the generated table's non-empty branch (Review Focus 3).

It runs in `target/`, not in a separate `CARGO_TARGET_DIR` as README `:390` suggests, because a second target dir is a cold build on a nearly full disk. The next normal build reruns `build.rs` (it emits `rerun-if-env-changed` for each variable) and embeds nothing again; the gate's build line in Step 12 is such a build.

Check the disk first (at least 2 GB free). Then, in one PowerShell call (the variables live only in that call):

```powershell
cargo build -j 8 -p manifold-view-webview2 -p manifold-view-pdfium
if ($LASTEXITCODE -eq 0) {
    $env:MANIFOLD_WEBVIEW2_HOST = (Resolve-Path target\debug\manifold-view-webview2.exe).Path
    $env:MANIFOLD_PDFIUM_HOST = (Resolve-Path target\debug\manifold-view-pdfium.exe).Path
    cargo test -j 8 -p manifold --bin manifold embedded_host
}
```

Expected: `the_embedded_hosts_extract_to_their_pinned_hashes ... ok`, and `test result: ok.` with no failure.

Then prove that the hosts were embedded, from bash:

```bash
for id in webview2 pdfium; do f=$(ls -t target/debug/build/manifold/*/out/$id-host.bin target/debug/build/manifold-*/out/$id-host.bin 2>/dev/null | head -1); echo "$id blob $(stat -c %s "$f") exe $(stat -c %s target/debug/manifold-view-$id.exe) $f"; done
```

Expected: two lines, `webview2` then `pdfium`, each with `blob` equal to `exe` and above 0.
- On 2026-10-29 the build script's blobs sat in `target/debug/build/manifold/<hash>/out/`, and all were 0 bytes.
- A 0-byte or missing blob means the test skipped: a stop-and-report, not a pass.

- [ ] **Step 7: Run `release-embedded` on this machine's release builds**

This is the spec's R2 mitigation. It runs the stage that writes into `$GITHUB_ENV` on real exes (Review Focus 1).
- `target/release/manifold-view-webview2.exe` (2026-10-29) and `target/release/manifold-view-pdfium.exe` (2026-10-16) were there on 2026-10-29. Their age does not matter: the step checks names, formats and hashes, not the hosts.
- `release-pins` cannot run here: `target/release` holds no `manifold-view-cef.exe` or Servo host, and building them is the release job's work (H14).

Build the gate from the final tree: `cargo build -j 8 -p gate` (expected: `Finished`). Then, with `S` set to your scratchpad directory in forward-slash form (it is in 8.3 form and has no space):

```bash
S="<scratchpad, forward slashes>"; rm -rf -- "$S/dist-local"
target/debug/gate.exe release-embedded v0.0.0 "$S/dist-local" > "$S/release-embedded.out" 2> "$S/release-embedded.err"; echo "exit $?"
cat "$S/release-embedded.out"; echo "other lines: $(grep -cvE '^[A-Z0-9_]+=.+$' "$S/release-embedded.out")"; echo "verbatim prefixes: $(grep -cF '\\?\' "$S/release-embedded.out")"
cat "$S/release-embedded.err"; ls -A "$S/dist-local" "$S/dist-local/embedded"
cat "$S"/dist-local/manifold-v0.0.0-windows-x64.embedded-*.sha256
sha256sum target/release/manifold-view-webview2.exe target/release/manifold-view-pdfium.exe
```

Expected:
- `exit 0`.
- Exactly two stdout lines, `MANIFOLD_WEBVIEW2_HOST=<absolute path>\embedded\manifold-view-webview2.exe` and then `MANIFOLD_PDFIUM_HOST=<absolute path>\embedded\manifold-view-pdfium.exe`, with `other lines: 0` and `verbatim prefixes: 0`.
- `dist-local` holds `embedded` plus `manifold-v0.0.0-windows-x64.embedded-webview2.sha256` and `manifold-v0.0.0-windows-x64.embedded-pdfium.sha256`, and nothing else. `embedded` holds the two exes.
- Each record reads `<64 lowercase hex>  <file>` (two spaces), and its hash equals the `sha256sum` hash of the same exe.

A missing release exe makes the stage fail with an error naming it. Report that as "not run" and build nothing in its place. Finally, `rm -rf -- "$S/dist-local"`.

- [ ] **Step 8: Check the docs**

Stale wording, outside the old plans and specs and the changelog's history (`CHANGELOG.md:77` and `:119` name `hosts.yml` as released entries and stay):

```bash
git grep -n -F -e '**GATE-CEF-DIR.**' -e '**One table per host.**' -e "**Servo's DLLs come from the newest mozangle output" -e 'ViewDesc::host_os' -e 'hosts.yml' -- . ':!docs/superpowers' ':!.superpowers' ':!CHANGELOG.md'
```

Expected: no output.
- At 10444ae5 it listed `PARKED.md:61`, `:157` and `:158` (the three entries Task 7 deletes) and `README.md:22` and `:388` (Tasks 7 and 6).
- The needles are the PARKED headings, not bare phrases, because new text may still say "newest mozangle" or name the spec.

New wording that the spec's "Docs" section requires:

```bash
grep -c 'manifold-hosts' README.md docs/decisions/2026-10-23-host-runtimes.md
grep -c 'release-pins' README.md docs/decisions/2026-10-18-hosts-in-the-release.md
grep -c -e 'release-embedded' -e 'embedded-webview2' -e 'embedded-pdfium' README.md
L=$(grep -n -F '**CI** (' README.md | cut -d: -f1); echo "CI line $L"; grep -n 'crate lists are repeated' PARKED.md | grep -c "README.md:$L"
grep -c 'Host::in_place' PARKED.md
grep -n -i -E 'host table|manifold-hosts|one table' ROADMAP.md
```

Expected:
- Each count is at least 1, for every file listed:
  - the crate map, the Linux CI line and the `runtime.rs` decision note name `manifold-hosts`;
  - README "Releases" and the 2026-10-18 decision's note name `release-pins`;
  - README names the new subcommand and record names;
  - the `-p` entry cites the CI paragraph's current line (`README.md:36` once Task 1's crate-map line is in: 35 at HEAD);
  - the re-cited PARKED `:156` entry names `Host::in_place`.
- The ROADMAP grep prints nothing: ROADMAP has no host-table line, and this plan adds none.

Then read the amended passages once:
- README's host-table paragraph (HEAD `:22`), the paragraph after it (`:33`), the CI paragraph (`:35`) and "Runtime-file placement" (`:80`), and the Releases bullets (HEAD line numbers; each is one line further down after Task 1's crate-map line);
- `docs/decisions/2026-10-23-host-runtimes.md` `:3`, `:7`-`9`, `:18` and `:68`-`69`, which keep their text and gain notes;
- `docs/decisions/2026-10-18-hosts-in-the-release.md` `:8`.

Check each against the code as it now stands. Fix anything stale in place and commit it as `docs: host table: <what>`. The report names each file fixed or found stale (the user's docs check).

- [ ] **Step 9: Final whole-branch review, and fixes**

1. Check the size: `git diff --shortstat <B>..HEAD`. Over about 300 changed lines, which this plan is, the one final review is Opus. Under it, it would be Sonnet (user rule).
2. Dispatch one review agent with `model: "opus"` and this brief:

```text
Review the branch <B>..HEAD in "C:\Users\Winston Li\Documents\GitHub\manifold". It implements the spec
docs/superpowers/specs/2026-10-29-host-table-design.md (decisions H1-H14 are approved and binding) by the plan
docs/superpowers/plans/2026-10-29-host-table.md. Read both first.

Report findings only; change nothing, commit nothing, push nothing. For each finding give a severity (Critical,
Important, Minor), file:line, what is wrong and a concrete failure scenario, and check it against the code before
reporting it. Say plainly when an area has no finding.

Check in particular:
1. The leaf crate's public API and its five rows match the spec's "The crate" section exactly; no method reads cfg or
   calls Os::here(); crates/manifold-hosts/Cargo.toml has no dependency of any kind.
2. Each of the plan's five "Review Focus" items: is the pin it names in place, and does the code behave as the line
   expects? For item 5, read all.rs's SITE_HOST predicate and sleep.rs's kill_hosts and say what each does for servo,
   cef, webkit and webview2.
3. Every new or rewritten test that builds a host file name also passes on Linux (ci.yml:24 runs -p manifold and
   -p manifold-hosts there): no ".exe" literal compared with something derived from Os::here().
4. Everything in the spec's "Deleted" section is gone, with no re-export, alias or wrapper left in its place, and no
   comment names a deleted item.
5. sync_host follows H9 and the spec's pseudo-code: own build folder first, NoBuildFolder, NothingToSync, a part kept
   in place, runtime_ready, exactly one CEF_PATH error and no folder created when nothing can be copied.
6. release.yml matches the spec's YAML (the two gate subcommands piped to $GITHUB_ENV with $LASTEXITCODE checked
   after each; manifold.exe in a step of its own; the records listed by -Filter "$stem.embedded-*.sha256" on the
   gh release create line), and the Linux webkit-host job is unchanged.
7. The layout literals left in code: run
   git grep -n -E '"manifold-view-[a-z0-9]+(\.exe)?"|"cef-runtime"|"libcef' -- crates/manifold/src crates/gate/src
   Each hit must be a test literal that pins a value on purpose (as fresh.rs's do) or an item in the spec's
   "Not covered" list. Name every other one.
8. The commit history: one commit per "Order of work" step (Task 2's parts 2A and 2B in one), each with Cargo.lock
   where the spec's Tooling section requires it.

You may run git, grep and the plan's test lines, one cargo at a time: run Get-Process cargo first and wait for any
running one. Never run Python reading stdin; if you need a script, write it to a file and run
python -P f.py < /dev/null, and prefer no Python. Do not spawn subagents.
```

3. Take each Critical and Important finding in turn:
   1. Check it against the code yourself.
   2. Fix it, inline when under about 10 lines, otherwise with a Sonnet implementer whose brief repeats the Python rule and says "change nothing beyond this; report every deviation".
   3. Run the test lines of the crates it touched.
   4. Commit it as `fix: host table: <finding in a few words>`, with the `<attribution>` trailer.
   5. Have one Sonnet re-review of all the fix commits together confirm each fix.
   A finding you reject gets a one-line reason in the report.
4. A Minor finding is fixed the same way when cheap. Otherwise it goes into PARKED.md (what, why parked, trigger, evidence with `file:line` and date, status) before Step 13 (VERSIONING.md: leftovers go to PARKED first).
5. After the last fix, run Steps 2, 3 and 5 again. Also run Step 4 if the fix touched the leaf, Step 6 if it touched `build.rs` or `embedded_host.rs`, Step 7 if it touched `release_hosts.rs`, and Step 8 if it touched a doc.

- [ ] **Step 10: Capture `gate sync-runtime-deps` after the change, and compare**

1. Build the final gate: `cargo build -j 8 -p gate` (expected: `Finished`).
2. Do the pre-run check, then from bash:

```bash
powershell -NoProfile -File "$USERPROFILE/.claude/runs/run.ps1" -Kind probe -Name sync-after -Command 'target\debug\gate.exe sync-runtime-deps'
BEFORE=$(ls -dt "$LOCALAPPDATA"/claude-runs/manifold/????-??-??-sync-before* | head -1); AFTER=$(ls -dt "$LOCALAPPDATA"/claude-runs/manifold/????-??-??-sync-after* | head -1); echo "$BEFORE"; echo "$AFTER"
grep -E '"(exit|commit)"' "$BEFORE/run.json" "$AFTER/run.json"
diff "$BEFORE/out.log" "$AFTER/out.log"; diff "$BEFORE/err.log" "$AFTER/err.log"
```

This is the first recorded run of the new sync (Task 3 Step 8 ran it unrecorded as a smoke test). Expected:
- both `"exit"` values are `0`;
- `AFTER`'s `"commit"` is the final HEAD with no `-dirty`;
- both diffs print nothing.

Why nothing should differ:
- The sync visits Servo, then CEF, in both versions, and their labels are the same: `target-servo/debug/<file>` and `cef-runtime/<name>` (spec, `sync_host`).
- WebKit, WebView2 and PDFium have nothing to sync, so they print nothing.
- Servo's DLLs now come from `target-servo/debug/build` first, and those bytes were the same as `target/debug/build`'s on 2026-10-29 (Start, Step S3).

Any diff line is either explained or a regression:
- Explained: a file that changed between the two runs. Name it, with its modified time and the command that changed it.
- Otherwise it is a regression in Task 3's code. Fix it as in Step 9, then run this step again; `run.ps1` names the new run `sync-after-2`.
- If `sync-before` does not exist (the Start steps were skipped), there is no comparison. The report says so. Never reconstruct a "before" from reading the old code.

- [ ] **Step 11: Pick the gate tier**

Run: `target/debug/gate.exe tier <B>`, with `<B>` from this plan's "Started at" line.
Expected: its last line is `TIER full` (the leaf crate and engine code changed; `tier.rs` has sent every crate but the app to Full since Task 1).

Run `full` whatever it prints (CLAUDE.md "Gate"; the spec's verification asks for tier `full`).

- [ ] **Step 12: Run the full gate**

Do the pre-run check, then from bash (CLAUDE.md "Gate", verbatim but for the run name):

```bash
powershell -NoProfile -File "$USERPROFILE/.claude/runs/run.ps1" -Kind gate -Name gate-host-table -Detach -Command 'cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate && target\debug\gate.exe all "%RUN_DIR%\gate-log.txt" full'
```

Wait for it (see "Waiting for a run", deadline 4500 s). Then:

```bash
tail -n 5 "$RUN/gate-log.txt"; grep -E '"(exit|conditions)"' "$RUN/run.json"
grep -cE '^GATE [^ ]+ exit=' "$RUN/gate-log.txt"; grep -E '^GATE [^ ]+ exit=' "$RUN/gate-log.txt" | grep -v ' exit=0 ' | cut -c1-200
```

Expected:
- `run.json`'s `"exit"` is `0`.
- The stage count matches the previous full gate's (20 stages in `2026-10-29-gate-host-stamp`).
- The last grep prints nothing: every stage line reads `exit=0`.

Then:
1. Set the headline, in the same form as the `2026-10-29-gate-host-stamp` row: `exit <n> at <commit>: <k> of <m> stages pass (...)`. Write it with the Edit tool into the row's headline cell in `docs/runs.md` and into `"headline"` in `$RUN/run.json`; `run.json` is the source of truth and the row is its copy (RUNS.md).
2. Keep the CPU load and the Defender state from `conditions`, and the `GATE start` and `GATE done` lines' `cpu=` and `defender_rt=`, for the report.
3. Diff the Downloads folder.

The previous full gate, `2026-10-29-gate-host-stamp` at b8562a4e, ended with `"exit"` 1: 19 of 20 stages passed, and `shots-aa` failed with its known signature. So `"exit"` 1 is also an expected result, but only when the last grep lists nothing but the two stages below and each shows its signature. It is then reported, not fixed here and not rerun under a new name. The row's headline names the failed stages and any rerun id.
- **`shots-aa`**: PARKED.md "`shots-aa` can differ by a few pixels where edges overlap" (`:146`, held for the ordered `Nodes` container).
  - Its signature is in `$RUN/gate-log.txt.shots-aa.log`. The lines before `FAIL: the two runs' shots match pixel for pixel` list only shots that differ by a few pixels (`<shot>.png: N px, max M, x ..`; N between 2 and 8 in earlier runs; `json-zoom-in`, 8 px, max 3 on 2026-10-29). The summary line ends `0 missing in B; 0 only in B`.
  - Any of these is a regression: a `size ... vs ...` line, a `missing in B` or `only in B` count above 0, or a differing shot with no edges crossing in it.
- **`sleep-history`**: a timing flake under load (`2026-10-28-gate-download-failure`; rerun `2026-10-28-sleep-history-rerun`, 3 of 3 passed in 5.7-6.3 s).
  - Its signature: its `GATE sleep-history exit=1` line in `gate-log.txt` has `took=` far over the 6 s a quiet run takes, and the CPU was high around its start.
  - `gate-log.txt.cpu` holds `<epoch seconds> <CPU %>` samples. The start epoch is the fourth whitespace-separated field of the `TIMING gate/sleep-history <took> <start epoch>` line (`awk '{print $4}'`).
  - Rerun it alone three times (about 20 s; do the pre-run check first):

```bash
powershell -NoProfile -File "$USERPROFILE/.claude/runs/run.ps1" -Kind gate -Name sleep-history-rerun-table -Command 'set "VIEWS=webview2"& set "CASES=history"& (target\debug\gate.exe sleep && echo run1 PASS || echo run1 FAIL)& (target\debug\gate.exe sleep && echo run2 PASS || echo run2 FAIL)& (target\debug\gate.exe sleep && echo run3 PASS || echo run3 FAIL)'
```

  It is the known flake only if `out.log` shows `run1 PASS`, `run2 PASS` and `run3 PASS`. Any `FAIL` there is a regression.

Any other failing stage, or either of these two without its signature, is a regression:
1. Read its work folder (`<run folder>/gate-<stage>-<pid>`) and its lines in `gate-log.txt`, and report the cause.
2. Fix it as in Step 9, and run Steps 2 and 10 again.
3. Run the gate again under a new name, `gate-host-table-2`.

Never mark a failed step as passed. Remove a failed stage's work folder once read.

- [ ] **Step 13: Mark the spec and the plan done, and commit**

Check first that every leftover from Step 9 is in PARKED.md and committed. Then:
- In `docs/superpowers/specs/2026-10-29-host-table-design.md`, change the front matter's `status: active` to `status: done`.
- Make the same change in `docs/superpowers/plans/2026-10-29-host-table.md`.

```bash
cargo fmt --all
git diff --stat
git add docs/runs.md docs/superpowers/specs/2026-10-29-host-table-design.md docs/superpowers/plans/2026-10-29-host-table.md
git commit -F - <<'EOF'
runs: sync-after, gate-host-table; host table spec and plan done

<attribution>
EOF
git log -1 --format=%B | grep -v '^$' | tail -n 1
```

Expected:
- `git diff --stat` lists only these three files. `docs/runs.md` holds the `sync-after`, `gate-host-table` and, if run, `sleep-history-rerun-table` rows.
- The last command prints the trailer line.

- [ ] **Step 14: Check for leftovers and report**

In PowerShell, run `Get-Process python, manifold, gate, msedgewebview2, cargo, manifold-view-servo, manifold-view-cef, manifold-view-webview2, manifold-view-pdfium, wsl -ErrorAction SilentlyContinue | Select-Object Id, ProcessName, StartTime, Path`.
- Anything this plan's runs or subagents started is a leftover: a `Path` in this repo or a run folder, or a start time inside a run. Report it.
- Leave the user's own processes alone. `wsl` may be the user's own session: report one only when its start time is inside a run.

Run `wsl.exe -d Ubuntu -e pgrep -af "[m]anifold-view-webkitgtk|[c]argo"`. Expected: nothing (exit 1). Report anything it lists, with its command line.

Check that `<scratchpad>/dist-local` is gone (Step 7).

The report says:
- **Status:**
  - what is committed locally (each commit's subject since B);
  - that nothing is pushed, tagged or bumped;
  - that `CHANGELOG.md` is untouched (its entry is written at the bump: Internal, the host table; Changed, the record asset names);
  - any uncommitted file.
- **Verification:**
  - Step 2's checks;
  - the WSL leaf run;
  - the dedicated embed run with the two blob sizes;
  - the local `release-embedded` run;
  - the `sync-before` and `sync-after` run ids and their diff (empty, or each line explained);
  - the gate run id and its result (exit 0, or exit 1 with only `shots-aa` and `sleep-history` failing, each with its signature and any rerun id);
  - the CPU load and Defender state the gate recorded.
- **Unverified, plainly:**
  - `release.yml`, which first runs in the workflow's dry run after the user pushes (H14);
  - `release-pins`, never run on real inputs (Review Focus 4);
  - CI's Linux `-p manifold` job, which runs on push;
  - the `WITH_ENGINES` steps (Review Focus 5).
- **Review:** each Step 9 finding with what was done (fixed with its commit, parked with its PARKED title, or rejected with the reason).
- **Docs check:** each file Step 8 fixed or found stale, or "none stale" with the greps run.
- **Architecture check:**
  - What still spells a host outside the table: `install-check`'s usage literal (`main.rs:185`, tied by a test), the Linux job's two literals (H10), the upload pattern and the `windows-x64` stem in the YAML, and `fresh.rs`'s `[&WEBVIEW2, &PDFIUM]`, one more spelling of the gate build line's hosts.
  - What would remove them: PARKED "Host table (2026-10-29)", "The `-p` crate lists are repeated"; the spec's "Weighed and not taken" `gate release-assets` and a leaf bin for the Linux job, each taken if its literal drifts.
- **Next steps, ranked, each with a one-line why.** Number them in one sequence with any options in the same message (options 1-3, next steps 4-6), so a bare "4 5" is unambiguous:
  - the user pushes the branch and runs `release.yml`'s dry run, before any tag (the plan never pushes and does not assume it; Q-dryrun, H14: the first run of the YAML change and of `release-pins`);
  - read CI's Linux job on that push (Review Focus 2);
  - a `WITH_ENGINES="servo webkit"` gate run, if the user wants the `SITE_HOST` and `kill_hosts` paths run live (Review Focus 5);
  - the PARKED "Host table (2026-10-29)" items.

---

## Decisions on the open questions (answered 2026-10-30: "as recommended")

The plan follows each answer. About fifty smaller choices inside the settled design (message wording, helper placement, test shape) are made and explained in the step that uses them; they needed no answer.

- **Q-tier. Decided: one `tier.rs` Full arm for every crate but the app, over a fourth per-crate arm (Task 1, Steps 12-15).** It replaces the protocol, pages, engine-crate and gate arms (`tier.rs:108`-`110`, `:122`-`:124`), so the next crate needs no edit, and it moves no tracked path (every crate but the app was Full). The arm reads "every path under `crates/` outside `crates/manifold`", the spec's wording. The spec's "Tests, Tier" and H12 are amended to match. Checked by compiling `tier.rs` alone: the old arms fail the new test at `tier.rs:283` (`left: "TIER light"`), the new arm passes all 6.
- **Q-engines. Decided: edit the two comments, over leaving them and parking them.** `session_sync.rs:121` and `store/model.rs:227` still say `ENGINES`, which no longer exists; Task 7 Step 14 changes them to `VIEW_KINDS`. Comment-only, and beyond the spec's "Docs" list.
- **Q-killhosts. Decided: error, over killing nothing silently.** `kill_hosts` returns an `InvalidInput` error when the view names no host (Task 4).
- **Q-streams. Decided: one stream, stdout.** Every sync skip line goes through `say()` (Task 3): the new ones, CEF's skip (stderr at HEAD, `sync_deps.rs:141`, replaced by the two `say()` lines in `sync_host`, and by an error when the host exe is built) and `sync_file`'s `no candidate found for <label>, skipping` (stderr at HEAD, `:72`). Errors stay on stderr. Step 8's before/after diff is unchanged on this tree, where neither skip line prints.
- **Q-embed-dir. Decided: `target/`, over a separate `CARGO_TARGET_DIR`.** The real-embed test builds in `target/` (Task 2 Step B16, Task 8 Step 6), because C: is nearly full. The next normal build reruns `build.rs`.
- **Q-verify-scope. Decided: local checks only, over adding engine runs.** No local release CEF or Servo build (so `gate package` and `release-pins` first run in the release dry run); `release-embedded` against whatever `target/release` holds (Task 8 Step 7 runs `v0.0.0`; Task 5 Step 8 smoke-runs `v0.0.0-local`); the final gate at `full` without `WITH_ENGINES`; WSL runs only `-p manifold-hosts`.
- **Q-dryrun. Decided: a dry run comes before any tag.** `release.yml` changes in Task 6 and first runs in the workflow's manual dry run, which needs the branch pushed. The plan never pushes; at the end of Task 8 the executor asks for the push and does not assume it, and no tag comes before the dry run (Step 14's ranked next steps).
- **Q-disk. Unchanged:** the Global Constraint says what to do under 2 GB free (stop and ask, delete nothing, make no new target dir) before every build and run; Step S1, the pre-run check and Task 8 Steps 1 and 6 check the same threshold. Free space swings with other sessions' builds: 5 GB on 2026-10-29, 26 to 28 GB on 2026-10-30.

---

## Review rounds

Rounds 1 to 3 were adversarial, each by eight read-only Sonnet reviewers (spec coverage A and B, Task 1, Task 2 Part A, Task 2 Part B with Task 3, Tasks 4 and 5, Tasks 6 and 7, cross-task seams). Rounds 4 to 6 came after the decisions and were focused on that delta: three reviewers each in rounds 4 and 5 (round 4: tier, decisions, round-3 edits; round 5: Q-streams, test threads, citations and the rest), one in round 6 (the round-5 fixes). Each reviewer checked its steps against the tree at HEAD and cited file:line evidence; one compiled Task 1's leaf crate in a scratch workspace.

| Round | Plan commit after | Material | Minor | What it caught |
|---|---|---|---|---|
| 1 | `5d77fa32` | 6 | about 36 (20 fixed, the rest nits) | Plan activation split across Start S4 and Task 1 Step 1; stackdump mentions; the `tail -n 1` trailer check printing a blank line; Task 3 Step 8 and Task 8 Step 8 comparing against the wrong baseline or a hard-coded README line; a clippy lint (`cloned-ref-to-slice-refs`) in Task 1's test; stale PARKED citations. |
| 2 | `076b8416` | 1 | about 17 | Task 7 Step 9 re-cited PARKED entries by reading old line numbers, which pointed at the wrong code (`process_manager.rs:355` is `set_visible`, `:1028` a doc comment); now by label. Plus a loose manifest assertion made exact, Task 8 Step 10 wording, `-j 8` vs test threads, README checks. |
| 3 | `a16ff790` | 0 | 7 distinct (10 reports) | `settings_page.rs:805` missing from Step 9; `-- --test-threads 8` on some whole-crate test lines (round 4 found three more); the cargo warning's backticks; a missing blank line; the disk figure. |
| 4 | `0bcb1296` | 2 | about 20 | Q-streams was half implemented (`sync_file`'s skip line stayed on stderr); `-- --test-threads 8` was missing on the three `-p gate` whole-crate lines in Tasks 4 to 6 and in one prose mention. Minors: stale line citations in Task 1 (`:107-113`, `:282`), Q-decisions worded as lettered options, `shots.rs` +2 rule, greps that matched several lines. |
| 5 | `f7f6643d` | 1 | about 12 | Task 3 Step 6 still called `sync_file` "copied unchanged" after Q-streams changed it (the same defect class as row 4). Minors: the CEF skip line is replaced by two lines, not moved; Review Focus 1's `sync_file` citation; a note that the `shots_net` tests share a fixed port; Q-disk and Step 14 wording. |
| 6 | (the commit recording this row) | 0 | 1 | One wrong function name in the Q-streams record (`copy_runtime` for `sync_host`), fixed. The delta check of round 5 found nothing else: Step 6's "copied unchanged" items diffed byte-identical against HEAD. |

Left as they were in round 3: the ENGINES comment edits in `session_sync.rs:121` and `store/model.rs:227` (beyond the spec; the user kept them, Q-engines) and the `package.rs` module-doc `windows-x64` wording (still true; optional).

The open decisions were answered on 2026-10-30 (see "Decisions on the open questions" above). Q-tier changed Task 1 Steps 12-15 and the spec, so focused rounds 4 to 6 re-checked that change, the round-3 edits and their own fixes (rows 4 to 6). Round 6 found no material issue, so the review stops here.
