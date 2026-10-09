# Changelog

One entry per tagged release, newest first. The annotated tag `vX.Y.Z` carries the same text. Releases before v0.4.0 have no entry: see `git log --first-parent --decorate`.

## Unreleased

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
- `scripts/gate.sh` is the pre-merge gate (`full` and `light`). It waits on `WAITIDLE` rather than fixed sleeps, and also runs `launch-forms-probe.sh` and `link-probe.sh`. The smoke test clicks the JSON "+ N earlier" row.
- `main` is the only long-lived branch; releases are tags.

### Known

- `two-instance-test.sh` scenario 7 failed once under heavy load; it now retypes the URL once and dumps the server log on failure. See the wave 2 plan's Outcome.
- Not measured: the feel of link gestures (drag smoothness, hover affordances) beyond the scripted clicks.
