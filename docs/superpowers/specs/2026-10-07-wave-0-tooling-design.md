# Wave 0: tooling, build and script hygiene (design)

Status: Agreed 2026-10-07, not started. Plan: `docs/superpowers/plans/2026-10-07-wave-0-tooling.md`. First wave of the
refactor program. Scope comes from `.superpowers/survey/verified-wave-0.md` (a fact-check of
`.superpowers/survey/build-tooling.md`); where the two differ the verified file wins, and where the verified file
differs from the code as of `c8b7485`, this spec says so ("Corrections"). Already done and not repeated:
`temp_dir` uniqueness (`c49eb55`) and the protocol fingerprint test (`c8b7485`).

## Decisions (agreed 2026-10-07)

1. **BT-1 (feature-set churn) is dropped**, except for an optional shared `windows` entry in
   `[workspace.dependencies]` holding the union of the features `manifold`, `manifold-engine-cef` and
   `manifold-engine-webview2` use, plus the three `webview2-com` 0.39.1 itself requests (`Win32_Globalization`,
   `Win32_System_Ole`, `Win32_System_Variant`). Its value is low: alternating selections already stays `Fresh` once
   both are built (verified #9); it saves one recheck per selection after a dependency or toolchain change, and disk. Workspace feature unification (`resolver.feature-unification = "workspace"`) is
   never used while Servo is a workspace member: it pulls Servo's `rustls/aws-lc-rs` (so `aws-lc-sys`, a CMake build),
   `tokio` multi-thread, `rayon` and more into every `manifold` check.
2. **Toolchain: pin `nightly-2026-09-23`** in `rust-toolchain.toml` (components `rustfmt`, `clippy`; profile
   `minimal`), with **no `rust-version`**. That date is the one in the installed nightly's channel manifest, so it is
   the compiler in use (`rustc 1.100.0-nightly (6bb1652a0 2026-09-22)`), not the survey's `nightly-2026-09-22`. The
   file also applies to the WebKit build inside WSL. Moving to stable (1.98.1 is installed) is deferred: it changes
   the rustc hash, so `target/` is rebuilt beside the old artifacts while C: is about 94% full.
3. **CI is the minimal two-job workflow**: Windows runs `cargo fmt --all --check`, the hook's clippy and the tests of
   the hook's three crates; Linux runs `cargo test -p manifold -p ipc-protocol` with no apt step, because neither crate
   needs GTK on Linux (`rfd` 0.15 resolves to `xdg-portal`; `gtk-sys` comes only from `manifold-engine-webkit`).
   README's "Linux needs GTK 3 development files for `rfd`" is stale and is corrected.
4. **BT-4 (smoke-test hygiene):** `kill_tree` cleanup in `headless-smoke-test.sh`, `scripts/smoke_eval.py` replacing
   its five stdin-python heredocs, `</dev/null` on its `import PIL` check, and the ten leftover
   `target/headless-test-appdata-*` profile dirs removed. The one other python heredoc in `scripts/`
   (`chrome-shots.sh`'s crop-and-sheet step) moves to `scripts/shots_sheet.py` in the same task, so no probe this
   wave runs feeds python from stdin.
5. **BT-5 (bad initial URL):** the Servo and WebView2 hosts open `about:blank` when the initial URL doesn't parse or
   doesn't navigate, instead of panicking. Not `HOST_FATAL`: the condition is recoverable (the user can type a URL into
   the tab). Full gate.
6. **BT-9 (atomic exports):** `EXPORTSTATE`, Export Session (`EXPORTSESSION`) and Export Group (`EXPORTGROUP`) write
   through `store::atomic::replace`; the probe sleeps that waited for a fully written file after it appeared become
   plain condition waits (`wait_file`). Full gate.
7. **`ipc-protocol` goes in `[workspace.dependencies]`** as a path-only entry, so a release no longer edits five
   manifests.
8. **Lint `allow_attributes_without_reason`** for the whole workspace, with a `reason` on every `#[allow]`. Its
   companion `allow_attributes` is **not** enabled: it pushes toward `#[expect]`, which on cfg-dependent code (the
   `permissions_set_readonly_false` test sites, the `cfg_attr(not(unix), ...)` in `engine_install.rs`) can go
   unfulfilled on one target and fail `-D warnings` there.
9. **`scripts/lib.sh`** absorbs the probes' duplicated helpers, one family per task: the settings seed, the
   `tail -f` launcher (with `send`, and setting the instance BelowNormal), `wait_file` and `check`. Mechanical work.
   What it leaves out of verified #8, and why, is under Not in scope.

## Scope

### Toolchain pin (Decision 2)
- New `rust-toolchain.toml`. README "Prerequisites" says what it pins and why (a clippy update can't fail the
  `-D warnings` hook with no code change), and README "Building" says WSL's rustup installs the Linux build of the same
  nightly on first use there.
- Same compiler commit, so the hook's clippy should find every unit `Fresh`. A rebuild means the dated install is not
  byte-identical; that is reported, not assumed away.

### `[workspace.dependencies]` (Decisions 1 and 7)
- `ipc-protocol = { path = "crates/ipc-protocol" }`; the five dependents (`manifold`, cef, servo, webkit, webview2)
  write `ipc-protocol.workspace = true`. README "Branches" drops "the `ipc-protocol` requirements in the crate
  manifests" from the release recipe.
- Optional, only on the user's yes: `windows = { version = "0.62", features = [...] }` with the union of features
  (webview2's list, `manifold`'s `Win32_Security_Cryptography`, and webview2-com's `Win32_Globalization`,
  `Win32_System_Ole`, `Win32_System_Variant`; checked against `cargo tree` of the hook's set, whose 53 features
  are the union over every requester, not just the manifests), carrying the "must match `webview2-com`" comment
  once. This removes the 36/53 feature split of `windows` between `-p manifold` and the hook's set, so the two
  share one copy and one recheck after a dependency or toolchain change, at the cost of `manifold` compiling
  `windows` modules it doesn't use. `serde_json` stays per crate (its feature sets differ on
  purpose: `preserve_order` in `manifold`).
- `crates/manifold/Cargo.toml` starts with a UTF-8 BOM; edits keep it.

### Lint (Decision 8)
- Root `Cargo.toml`: `[workspace.lints.clippy] allow_attributes_without_reason = "warn"` (the hook's `-D warnings`
  makes it an error); every crate gets `[lints] workspace = true`.
- Every allow site gets a `reason`, moving its existing comment into it. `graph_ui.rs`'s `draw_graph_cards` (the
  verified file's `:1618`) has none; its reason: one call site, inside `build_graph_ui`'s Scene closure, whose locals
  the arguments are.

### CI (Decision 3)
- New `.github/workflows/ci.yml`, two jobs, on push and pull request. Leaves out CEF (download, CMake, ninja), Servo
  (33-minute build), WebKit (WSL/GTK) and every probe (desktop, GPU).
- README line 18 corrected, and a short "CI" note added.
- A test that fails only on Linux, or only on the runner, is a finding to report, not a CI bug to paper over.

### Smoke-test hygiene (Decision 4)
- `headless-smoke-test.sh` sources `winpid.sh`; `cleanup` runs `kill_tree` on the coproc (its proxy's Windows tree
  holds `manifold.exe`) and on the probe server, waits, then retries `rm -rf` of the profile for up to 5 s (the
  `session.mnf.lock` is held until `manifold.exe` is gone).
- `scripts/smoke_eval.py <state|query|nine|border|converted> ARGS...` holds the five heredoc bodies unchanged, each at
  module level (so the expressions they `eval` see the same names), after `cmd = sys.argv.pop(1)` (so each body's
  `sys.argv[1]`, `[2]`... are its old arguments). Every call is `python -P ... </dev/null`.
- The ten leftover dirs (one from 2026-09-27, nine from 2026-10-05) are deleted once; only dirs older than two hours,
  so a smoke test another session is running keeps its profile.
- `chrome-shots.sh`'s last step (`python -P - ... <<'EOF'`, crops and `sheet.png`) becomes
  `python -P scripts/shots_sheet.py OUT CROP_PX VIEWS </dev/null`, body unchanged.

### Bad initial URL (Decision 5)
- Servo (`manifold-engine-servo/src/main.rs`, the `.url(Url::parse(&initial_url).expect(...))` builder argument):
  parse, and on error log and use `about:blank`.
- WebView2 (`manifold-engine-webview2/src/main.rs`, the initial `Navigate(...).expect("initial navigate")`): on
  error log and navigate to `about:blank`.
- New `scripts/bad-url-probe.sh` (WebView2): a tab whose stored url is `not a url`, and one with no url (the host then
  gets `ENGINE_INITIAL_URL=""`, so the hosts' `https://example.com` default doesn't apply), must end loaded with no
  `panicked` in the host log. It must fail before the fix (that is the test); if the no-url case already passes before
  the fix, the app substitutes a URL upstream and the case stays as a guard.
- Accepted behavior: once the host reports its navigation, the tab's stored URL becomes `about:blank`, so the
  unparsable text is lost (as it is today when the tab fails).

### Atomic exports (Decision 6)
- The three writers (`debug_control.rs` `ExportState` arm, `session_sync.rs` `export_session_to`, `main.rs`
  `write_json_export`) call `store::atomic::replace` instead of `std::fs::write`. The `ExportState` arm's write moves
  into `App::write_export_state(&Path)` so a unit test can reach it (the arm needs an event loop).
- Probes: the 13 post-appearance sleeps go (`focus-loss-probe.sh` 9, `link-probe.sh` 3, `chrome-shots.sh`'s `state()`
  1). The temp name `<file>.<pid>-<n>.tmp` matches none of the probes' `*.json` globs.
- Kept: `headless-smoke-test.sh`'s size-stable loop (it also guards `SCREENSHOT` PNGs, whose writer is not atomic), and
  the 5-15 s negative-window sleeps in `two-instance-test.sh` / `scale-test.sh`.
- `debug_control.rs`'s `ExportState` doc and README "Headless mode" say the `EXPORT*` files are replaced atomically.

### `scripts/lib.sh` (Decision 9)
- `seed_settings PROFILE [ENGINE] [MORE]`: `mkdir -p PROFILE/manifold` and the settings JSON the probes write today,
  byte for byte (`MORE` is extra members starting with `, `).
- `launch_headless EXE PROFILE CMDFILE LOG`: the `APPDATA=... EXE --headless < <(exec tail -n +1 -f --pid=$$ CMDFILE)`
  launch, in the background, then `below_normal` on it (verified #8's `probe_start` did the same), keeping its Windows
  pid in `launched_winpid`; the caller reads `$!` right after (the lookup runs in the foreground, so `$!` is still the
  instance). Seven of the ten probes (chrome-shots, failed-spawn, hscroll, key, page-kind, scale-test, sleep) ran the
  app at Normal priority until now; link, focus-loss and jar drop their own `below_normal` call for it. Extra
  environment goes on the call (`HTTPS_PROXY=... launch_headless ...`). The caller still empties `CMDFILE` itself,
  since some write commands first.
- `send LINE...`: appends to `$cmd`.
- `wait_file PATH`: until non-empty, for `WAIT_FILE_SECS` (default 60) seconds. `chrome-shots.sh` sets 30, its old
  value. One knob instead of the verified file's per-call `[secs]`, because every probe used a single value.
- `check NAME WANT GOT`: PASS/FAIL line, sets `status=1` on FAIL (the caller starts `status=0`).
- New `scripts/lib-test.sh` checks every helper without the app in seconds.
- Users: the ten probes with the launcher (`chrome-shots`, `failed-spawn-probe`, `focus-loss-probe`, `hscroll-probe`,
  `jar-probe`, `key-probe`, `link-probe`, `page-kind-probe`, `scale-test`, `sleep-probe`); `wait_file`, `send` and
  `check` only where an identical copy exists today.

## Corrections to the verified file (found writing this spec, against `c8b7485`)

- **A 13th allow site:** `engine_install.rs:436`, `#[cfg_attr(not(unix), allow(dead_code))]`. On Windows the
  `cfg_attr` expands to an allow, which the lint is expected (not yet verified) to see; it gets a `reason` either way.
- **Host logs are not under the profile:** a host writes `%TEMP%\manifold-<tab>.log` (`process_manager.rs`,
  `per_tab_log_path` uses `std::env::temp_dir()`), so the BT-5 probe points `TEMP`/`TMP` into its own directory.
- **An imported tab sleeps:** a tab seeded with `--import-json` (as the probes seed theirs), like one from
  `IMPORTGROUP`, is an `UnloadedState::Sleeping` placeholder, so `WAITTABS` alone starts no host and the BT-5 probe
  opens the tab (`GRAPHDOWN`, `GRAPHOPEN`, as `hscroll-probe.sh` does) before
  checking.
- **A sixth stdin-python heredoc:** besides the smoke test's five, `chrome-shots.sh:662` runs `python -P - ... <<'EOF'`.
- **`probe_init` under `target/probe/` would break probes:** the probes keep `mktemp -d` because "a path with a space
  (this one has "Winston Li") stops the app reading its commands" (`link-probe.sh`). `probe_init` and its shared trap
  are left out (see Not in scope).
- **Disk:** C: is about 94% full (56-59 G free on 2026-10-07), not the ~98% / 22 G the verified file and memory had
  from 2026-09-30.
- **`check` has 3 identical copies, not 5:** `chrome-shots.sh`'s takes its arguments in another order and sets no
  status, and `package.sh`'s `check` is its `--check` mode.

## Not in scope

- BT-1 beyond the optional `windows` entry; feature unification of any kind.
- Moving to stable Rust; `rust-version`.
- CEF, Servo, WebKit or probes in CI; a `workflow_dispatch` CEF job.
- A live Servo run of the bad-url probe (its host is not built into `target/debug`); the Servo change is compiled only
  where `target-servo` already holds a build.
- `lib.sh` for `headless-smoke-test.sh` and `two-instance-test.sh` (coprocs; the gate depends on them), `package.sh`
  (not a probe), `drive.sh` and `json-pan-profile.sh` (other settings path). `probe_init`, a shared cleanup trap, and the
  per-probe `x`/`py`/`field`/`kv` variants (they differ by directory variable and output).
- Verified #8's `export_state NAME` (`rm -f`, `EXPORTSTATE`, `wait_file`) and `probe_stop` (`QUIT`, wait): most probes
  queue their `EXPORTSTATE`s and `QUIT` in one batch and read the files after the instance exits, so neither helper
  has users beyond link/focus-loss's `x()` (excluded above) and three one-line stops with the same body under
  different names (`stop` in link, `quit` in focus-loss, `engine_stop` in chrome-shots, each `send QUIT; wait
  "$inst"; inst=""`). Moving those means renaming every call site for one line saved per probe: parked, with no
  behavior gain.
- Verified #8's per-call `wait_file [secs]`, one-probe-per-commit migration, and "same PASS/FAIL on two runs" per
  probe. Instead: one `WAIT_FILE_SECS` knob (every probe used a single value); one helper family per commit across all
  ten probes, so each commit is one mechanical same-shape edit; and one baseline sweep before Task 8 compared against
  one after Task 11 (exit codes and verdicts, digits masked, plus no leftover `manifold.exe`), with any differing probe
  rerun twice to tell a flake from a regression.
- Pre-commit hook triggers on `Cargo.toml`/`rust-toolchain.toml` (today it runs only when a `.rs` or `rustfmt.toml` is
  staged); BT-10 (the hook's and CLAUDE.md's "Servo doesn't build on Windows"). Both are follow-ups.
- Keeping an unparsable URL's text on the tab.
- Making `SCREENSHOT` atomic.

## Risks

- **Toolchain download:** the first cargo call under the pin downloads `nightly-2026-09-23` (a few hundred MB) with C:
  at about 94%. Check free space first. The old rolling `nightly` stays installed: other projects on this machine use
  it as their default.
- **WSL:** the WebKit build in WSL now also installs the pinned nightly, and `target-linux` rebuilds once if WSL was on
  another toolchain.
- **CI first run:** the Linux job is the unknown (README's last Linux pass was 2026-10-02). Tests needing network or a
  GPU were not audited.
- **Rename on Windows:** `store::atomic::replace` renames over the target. A reader holding the old file open without
  `FILE_SHARE_DELETE` makes the rename fail where `std::fs::write` might have succeeded. Probes read each export in a
  python process that exits before the next export, and settings and sessions already go through `replace`.
- **Cleanup race:** a hard kill of the smoke test's bash (TerminateProcess) still skips its EXIT trap; the app then
  quits on stdin EOF, but the profile dir stays. Nothing in bash can catch that.
- **`kill_tree` on a coproc** finds the Windows pid of the coproc's proxy and kills its tree; that `manifold.exe` is in
  that tree is the claim `two-instance-test.sh` documents. The interrupt check proves it here.
- **Temporary environment on a function call** (`HTTPS_PROXY=... launch_headless`) relies on bash exporting prefix
  assignments to the commands a function runs; `lib-test.sh` checks it.
- **focus-loss-probe.sh steals the OS foreground** for a few seconds per case; it runs only after telling the user.

## Testing

- Rust (TDD): one unit test per export writer, each blocking the target path with `store::atomic::block_for_test`. A
  writer that goes around `replace` writes the file anyway, so each test fails before the change and passes after.
- Scripts: `scripts/lib-test.sh` for every `lib.sh` helper; `bash -n` on every edited script; `smoke_eval.py` run on
  small fixtures; the smoke test run whole, and interrupted mid-run (after its first PASS, waited for by condition)
  both on a copy with the old `cleanup()` and on the new script, since the app also quits on the SIGINT's stdin EOF:
  the new one must leave no `manifold.exe` and no profile dir; what the old one leaves is recorded.
- Probes: `link-probe.sh` and `focus-loss-probe.sh` three times each without the sleeps; a sweep of every
  `lib.sh`-migrated probe before the first `lib.sh` task and after the last, with identical exit codes and PASS/FAIL/SKIP lines (digits masked: ids, ports and timings vary per run).
- BT-5: `scripts/bad-url-probe.sh` fails before the host change and passes after. It polls `EXPORTSTATE` until the
  tab is loaded (30 tries, 120 s per file so the first export can wait behind `WAITTABS`); its one fixed wait is
  hscroll-probe's `SLEEP 4` before `GRAPHDOWN`, kept because `EXPORTSTATE` shows no signal for it.
- Gates: `scripts/gate.sh LOG full` after the atomic-exports task (covers everything before it) and after the BT-5
  task (covers everything after the first gate), each with CPU load and Defender state.
- CI: the first pushed run of both jobs (pushing needs the user's go-ahead).
