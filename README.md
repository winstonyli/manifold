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

Starts on **Graph view**, with one root tab (Servo, loading `https://example.com`) already spawned and focused — click its node to enter Tab view. The graph is the primary view for managing multiple tabs:

- **Graph view**: every tab is a node (its last frame as a downscaled thumbnail, favicon, title, current engine) connected by an edge to the tab it was opened from. Hover a node to peek an enlarged preview without changing focus; click a node to focus it (switches to Tab view); drag a node to reposition it (a force-directed layout otherwise keeps nodes spread out and settles on its own — edges settle to a rest length of 320 points, up from 180, so sibling nodes no longer crowd/overlap each other at equilibrium); the small "x" at a node's corner closes that tab, reparenting its children to its own parent. "+ New Tab" opens a child of whichever tab is currently focused (or a new root if none is). Drag empty background to pan, scroll to zoom (centered on the cursor); "Reset View" returns to the default framing, now at a closer-in default zoom (the scene's default world-space extent shrunk from 1600×1100 to 900×620) so nodes read larger on first open instead of needing an immediate manual zoom-in.

Switching between Graph and Tab view (clicking a node, or the "Graph"/Escape button) animates as a 220ms shrink/expand: the transitioning tab's live content morphs between full-window size (Tab view) and its graph node's thumbnail-sized rect (Graph view), smoothstep-eased, rather than cutting instantly. This reuses the exact same world→screen transform the graph's own `Scene` container uses to place nodes (replicated by hand in `App::graph_world_to_screen_transform`, matching egui 0.36's internal `fit_to_rect_in_scene` formula — not exposed publicly, so not a dependency, just the same math) so the animated rect lands exactly on the real node position, not an approximation. During the animation the app stays logically in Graph view (chrome, physics tick, node layout all keep running underneath) with only the one transitioning tab's content quad drawn at an interpolated viewport rect, layered *on top of* the graph (not underneath it) so the shrinking/expanding content is never hidden behind other nodes/edges as it animates.
- **Tab view**: today's chrome for whichever tab is focused — address bar (type a URL, press Enter), back/forward, an engine-picker dropdown (swaps the tab live between Servo/CEF/WebKit), favicon next to the title. The "Graph" button (or Escape) returns to the graph.

A gear icon (in either view's toolbar) opens **Settings** — default search engine (DuckDuckGo/Google/Bing; only where a search actually goes, suggestions always come from DuckDuckGo's own endpoint regardless), default starting engine for new tabs, zoom (a linear multiplier, applies to new tabs and immediately to whichever tab is currently focused — not retroactively to other already-open tabs), and home page. Every control saves immediately on change (no separate "Save" step) to `%APPDATA%\multiengine-browser\settings.json`; a missing or corrupt file just falls back to defaults rather than failing to start. Zoom is implemented on all three engines: Servo (`WebView::set_page_zoom`, a direct linear factor) and CEF (`BrowserHost::set_zoom_level`, log-scale internally — converted via `log_1.2(factor)` so the same setting means the same visual size everywhere) were added along with this settings feature and verified live (visibly larger text at 130%, both engines); WebKit's `set_zoom_level` handler already existed from earlier work and receives the same message the same way — not re-verified live in this pass (see the WebKit-hang caution in project memory — avoided touching that engine-host this round), but nothing about it should differ.

Typing into the address bar shows an autocomplete dropdown below it: matching open tabs first, then DuckDuckGo search-query suggestions (debounced, fetched from DuckDuckGo's own `/ac/` autocomplete endpoint). Click a row to accept it — clicking an open-tab row focuses that tab, clicking a search row navigates to that search. Up/Down move a highlight through the rows (wrapping at both ends); Enter accepts whichever row is highlighted, or falls back to navigating the typed text directly if none is. Non-URL input (no scheme, no dot, or contains a space) is treated as a search query on plain Enter too, resolved the same way as the dropdown's own suggestions. Escape dismisses the dropdown without leaving Tab view; a second Escape (or with no dropdown showing) returns to Graph view as usual.

Only the focused tab ever actually renders — every other tab's engine-host process stays alive (so its page keeps running, JS timers and all) but stops producing frames until it's focused again; its node just shows the last frame it produced. New tabs are created via "+ New Tab", or automatically as a child node when a page opens one itself (`target=_blank`/`window.open`/ctrl+click) — CEF and WebKit both intercept and forward these; Servo does not (falls back to its own unhandled-popup default), see Known limitations.

If an engine-host process dies (crashes, or is killed) — focused or not — the tab automatically respawns the same engine at its last-known URL, keeping its place in the graph.

The URL carries over on an engine swap; other state (cookies, scroll position, etc.) does not — a known v1 limitation, not a bug.

Each engine-host writes its own log to `%TEMP%\multiengine-browser-<tab_id>.log`.

## Headless mode (automated testing)

```bash
target/debug/browser-ui.exe --headless
```

Runs the same app, but the window is positioned far off-screen (not `--visible(false)` — a genuinely invisible window's swapchain never successfully acquires a frame on Windows, DWM permanently reports it occluded; an off-screen-but-visible window keeps rendering normally while never overlapping the user's actual screen) and driven entirely by a text-command protocol read from stdin, one command per line, instead of real OS input. This exists so live GUI testing of this app doesn't have to fight Windows' foreground-lock (`SetForegroundWindow` can be silently refused while the user is active on their own machine — see project memory) or contend visually with whatever the user is doing.

Commands:
- `MOUSEMOVE <x> <y>`, `MOUSEDOWN <x> <y> [left|right|middle]`, `MOUSEUP <x> <y> [left|right|middle]`, `CLICK <x> <y> [left|right|middle]` — dispatched as genuine synthetic `winit::event::WindowEvent`s, not a logic bypass, so they exercise real egui hit-testing exactly like real input would.
- `SCROLL <x> <y> <delta_x> <delta_y>`
- `ADDRESSBAR <text>` — sets the address bar's text and focus state directly and recomputes suggestions (headless has no way to synthesize a real click-driven egui focus transfer, since `winit::event::KeyEvent` has no public constructor outside the winit crate — see `debug_control`'s doc comment).
- `ENTER` — navigates using the address bar's current text.
- `ESCAPE` — dismisses the suggestion dropdown, or leaves Tab view for Graph view if none is showing.
- `ARROWDOWN` / `ARROWUP` — moves the address-bar dropdown's highlighted row (wrapping at both ends); a following `ENTER` accepts it.
- `TYPE <text>` — sends each character to the focused tab's page (not the address bar).
- `KEYPAGE <name>` — sends a KeyDown+KeyUp of the named key (`Enter`, `Backspace`, `ArrowLeft`, a single letter/digit, etc.) to the focused tab's page.
- `SCREENSHOT <path>` — saves the next composited frame (chrome, any open dropdown, and content) as a PNG, read back directly from the GPU rather than `PrintWindow`/`CopyFromScreen` (see project memory's gotchas on GDI-handle exhaustion and foreground-lock unreliability with that approach).
- `RESIZE <width> <height>`
- `SETVISIBLE <true|false>` — sends `SetVisible` directly to the focused tab, bypassing `focus_tab`/`leave_to_graph`'s own visibility changes (which always come bundled with a `Resize`, since Tab vs. Graph view's chrome heights differ). For testing the background-tab suspend/resume path in isolation.
- `QUIT` — exits cleanly. Also sent automatically on stdin EOF, so a driving script that closes its write end doesn't leave the process hanging.

`scripts/headless-smoke-test.sh` is a runnable example (and a real regression smoke test): drives the address-bar autocomplete flow end to end via a bash `coproc` and asserts each screenshot was actually produced and isn't suspiciously small — not full visual-regression testing (there's no state-query debug command yet to assert exact content), but enough to catch a crash or total breakage. Run it after `cargo build -p browser-ui`. Note for anything driving this from Git Bash/MSYS: a `SCREENSHOT` path travels over stdin, not argv, so it's not auto-translated from a POSIX path (`/c/Users/...`) the way a command-line argument to a spawned native exe would be — convert with `cygpath -w` first.

## Known limitations

- Session state (cookies, localStorage, IndexedDB) is not portable across an engine swap — only the URL carries over.
- **WebKit tab input (mouse, keyboard)** is wired up via synthetic GDK events, same as Servo/CEF. An earlier attempt at this caused a runaway-CPU event storm and was reverted (see project memory) — the fix was attaching a real `GdkDevice` to each synthetic event (`gdk_event_set_device`), which the first attempt never did. Verified via repeated live keystrokes/clicks with no CPU escalation, and (after the frame-freeze fix below) correct on-screen text — no dropped or doubled characters. Scroll is implemented the same way but not yet cleanly live-verified (test-harness input-injection reliability issues, not a known code problem).
- **Fixed:** an engine-swapped-to-WebKit tab's rendered frame used to go permanently stale after respawn (title/URL kept updating, the visible content never did) — `GtkOffscreenWindow`'s backing surface never actually adopted a post-spawn `Resize` message's size, so the old size-matching gate on sending `FrameReady` never passed even once. Fixed by sending whatever size the captured pixbuf actually is, rather than requiring it to match what was requested. Verified live (a real page navigated to and rendered correctly after an engine swap).
- Page-triggered new tabs (ctrl+click, `target=_blank`, `window.open`) work for CEF and WebKit tabs — verified live for WebKit (clicking a `target=_blank` link spawned a real child node in the graph at the right URL). Servo tabs don't send these — its popup-interception API doesn't fit this project's process-per-tab architecture (see `ipc_protocol::EngineToHost::OpenTabRequested`'s doc comment). **Confirmed safe, not just assumed:** clicking a `target="_blank"` link on a Servo tab is a clean no-op — `servo`'s `WebViewDelegate::request_create_new` has an empty default body when left unimplemented (as this project leaves it), so the click registers (visible focus outline) but nothing else happens — no crash, no hang, no phantom tab, no same-tab navigation, confirmed via a live headless-mode test with a `data:` URI test page.
- Favicons show for Servo. WebKit's favicon signal needs a `WebContext` favicon-database directory set to fire at all (now set) but real favicons still weren't observed being fetched/stored in live testing under this WSL2/Xvfb setup — root cause not found, not working yet. **CEF favicons: root cause found, not fixable from this project's side.** `on_favicon_urlchange` genuinely fires (confirmed with a fully-loaded, screenshotted real page, e.g. github.com) but its `icon_urls` list is always empty. Per CEF's own issue tracker (`chromiumembedded/cef` #3293/#3685), `windowless_rendering_enabled` forces CEF's legacy "Alloy" runtime (the modern "Chrome" runtime doesn't support off-screen rendering at all), and Alloy never fully wired up the chrome-layer favicon-candidate collection a normal windowed browser gets — the callback exists and is called, it just never carries real candidates in windowless mode. Not something a handler-side fix can work around; left as a graceful no-op (cosmetic, never load-bearing) with diagnostic logging kept in place in case CEF's behavior here ever changes.
- Graph-view thumbnails are downscaled (box-sampled to at most 256×192) before upload — Tab view's own live content stays full resolution.
- **Fixed:** two bugs found while building address-bar autocomplete. (1) The address bar's typed text used to snap back to the tab's real URL mid-keystroke — egui's own focus tracking transiently leaves the `TextEdit` for one frame the moment the suggestion dropdown's buttons first appear, so a naive `if !focused { resync }` fired on that blip; fixed by also treating non-empty suggestions as "still editing." (2) The exact same blip also hid the dropdown's own `Area` (buttons included) on the one frame a click landed on a row, silently swallowing the click — clicking a suggestion looked like it did nothing. Fixed the same way: gate the dropdown on the broader "still editing" check, not raw focus. (3) Separately, the very first HTTPS request `browser-ui`'s process ever makes (`ureq`/`rustls`'s one-time TLS setup plus a cold DNS+TLS handshake) was slow enough, the first time it happened to coincide with the user's first search query, to trip Servo's engine-host into a crash/respawn loop; fixed with a throwaway warm-up fetch at startup so that one-time cost is absorbed before the user can hit it.
- **Fixed:** a background Servo tab refocused from Graph view didn't repaint — content stayed black indefinitely even though chrome (title, spinner) kept updating and the engine-host process was alive. Root cause: `SetVisible(true)`'s capture-and-`FrameReady`-send block was gated on `notify_new_frame_ready` having fired again on its own, which never happens for a static page that received no input while hidden — the gate stayed shut forever. Fixed by forcing one repaint on the `false`→`true` transition instead of waiting for Servo to decide something changed. **Verification gap, now narrowed further (not fully closed):** added a `SETVISIBLE <bool>` headless debug command specifically to toggle a tab's visibility with zero `Resize` involved (the earlier suspicion — that `focus_tab`'s always-bundled `Resize` was masking the race — needed a way to rule it out). With `Resize` fully out of the picture, a negative-control test (temporarily reverting the fix, rebuilding, retesting) *still* didn't reproduce a black screen — meaning `Resize` wasn't the (or wasn't the only) masking factor. Something in Servo appears to keep re-arming `notify_new_frame_ready` periodically even for a page that looks static (a compositor heartbeat, caret blink, or similar), so `frame_ready` is essentially never caught sitting `false` at the exact instant a real toggle happens within a scripted multi-second window. The fix remains correct by inspection and provably safe either way (worst case: one harmless extra repaint) — the `SETVISIBLE` command is kept as a tool for whoever next has better luck (or a way to force Servo fully idle first) pinning down the precise repaint cadence needed for a deterministic repro.
