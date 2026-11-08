# Suppressing an engine's stale url: timer or navigation id (2026-11-07)

Status: decided 2026-11-07: one pending Move (below). The question is the fix named by PARKED.md's "A2" stale-URL
entry: can the host stop hiding the url a navigation left by timer, and key it on a navigation id instead? No id: the
timers stay, merged into one mechanism.

## Decision

`navigated_from` and `App::reported_url` are deleted. `navigate_tab` calls `NavHistory::navigate`, a `New` move (trail
spec decision 3, step 3), and a trail move loads through `App::load_url`, keeping its own move. Picks, 2026-11-07:

- The trail sees the url the engine reported, never the typed one; a held report returns `stale` and the tab keeps its
  url.
- Every move, before its commit, hides every report of its origin except the one the 204 rule takes: a loading report
  while the last was settled marks the move started, a settled one after that ends it. One bit (`started`); an engine
  finishing its own earlier load is not a start.
- A `New` move doesn't block Back or Forward; they replace it.
- The origin is the cursor's URL (`origin: String`, replacing `from`); no move on an empty trail (parked) or to the
  cursor's own URL.
- A `New` move commits on the first report of another URL as a push, never a redirect of the origin's own load.
- A move that ends before its reports do is retired into `NavHistory::held`, whose rules and known limits are the
  trail spec's fold step 3 "Held urls" (`docs/superpowers/specs/2026-10-14-trail-design.md`). Why: two reviews of
  77898981 and one of e029e441 (2026-11-07) found that dropping a replaced move's holds let late reports push
  `a, b, a`, `p, a, b, p`, `a, b` and `a, b, a`. The holds are timed because no engine gives a report a navigation id.

## Before 2026-11-07

Two mechanisms hid one fact, each with its own clock:

- `navigated_from` (`tabs.rs`), read by `App::reported_url` (`event_loop/view_ipc.rs`): set only by `navigate_tab`
  (`viewport.rs`). Until its deadline (`NavHistory::move_window`: 3 s, or 30 s while the engine is loading or new) a
  report of the old url, loading or settled, is shown as the typed url; a settled one ends it once a loading one was
  seen.
- `NavHistory`'s pending `Move` (`trail/history.rs`): set by `start`, `go` and `go_to`. After the target commits, a
  settled report of the origin is held until the deadline (`is_stale_origin`, fold step 3). Native Back/Forward rely
  on this alone (`trail/nav.rs` `send_nav_action`); a plain trail move sets both (`NavAction::Navigate` goes through
  `navigate_tab`).

Other clocks of the same shape: `HOST_START_WINDOW` 2 s (engine side, `nav_label.rs`) and `HOST_LOAD_GRACE` 5 s
(`session_sync.rs`, used by `shares_url_change` when the engine's label is `None`).

## What an id would need

- CEF's leftover old-url report arrives after the host's request and before the engine starts the navigation
  (`viewport.rs` `navigate_tab`, `view_ipc.rs` `reported_url` doc). An id the engine echoes on receipt would already
  be on that report, so the engine must count navigation starts and say which host request caused each one.
- `HOST_START_WINDOW` stays even then: a request can change nothing, and an unexpired label would mislabel the user's
  next click (`nav_label.rs`).
- Requests an engine ignores send nothing (refused Back/Forward guards in every host, CEF with no main frame, Servo on
  an unparsable url). Dropping the deadline needs a "not started" message from every engine, or a timer stays.
- `PROTOCOL_VERSION` 8 to 9, a re-recorded wire fingerprint (`messages.rs` tests) and fixture changes.
- Not removed by any id: same-document changes (no engine gives them an id), the trail's shadow list for
  engine-initiated moves, and A2's open path on engines that can't supply one.

## What each engine offers

Read from the locked crate sources (checksums matched `Cargo.lock`), 2026-11-07. Runtime order is not in the source;
each "undetermined" needs a logged run.

| Engine | Id from the engine | Bound to a start event | Undetermined |
|---|---|---|---|
| WebView2 (`webview2-com-sys` 0.39.1) | `NavigationId: u64` on NavigationStarting, NavigationCompleted, ContentLoading, DOMContentLoaded and the frame events; none on SourceChanged or HistoryChanged; `Navigate` returns `()` | yes, NavigationStarting; matching it to the host's request is by order | id kept across redirects; order of a cancelled navigation's Completed; what a 204 or download reports |
| CEF (`cef` 152.1.0) | `Request::identifier() -> u64`, "globally unique identifier for this request or 0 if not specified": a network request, not a navigation; no id on frame, browser or navigation entry; `load_url` returns nothing | `on_before_browse` (request, `user_gesture`, `is_redirect`); `on_load_start` fires after commit and names no request | id non-zero in `on_before_browse`; kept across redirects; order of `on_address_change` against `on_loading_state_change`; 204 and download |
| Servo (`servo` 0.5.0) | none on the delegate path: `NavigationRequest` exposes only `url`; `load` and `reload` return `()`; `go_back`/`go_forward` return a `TraversalId` echoed by `notify_traversal_complete` (traversals only, at completion) | not on the delegate: `LoadStatus::Started` has no payload and repeats are collapsed; `notify_url_changed` fires on the constellation's `HistoryChanged`. `execute_webdriver_command(LoadUrl \| Refresh \| GoBack \| GoForward)` takes a per-request sender that receives `WebDriverLoadStatus::NavigationStart` | whether `request_navigation` fires for `load`; WebDriver brackets' order against the delegate callbacks; side effects of loading through WebDriver |
| WebKitGTK (`webkit2gtk` 2.0.2) | none; `load-changed` carries only a `LoadEvent` (Started, Redirected, Committed, Finished); the back-forward list's "changed" signal is not bound | Started, by order only (the host polls today and subscribes to neither `load-changed` nor `decide-policy`) | whether host loads and script loads differ in `decide-policy`; `uri()` pending vs committed; Started for traversals and same-document changes |

So only WebView2 can name a navigation, CEF can at best tag its first request, and Servo and WebKitGTK attribute by
order. An id cannot delete the timers while any engine lacks one.

## Options

- **One pending Move** (chosen, see Decision): delete `navigated_from` and `reported_url`; `navigate_tab` sets a
  `Move` with no existing target entry that carries whether loading started. One mechanism and one clock, no protocol
  change, the same on every engine. Leaves A2's open path.
- **Engine navigation counter**: CEF and WebView2 count starts and report `caused_by`; protocol 9 and a "not started"
  message. Fixes A2 there; Servo and WebKitGTK keep timers, so both mechanisms remain.
- **Counter plus timer**: the counter where available, the timer elsewhere. Two mechanisms for good.
- **Keep as is.**
- Weighed and rated lower: holding reports inside the engine until the new url shows (CEF and WebView2 only, and CEF's
  hold would have to last until the address changes, not until the label is consumed).

Evidence: the survey workflow `wf_7ae3bce2-31c` and the source check `wf_18dc5d79-37b` (session transcripts, not kept
in the repo); the claims above are the checked ones.
