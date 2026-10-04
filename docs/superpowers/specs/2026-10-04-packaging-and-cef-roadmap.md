# Packaging and CEF versioning: decisions and parked work

Status: v0.2.0 ships the small packaging fixes (step 1). Steps 2 and 3 are parked until the user picks them up.

## Decisions (2026-10-04)

- Audience today: the author only; keep options open for others later.
- Platforms: Windows now, Linux maybe.
- Releases: built both by GitHub Actions on a tag and locally by `scripts/package.sh`. Pushing the repo to GitHub is acceptable (nothing is pushed yet; `dev` is ~300 commits ahead of `origin/dev`).
- Shape: a portable zip (no installer, no signing, no auto-update) with CEF's runtime downloaded on demand.
- CEF runtime compatibility (done, in `2026-10-02-engine-install-design.md`, "Runtime compatibility"): a runtime of the pin's major or later is accepted while it keeps CEF API 15200 in its supported range; an older one is "Update needed" and a host on an unsupported API fails cleanly. Verified live on 154.0.33 (accepted) and 151.3.24 (clean failure), Windows, 2026-10-04.

## Parked 1: GitHub CI plus automatic re-pinning (CEF "Phase 2")

- Workflow on tag: build release, run `package.sh --check`, attach the zip to a GitHub release.
- A scheduled job that checks `index.json` for the pinned CEF major's newest build, regenerates `crates/manifold/src/cef_pin.rs` (version, URL, SHA-256) and opens a PR, so a re-pin is a review, not a chore.
- Mirror the pinned archives (release assets) so a CEF CDN change does not break installs.
- Pain it removes: re-pinning is heavy (pin file, hash, docs) and the runtime was tied too tightly to the app version.

## Parked 2: packaging strategy spec

- Write the full spec: portable zip, CI on tag, local `package.sh`, and the CEF options (A: pin per app release as today, B: accept same-or-later major as now, C: matched packs per app version, only if B proves insufficient).
- Open questions: Linux packaging (AppImage or tarball), a code-signing story if others will run it, a Servo host in the zip (a product decision; Servo is "Unavailable" when its host is missing), and CEF "Phase 3" matched packs.
