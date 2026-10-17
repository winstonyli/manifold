---
status: done
---

# Sleep with page state: dirty-form skip and scroll restore

Status: implemented 2026-09-30 (plan
`docs/superpowers/plans/2026-09-30-sleep-page-state.md`); see
"Deviations" at the end for where the code differs from this text. Builds
on manual Sleep and idle sleep (`2026-09-27-lazy-tabs-design.md`, "Sleep"
and "Idle sleep").

## Problem

Sleeping a tab closes its page; waking reloads the url. Two losses follow:

1. Idle sleep can discard what the user typed into a form.
2. Every wake returns to the top of the page.

## Scope

- Idle sleep skips a tab whose page has a dirty form.
- Any sleep (idle or manual) saves the scroll position; a wake restores it.
- Live-tested on WebView2, CEF and Servo. WebKit gets the same handlers but
  is not live-tested (as for `PAGE_KIND_SCRIPT`); it waits for a native
  Linux build.

Out of scope, decided 2026-09-30: detecting live connections (websockets,
calls). It needs a `WebSocket` patch injected at document start, which
is too invasive. Audio (already tracked as `audio_playing`), pinned and
loading tabs are still skipped by idle sleep. The find bar needs no check
(see "Idle sleep"). Persisting scroll across an app restart is out of scope too:
the scroll position lives in memory with the sleeping tab, like `zoom_factor`.

## Protocol

`GetState` / `StateSnapshot` is the existing request/reply pair; this adds
a second one, in `ipc-protocol`:

- `HostToEngine::QueryPageState`
- `EngineToHost::PageState(PageState)`, with
  `PageState { scroll_x: f64, scroll_y: f64, dirty_form: bool }`.
- `HostToEngine::RestoreScroll { x: f64, y: f64 }`.

Both scripts are shared constants beside `PAGE_KIND_SCRIPT`, so an engine
only runs a string and forwards the result:

- `PAGE_STATE_SCRIPT` returns a JSON string `{"x":..,"y":..,"dirty":..}`
  and `PageState::parse` reads it (as `PageKindReply::parse` does).
  `dirty` is true if any `input` (except button-like types, and
  checkbox/radio/select compare `checked`/`selected` against their
  defaults), `textarea` or `[contenteditable]` element has a value that
  differs from its default value. Pages that keep state outside form
  elements (canvas editors, custom widgets) are not detected.
- `restore_scroll_script(x, y)` scrolls to `(x, y)`, retrying on
  `requestAnimationFrame` for up to 3 s until `document.documentElement.
  scrollHeight` is large enough to reach `y`, then scrolls to the
  nearest reachable point.

Engines: WebView2 `ExecuteScript`, WebKit `run_javascript`, Servo
`evaluate_javascript`, CEF through its V8 round trip (as its
`document.contentType` read does). A reply that fails to parse is dropped:
the tab then simply does not sleep.

## Host: two-phase sleep

`App::sleep_tab` today sleeps at once. The new flow:

1. A request (`sleep_tab(id, why)`, `why` = `Manual` or `Idle`) validates
   as now (not open, not the split tab, is a live tab), sends
   `QueryPageState`, and records `pending_sleep[id] = why` with the time sent.
2. On `PageState` for `id`: if `why` is `Idle` and `dirty_form`, drop the
   pending request (the tab stays awake, and the idle clock is reset so it
   is not retried every sweep). Otherwise store the scroll in the new
   `UnloadedTabPayload::scroll: Option<(f64, f64)>` and sleep.
3. A pending request older than 5 s is dropped (engine busy or dead); a tab
   that becomes the open tab while pending is dropped too. The sweep does not
   re-request a tab that is already pending.

Manual Sleep ignores `dirty_form`: it is an explicit act. `sleep_group`
requests each member the same way and returns how many were requested,
not how many slept.

Headless: `SLEEPNODE id` and `IDLESWEEP secs` behave the same; the probe
waits a second after them.

## Wake

`wake_tab` starts the engine as today. When that tab's first navigation
completes (the `loading` true-to-false edge `drain_tab_events` already
sees) and the unloaded payload held a scroll, the host sends
`RestoreScroll` once and clears it. A tab the user scrolls first is not
overridden: the script gives up as soon as the page's scroll position
changes by a user action (a `wheel`/`keydown`/`touchstart` listener
cancels the retry).

## Testing

- Unit: `PageState::parse` (valid, bad, missing fields); the two-phase
  decision as a pure function (`Manual` sleeps when dirty, `Idle` skips,
  timeout drops); `restore_scroll_script` formats finite numbers only.
- `scripts/sleep-probe.sh` (WebView2 and CEF), new cases on a probe page
  with a tall body and a text input:
  - scrolled tab: `SLEEPNODE`, wake, read back `window.scrollY` is within
    a few pixels of what it was (via a page that reports it to the log
    like the key probe does);
  - dirty tab: type into the input, `IDLESWEEP` leaves it `Tab`, while
    `SLEEPNODE` still sleeps it.
- `probe_server.py` gains `/tall` (the page above).

## Docs

README Sleep paragraph (what is kept and lost on wake), the lazy-tabs
spec's Deferred list (remove the dirty-form and scroll items), and
`sleep-probe.sh` usage.

## Risks

- A scroll restored into a page that lays out slowly or changes height
  lands short; accepted (the retry window is 3 s).
- `dirty_form` misses non-form state, so "idle sleep never loses input" is
  not promised; the README says "forms".
- The two-phase flow adds a state machine to a path that was a single
  call; the pending map and its timeout are the only new state.

## Deviations

Decided while planning (plan, "Global Constraints"):

1. `PAGE_STATE_SCRIPT` returns plain text `"<x> <y> <1|0>"`, not JSON,
   because `ipc-protocol` has no JSON dependency.
2. The saved scroll lives in `App::saved_scroll` (keyed by tab id, held
   until the first load after a wake), not in a new
   `UnloadedTabPayload::scroll` field: that struct is built in about ten
   places, and scroll is kept in memory either way.

Also as built: `sleep_tab` became `request_sleep(id, why)` (first phase),
`finish_sleep(id, state)` (second) and the private `sleep_now` (the old
immediate sleep). The probe's dirty case fills the input from the page's
own script (`/tall?dirty=1`) rather than by typing. The probe ran on
WebView2, CEF and Servo (all four cases pass, 2026-09-30). WebKit compiles
with a real handler but has not been run: its host lives in WSL2, which
cannot reach the probe server on Windows loopback. Decision 2026-09-30: no
WSL workaround; test it on a native Linux build (the probe server must be
reachable from the engine host there, which loopback is).
