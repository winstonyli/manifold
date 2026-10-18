status: draft

Draft of the v0.7.0 entry, for the bump commit to move into `CHANGELOG.md` (delete this file then). Written 2026-10-18 from `git log v0.6.0..HEAD` (137 commits at a396333) and the v0.6.1 text unreserved in 7126a5e (still in d94a780), folded in here. Format versions are unchanged since v0.6.0 (`PROTOCOL_VERSION` 4, `RENDER_PROTOCOL_VERSION` 1, journal `FORMAT_VERSION` 2, local `FORMAT_VERSION` 1), so no format line. Commit links are added at the bump.

## 0.7.0 - YYYY-MM-DD

*v0.6.0 was tagged but never released; v0.7.0 is the first release with its changes.*

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
