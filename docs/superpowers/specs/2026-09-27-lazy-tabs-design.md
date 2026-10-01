# Lazy Tabs

## Goal

Stop starting every saved tab's engine at launch. Measured 2026-09-26
(`scripts/scale-test.sh`, debug build, a light local page): a WebView2 tab
costs ~78 MB private and a CEF tab ~177 MB and six processes, so 100
tabs cost 8.3 GB (WebView2) before any real page weight, and a 300-tab
session is unusable. Spawning itself is fast (100 tabs in ~6 s): memory
is the limit. After this change a restored session starts one engine,
the focused tab's, and every other tab sleeps until it is opened, as
browsers restore tabs unloaded.

## Decisions

- **Only the focused tab starts at launch.** Every other tab restores
  asleep; opening one waits a second or two for its engine. Chosen over
  "focused plus the current level" and a most-recently-used budget: the
  cheapest, and the size of a session no longer matters.
- **No re-sleeping in this version.** A woken tab stays loaded until it
  is closed or the app restarts. A manual "Sleep" action and sleeping
  after idle are later work.
- **A sleeping card shows the saved title and the url's host, dimmed.**
  A cached last thumbnail is the planned follow-up (see Deferred).
- **One "unloaded tab" node instead of a new kind.** The failed-tab card
  (`FailedTabPayload`) is already a tab without an engine: it keeps url,
  engine, pinned and zoom, saves as a plain tab, Tab view falls back to
  Graph for it, personal state covers it (`live_url`), and a click
  respawns it under the same id. It widens into an unloaded tab with a
  state. Rejected: a separate `SleepingTab` kind (duplicates most of the
  24 failed-tab sites, and the two would drift), and an optional engine
  inside `TabPayload` (all 78 live-tab sites would have to handle a
  missing engine).

## Behaviour

- **Startup.** Every saved tab restores as a sleeping card: its live url
  when personal state has one for the current shared url (as today),
  otherwise the stored url; its saved title, engine, zoom and pinned
  flag. Then the focused tab, if there is one, wakes.
- **What wakes a tab.** Anything that opens it: a click on a sleeping or
  waking card (`NodeAction::Open` → `focus_tab`), choosing it in the
  Quick Switcher, next/previous-tab, and any other path that makes it the
  focused tab in Tab view. A failed card's click is the one exception --
  it retries through `retry_missing_file`'s tab branch instead. That
  retry goes through `wake_tab` too, so since Task 2 it is a
  `SpawnPurpose::Wake` (it was `Respawn`) and also gets `note_host_load`. Waking is a host-started load, so its
  redirects stay local (`note_host_load`), like a restore.
- **What does not.** Saving, exporting, closing, moving, grouping,
  coloring and pinning act on the card as they do on a failed card now.
  Another instance's url change updates a sleeping tab's url without
  waking it; it opens there when woken. Closing a sleeping card is
  closing a tab: its children move to its parent, as `destroy_tab` does
  (amended by the plan: a failed card used to go through `remove_node`,
  which left children naming a removed parent).
- **When the engine is ready.** (Amended by the plan.) Opening a tab sets
  focus and Tab view at once, and Tab view shows the respawn placeholder
  ("Loading…") until the engine lands. The landing then behaves like a
  crash respawn: visible if the tab is still focused, a background tab
  otherwise. It doesn't call `focus_tab` itself, so a user who went back
  to Graph meanwhile stays there. A failure turns the card into a failed
  card with the error, as today.
- **Open Session sleeps too.** (Amended by the plan.)
  `replace_running_session` (Open Session, `OPENSESSION`) replaces the
  whole session as a launch does, so its tabs restore asleep and the
  focused one wakes. Group import and Recently Closed sleep too (see
  Outcome).
- **Cards.** Sleeping: dimmed, title and host, tooltip "Sleeping — click
  to load". Waking: the spinner a retry shows now, tooltip "Loading…".
  Failed: unchanged (warning icon, error tooltip).

## Design

### Payload (`node.rs`)

`FailedTabPayload` becomes `UnloadedTabPayload` and `NodePayload::
FailedTab` becomes `NodePayload::Unloaded`:

```rust
pub(crate) struct UnloadedTabPayload {
    pub(crate) url: String,
    pub(crate) title: String,
    pub(crate) kind: EngineKind,
    pub(crate) pinned: bool,
    pub(crate) muted: bool,
    pub(crate) zoom_factor: f64,
    pub(crate) state: UnloadedState,
}

pub(crate) enum UnloadedState {
    Sleeping,
    /// A spawn is in flight: the card shows a spinner, and further opens wait.
    Waking,
    Failed(String),
}
```

`retrying` becomes `Waking`; `error` becomes `Failed`'s string. The
payload keeps `muted` as well as `title` (amended by the plan), so
personal state round-trips both for a tab that is never woken; a wake
carries both into the live tab (`TabCarry`), and a muted tab is muted
again on landing.
`ItemNodeKind::FailedTab` becomes `Unloaded`. Each existing failed-tab
site becomes a match on `state` where the states differ (card drawing,
tooltip, menu) and stays as is where they don't (saves, `shared_nodes`,
`local_state`, closing, grouping). A failed spawn of a live tab (engine
swap, crash respawn) still produces `Failed`.

### Restore (`session_sync.rs`)

- `materialize` for a `Tab` inserts a sleeping `Unloaded` card (same
  id, position, group, color, parent) instead of spawning, whoever
  calls it: `restore_session`, `replace_running_session`, `apply_remote`
  and `land_remote_in_flight` for a tab another instance created, and
  Recently Closed and group import through `materialize_imported` (see
  Outcome). A `TabStart { Spawn, Sleep }` parameter chose between the
  two while some callers still spawned; it went once none did.
- `restore_session`, after `apply_local` has set `focused` (and
  `split_tab`), wakes each of them that is asleep (`wake_focused`); so
  does `replace_running_session`. No focused tab wakes nothing; without
  personal state the first tab is focused. `wake_focused` also wakes a
  saved split tab (final-review Fix 2): `apply_local` restores
  `split_tab` regardless, so without this the split pane came back
  empty, showing a sleeping card no one had woken.
- `apply_local` copies personal state's `title` into an unloaded card, as
  it does into a live tab now; its existing zoom arm stays.
- `materialize` updating an existing unloaded card sets its url (and
  engine) from the store without waking it, under the existing
  `last_shared` rules.

### Waking (`main.rs`)

- `App::wake_tab(id)` generalizes `retry_failed_tab(id, kind)`: ignored
  while `Waking`; otherwise sets `Waking` and respawns on the card's
  engine (a failed card's "open in default engine" passes the default
  engine, as now). `note_host_load` runs at the landing (below).
- A new `SpawnPurpose::Wake`, which lands like `Respawn` (amended by the
  plan): visible if `focused` is still the tab, invisible as a
  background tab otherwise. It exists only so the landing calls
  `note_host_load` when the engine starts; calling it at spawn time
  would let `HOST_LOAD_GRACE` run out during a slow spawn. A failure
  produces a `Failed` card.
- Every path that opens a tab funnels into it:
  - `focus_tab` (and so `focus_item`) on an `Unloaded` node sets
    `focused`, Tab view and `pending_respawn_focus`, and wakes it;
  - a click on a sleeping or waking card (`NodeAction::Open` →
    `focus_tab`); a failed card's click retries through
    `retry_missing_file`'s tab branch;
  - `quick_switcher_confirm`;
  - `cycle_focused_tab`.

### Tab lists

`quick_switcher_matches` and `cycle_focused_tab` list unloaded tabs too
(matching on title and url). Every other `as_tab()` site keeps its
meaning, "a tab with an engine"; the plan audits all 78 and records any
that should also see unloaded tabs. The audit added: open-tab
suggestions, graph arrow navigation, rect selection, Close All / Close
Others, drag-and-drop targets, layout and every pin control (card,
menu, pin-all, `Ctrl+P`, `EXPORTSTATE`), and Duplicate (via
`saved_tab`). None of these wakes a tab except by opening it.

### Cards (`card.rs`, `graph_ui.rs`)

The unloaded card body draws by state: sleeping shows the title (or the
url when the title is empty) and the host, dimmed; waking shows the
spinner; failed is unchanged. No border change (amended by the plan): a
failed card never had a `DANGER_COLOR` border. The failed-only red is
the minimap dot, which sleeping and waking tabs draw in the tab color.

## Testing

Unit tests (windowless `App`, spawns parked in `test_spawns`):

- A restore of several tabs spawns exactly one, the focused tab; the
  rest are sleeping `Unloaded` cards with their saved titles.
- A restore with a live url sleeps at the live url, and waking loads it.
- Opening a sleeping tab by card click, `focus_item`, the Quick Switcher
  and `cycle_focused_tab` each issues one spawn; a second open while
  waking issues none.
- A `Wake` landing switches to Tab view only if the tab is still focused.
- A remote url change on a sleeping tab updates its url and spawns
  nothing.
- A sleeping tab's stored fields are unchanged by saves (it writes what
  the live tab would).
- A woken tab's first unlabelled url change stays local
  (`note_host_load`).
- Open Session restores asleep; closing an unloaded card reparents its
  children; personal state keeps an unloaded tab's title and mute; a
  failed wake leaves a failed card with the title.
- A restored split session wakes the focused and the split tab and
  nothing else. Open Session with several tabs wakes only the focused
  one; without personal state a restore focuses and wakes the first tab.
- A tab another replica creates arrives asleep.
- Quick Switcher row labels, the graph filter and "Mute other tabs" see
  sleeping tabs; closing a tab whose wake is in flight deletes it from
  the store when the landing arrives; closing clears a
  `pending_respawn_focus` naming it.
- Not unit-testable: a successful landing (there is no fake
  `EngineTab`). So the Wake landing's visibility, its `note_host_load`,
  and a pin or mute changed while waking surviving the landing (the
  landing reads them from the card) are checked by reading the code and
  by the scripts below.

Scripts:

- `scripts/scale-test.sh 100`: one engine's processes plus the host
  (against 206 processes and 8.3 GB before).
- `two-instance-test.sh` (both engines) and `headless-smoke-test.sh`
  still pass. Scenario 6 (a restart reopens a tab's live url) expects the
  next instance to load C's tab; that tab must be the focused one, or the
  scenario must open it. No script isolates the Wake landing's
  `note_host_load`: with it removed every scenario still passes, since
  a server redirect of a load the host started isn't shared anyway
  (`shares_url_change`). It is a second guard.

## Outcome

Implemented 2026-09-27 (plan `docs/superpowers/plans/2026-09-27-lazy-tabs.md`,
commits 270f2f4 through 2055140, the final-review fixes, on `dev`). Measured with
`scripts/scale-test.sh` (debug build, CPU ~23% busy from other work,
Defender real-time protection off), memory 15 s after the restore
settled:

| Session | Before | After |
|---|---|---|
| WebView2, 1 tab | — | 8 processes, 616 MB private |
| WebView2, 100 tabs | 206 processes, 8.3 GB | 8 processes, 620 MB, settled in 2.3 s |
| CEF, 25 tabs | 151 processes, 4.8 GB | 7 processes, 585 MB |
| CEF, 100 tabs | — | 7 processes, 587 MB |

A session's startup cost no longer depends on its size; the host
process itself (~410 MB in a debug build) is now most of it.

Scripts: `scale-test.sh` checks one live tab and N−1 asleep; its counts
needed `|| true`, since `grep` finding none fails the pipeline under
`set -e -o pipefail` (a 1-tab run has none asleep). The smoke test and
`two-instance-test.sh` (WebView2 and CEF) pass unchanged. Scenario 6 (a
restart reopens a tab's live url) needed no change: the next instance's
focused tab is the one C typed into. It fails, as it should, both with
the live-url restore disabled and with `wake_focused` disabled.

**Sleeping-card polish** (added after the review). A url with no host
(`file:`, `about:`) showed its url on both lines; `url_host` now returns
`None` for it and the card skips the host line. A sleeping or waking
card also draws its origin's favicon, dimmed. Favicons live in memory
only, and before this only a live tab's url change fetched one, so the
card requests its own origin through the same `favicon_resolved` /
`FaviconFetcher` dedupe. Only on-screen cards drawn in full detail do
this (culled and LOD-simplified cards return earlier), so a big
restored session fetches once per origin in view, not for every tab.
Fetched favicons are then kept on disk too (`profile::favicon_cache_dir`,
one PNG per origin; used for 7 days before a refetch, the stale copy kept
if that fails, and files untouched for 30 days pruned at launch), so a
relaunch shows them without the network.

A whole-branch review then found eight gaps, fixed in 2055140: Quick
Switcher rows skipped sleeping tabs; a split session came back with an
empty pane; a tab closed while waking stayed in the store; the graph
filter ignored sleeping titles; a pin or mute changed while waking was
undone by the landing; a stale `pending_respawn_focus` outlived its tab;
a waking card's tooltip said "Sleeping"; "Mute other tabs" skipped
sleeping tabs.

**Remote-created tabs sleep too** (added after the review). A tab
another instance creates (`apply_remote`, `land_remote_in_flight`) now
arrives asleep, so another instance opening a big session starts no
engines here. `two-instance-test.sh` scenario 3 relied on B's copy of
A's new tab loading by itself; B now opens it through the Quick
Switcher once A's url reaches it, then returns to Graph view (C inherits
B's personal state, and scenario 6 toggles C into Tab view from Graph).

**Recently Closed and group import sleep too** (added after the
review). Reopening or importing a big group started every member's
engine at once. Both now arrive asleep, like every other way a tab is
created from saved data, so `TabStart` went. A reopened tab's zoom
(`doc.local["zoom"]`) is set on its sleeping card directly instead of
waiting in `pending_local` for a landing. Nothing gains focus on a
reopen, so nothing wakes; the first click starts the engine. Titles are
personal state too, so `closed_entry` records them beside zoom
(`doc.local["title"]`): a reopened sleeping card shows its title, and
the Recently Closed list names a tab by it rather than by its url.

## Sleep (manual, 2026-09-30)

A live tab's "Sleep" (tab context menu; "Sleep Tabs" on a group; headless
`SLEEPNODE id`) is two-phase since 2026-09-30
(`2026-09-30-sleep-page-state-design.md`): `App::request_sleep` asks the
engine for the page's state (`QueryPageState`: scroll position, and whether
a form field differs from its default), and `finish_sleep` sleeps on the
answer through `sleep_now`. A query unanswered after 5 s is settled by
`expire_pending_sleeps` (run by the 10 s idle sweep and by the next request):
a manual one sleeps the tab anyway, without a scroll to restore, so Sleep
works on a hung page; an idle one is dropped and the tab's idle clock
restarts. `sleep_now` swaps the tab's payload for an
`UnloadedTabPayload` in `Sleeping` state, which drops the `EngineTab` and so
shuts the engine host down, and frees the tab's GPU textures. The card keeps
its id, place, url, title, pin, mute and zoom, and saves as the same tab.
Opening it is the ordinary wake: the engine starts again and reloads the
url. The scroll position is kept in memory (`App::saved_scroll`, lost on
app restart) and restored once, after the first load completes, unless the
user scrolls first; other page state (form input, a playing video) is lost.
Manual Sleep ignores a changed form; idle sleep skips it (see "Idle
sleep"). Refused for the tab Tab view is showing and for the split pane's
tab. A group's "Sleep Tabs" requests each of its live tabs except those
two. `scripts/sleep-probe.sh` checks tab, asleep, woken, scroll kept and
the dirty-form skip on WebView2 and CEF.

## Idle sleep (2026-09-30)

`Settings::idle_sleep_minutes` (`None` = never, the default; the settings page offers 10, 30 or 60). `idle_sleep::IdleClock` keeps each live tab's last moment on screen; `App::drain_tab_events` calls `sweep_idle_tabs` every 10 s, which updates the clocks (setting on or off, so turning it on counts earlier idle time) and runs `request_sleep` on tabs out of sight for the limit. A tab whose page answers with a form field changed from its default (inputs, textareas, selects, `contenteditable`; not state a page keeps elsewhere) stays awake and its idle clock restarts. On screen means the Tab-view tab or the split pane's tab; a tab playing audio, pinned (the graph pin) or still loading counts as seen. The find bar needs no check: it exists only on the focused tab in Tab view and closes when focus moves. Headless `IDLESWEEP secs` runs one sweep with a limit in seconds; `scripts/sleep-probe.sh` covers it.

## Deferred

- **Cached thumbnails.** Save each tab's last picture as a small image
  in the profile and show it, dimmed, on its sleeping card. Needs a
  cache with cleanup, and stores images of visited pages on disk.
- **Smarter idle sleep.** The plain timer and the dirty-form skip are
  done (see "Idle sleep" above). Not done: skipping a tab with a download in flight (no per-tab
  flag on the host yet), and choosing victims by memory pressure instead
  of a fixed time: sleep only when engine memory passes a budget, ranked
  by idle time x engine-process memory with graph distance from the
  focused node as the tie-break.
  Per-tab memory is readable on Windows (spike 2026-09-30, one live
  tab per engine, debug build, a local page): each engine host owns its
  own process subtree, WebView2's browser tree included (a null user-data
  folder did not make hosts share one), so a tab's cost is the sum over
  its host's descendants. WebView2 host + 6 `msedgewebview2` ≈ 380 MB
  working set / 170 MB private; CEF host + 5 children ≈ 320 MB / 180 MB;
  `manifold.exe` itself ≈ 210 MB working set. Use private bytes
  (`GetProcessMemoryInfo`, `PrivateUsage`), not working set, which counts
  shared pages; a snapshot walk (`CreateToolhelp32Snapshot`) finds the
  descendants; system free memory is `GlobalMemoryStatusEx`. Needs the
  windows-sys features `Win32_System_Diagnostics_ToolHelp`,
  `Win32_System_ProcessStatus` and `Win32_System_SystemInformation`.
  Not measured: several tabs at once, or a heavy page.
  `Ctrl+Tab` (`cycle_focused_tab`) still wakes every sleeping tab it
  passes through on its way to the one the user stops on, and each stays
  loaded until slept. Opening a failed card retries
  it (`retry_missing_file`'s tab branch), and a second failure returns it
  to Graph view with a toast rather than leaving Tab view on a dead
  placeholder.
- **Waking neighbours ahead of time**, e.g. the tabs next to the focused
  one, if opening a tab feels slow in use.
