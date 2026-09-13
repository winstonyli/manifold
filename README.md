# multiengine-browser

A browser where each tab can run on a different rendering engine — Blink (via CEF), Servo, or WebKit (via WebKitGTK) — and the engine backing a tab can be swapped live without closing it.

Status: M0–M4 complete — single-engine tabs on all three engines, live engine swap, a real chrome (address bar, back/forward, engine picker), and crash recovery, all verified end-to-end. Full milestone plan: `C:\Users\Winston Li\.claude\plans\stateless-napping-sunset.md`.

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

### Manual runtime-file placement (required after every build)

This step is not automated yet — see the `TODO` in `crates/browser-ui/src/process_manager.rs`. Both CEF and Servo need extra files placed by hand after each build, because they'd otherwise collide (both ship an ANGLE `libEGL.dll`/`libGLESv2.dll` under the same filenames, and Windows resolves an implicit DLL dependency from the loading EXE's own directory first — whichever copy landed there last silently wins for *both* engines).

1. **Servo's ANGLE DLLs** — copy into `target/debug/` (next to `engine-host-servo.exe`):
   ```bash
   D="target/debug"
   HASH_DIR=$(ls -dt "$D"/build/mozangle-*/out | head -1)
   cp "$HASH_DIR/libEGL.dll" "$HASH_DIR/libGLESv2.dll" "$D/"
   ```

2. **CEF's full runtime**, isolated in its own subdirectory so it can't collide with Servo's copies above:
   ```bash
   D="target/debug"
   mkdir -p "$D/cef-runtime"
   HASH_DIR=$(ls -dt "$D"/build/cef-dll-sys-*/out/cef_windows_x86_64 | head -1)
   cp -r "$HASH_DIR"/* "$D/cef-runtime/"
   cp "$D/engine-host-cef.exe" "$D/cef-runtime/engine-host-cef.exe"
   ```

Re-run both steps after **any** rebuild — not just an obvious trigger like `cargo clean` or a `Cargo.toml` edit. In practice, Cargo has regenerated these fingerprint hashes (a new `mozangle-<hash>`/`cef-dll-sys-<hash>` directory) even for a rebuild that changed neither `Cargo.toml` nor `Cargo.lock`, leaving the manually-placed copies pointing at stale, orphaned build output. The old copies keep working right up until they silently don't (Servo: panics with `egl function was not loaded`; CEF: silently runs stale code). Don't trust file timestamps to tell you they're stale, either — `cmp` the placed copy against the current hash directory's output directly:
```bash
D="target/debug"
HASH_DIR=$(ls -dt "$D"/build/mozangle-*/out | head -1)
cmp "$HASH_DIR/libEGL.dll" "$D/libEGL.dll" || echo "stale, recopy needed"
```
A rebuild also tends to leave the *previous* hash's directory behind alongside the new one (Cargo doesn't clean up an old fingerprint just because it's no longer used) — these accumulate across sessions and are a repeat cause of this project filling the disk. Prune anything under `target/debug/build/{mozangle,cef-dll-sys}-*` except the newest of each once you've confirmed the copies are up to date.

## Running

```bash
target/debug/browser-ui.exe
```

Starts a single tab in Servo, loading `https://example.com`. The top strip is the tab (title/loading indicator); below it, the toolbar has back/forward buttons, an engine-picker dropdown (swaps the tab live between Servo/CEF/WebKit), and an address bar (type a URL, press Enter). The URL carries over on an engine swap; other state (cookies, scroll position, etc.) does not — a known v1 limitation, not a bug.

If an engine-host process dies (crashes, or is killed), the tab automatically respawns the same engine at its last-known URL — you don't need to do anything.

Each engine-host writes its own log to `%TEMP%\multiengine-browser-<tab_id>.log`.

## Known limitations

- Single tab only (M5 — multi-tab — not started).
- Session state (cookies, localStorage, IndexedDB) is not portable across an engine swap — only the URL carries over.
- WebKit tab input (mouse/keyboard) is not wired up yet — rendering only. Servo and CEF handle mouse; keyboard input isn't wired up on any engine yet.
