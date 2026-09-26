# manifold

## Branches

Work on `dev`, never on `main`. `main` moves only when a release merges `dev` into it (`git merge --no-ff dev`, then a `vX.Y.Z` tag), and only when the user asks for a release. New branches and worktrees start from `dev` and merge back into `dev`. See README "Branches".

## Formatting

Run `cargo fmt --all` before each commit; `cargo fmt --all --check` must pass. The style is `rustfmt.toml` (width 120, `use_small_heuristics = "Max"`). Mark an item `#[rustfmt::skip]` only where hand layout carries meaning, such as an enum of one-line struct variants (a doc comment on any variant makes rustfmt expand them all). The workspace-wide reformat is listed in `.git-blame-ignore-revs`; run `git config blame.ignoreRevsFile .git-blame-ignore-revs` once per clone.
