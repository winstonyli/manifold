# Downloadable hosts ride in the release (2026-10-18)

Supersedes the host half of `docs/superpowers/specs/2026-10-16-ci-release-design.md` (decisions 1 to 6: `hosts-KIND-FP`
releases, source fingerprints, `host_pin.rs` as a committed table, `gate host-pin --adopt` and `--check`).

## Decision

A release holds the base exe and every downloadable host. `release.yml` builds the hosts first, writes a manifest of
their names, sizes and SHA-256 hashes (`manifold-vX.Y.Z-windows-x64.host-pins.json`), and builds `manifold.exe` with
`MANIFOLD_HOST_PINS=<manifest>`. `build.rs` embeds the manifest and `host_pin.rs` parses it. A host's URL is its own
release's: `releases/download/v<exe version>/<file>-<platform>`. The hash compiled into the exe stays the trust anchor,
so an exe can only ever install the exact files its release was built with. The CEF runtime archive (`libcef`, from the
Spotify CDN) is not our file and keeps its own pin, `cef_pin.rs`, set by `gate cef-pin`.

Since the host table (2026-10-29, `docs/superpowers/specs/2026-10-29-host-table-design.md`, H5), `gate release-pins`
writes the manifest from the table's downloadable hosts and `release.yml` calls it; the decision itself is unchanged.

A build with no manifest (a dev build, the gate) pins nothing: its engines read Unavailable unless a sibling host
exists. A debug build also reads the file named by `MANIFOLD_HOST_PINS_FILE`, which is how the gate shows the download
states with a made-up host. A release build ignores that variable.

## Why

The old design committed the hash to git, so a host had to exist before the commit that pinned it, and that commit had
to precede the tag. Any edit to a host's dependency closure made the pin stale, and the staleness surfaced only on the
tag push (v0.7.0 failed `release-check` on a stale CEF pin and was never released). Producing the manifest in the same
job that builds the exe removes the ordering problem: there is nothing to commit before tagging, and host and exe come
from the same tag, so the protocol versions match by construction.

## Cost

Every release rebuilds every host (CEF now; Servo, about 33 minutes, and a Linux WebKit job later). PE output is not
bit-reproducible, so an upgrade re-downloads the small host exe (never the runtime, which is keyed by the CEF version).
Releases must never be deleted: an old exe fetches its host from its own tag. If rebuild time hurts, reusing a previous
release's asset when the dependency closure is unchanged can be added without rework.

## Decided against

- **Signing hosts with a CI-held key** and embedding the public key. The exe is unsigned, so trust bottoms out at the
  exe's hash either way; a signature accepts any file CI ever signed (no rollback protection) and needs min-protocol
  rules. A hash pins exactly one file.
- **Trusting the release URL** (download a `SHA256SUMS` from the same release). Simpler still, but the URL would be
  the trust anchor, which this project chose not to do.
- **Bundling CEF in the exe or an installer.** Policy since 2026-10-02: opt-in on demand, never bundled; CEF is hundreds
  of MB against a 27 MB release.
