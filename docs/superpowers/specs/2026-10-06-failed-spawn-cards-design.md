# Failed spawns become cards; the "No tabs." screen goes (design)

Status: Implemented 2026-10-06, plan `2026-10-06-failed-spawn-cards.md`, commits 86913df, 8e4f9a6, 2729ba1, 8970810 and the Task 4 commit (`git log --grep="empty graph keeps its canvas"`). Was: design for review. Architectural: a failed Root or New tab becomes a persisted, synced document
node, and the empty state, the error display and the toast path all change.

## Problem

1. An empty Graph view skips the canvas and shows one centered label, "No tabs. Click "+ New Tab" to start browsing."
   (`graph_ui.rs`, `build_graph_ui`, the `self.nodes.is_empty()` branch). The canvas, drops and shortcuts are dead
   there.
2. The same branch is the only place a failed spawn explains itself on an empty session: a Root, New or Split spawn
   that fails sets `App::tab_spawn_error`, which is drawn there and in the graph toolbar (`toolbar_spawn_error`).
   A spawn failure for a tab that already existed (Respawn, Restore, Wake) instead keeps its place as a failed card
   (`UnloadedTabPayload` in `UnloadedState::Failed`, `tabs.rs`, `handle_tab_spawn_result`) plus a toast. So there are
   two error models for one event.
3. A restore of many tabs that fail for one cause queues one toast per tab (`queue_toast`).

## Decisions (agreed 2026-10-06)

1. **No "No tabs." screen.** The canvas always renders. While the level has no nodes, a faint, non-blocking hint is
   painted in the centre ("Ctrl+T for a new tab, drop a file or URL, F1 for shortcuts"). It takes no input, so panning
   and dropping still work. It has no error variant.
2. **A failed Root or New spawn becomes a failed card**, by the path Restore, Respawn and Wake already take: an
   `Unloaded` tab in `Failed` with its url, engine, group and position, persisted and synced like the Restore failure
   card. **Split stays a toast**: its secondary pane is `App::split_tab`, not a card.
3. **`tab_spawn_error` and `toolbar_spawn_error` are deleted**, with their clears (`spawn_root_tab`,
   `spawn_new_tab_with_kind`, the group-spawn path). Nothing is left for them to show.
4. **Cards are deduplicated, not coalesced.** A card carries intent (url, engine, position, group); `wake_tab(id)`
   retries exactly that, so merging cards would need a new node kind that holds a list. Before creating a failed card,
   find a failed card with the same (url, engine, group) and refresh its error text and re-toast instead of adding one.
   A different url is a different intent and gets its own card.
5. **Success clears matching failed cards.** When a Root or New spawn of a url succeeds (a fresh attempt; Respawn,
   Restore and Wake landings are existing tabs, not new intent, and clear nothing), failed cards with the same url (any
   engine) are removed through the normal delete queue (`queue_delete`). A card for another url stays. A failure on
   another engine for that url is still informative and is not cleared by a success on a different url.
6. **Failure toasts are coalesced by cause, not by node.** Failures with the same cause (engine and error text) inside
   a short window (about 2 s) read as one toast: "10 tabs couldn't start: Blink (CEF) is not installed". One failure
   reads as it does today. It covers the Restore, Respawn, Wake and new-spawn failure paths.
7. **Retry is unchanged.** A failed card's Retry is `retry_missing_file` then `wake_tab(id)`, which keeps the same id,
   so retrying the same tab never adds a card.

## Out of scope (parked)

- "Retry all N failed tabs" on a level when two or more failed cards share a cause: coalesced action without a
  coalesced node. Add it if the dedup and toast change leaves a real need.
- A card for a failed Split pane.
- **Explore later (parked 2026-10-06):** (a) *coalescing cards*: one card standing for several failed tabs of one
  cause. Needs a node that holds a list of intents, so its type, Retry, move, sync and close semantics are open; this
  design deduplicates same-intent cards instead (decision 4). (b) *toasts as cards*: notifications that are nodes on
  the canvas rather than transient overlays, which would make messages persistent, syncable and closable. Needs a
  decision on what a message node is and when it expires; decision 6 coalesces the existing toast text only.
- A hint variant that lists Recently Closed.

## Risks and open questions

- **Root failure at startup** puts a node into a session that had none. Success switches to Tab view (`spawn_root_tab`
  doc); on failure the view must stay in Graph so the card is visible. The plan checks what view the app is in while
  the Root spawn is in flight.
- **Sync.** Another replica sees a failed tab for an engine it may have. Retry runs locally, so it is harmless, but the
  card reads "failed" there until retried. Same as the Restore failure card today.
- **Auto-clear deletes a document node** (decision 5). It must go through `queue_delete` (2b-5 funnels); the debug
  invariant `check_graph_matches_document` will catch a missed funnel.
- **Dedup key.** (url, engine, group) merges repeats of the same intent. A "+ New Tab" with the home page url repeated
  while the engine is down therefore leaves one card, which is the point.
- **Toast coalescing window.** Toasts are one at a time from `save_toast_queue`, with their duration starting when
  shown (`build_save_toast`). The mechanism (merge into the not-yet-shown queue tail, or a short pending buffer) is the
  plan's choice; the observable rule is decision 6.

## Testing

Rust tests (no engine needed, a failed spawn is injected as `fail_spawns` does in `session_sync/tests.rs`):

- A failed Root and a failed New spawn each leave one persisted failed card (type tab, `loaded` false, Failed state,
  url, engine, group), in the document.
- A second failure of the same (url, engine, group) leaves one card with the new error text; a different url leaves two.
- A success for a url removes its failed cards (and only those), through `queue_delete`.
- Toasts: ten same-cause failures in the window produce one toast with the count; failures of two causes produce two;
  a failure outside the window is a separate toast; a single failure reads as before.
- A Split failure still toasts and makes no card; `tab_spawn_error` no longer exists.
- The empty graph builds the canvas and the hint; the hint is not painted once a card exists.

Live: a probe that forces a spawn failure (the missing-engine case in `page-kind-probe.sh` already exists) and checks
the failed card appears in EXPORTSTATE with `type` the tab type and `loaded` false, that Retry keeps the id, and that
the empty-session screenshot shows the hint and not the old label. Update README lines that quote "No tabs".

## Files expected to change

`graph_ui.rs` (the empty branch, the hint, `toolbar_spawn_error`), `tabs.rs` (`handle_tab_spawn_result`, the spawn
entry points), `main.rs` (`tab_spawn_error` field, toast coalescing), `session_sync/tests.rs` and the new tests,
`scripts/` probe, README.
