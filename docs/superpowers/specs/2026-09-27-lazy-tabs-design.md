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
- **What wakes a tab.** Anything that opens it: a click on its card,
  choosing it in the Quick Switcher, next/previous-tab, and any other
  path that makes it the focused tab in Tab view. Waking is a host-started
  load, so its redirects stay local (`note_host_load`), like a restore.
- **What does not.** Saving, exporting, closing, moving, grouping,
  coloring and pinning act on the card as they do on a failed card now.
  Another instance's url change updates a sleeping tab's url without
  waking it; it opens there when woken.
- **When the engine is ready.** If the woken tab is still focused, the
  view switches to it in Tab view; if the user has moved on, it becomes a
  live background tab. A failure turns the card into a failed card with
  the error, as today.
- **Cards.** Sleeping: dimmed, title and host, tooltip "Sleeping — click
  to load". Waking: the spinner a retry shows now. Failed: unchanged
  (warning icon, error tooltip).

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

`retrying` becomes `Waking`; `error` becomes `Failed`'s string.
`ItemNodeKind::FailedTab` becomes `Unloaded`. Each existing failed-tab
site becomes a match on `state` where the states differ (card drawing,
tooltip, menu) and stays as is where they don't (saves, `shared_nodes`,
`local_state`, closing, grouping). A failed spawn of a live tab (engine
swap, crash respawn) still produces `Failed`.

### Restore (`session_sync.rs`)

- `materialize(.., SpawnPurpose::Restore, ..)` for a `Tab` inserts a
  sleeping `Unloaded` card (same id, position, group, color, parent)
  instead of spawning. Every other purpose still spawns: a tab another
  instance created, Recently Closed, imports.
- `restore_session`, after `apply_local` has set `focused`, wakes that
  tab if it is unloaded. No focused tab wakes nothing.
- `apply_local` copies personal state's `title` into an unloaded card, as
  it does into a live tab now; its existing zoom arm stays.
- `materialize` updating an existing unloaded card sets its url (and
  engine) from the store without waking it, under the existing
  `last_shared` rules.

### Waking (`main.rs`)

- `App::wake_tab(id)` generalizes `retry_failed_tab(id, kind)`: ignored
  while `Waking`; otherwise sets `Waking`, calls `note_host_load(id)`, and
  respawns on the card's engine (a failed card's "open in default
  engine" passes the default engine, as now).
- A new `SpawnPurpose::Wake`: at completion, if `focused` is still the
  tab, `focus_tab` (Tab view, visible); otherwise the tab lands invisible
  as a background tab. A failure produces a `Failed` card.
- Every path that opens a tab funnels into it:
  - `focus_item` on an `Unloaded` node sets `focused` and wakes it;
  - a click on the card (`retry_missing_file`'s tab branch);
  - `quick_switcher_confirm`;
  - `cycle_focused_tab`.

### Tab lists

`quick_switcher_matches` and `cycle_focused_tab` list unloaded tabs too
(matching on title and url). Every other `as_tab()` site keeps its
meaning, "a tab with an engine"; the plan audits all 78 and records any
that should also see unloaded tabs.

### Cards (`card.rs`, `graph_ui.rs`)

The unloaded card body draws by state: sleeping shows the title (or the
url when the title is empty) and the host, dimmed; waking shows the
spinner; failed is unchanged. The border keeps `DANGER_COLOR` only for
failed.

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

Scripts:

- `scripts/scale-test.sh 100`: one engine's processes plus the host
  (against 206 processes and 8.3 GB before).
- `two-instance-test.sh` (both engines) and `headless-smoke-test.sh`
  still pass. Scenario 6 (a restart reopens a tab's live url) expects the
  next instance to load C's tab; that tab must be the focused one, or the
  scenario must open it.

## Deferred

- **Cached thumbnails.** Save each tab's last picture as a small image
  in the profile and show it, dimmed, on its sleeping card. Needs a
  cache with cleanup, and stores images of visited pages on disk.
- **Sleeping again.** A manual "Sleep" action (tab and group menus), and
  sleeping after idle (skipping tabs playing audio).
- **Waking neighbours ahead of time**, e.g. the tabs next to the focused
  one, if opening a tab feels slow in use.
