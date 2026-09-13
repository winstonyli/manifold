# multiengine-browser

A browser where each tab can run on a different rendering engine — Blink (via CEF), Servo, or WebKit (via WebKitGTK) — and the engine backing a tab can be swapped live without closing it.

Status: M0–M5 complete — single-engine tabs on all three engines, live engine swap, a real chrome, crash recovery, and multiple tabs shown as a force-directed graph (peek to preview, click to focus), all verified end-to-end. Full milestone plan: `C:\Users\Winston Li\.claude\plans\stateless-napping-sunset.md`.

## Architecture

```
browser-ui           winit + wgpu compositor, process manager, swap logic
engine-host-servo     Servo, same-machine child process (named pipe + shared memory)
engine-host-cef       CEF (Blink), same-machine child process (named pipe + shared memory)
engine-host-webkit    WebKitGTK, hosted inside WSL2 (TCP transport, no shared memory across the VM boundary)
ipc-protocol          shared message types + both transports
```

Only `engine-host-webkit` is Linux-only (no Windows WebKit embedding exists); everything else builds natively on Windows.

## Prerequisites

- **Rust** (stable), via rustup.
- **Visual Studio 2022, 17.14 or newer.** Older toolsets are missing STL symbols that Servo's build needs — this is a hard requirement, not a suggestion.
- **A real `ninja.exe`** on PATH (`scoop install ninja`), for CEF's build script.
  - If a Chromium `depot_tools` checkout is also on your PATH, **exclude it** for this project's builds — its extensionless `ninja` shell script shadows the real `ninja.exe` and breaks the CEF build. See the "Building" commands below, which strip it automatically.
- **WSL2, distro `Ubuntu`**, with:
  ```bash
  sudo apt-get install -y libwebkit2gtk-4.1-dev libgtk-3-dev xvfb build-essential
  ```
  (Rust/cargo must also be installed inside that WSL2 distro, separately from the Windows-side toolchain.)

## Building

**Windows-side crates** (`browser-ui`, `engine-host-servo`, `engine-host-cef`) — from a regular shell at the repo root:

```bash
PATH=$(echo "$PATH" | tr ':' '\n' | grep -v -F "depot_tools" | tr '\n' ':') cargo build -p browser-ui -p engine-host-servo -p engine-host-cef
```

The `PATH` filtering is only needed if `depot_tools` is on your PATH; harmless otherwise.

**WebKit engine-host** — built *inside* WSL2, into its own target directory (it can't share `target/` with the Windows build — different OS, different toolchain):

```bash
wsl.exe -d Ubuntu -- bash -c 'cd "/mnt/c/Users/<you>/Documents/GitHub/multiengine-browser" && CARGO_TARGET_DIR=target-linux cargo build -p engine-host-webkit'
```

Adjust the path for wherever this repo actually lives on your machine (WSL2 sees your Windows drives under `/mnt/c/...`).

### Runtime-file placement (required after every build)

Cargo doesn't do this itself — see the `TODO` in `crates/browser-ui/src/process_manager.rs`'s `engine_binary_path`. Both CEF and Servo need extra files placed next to their binaries after each build, because they'd otherwise collide (both ship an ANGLE `libEGL.dll`/`libGLESv2.dll` under the same filenames, and Windows resolves an implicit DLL dependency from the loading EXE's own directory first — whichever copy landed there last silently wins for *both* engines).

Run this after every build that touches `engine-host-servo` or `engine-host-cef`:
```bash
bash scripts/sync-runtime-deps.sh
```
It finds Servo's ANGLE DLLs and CEF's runtime under `target/debug/build/{mozangle,cef-dll-sys}-*/`, `cmp`s each against the copy already in place, and only recopies what's actually stale (safe to run any time — a no-op if nothing changed). Not a build.rs: a build script only sees its own crate's `OUT_DIR`, not a sibling crate's, and there's no reliable cross-crate coordination here without those upstream crates cooperating (they don't) — a plain script is simpler and more honest than fighting Cargo's model.

Why this needs a script at all rather than a one-time manual copy: Cargo has regenerated the `mozangle-<hash>`/`cef-dll-sys-<hash>` fingerprint directory these live under even on a rebuild that touched neither `Cargo.toml` nor `Cargo.lock`, leaving a stale copy in place that keeps working right up until it silently doesn't (Servo: panics with `egl function was not loaded`; CEF: silently runs stale code). The script picks the candidate whose own output *file* has the newest mtime, not the containing directory's — a build touches every candidate directory's own fingerprint bookkeeping on every invocation regardless of whether it actually recompiled that crate, which can leave several near-identical timestamps and make directory-mtime-based "newest" ambiguous.

A rebuild also tends to leave the *previous* hash's directory behind alongside the new one (Cargo doesn't clean up an old fingerprint just because it's no longer used) — these accumulate across sessions and are a repeat cause of this project filling the disk. Safe to prune anything under `target/debug/build/{mozangle,cef-dll-sys}-*` except whichever one `scripts/sync-runtime-deps.sh` just used — re-run the script afterward to confirm nothing needed recopying. Getting this wrong (deleting the wrong one) just means the next build regenerates whatever's missing, not silent breakage — but it's wasted rebuild time.

## Running

```bash
target/debug/browser-ui.exe
```

Starts on the **graph view**, with one tab (Servo, loading `https://example.com`) shown as a node. This is the primary view for managing multiple tabs:

- **Graph view**: every tab is a node (its last frame as a thumbnail, title, current engine) connected by an edge to the tab it was opened from. Hover a node to peek an enlarged preview without changing focus; click a node to focus it (switches to Tab view); drag a node to reposition it (a force-directed layout otherwise keeps nodes spread out and settles on its own); the small "x" at a node's corner closes that tab, reparenting its children to its own parent. "+ New Tab" opens a child of whichever tab is currently focused (or a new root if none is).
- **Tab view**: today's chrome for whichever tab is focused — address bar (type a URL, press Enter), back/forward, an engine-picker dropdown (swaps the tab live between Servo/CEF/WebKit). The "Graph" button (or Escape) returns to the graph.

Only the focused tab ever actually renders — every other tab's engine-host process stays alive (so its page keeps running, JS timers and all) but stops producing frames until it's focused again; its node just shows the last frame it produced. New tabs are only ever created via "+ New Tab" — a page's own `target=_blank`/`window.open`/ctrl+click does not spawn a node (not implemented).

If an engine-host process dies (crashes, or is killed) — focused or not — the tab automatically respawns the same engine at its last-known URL, keeping its place in the graph.

The URL carries over on an engine swap; other state (cookies, scroll position, etc.) does not — a known v1 limitation, not a bug.

Each engine-host writes its own log to `%TEMP%\multiengine-browser-<tab_id>.log`.

## Known limitations

- Session state (cookies, localStorage, IndexedDB) is not portable across an engine swap — only the URL carries over.
- WebKit tab input (mouse/keyboard) is not wired up yet — rendering only. Servo and CEF handle mouse; keyboard input isn't wired up on any engine yet.
- New tabs only via the explicit "+ New Tab" action — no page-triggered new tabs (ctrl+click, `target=_blank`, `window.open`).
- A suspended tab's cached thumbnail is a full content-resolution RGBA buffer retained per background tab (not downscaled) — fine for tens of tabs, a real scaling limit past that.
