# manifold

## Branches

`kind: app` (see VERSIONING.md). Version field: workspace `Cargo.toml`; changelog: `CHANGELOG.md`; format versions: `PROTOCOL_VERSION`, `RENDER_PROTOCOL_VERSION`, the store `FORMAT_VERSION`s.

Work on `main`. A release is a version bump commit plus an annotated `vX.Y.Z` tag on `main`, and only when the user asks for one. New branches and worktrees start from `main` and merge back into it. A hotfix for an old release branches from its tag as `release/x.y` and is deleted after. See README "Branches".

## Formatting and lints

Run `cargo fmt --all` before each commit. `cargo fmt --all --check` and `cargo clippy -j 8 -p manifold -p manifold-view-protocol -p manifold-pages -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings` must pass (CEF is left out: its build script needs the CEF download; WebKit builds only on Linux; Servo does build on Windows but takes about 33 minutes, see README "Building Servo on its own"); `.githooks/pre-commit` checks both when Rust files are staged (enable once per clone: `git config core.hooksPath .githooks`). Never bypass it with `--no-verify`. Don't silence a lint to pass: fix it, or `#[allow(..., reason = "...")]` it saying why (the workspace lint `allow_attributes_without_reason` enforces the reason). The style is `rustfmt.toml` (width 120, `use_small_heuristics = "Max"`). Mark an item `#[rustfmt::skip]` only where hand layout carries meaning, such as an enum of one-line struct variants (a doc comment on any variant makes rustfmt expand them all). The workspace-wide reformat is listed in `.git-blame-ignore-revs`; run `git config blame.ignoreRevsFile .git-blame-ignore-revs` once per clone.

## Tests and the user's profile

No test may read or write the real `%APPDATA%\manifold`. Every path into it starts from `profile::dir()` (or `profile::view_data_dir()`, the `etcetera` directory inside it), both `None` under `cfg(test)`, so `App::default()` in a test loads defaults. A test that needs files uses `store::testutil::temp_dir` and passes the path in. `profile::tests` fails if any other source file mentions `"APPDATA"` or `etcetera`.

## Tagging

Tag only with `target/debug/gate.exe release-tag vX.Y.Z` (after `cargo build -p gate`) on the bump commit, once the pre-tag gate below has passed on it; never build the tag message by hand. Then push on request. See README "Releases".

## Gate

Before merging or tagging, run `target/debug/gate.exe all LOG full` (sync, store, mirror, session or engine changes), `light` (UI code that writes the store or reaches an engine, scripts) or `ui` (drawing, input and view-state modules, docs; `target/debug/gate.exe tier` advises) after `cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate` (`-p manifold` alone leaves the WebView2 host and PDF view old, and the stages run `target/debug/gate`), detached at BelowNormal; see README "Headless mode". Report CPU load and Defender state with the result. Launch it through `~/.claude/runs/run.ps1` (RUNS.md), which records both: `-Kind gate -Command 'cargo build ... && target\debug\gate.exe all \"%RUN_DIR%\gate-log.txt\" full'` (quote the log path, it has a space), started with `Start-Process`; then set the row's headline from the log's last lines.
