# multiengine-browser

A browser where each tab can run on a different rendering engine — Blink (via CEF), Servo, or WebKit (via WebKitGTK) — and the engine backing a tab can be swapped live without closing it.

Status: M0–M3 complete (single-engine tabs on all three engines, live swap between them, verified end-to-end). No tab UI yet — see [Running](#running) for the current test-only controls. Full milestone plan: `C:\Users\Winston Li\.claude\plans\stateless-napping-sunset.md`.

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

Re-run both steps after any rebuild that could regenerate these dependencies with a new Cargo fingerprint hash (a `cargo clean`, a `Cargo.toml` change like a profile setting, a dependency bump) — the old copies keep working right up until they silently don't (Servo: panics with `egl function was not loaded`; CEF: silently runs stale code). Compare `stat` timestamps between the fresh `build/<crate>-<hash>/out/` directory and the manually-placed copy if anything looks wrong after a rebuild.

## Running

```bash
target/debug/browser-ui.exe
```

Starts a single tab in Servo, loading `https://example.com`. Press **S** to cycle the tab's engine: Servo → CEF → WebKit → Servo. The URL carries over on each swap; other state (cookies, scroll position, etc.) does not — this is a known v1 limitation, not a bug.

There is no tab UI, address bar, or crash recovery yet (M4). Each engine-host writes its own log to `%TEMP%\multiengine-browser-<tab_id>.log`.

## Known limitations

- Single tab only (M5 — multi-tab — not started).
- No crash recovery: if an engine-host dies, the tab does not auto-restart (M4).
- Session state (cookies, localStorage, IndexedDB) is not portable across an engine swap — only URL, history, and zoom are carried over, and history/zoom replay is itself still a stub (`TODO(M4)` in each engine-host).
- WebKit tab input (mouse/keyboard) is not wired up yet — rendering only.
