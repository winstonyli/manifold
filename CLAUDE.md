# manifold

## Branches

Work on `dev`, never on `main`. `main` moves only when a release merges `dev` into it (`git merge --no-ff dev`, then a `vX.Y.Z` tag), and only when the user asks for a release. New branches and worktrees start from `dev` and merge back into `dev`. See README "Branches".
