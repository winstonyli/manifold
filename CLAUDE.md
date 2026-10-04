# manifold

## Branches

Work on `dev`, never on `main`. `main` moves only when a release merges `dev` into it (`git merge --no-ff dev`, then a `vX.Y.Z` tag), and only when the user asks for a release. New branches and worktrees start from `dev` and merge back into `dev`. See README "Branches".

## Formatting and lints

Run `cargo fmt --all` before each commit. `cargo fmt --all --check` and `cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 --all-targets -- -D warnings` must pass (CEF is left out: its build script needs the CEF download; WebKit and Servo don't build on Windows); `.githooks/pre-commit` checks both when Rust files are staged (enable once per clone: `git config core.hooksPath .githooks`). Never bypass it with `--no-verify`. Don't silence a lint to pass: fix it, or `#[allow]` it with a comment saying why. The style is `rustfmt.toml` (width 120, `use_small_heuristics = "Max"`). Mark an item `#[rustfmt::skip]` only where hand layout carries meaning, such as an enum of one-line struct variants (a doc comment on any variant makes rustfmt expand them all). The workspace-wide reformat is listed in `.git-blame-ignore-revs`; run `git config blame.ignoreRevsFile .git-blame-ignore-revs` once per clone.

## Tests and the user's profile

No test may read or write the real `%APPDATA%\manifold`. Every path into it starts from `profile::dir()` (or `profile::engine_data_dir()`, the `etcetera` directory inside it), both `None` under `cfg(test)`, so `App::default()` in a test loads defaults. A test that needs files uses `store::testutil::temp_dir` and passes the path in. `profile::tests` fails if any other source file mentions `"APPDATA"` or `etcetera`.
