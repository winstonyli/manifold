# Releases on GitHub Actions (design)

Status: approved 2026-10-16; P1 (CEF) implemented per `docs/superpowers/plans/2026-10-16-ci-release.md`, whose "Rulings against the spec" are folded in below. Supersedes the local `scripts/release.sh` flow
(`docs/superpowers/plans/2026-10-11-bare-exe-host-downloads.md`, decisions 7 and 11).

## Why

A release today is cut on the developer's machine: `scripts/release.sh` builds the downloadable engine hosts
(CEF, Servo, WebKit) and the embedded ones (WebView2, PDF view), hashes the downloadable ones into
`crates/manifold/src/host_pin.rs`, builds `manifold.exe` with the pins compiled in, and the developer then commits the
pins, tags, uploads. Costs seen on 2026-10-16: the build competes with other work for CPU and disk (C: had 29 GB
free; Servo needs 30), it depends on machine traps (`depot_tools` on `PATH`, WSL for WebKit, `bash` resolving to WSL's),
and the ordering is easy to get wrong. The tag was pushed before the pins existed, so the pushed `v0.6.0` points at a
commit whose pins do not match the exe built from the next one. A fix needed a force-push of a published tag.

The ordering problem is structural. A pin holds the hash of a host file, so the host must be built before the commit
that pins it, and that commit must precede the tag. A workflow triggered by a tag push cannot produce the pins.

## Decisions

1. **Hosts and the app release separately.** Downloadable hosts are published by their own workflow under a name
   derived from their sources. The app release only builds the exe and references hosts that already exist. The pin
   commit is an ordinary commit made before tagging, so the tag always contains the pins its exe was built with.
2. **A host's identity is a source fingerprint.** `gate host-fingerprint KIND PLATFORM` prints 12 hex digits (SHA-256
   truncated) over everything that determines the host binary:
   - the host package's resolved dependency closure from `cargo metadata --locked` for that target: for each path
     package, the git blob ids and modes of its tracked files (`git ls-tree -r HEAD <dir>`); for each registry or git
     package, name, version and source (a registry version is immutable; `cargo metadata` has no checksum); every
     package also contributes its resolved features, and a workspace crate does not contribute its version (the
     workspace version would change every host each release);
   - the toolchain (`rust-toolchain.toml` content), `.cargo/config.toml` and the root `Cargo.toml`'s `[profile*]`,
     `[patch*]` and `[replace]` tables;
   - the build recipe: the flags and environment the workflow builds that host with, taken from one file the
     workflow and the fingerprint both read (`.github/hosts.json`, decision 6).
   The digest reads committed state: uncommitted changes in those inputs are an error unless `--allow-dirty`. Cargo
   unifies features across the workspace, so the fingerprint can change for an edit that does not reach the host
   (an extra rebuild, never a stale host).
   Because the closure comes from cargo, the workspace version string, `Cargo.lock` entries of unrelated crates and
   edits to other hosts do not change a host's fingerprint, and a `cef` or `ipc-protocol` change does. No per-crate
   list is kept. `PROTOCOL_VERSION` is inside `ipc-protocol`'s files, so it is covered.
3. **Hosts live in per-kind releases named `hosts-KIND-FP`** (for example `hosts-cef-3fa9c01d27ab`), one per
   fingerprint, public, holding `FILE-PLATFORM` assets and a `pin.json` (kind, platform, fingerprint, per-file name,
   size, SHA-256). The hosts workflow skips the build when that release exists with its `pin.json`. Host releases are not drafts (`gh`
   cannot look a draft up by tag): the workflow creates the release with the host files, downloads them back and
   verifies, uploads `pin.json` last, and deletes the release on any failure; a release without `pin.json` counts as
   absent and is replaced.
4. **Pins carry the fingerprint.** A `Component` URL is `.../releases/download/hosts-KIND-FP/FILE-PLATFORM`. The pin
   table's validity is checkable without the network: for each entry, the fingerprint in its URL must equal
   `gate host-fingerprint` of the current tree. `gate host-pin --check` does this and runs in `ci.yml` on every push, so
   a source change that needs new hosts fails CI before anyone tags. The SHA-256 stays the trust anchor, as now.
5. **`gate host-pin --adopt KIND PLATFORM`** replaces `--asset`/`--keep-from`: it downloads `pin.json` from the
   current fingerprint's hosts release, downloads the files, verifies their hashes, and rewrites the table. The
   developer commits the result. Reuse is therefore the only path: there is no `--reuse-hosts-from`.
6. **Two workflows, and a build recipe file.**
   - `hosts.yml` (`workflow_dispatch` with `kind`, plus `push` to `main` when host paths change): per kind and platform
     it computes the fingerprint, exits if the release exists, builds from `.github/hosts.json`, creates the release,
     downloads the assets back and checks the hashes, then uploads `pin.json` (decision 3).
   - `release.yml` (`push` of tag `v*`): described next.
   - `.github/hosts.json` (JSON, not TOML: `serde_json` is already a `gate` dependency and PowerShell reads it) lists, per kind and platform: runner, package, extra RUSTFLAGS/features/env, and the files
     to publish. The workflow reads it directly; the fingerprint hashes the same recipe.
7. **`release.yml` on a `vX.Y.Z` tag** (windows-latest), in order, failing closed:
   1. `gate release-check`: the tag is annotated, its message is `manifold vX.Y.Z`, a blank line and the
      `CHANGELOG.md` entry's body (made with `git tag -a --cleanup=verbatim`, or git drops the `###` headings), and
      `Cargo.toml` matches the tag;
   2. `gate host-pin --check` (decision 4) and that each pinned asset is downloadable with the pinned size and hash;
   3. build the embedded WebView2 host and PDF view with the static CRT, then `manifold.exe` with
      `MANIFOLD_WEBVIEW2_HOST` and `MANIFOLD_PDFIUM_HOST` (as `release.sh` step 4);
   4. `gate package` on the exe (headless; the runner has no GPU) and `gate release-verify`;
   5. create a **draft** GitHub release with the exe, its `.sha256`, the embedded hosts' hash records and the table
      hash in the notes. The developer publishes it after looking at it. Nothing is public before that.
8. **`gate release-verify`** is rewritten around the draft release: it checks the tag's pin table against the hosts
   releases (existence, sizes, hashes) and the exe against its `.sha256`, instead of a local `dist/hosts`. The embedded
   hosts' recorded hashes remain, since they are not downloadable and have no pin row.
9. **Delete what this supersedes.** When a kind's CI path works, its host building leaves `release.sh`; when the last
   leaves, `scripts/release.sh`, `dist/hosts`, `--reuse-hosts-from`, `--asset`/`--keep-from` and the README's local
   packaging text go. No shim keeps both paths for one kind.
10. **`PIN_VERSION` goes.** The table is no longer cut for an app version: `host_pin.rs` loses `PIN_VERSION`, its test
    `check_table(TABLE, PIN_VERSION, CARGO_PKG_VERSION)` and `release-verify`'s `PIN_VERSION is X` check, and
    `host-pin --check` (fingerprint freshness) replaces them. A host and the app agree through `PROTOCOL_VERSION` only
    (the handshake in `ipc-protocol/src/channel.rs` sends nothing else), so a host built at an older workspace
    version works.
11. **Failure modes are loud.** A tag pushed before its pins exist fails `release.yml` at step 2 with the stale entry
    named, instead of shipping a mismatched exe. A push that changes a host's sources turns `ci.yml` red until the new
    hosts are built and adopted; that is the intended signal. `hosts.yml` runs under a concurrency group per
    fingerprint so two runs cannot race to create one release. Workflows get `contents: write` only in
    `hosts.yml` and `release.yml`.
12. **Tagging stays manual.** The developer bumps the version and writes the `CHANGELOG.md` entry, commits, pushes,
    and pushes the annotated tag (`git tag -a vX.Y.Z -F entry`). A release-PR bot is a separate, later change.

## Release procedure after this lands

1. Change code. If `ci.yml`'s pin check fails, a host's fingerprint changed: run `hosts.yml` for that kind
   (or let the push trigger run), then `gate host-pin --adopt KIND PLATFORM`, commit.
2. Bump the version and `CHANGELOG.md`, commit, push `main`.
3. `git tag -a vX.Y.Z -F entry && git push origin vX.Y.Z`. `release.yml` runs and leaves a draft.
4. Read the draft, publish it.

## Phasing

Each phase is separately useful and leaves no half-state.

- **P1:** fingerprint, `hosts.toml`, `hosts.yml` for CEF (windows-x64), `host-pin --check/--adopt`, `release.yml`,
  `release-verify` rewrite. CEF only, as in v0.5.1 and the attempted v0.6.0. Servo and WebKit read Unavailable.
- **P2:** Servo (windows-x64, not static CRT, ANGLE DLLs as additional files). Expected 1 to 2 hours on a hosted
  runner, inside GitHub's 6-hour job limit; Servo's 30 GB disk need must be checked against the runner's free space.
- **P3:** WebKit (linux-x64, `ubuntu-latest` with the WebKitGTK development packages; replaces the WSL build).
- `scripts/release.sh`, `--asset`/`--keep-from` and `dist/hosts` were deleted in P1: the new URL form breaks them, so
  Servo and WebKit read Unavailable until P2 and P3. P3 also needs `fingerprint.rs` and `pin.rs` to build on Linux
  (they are `cfg(windows)` today).

## Open points

- **v0.6.0.** The pushed tag has no matching pins and no release. The first CI release is `v0.6.1`, with a
  changelog line saying `v0.6.0` was never released. The alternative is moving the tag, which needs a force-push of a
  published tag and so the maintainer's explicit go-ahead.
- **Working tree.** `crates/manifold/src/host_pin.rs` is dirty from the local `release.sh` run (v0.6.0 URLs and a
  CEF host hash that was never uploaded). It is discarded with `git checkout`, not committed.
- **CEF in CI.** `cef-dll-sys` downloads the CEF distribution during the build and `depot_tools` is absent on the
  runner, so the local PATH workaround should not be needed; unverified until the first run.
- **Fingerprint holes.** Anything that changes a host binary but is neither a path file, a locked dependency, the
  toolchain nor `hosts.toml` would let a stale host be reused (for example a `build.rs` reading an environment
  variable). P1 includes a test that builds a host twice from the same tree and compares fingerprints, and the
  reviewer of `hosts.toml` is told to list any build-affecting environment variable there.
- **Runner differences.** `gate package` runs headless on a GPU-less runner; the engine-launching checks that need
  a real WebView2 runtime rely on the preinstalled one on `windows-latest`. Unverified until the first run.

## Testing

- Unit tests in `crates/gate`: fingerprint stability (same tree, same digits), sensitivity (touch a host file, bump a
  locked dependency, change `hosts.toml`: digits change; bump the workspace version, edit another host's file: digits
  unchanged), `host-pin --check` against a table with a stale fingerprint, `--adopt` against a local directory
  standing in for the release.
- `hosts.yml` and `release.yml` are exercised by running them: P1 ends with a real `v0.6.1` draft release whose exe
  passes `gate package` on the runner and, downloaded, on this machine.
