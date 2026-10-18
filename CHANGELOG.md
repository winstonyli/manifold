# Changelog

One entry per tagged release, newest first. The annotated tag `vX.Y.Z` carries the same text. Releases before v0.4.0 have no entry: see `git log --first-parent --decorate`.

## 0.7.1 - 2026-10-18

*v0.7.0 was tagged but never released (its release workflow failed, see Fixed); v0.7.1 is the first release with its changes.*

### Changed

- Ship the downloadable engine hosts in the release itself: `release.yml` builds them, writes a manifest of their sizes and SHA-256 hashes, builds `manifold.exe` with that manifest compiled in, and attaches each host as `<file>-<platform>` to the same release; the exe fetches `releases/download/v<its version>/<file>-<platform>` on demand and still checks the hash ([75811fb](https://github.com/winstonyli/manifold/commit/75811fb), [4e90905](https://github.com/winstonyli/manifold/commit/4e90905)). The CEF host is the only one built so far.

### Removed

- Remove the `hosts-KIND-FP` host releases, `hosts.yml`, the committed host pin table and `gate host-pin` / `gate host-fingerprint`; a stale host pin can no longer block a tag ([75811fb](https://github.com/winstonyli/manifold/commit/75811fb)).

### Fixed

- Make `gate release-check` and `gate release-notes` find a CHANGELOG entry headed `## X.Y.Z - DATE`; they only read `## vX.Y.Z (DATE)`, so v0.7.0's release workflow failed ([3602ca3](https://github.com/winstonyli/manifold/commit/3602ca3)).

### Internal

- Read the host pins from a manifest (`MANIFOLD_HOST_PINS`, embedded by `build.rs`; a debug build also reads `MANIFOLD_HOST_PINS_FILE`), so the gate's download-state shots pin a made-up host instead of a real one ([75811fb](https://github.com/winstonyli/manifold/commit/75811fb)).

## 0.7.0 - 2026-10-18

*v0.6.0 and v0.7.0 were tagged but never released (v0.7.0's release workflow failed); 0.7.1 is the first release with their changes.*

### Changed

- Use `Immediate` present mode with a Fifo fallback, a present queue depth of 1, a frame cap, and a redraw when a page frame lands in an empty slot: median key-to-screen latency is 34.0 ms against 100.1 ms on Fifo (run `keylat-final`), and idle page-initiated frames take 0.5-1.5 ms instead of 62-97 ms.
- Open every list popup, the node menu and the view picker through one shared shell (`menu_popup`) with one just-opened rule: the popup opens on its first row, and so does a node-menu page after a page swap.
- Enter a list popup with Up or Down when nothing is focused (the node menu's Color page): Down lands on the first row, Up on the last.
- Count every arrow press that lands in one frame in list popups, the quick switcher, the address-bar suggestions and graph focus; a slow frame or key repeat no longer drops steps.
- Lead each row of the quick switcher, address-bar suggestions, History, bookmarks and Recently Closed with the page's favicon or its engine-coloured host tile; the quick switcher's selected row highlights its full width.
- Let a typed navigation ask for the new origin's favicon.
- Centre the address bar's text vertically, and centre a tile's letter on its ink.

### Added

- Add the **trail strip**: the focused tab's page tree hangs under its card; click a page to navigate, drag one out to open it as a card, right-click for its menu.
- Add the **trail popup**: a pannable, zoomable miniature graph of the same pages, recentred on every open, with Tab stepping through pages and Escape leaving the popup while a menu is open.
- Label trail pages by their short URL, or by what differs when only the query or fragment does, with an ellipsis and a full-URL tooltip.
- Add mouse buttons 4 and 5 as Back and Forward on the focused tab.
- Spin the header icon of a waking tab, a decoding image, a rendering PDF and a tab awaiting its first frame.

### Fixed

- Drop every machine path from a foreign import, so restore never opens a UNC share the file names; restore tests the URL before the file.
- Drop onto the topmost of overlapping cards, and say so when an engine pick arrives during a swap.
- Claim a conflict path with `create_new`, and hash the host-pin fingerprint recipe with sorted keys so it no longer depends on what else was built.
- Bound the token read as a whole, zero non-finite positions instead of dropping them, and treat a NaN camera as no change.

### Internal

- Release from GitHub Actions: pushing a `vX.Y.Z` tag runs `release.yml`, which checks the tag (`gate release-check`), builds the embedded WebView2 host and PDF view and `manifold.exe`, runs `gate package` and `gate release-verify`, and leaves a draft release; `scripts/release.sh` is gone.
- Name engine hosts by a source fingerprint: `hosts.yml` builds a host when its sources change and publishes it as `hosts-KIND-FINGERPRINT`, the pin table points at that release, and `gate host-pin --check` (run in CI) fails on a stale pin. `PIN_VERSION` is removed: host and app agree through `PROTOCOL_VERSION`.
- Build only CEF in CI so far; Servo and WebKit read Unavailable in this release until their workflows land.
- Rename the `ipc-protocol` crate to `manifold-view-protocol`.
- Add virtual cards (a `VirtualSource` and per-kind `Caps`) under the trail strip, one `Identity` painter and favicon lookup, one URL splitter, one tree layout shared by the JSON level and the trail, and shared graph camera pieces.
- Add gate checks for the trail's favicon and dropdown, filter typing in the History popup (`popup-history`) and the node menu walked by keyboard (`graph-menu-key`); the debug command `GRAPHZOOM`, and `recentlyclosed` and `back|forward` for `TOGGLEPOPUP`; and `EXPORTSTATE` rows that carry `selected`.
- Keep `PARKED.md`, `docs/runs.md` and spec front matter, and measure latency with `scripts/keylat.ps1`.

## v0.6.0 (2026-10-16)

### Features

- **Back and Forward are one traversal over a tab's pages.** The trail is a tree of the pages a tab visited, kept by the app and saved in `tab-history.json`, so it survives sleep, a crash restart, an engine swap and an app restart. Back at a tab's first page goes to the card it was opened from, and Forward there returns. The Back and Forward chips show a branch marker, and a branch list opens a sibling page or opens it as a card. Alt+Left and Alt+Right are Back and Forward. A page's own `history.back()` reads as a Back. A closed tab keeps its trail across reopen and restart. Checked on WebView2, CEF, Servo and WebKit by `gate trail`.
- **Jars are cards.** The Default jar is a card; tabs and groups name their jar through one edge, resolved by walking up. A jar card's menu has Clear Cookies and Ephemeral; Settings no longer lists named jars. Right-click on a jar chip opens the card menu on its Jar page. Jar cookies and fetched content share one backing-file keep set and prune.
- **Closed is a field.** Closing a card sets `closed` and reopening clears it; eviction starts past 20. History is derived from the document, and sync has a three-state remote change with a hide path.
- **A node of a type this build doesn't know is a live card:** generic fields, copies and links, and it sits in groups. Every stored node carries its type string.
- **Rename and Duplicate work on any node,** as do Pin and selection. Ctrl+P pins the whole selection, or the focused card when none is selected. Close Selected names the cards inside selected groups that close with them, and a group's close control closes it. The card context menu is shorter: Jar and Color open pages in the same popup.
- **PDFs render in their own process,** `manifold-view-pdfium`, embedded in the release exe like the WebView2 host.
- Image nodes decode on two worker threads instead of the UI thread, and a panic in one decode becomes that image's error.

### Internal

- The `manifold-engine-*` crates are `manifold-view-*` (exe names unchanged); code, stored keys, environment variables and debug commands say view instead of engine.
- Engine hosts no longer inherit manifold's stdin. The WebKit host reports the history's current item as the url.
- The gate is one Rust program: the remaining shell and python probes (jar, sleep, focus-loss, unresponsive, scale, package, chrome-shots, json-pan, tier, pins, release-verify and others) are native `gate` stages, and `gate all` and `gate fresh` orchestrate them. Waits are on state and requests, not fixed sleeps. New stages: `closed`, `unknown-type`, `trail`, `sleep-history`. `SITE_HOST` addresses the page server for an engine in WSL; `WEBKIT_SITE_HOST` and `WITH_SERVO=1` add opt-in WebKit and Servo stages to `gate all`.
- `gate layout` takes one launch per window size and page kind (422 s to 308 s).
- Two racy writer tests and the `gate link` placement flake are fixed.

## v0.5.1 (2026-10-12)

### Internal

- A racy unit test (`a_stall_is_reported_once_until_a_write_completes`) failed on the v0.5.0 tag's CI; it now waits for the old write to complete. No app change.
- `gate package` expects "Unavailable: this build has no Servo download" when the pin table has no Servo row (a release made without `--with-servo`).

## v0.5.0 (2026-10-12)

### Features

- **The release is one exe.** CEF, Servo and WebKit hosts download on demand (WebKit's is the Linux ELF, run from the Windows data folder by WSL; its WSL prerequisites, a distro named Ubuntu with xvfb-run and the WebKitGTK libraries, stay yours to install and the download dialog says so), each file pinned by SHA-256 in the exe; the WebView2 host is embedded and extracted on first use. The engine picker, failed cards and Settings, Engines show each engine's size, Download/Update/Retry/Remove and "Clear leftovers" from the same component model. Hosts must be reinstalled after an update: a downloaded host belongs to the release that pinned it and reads Update needed until reinstalled. `ENGINESTATE` and `EXPORTSTATE` report what the disk holds per engine.
- **A stuck engine no longer freezes the app.** Sends to an engine go through a per-tab writer thread, so an engine that stops reading cannot block the window, closing its tab, or quitting. While an engine is behind, pointer moves and resizes merge (latest wins) and wheel steps add up; nothing else merges or reorders.
- **Not responding.** A tab whose send queue is full and whose write is blocked for 50 ms, or whose write is stuck for 2 s, shows a warning badge on its card and a "Not responding" toast. Its right-click menu leads with Restart Engine (a new engine at the tab's URL) and End Engine (leaves a failed card). Nothing is killed automatically. Headless: `RESTARTENGINE id`, `ENDENGINE id`, and `unresponsive` on `EXPORTSTATE` cards.
- Engine messages wake the event loop instead of waiting for the next poll, and one frame applies at most 64 messages per tab.
- **Less work per frame.** An engine copies a frame once, straight into the shared frame file, and reports which rows changed; the app uploads only that band to the GPU (a whole 1080p upload cost 8.5 ms of CPU, a 64-row band 0.6 ms), and tab thumbnails are redrawn only when drawn. CEF stops painting hidden tabs. Frame files are named by generation and the app keeps one mapping per engine instead of reopening it every frame (about 2.9 ms down to 0.1 ms per frame at 1080p). CEF reads only the rows it says it painted, so a small animated spot costs about 0.2 ms a paint instead of 2.8 ms. WebView2 and CEF already sent nothing for a static page, so the gain is on pages that animate. Details and measurements: `docs/superpowers/plans/2026-10-11-frame-path.md`.
- F12 and Ctrl+F on an engine that can't (DevTools: Servo, WebKit; find: Servo) now say so in a toast instead of doing nothing.
- **Popups fit the window.** The shortcuts dialog narrows and wraps in a narrow window, and a list popup (a card's menu, Recently Closed, Bookmarks, History, Downloads) scrolls when it is taller than the window. The empty-canvas menu is drawn above the cards, at a constant size at any zoom. Menu separators and key-hint spacing are one value each.

### Internal

- `EngineDesc::jar` replaces five engine lists; per-engine wildcard arms are exhaustive (`engine_runtime_dir`, `platform_unavailable_reason`, one `on_disk_reader` shared by the engine lists and spawn).
- `main.rs` split into modules by concern (3,895 to 710 lines) and `App`'s fields grouped into 14 sub-structs; no behaviour change. `tabs.rs`, `process_manager.rs` and `gpu.rs` were split the same way (pure moves, same 1,124 tests).
- `gate layout` opens every menu and dialog at three window sizes and checks each lies inside the window and that no card shows through it (`EXPORTSTATE` gained `window` and `popups`); `gate chrome-shots-sizes` shoots the chrome at several sizes.
- The gate's headless flows are one Rust program, `crates/gate` (`gate link|smoke|two`), replacing `link-probe.sh`, `headless-smoke-test.sh` and `two-instance-test.sh` and their python helpers. Checks are closures over the exported state, `two` now prints a `PASS:` line per check, and a full gate takes about 5.5 min instead of about 11 (measured under 38-45% load from other sessions). `gate-timing.sh` also reads a Windows-path CPU log correctly now. CI lints and tests `gate` on Windows.
- The engine's own inbox is bounded (`ENGINE_INBOX`, 256), so a stuck engine main loop backs the app up instead of growing the engine's memory. `ipc_protocol` writes each message in one `write`.

### Known

- WebKit's resize keeps the old picture size, as before.
- **Rebuild every engine host after pulling: `PROTOCOL_VERSION` is 4** (`FrameReady` carries `generation` and drops `frame_id`; frame files are named by generation). A host built before is refused with the protocol-mismatch message and its tab shows a failed card.
- **Rebuild every engine host after pulling.** A host built before the bounded inbox has an unbounded one. CEF, Servo and WebKit were not built or run with the bound (WebView2 was).
- A WebKit tab force-closed while its writer is stuck leaves that thread until the Linux host exits.
- A Restart affordance in Tab view is not done: the badge and menu rows are on the Graph card.

## v0.4.1 (2026-10-09)

20 commits since v0.4.0.

### Features

- **Link gesture polish.** Connect handles are 28 by 16 pills, show on hover even when the card has nothing to link to, and show on every card while you drag. A release within 24 points of a card links it, so a near miss counts. Links and opened-from lines are now one curved style between the facing sides of the two cards, and clicking a link means clicking its curve.
- **Drag a link to retarget it.** Drag a link's curve and the end nearer your grab comes off its card and follows the pointer; drop it on another card to move that end there (the link keeps its identity and direction). Dropping on empty canvas, or on a card already linked to the other end's card, deletes the link; dropping on the old card or pressing Esc leaves it as it was, and any other card that refuses the pair shows the usual toast. While you drag, the line shows what letting go would do: red with an "x" where the link would be deleted, blue with a slashed circle where the drop is refused, and the card under the pointer is ringed in the same colour. A click on the curve still deletes the link.
- **Deselect by clicking.** A left-click on empty graph canvas clears the tab selection, and Ctrl-click on a tab card toggles it in or out of the selection without opening it.

### Integrity

- The engine installer stops a download at the pinned size and deletes its `.part` if the stream runs past it, instead of reading to the end of the stream.
- Redirect fetches run at most four at once; the rest wait. Favicon fetches share the same limit (`net::Throttle`).
- The WebKit engine's command line carries the start URL as hex (`ENGINE_INITIAL_URL_HEX`), so page-controlled text no longer passes through the WSL shell. `ENGINE_TAB_ID` is dropped.

### Known

- The WebKit host (`manifold-engine-webkit`) must be rebuilt in WSL after pulling: an old host ignores `ENGINE_INITIAL_URL_HEX` and opens its default page instead of the tab's URL.
- Not done: owner-only local engine pipes with a peer PID check (held for a cross-user manual check).

## v0.4.0 (2026-10-08)

101 commits since v0.3.0.

### Features

- **Links between cards.** Any two cards at one level can be linked. A link is a stored node, so it syncs, survives close and reopen, import and export, and goes when either end is deleted. Links draw as dashed lines and never move cards. The node menu gains "Link to...", "Opened from..." and "Disconnect", with a pick-target mode. Merging groups retargets their links.
- **Link gestures.** Click a link line to delete it. Hover a card to show a handle on each edge; drag a handle onto another card to connect. Esc cancels, and a right-press during a drag opens no menu.
- **Large JSON cards.** A far reveal moves a card's window ("+ N earlier"), and only the rows on screen paint.
- **Jar menu.** The current jar is the selected row, the colour dot sits in a fixed slot, and menu sections share one layout grid.
- **Failed startup cards.** A failed spawn is retried at the next launch. A crashed tab beside a failed card leaves one card. Toasts are counted by intent and a crash toast coalesces.
- **Bad initial URL.** It opens `about:blank` instead of panicking.
- **Focus.** Headless mode and the WebView2 host no longer take the OS foreground. A card drag ends in place when the window loses focus.

### Performance

Timings were taken on a shared machine, often at 40-90% CPU, so read them as upper bounds. Details are in the wave 2 plan's Outcome.

- Layout settle uses a size index: top level 904 to 131 ms.
- Revealing deep in a large JSON file: `reveal-numbers` 1079 to 49 ms per command, `reveal-objects` about 540-780 to about 23.
- JSON search: 29.5 to 4.5 ms.
- Link target lookup at 100 links: 3.3 to 0.50 ms.
- A burst of cookie changes saves the jar once instead of once per tab (15 to 1). A crash can now lose up to about 2.1 s of jar changes, on top of the existing 30 s pull interval.

### Integrity

- Session exports (`EXPORTSTATE`, Export Session, Export Group) are written atomically.
- An unknown command-line flag, or one missing its value, is refused with exit code 2.
- A test fails if the wire shape changes without a `PROTOCOL_VERSION` bump.

### Development

- The toolchain is pinned to `nightly-2026-09-23`, and every `#[allow]` gives a reason.
- CI runs fmt, clippy and tests on Windows, and tests on Linux, on every push.
- `scripts/gate.sh` is the pre-merge gate (`full` and `light`). It waits on `WAITIDLE` rather than fixed sleeps, and also runs `launch-forms-probe.sh`; `gate two`, `gate smoke` and `gate link` are the Rust stages (`crates/gate`). The smoke test clicks the JSON "+ N earlier" row.
- `main` is the only long-lived branch; releases are tags.

### Known

- `two-instance-test.sh` scenario 7 failed once under heavy load; it now retypes the URL once and dumps the server log on failure. See the wave 2 plan's Outcome.
- Not measured: the feel of link gestures (drag smoothness, hover affordances) beyond the scripted clicks.
