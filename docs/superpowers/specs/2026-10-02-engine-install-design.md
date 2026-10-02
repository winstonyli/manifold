# On-demand engine install (CEF) — design

Status: draft for review, 2026-10-02. Nothing is built. Survey and decisions came from a brainstorm; the README's "Speculative groundwork" entry has the earlier survey.

## Goal

A packaged build ships `manifold`, `manifold-engine-webview2` and the small `manifold-engine-cef` host (about 1 MB), and downloads CEF's runtime (about 172 MB compressed, about 430 MB unpacked in the debug tree) only when a user picks CEF. Engine availability is always visible and explicit: no engine is ever silently missing, silently installing, or silently substituted.

## Scope

- CEF only, Windows first. Servo stays bundled (decided). WebView2 is the default and always present. WebKit is unchanged (WSL/macOS-native).
- Out of scope: installer integration, a package-manager add-on, delta updates, a hosted manifest or mirror.
- Open product question, not an engineering one: whether packaged builds offer CEF at all (WebView2 is already Chromium). This spec only applies if they do.

## Decisions (and the alternatives dropped)

- **Source: CEF's own CDN, pinned in the app.** `https://cef-builds.spotifycdn.com/index.json` lists every build with size and SHA-1; the `minimal` archive for CEF 152.0.6 is `…_windows64_minimal.tar.bz2` (171.6 MB; verified 2026-10-02 to answer ranged GETs). Hosting nothing beats a hosted manifest (needs a signing key, and the exact `cef` crate pin means no independent bumps anyway) and a full mirror (re-upload per bump). A GitHub Release mirror is the fallback only if the CDN proves to prune builds.
- **Trust anchor: our own SHA-256, compiled into `manifold`.** SHA-1 from the index is checked too but is not the anchor. A compromised CDN would have to match a hash that ships in the signed app.
- **Bring-your-own runtime** is also supported (a "CEF directory" setting), validated against the pin.

## 1. Pin

A table in the app: CEF version string, archive URL, size, SHA-1, SHA-256. A release script (`scripts/cef-pin.sh`) reads `index.json`, downloads the archive once, computes the SHA-256, and writes the table. A test fails when the pinned version differs from the `cef` crate's CEF version, so a crate bump cannot ship with a stale pin.

## 2. Engine states, always shown

Each engine is in exactly one state, computed in one place (`engine_availability(kind)`), and every place that lists engines shows it:

| State | Meaning | Shown as |
|---|---|---|
| **Installed** | present and matching the app's pin (bundled engines always) | no extra marker beyond the name |
| **Not installed** | needs the download | "Not installed · 172 MB" with a Download button |
| **Installing** | download or unpack in progress | progress bar with bytes and Cancel |
| **Update needed** | installed for an older pin (after an app update) | "Update needed · 172 MB" with an Update button |
| **Failed** | last install failed | the reason and a Retry button |
| **Custom** | using a user-chosen CEF directory | the path, with its validation result |

Where it appears: the Settings default-engine picker, the Settings Current Tab picker, the Tab-view Split picker (all three share `build_anchored_engine_picker`), the engine cards in `engine_info.rs` (a state line under each name), and the Settings page's own "Engines" section (installed size, Remove). An engine that isn't Installed is still selectable; choosing it starts the install flow (below), never a silent no-op or a silent fallback to another engine.

A tab, session or restored node that needs a missing engine shows its card as "CEF not installed" with a Download button (the existing failed-tab card with Retry/Open in Default Engine already has this shape), not a generic spawn error.

## 3. Install flow

1. Trigger: choosing the engine, or the card's Download button, or opening a session that uses it. Always preceded by a confirm that states the size ("Download CEF, 172 MB?"), because it is a large network action.
2. Download to `engine-hosts/cef-runtime.part` with HTTP Range resume; progress shown as in section 2 (reusing the download-card UI).
3. Verify SHA-256 (and SHA-1) of the archive before unpacking anything.
4. Unpack only `Release/` and `Resources/` (bz2 + tar) into a temp directory next to the final one; drop build leftovers (`include/`, `cmake/`, `libcef_dll/`, `libcef.lib`, `CMakeLists.txt`, `debug.log`); write a `ready` marker holding the pin; rename the directory into place.
5. Failure at any step deletes the temp directory and `.part` only when verification failed (keep `.part` for resume on network errors), sets state Failed with the reason, and leaves any previous install untouched.
6. Cancel stops the download and keeps `.part`.

## 4. Lookup

`engine_binary_path` (`process_manager.rs`) keeps its sibling-then-`engine-hosts/` order. The CEF host exe ships with the app; the runtime is resolved from the data directory, and only when the `ready` marker matches the pin. A stale or missing marker yields state Not installed or Update needed, never a spawn that crashes.

## 5. Management

Settings shows installed size per downloaded engine and a Remove button (deletes `cef-runtime/`). An app update that changes the pin turns the state to Update needed; the old runtime keeps working until the user updates, unless the host/runtime protocol is incompatible (then the engine is unusable until updated, and the state says why).

## 6. Bring-your-own

A "CEF directory" setting accepts a runtime directory. Validation reads its `archive.json` (or `libcef.dll` version) against the pin; a match is state Custom, a mismatch shows the found and expected versions and refuses it.

## Testing

The download-and-verify unit takes its URL from the pin, so tests point it at a local HTTP server (like `scripts/probe_server.py`): resume after a cut connection, a wrong hash rejected, no partially unpacked directory ever visible at the final path, a stale marker reported as Update needed, a custom directory accepted and refused. `engine_availability` is a pure function over the filesystem state and the pin, unit-tested per state. `scripts/chrome-shots.sh` gains views for each state in the picker and the Settings Engines section.

## Open questions

- Release-build runtime size (only the debug tree was measured), and whether the `signed` archive (136 MB) holds everything and is worth preferring.
- Whether to trim locales or other files after unpack.
- Where the Settings "Engines" section lives relative to the existing sections.
- Servo and WebKit availability: both always "Installed" for now; the same states apply if either ever becomes downloadable.
