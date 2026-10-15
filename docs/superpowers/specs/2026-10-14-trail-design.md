# The trail: Back and Forward over a tab's pages and over opened-from

Status: implemented 2026-10-15 (plan docs/superpowers/plans/2026-10-15-trail.md); design draft 3 as before. (draft 3: draft 1 plus the owner's calls J1-J7 and review rounds 1-8; round 8 found only clarifications, all applied; step 4b (inferred move) added after and revised twice from its reviews). Decisions
were made with the owner in one session.

## Goal

Back and Forward become one traversal. Inside a tab they walk the tab's pages, as now. At the start of a tab's pages, Back
goes to the card the tab was opened from; Forward from there returns to the tab. Pages left behind by navigating from the
middle of the trail stay reachable as branches. Nothing creates a card unless the user asks.

## What exists (checked 2026-10-14)

- `nav_history.rs`: one `NavHistory` per tab id, a vector of URLs plus a cursor, kept in step with the engine's own history
  (native) until the engine is replaced, then used directly (hosted: Back/Forward are a plain `Navigate` to the neighbour
  URL). Saved machine-locally to `tab-history.json`. Fed by `observe(url, loading)` from the `NavigationStateChanged`
  handler (`event_loop.rs:1435`). Back/Forward is `navigate_history` (`tab_view.rs:142`, a second caller at
  `debug_control.rs:1362`), enabled from `can_go_*` (`tab_view.rs:229-231`).
- `opened_from`: field `PARENT`, read by `App::opened_from(id)` (live parent only; reads none while the parent is not
  shown, spec 2026-10-14-closed-field-design.md decision 12). Written by `spawn_tab_from` (`tabs.rs:465`) for every tab
  opened from another: its only two callers are `OpenTabRequested` (target=_blank, window.open, ctrl+click;
  `event_loop.rs:1534`) and "Open in new tab" (`node_menu.rs:742`). Split sets the same edge through
  `spawn_new_tab_with_kind(Some(focused), ...)` (`tabs.rs:604`), not `spawn_tab_from`. Duplicate does not make a child: the
  copy shares the original's parent (`tabs.rs:435-449`). Graph "+ New Tab" parents the new tab to the focused node of any
  kind (`graph_ui.rs:283`), so a parent need not be a tab. Same-tab navigation never creates a node. `GROUP` (membership)
  is a separate field.
- Engines send `NavigationStateChanged { url, title, can_go_back, can_go_forward, loading, user_started: Option<bool> }`
  and `OpenTabRequested { url, user_gesture }`. No engine reports a navigation type, and `user_started` is not reliable
  (`Some(false)` also for the host's own `Navigate`, `crates/ipc-protocol/src/nav_label.rs:30-37`; `None` on Servo and WebKit). The trail does not
  use it.

## Decisions

1. **Scope: tabs.** Only tab nodes have a trail now. The trail is a plain struct with no tab-specific field in its
   interface (locations are strings), so a kind with a location can adopt it later. Pdf, Json, Image and Download have no
   location data to traverse and are out of scope.

2. **The trail is a tree.** `NavHistory` holds an arena `entries: Vec<Entry>` with `Entry { url, title, parent:
   Option<usize>, fwd: Option<usize> }` and a cursor `at`. Entry 0 is the root (the only entry with no parent). The current
   path is the parent chain from `at` to the root. Back goes to `at`'s parent. Forward goes to `at`'s `fwd`. `fwd` of an
   entry is the child the cursor last left it for: it is set when the cursor moves from an entry to a child (a new page, a
   sibling pick, Forward) and when Back leaves a child for its parent, so Forward after Back returns where Back came from.
   Siblings are the children of the same parent. Forward is enabled whenever `fwd(at)` is set: enablement comes from the
   tree, not from the engine.

3. **Forking (J1: always fork).** A new URL that is not a redirect, not the cursor's URL and not a Back/Forward arrival is
   handled by one rule:
   - if it equals an existing child of `at`, the cursor moves to that child (no new entry);
   - else it is pushed as a child of `at` (a new branch; older children stay).
   There is no "drop the forward subtree" case: the engine truncates its own list, the tree keeps everything, and the cap
   (decision 5) bounds growth. So `observe` does not need `user_started`, and scripted changes, typed URLs, another
   replica's URL and a restore all fork alike.
   A user move (Back, Forward, a sibling pick) is one mechanism, a **pending move**: `go()` changes nothing in the tree
   (one exception: the equal-URL commit in step 3). It
   records `Move { target: usize (entry index), from: usize (entry index), native: bool, deadline: Instant }` and returns
   the action for the caller (`GoBack`/`GoForward` or `Navigate(url)`). `at`, `fwd`, `lin` and `lin_at` change only when the
   move commits on its arrival, so a move that never arrives has nothing to undo. `go()` and `observe` take `now` (tests
   need no sleeps); `go()` is ignored while a non-stale move is pending (a rapid second press would otherwise send a second
   `GoBack`). The sibling pick enters through `go_to(entry_index)`, which builds the same plain move.
   The fold, in order:
   1. empty: create the root, `lin = [0]`, `lin_at = 0`.
   2. a stale pending move (past its deadline) is dropped.
   3. pending move, native or plain: a report whose URL equals `target`'s URL commits it. A report equal to `from`'s URL is
      ignored and the move kept (some engines repeat the old URL once; Servo reports only when a document commits, so the 3 s deadline restarts on each report at the origin's URL). A report equal to neither drops the move and falls
      to the rules below, no rewrite (an unrelated navigation during the window, an error page; a redirect that skips the
      `target` URL report is a known limit and forks).
      If `target`'s URL equals `from`'s URL no report can tell them apart, so `go()` commits at once. Native: the engine
      moves a real slot (`GoBack`/`GoForward`), `lin_at` +- 1, and the action is returned. Plain: the tab already shows
      that page and a same-URL `Navigate` is a reload that replaces the slot (measured 2026-10-15: WebView2 keeps the forward entry, CEF consistent; Servo and WebKit not run), so
      `go()` returns `NavAction::None`, sets `lin[lin_at] = Some(target)` with no truncate and no push, and the tab is not
      reloaded. Invariant: `lin` gains a slot only for a real engine navigation.
      **Commit** sets `at = target`, marks `target` *fresh*, sets `fwd` (Back: `fwd(target) = from`; any other move, a sibling pick and Forward included:
      `fwd(parent(target)) = target`), clears `left_to` when the node is a parent (decision 7), and updates `lin`/`lin_at`
      per decision 4.
   4. same URL as the cursor entry: nothing (this runs before the inferred-move step, so a settled repeat of the cursor URL
      and a `reported_url`-masked report, which equals the cursor URL, never reach it).
   4b. **inferred move** (runs after steps 3 and 4, so also after a stale-move or mismatch drop; page script called
      `history.back()` or `forward()`). Engines report the URL before their `can_go_*` flags settle (WebView2's
      NavigationStarting reports the new URL with the old position's flags, `nav_state.rs:122-146`; CEF and Servo get URL
      and flags from separate callbacks), so the flags on a `loading` report cannot be trusted. Inference never decides on
      a `loading` report. The candidate is `Option<(Dir, url, deadline)>`; its target is recomputed from `at`, `lin` and
      `fwd` at resolution, never stored as an index. Candidates: Back: `parent(at)` is `Some(p)`, `lin_at > 0`,
      `lin[lin_at - 1] == Some(p)` and `entries[p].url` equals the report's URL. Forward: `fwd(at)` is `Some(f)`,
      `lin[lin_at + 1] == Some(f)` and `entries[f].url` equals the URL.
      A `loading` report that fits a candidate is **held**: nothing is forked, the report is discarded as if never seen
      (`loading` and `was_loading` are not updated, so the pre-hold values stay for steps 5 and 6), and the candidate is set
      (deadline: the move window, 3 s). A settled report with no prior loading report (CEF, Servo) decides directly.
      Resolution on a settled report whose URL equals the candidate's: Back commits only if that report's `can_go_forward`
      is true (a push leaves no forward entries, so this tells a real Back from a link click to the parent's URL); Forward
      commits if `can_go_forward` is true or `lin_at + 1` is the last slot (at the last slot a click and a Forward give the
      same tree and `lin`; at other slots only the flag separates them, because `lin` is truncated only when the fork is
      processed, so the slot is still present at the report). A commit is the commit of step 3 (`lin_at` +- 1, no `lin`
      push; marked fresh; `loading` taken from the settled report). Otherwise the candidate is dropped and the report falls
      to steps 5-6 with the pre-hold `loading` value, so a link click forks and a redirect after the hold (loading A held,
      then loading B) forks B as a child of `at` (A never gets an entry), or rewrites it only if the pre-hold `loading` was true.
      Known limit: a settled report that carries the new URL with still-stale `can_go_forward` (measured 2026-10-15: WebView2
      reports a `loading` URL with the old flags and then a settled report with fresh flags, as assumed; CEF's settled
      reports carry fresh flags and a `loading=true` report for the same URL follows them, which step 4 absorbs; Servo not
      run, so unverified there) drops a script Back into a fork, and the next same-URL report is ignored by step 4;
      it heals through the cap.
      The candidate only suppresses the fork of the held loading report; a settled report for a candidate's URL decides
      the same way with or without a live candidate, so its deadline and clearing never change a settled outcome. It is
      cleared by `go()`/`go_to`, `view_reset`, any commit, fork or push, any report with a different URL,
      and its deadline. `go(n)` with |n| > 1 and `replaceState`/`location.replace` are not inferred (the latter replace a
      slot and still fork, the known limit). `observe` takes the report's `can_go_back` and `can_go_forward`.
   5. `was_loading` redirect: if the cursor entry is fresh it is rewritten (the user chose that entry; pushing a child
      there would make Back onto a redirecting entry loop, and the usual report order is `T` loading, then the redirect
      `T2`); else (J5) if it already has children the redirect is pushed as a child (a rewrite would change the page its
      children were reached from; the equal-child check above applies, and `fwd(at)` is set to that child); else rewritten.
   6. the fork rule above (sets `fwd(at)` to the new or reused child).
   *Fresh* is a `bool`: "the cursor entry is fresh". It is set by a commit, and by `view_reset` when the tab's URL matches
   `at` (a restored or replaced view reloads the entry, and a redirect there must rewrite it). Steps 5 and 6 read it first.
   It is cleared on every cursor change (a commit sets it anew; a fork, a reused child, a J5 push, a dropped move, an
   eviction that moves the cursor), so it never needs remapping, and at the end of **every** fold branch (steps 3's ignore
   and step 4 included, with the `loading` update) by a settled (`!loading`) report other than the one report that
   triggered the commit. A `go()`-time commit has no such report, so the first settled report, even a same-URL one, clears it. It is not
   persisted and not in the dirty tuple. Move indices (`target`, `from`) need no remap under eviction: eviction follows a
   push, and a push happens only after step 3 drops or commits the move. `go_to(at)` (the cursor entry itself) is ignored.

4. **Native versus hosted (J2: shadow of the engine's list).** The engine's history stays linear and truncates as browsers do.
   `NavHistory` keeps a shadow of it: `lin: Vec<Option<usize>>` (entry indices, `None` a tombstone, oldest first) and `lin_at` (the engine's cursor in
   `lin`). The sticky `hosted` flag is deleted.
   - Native Back (`GoBack`) is allowed iff the engine's `can_go_back` holds, `parent(at)` is `Some(p)` and `lin_at > 0`
     and `lin[lin_at - 1] == Some(p)`. Native Forward (`GoForward`) likewise with `can_go_forward`, `fwd(at)` = `Some(f)` and
     `lin[lin_at + 1] == Some(f)`. The comparison is on `Some` only: a tombstone (`None`), the root's missing parent and
     a missing `fwd` must never compare equal. At the root, Back is opened-from (decision 6), never native. Otherwise the
     move is a plain `Navigate` to that entry's URL.
   - Native and plain moves are both pending moves (decision 3). A native move on commit sets `lin_at` +- 1. A plain move
     on commit is a new engine navigation (below). `go(dir, can_go_back, can_go_forward, now)` and
     `go_to(entry, now)` take the engine's `can_go_*` flags (from the last report) to choose native or plain.
   - A redirect resolution (a rewrite, or the J5 child push) is not a new engine navigation: the engine replaces its current
     slot in place and keeps its forward entries, so it sets `lin[lin_at]` to the resulting entry with no truncate and no
     push. (Native Back then correctly fails the guard and falls to a plain `Navigate`.)
   - `lin` is capped at `MAX_ENTRIES`: past it, drop from the front and decrement `lin_at`.
   - A native move whose arrival lands after the deadline is dropped and is then read by step 4b as an inferred Back or Forward when its URL and
     flags fit, else as a mismatched arrival (a spurious fork); it heals itself and the cap bounds it. Also a known limit: `location.replace` and `replaceState` replace the engine's slot but
     read as a fork, so `lin_at` runs one ahead; a native Back then lands on an older page, reads as a mismatched arrival and
     forks a spurious child. It heals itself and the cap bounds it.
   - Another known limit: a script `history.forward()` is read as a Back when the cursor's parent and `fwd` have the same URL, because `inferred()` tries Back first; the `lin` shadow heals on a later mismatch fork.
   - A new engine navigation (the fork rule, or a plain move's commit) truncates `lin` after `lin_at`, pushes the
     arrival entry's index, and sets `lin_at` to it.
   - On `view_reset` (the engine was replaced, so its list is empty) the pending move is cleared, and: if the tab's URL matches the cursor entry, `lin =
     [at]`, `lin_at = 0`; if it does not, the arena is emptied as `entries.clear()` does today (`nav_history.rs:111-114`),
     and the next report creates the root. `view_reset` runs only when a history already exists (`spawn_result.rs:37`); a new
     tab's first report creates the root through the "empty" fold step.
   - `lin` is not persisted: after a restart `view_reset` rebuilds it, so the first Back/Forward after a restart is a
     plain `Navigate`.
   Cost, documented: a plain `Navigate` loses scroll position and form state (native moves keep them).

5. **Bound (J7).** At most `MAX_ENTRIES` (50) entries per tab. Over the cap, one entry is dropped, chosen in order:
   (a) the oldest leaf that is not on the protected set (the root-to-cursor chain and the `fwd` chain from the cursor);
   (b) if every entry is protected the tree is a chain, so drop the root: its only child becomes the root. If the cursor was
   the root it moves to the new root (reachable only when `capped` trims a loaded file; in live use eviction follows a push,
   so the cursor is the new leaf). Entries are renumbered after a drop (parent < child is preserved); `at`, `fwd` and the
   `lin` indices are remapped. A `lin` slot that names the dropped entry becomes a tombstone (`None`) that never equals a
   real entry, so the engine's page stays counted and native Back/Forward stays aligned (removing the slot would let a
   native `GoBack` land on the dropped page while the tree says otherwise); `lin_at` is unchanged. A `fwd` that named the
   dropped entry becomes `None`. Dropping the root is how the trail shortens from its old end: its Back then still goes to
   opened-from (decision 6) because the new root has no parent.

6. **Trail start follows opened-from.** When the cursor is at the root, Back goes to the raw `PARENT` of the tab, read from
   the mirror's `parent(child)` (the child is shown, and the mirror sees a pending edit or a Create still in
   `pending_creates` that the document does not yet have; not `App::opened_from`, which reads none while the parent is not
   shown; `links.rs:253`). The parent's own state (live, group, closed) is read from the document, as below. In the document
   the parent is `node.get(field::PARENT)` (a `FieldValue::Id`), not a `Node` field:
   - parent shown: focus it, navigating to its level (`show_level_of`, `graph_ui.rs:381`) if it is on another level.
     `focus_item` (`tabs.rs` ~575) has an Item view only for a tab and an image and asserts for other kinds, so for a note,
     pdf, json or unknown-type parent the step is "show its level, select it" through the same level helper;
   - parent live but hidden (closed itself, or inside a closed group): reopen the closed root that hides it
     (`reopen_node`), then focus it. No helper finds that root: walk the `group` chain from the parent, parent included,
     and reopen the outermost closed node; if the parent is still hidden (a nested closed node stays closed,
     `closed.rs:153-164`), repeat until it is shown or no closed node remains on the chain. A reopen at another level only
     toasts (`closed.rs:176`) and does not move the level, so the step finishes with the level helper above, called after the
     reopen has materialised the node (`show_level_of` returns the top level for an id not in `nodes`, `graph_ui.rs:383`).
     `reopen_node` only queues `closed_flags` and calls `save_session()` (`closed.rs:134`), and a tab still spawning or
     waking has no `nodes` entry, so materialisation is not assumed synchronous: the step records a pending focus target (as
     `pending_respawn_focus` does) that is consumed, with the level change and selection, when the id appears in `nodes`;
     the nested-closed repeat re-reads closed state from the document each time it runs;
   The parent is resolved once: p = `PARENT` from `mirror.parent`; while p is not live in the document, p = that node's own
   `PARENT` from `doc.nodes` (bounded by `doc.nodes.len()`). Back is disabled when the walk finds no live ancestor. A closed or never-shown parent deleted (remotely, or evicted past MAX_CLOSED) ends the walk: `OpBody::Delete` clears the node's fields (`store/ops.rs`), and nothing reparents a hidden node's children, so it has no `PARENT` to follow, and Back is disabled (plan D16); a local delete reparents the children first, so it reaches the grandparent. The
   reopen loop above breaks if the outermost closed id is unchanged after a reopen (a failed or read-only save leaves the
   closed flag), and then toasts. A non-tab parent: the step also leaves the Item view for the Graph view
   (`leave_to_graph`, `tabs.rs:636`, which also hides the focused tab or image), since the Back chip exists only in the Item view; an image parent can use
   `focus_item`.
   All of this resolves through the document (`journal.doc().nodes`: `parent`, `group`, `closed()`, `is_live()`, as
   `reopen_node` does), not the mirror: `hide_node` removes a hidden node's `links` entry (`session_sync.rs:921`,
   `mirror.rs:102-109`), so the mirror has no parent or group for a hidden node. "Hidden" is live in the doc and absent from
   `nodes`; "dangling" is not live in the doc (an evicted closed subtree, or a remote delete). A local delete reparents the
   children to the deleted node's own parent (`drop_tab`, `tabs.rs:829-833`; `drop_node`, `node_store.rs:90-93`), so Back
   from a child of a deleted tab goes to the grandparent; Back is disabled only when no live ancestor remains.
   Back is enabled when the trail can go back or when such a parent exists.
   Split: a split tab's parent is the tab it was split from, so Back at its root goes there (J6). Duplicate: the copy has
   the original's parent and no trail of its own (J6: Duplicate gets no trail), so its Back goes to that parent.

7. **Forward from the parent (J4).** The parent's `NavHistory` (any tab) records `left_to: Option<child id>` when Back left a
   child for it. Forward on the parent uses its own forward entries first: if `fwd(at)` is set it wins and `left_to` is
   ignored. `left_to` is cleared at the commit of any move, and at any fork, in the parent (not at `go()`), when the child is deleted, and when the parent
   is hidden or forgotten; it is validated on use against the document (the child must be live and the decision-6 ancestor walk from its `parent`
   must resolve to this node, so a grandparent that Back resolved to after a deletion keeps its `left_to`; else it is
   cleared and Forward is not offered; a closed child has no mirror parent). The toolbar computes Forward as
   `h.can_go_forward() || left_to_valid(doc)`, so enablement needs the document each frame. Setting `left_to` on a
   tab parent with no `NavHistory` creates one (`entry().or_default()`); a non-tab parent never gets one. A history holding
   only `left_to` is not saved and does not count as dirty. With no forward entry, Forward goes to `left_to` (reopening it if closed, then focusing it, as in decision
   6). A non-tab parent (a pdf that opened a tab) has no `NavHistory`: Back from the child still focuses it, and Forward is
   not offered there.

8. **Branch list on the chip.** The tab toolbar's Back chip shows a fork marker when the cursor entry has siblings; the
   Forward chip shows it when the cursor entry has more than one child. Clicking a marker opens a small list of those
   entries (title, or the URL when the title is empty). A row click starts a plain move to that entry (`go_to`; decisions 3 and 4). A row's context menu offers "Open as card". No trail strip (parked).

9. **Promotion.** "Open as card" calls `spawn_tab_from(opener = this tab, url)`, which sets opened-from and the group as for
   any opened tab. The trail is not changed. No navigation creates a card.

10. **Persistence (J3: the trail survives close and reopen).** `tab-history.json` stores each tab's tree: `{ entries: [{
    url, title, parent, fwd }], at }`. `lin`/`lin_at` and `left_to` are not saved (see 4 and below). Load keeps a list only
    if `at` is in range, entry 0 is the only entry with no parent, every other `parent` index is less than its own index,
    and every `fwd` names a child of its entry, else drops it. The 50-entry cap also applies on load by running the decision-5 eviction loop
    (the window-around-the-cursor `capped`, `nav_history.rs:177`, is deleted). A file in the old format fails to parse and the whole file is moved aside by the existing loader
    (`json_file.rs:39-59`, to `tab-history.unreadable-<secs>.json`; `load_from` takes the `Err` as empty, no toast): no
    migration (nothing is released). Lists with one entry are not saved. The file stays machine-local.
    A closed tab keeps its trail, through two changes. (1) `hide_node` must not drop it: today `hide_node` -> `unshow` ->
    `forget_id` removes `nav_history[id]` (`closed.rs:123` calls `hide_node`, `session_sync.rs:912-924`; `node_store.rs:132,155`). `unshow` and
    `forget_id` stay as they are (a delete of a shown tab or node also reaches `unshow`, and must drop the list at once;
    `forget_id_clears_every_id_keyed_table` in `main/tests.rs` stays valid). Instead `hide_node` follows the existing
    `hidden_local` pattern ("a hide re-inserts its entry after this", `node_store.rs:150`): it takes `nav_history.remove(id)`
    before `unshow` and re-inserts it after, clearing `left_to`. A delete, an eviction past `MAX_CLOSED` and a remote delete
    of a hidden node drop it through `forget_deleted` -> `forget_id` (`session_sync.rs:941-943`). (2) The prune predicate
    (`keep`, `flush_history`, `session_sync.rs:1318`) becomes `nodes.contains_key(id) || journal.map_or(true, |j| the id
    parses to a NodeId and j.doc().nodes.get(&id).is_some_and(Node::is_live))`: document liveness is added to the `nodes`
    test, not substituted for it, and with no journal everything is kept (`flush_history` runs without a document, on a
    store error, and for a tab whose Create is still in `pending_creates`). So a closed tab (and a closed parent) survives
    the save, Ctrl+Shift+T and a restart, with no dependence on when `hidden_local` is loaded (it can be filled late by
    `retry_read_local` and holds only ids with a local entry). A restored live tab materialises as a Sleeping placeholder in
    `nodes`, so it needs no special case. `teardown_running_session` (`session_sync.rs:2211`) calls `forget_id` for ids in
    `nodes` and `spawning` only; it gains an explicit `self.nav_history.clear()` (the default jar is not a tab), so a stashed
    list does not survive a session replace. Leak bound: lists are created only by local `observe`, `go()` and `left_to`, so
    other replicas' tabs get none; the extra lists are those of closed roots (`MAX_CLOSED` = 20 plus their subtrees) and live
    tabs, each at most 50 entries, and a node no longer live is pruned at the next flush.
    `left_to` is not saved: the opened-from edge reconstructs the link from the child's side; Forward on the parent after a
    restart is not offered.
    The dirty-tuple that decides whether a save is needed (`nav_history.rs:66`) must cover the tree (entries, parents, `fwd`,
    titles, `at`), not only the old (urls, index).

11. **Titles.** `observe` also takes the report's `title` and stores it on the cursor entry when non-empty (after a move commits, that is the target). A title that
    arrives after the URL (the usual order) updates the cursor entry in place and does not count as a navigation. A title
    arriving in the same report as a new URL is applied to the new entry only when the report is settled (`!loading`), since
    some engines send the previous page's title first; a transient wrong title is accepted otherwise.

12. **No engine signal needed.** The trail uses only `url`, `title` and `loading` from the report, so Servo and WebKit
    (`user_started` is `None`) behave like the other engines. The cap bounds the cost of scripted changes forking.

## Touch points

`nav_history.rs` (tree, fork rule, `lin` shadow, eviction, persistence, `left_to`), `event_loop.rs` (`observe` call gains
`title`), `tab_view.rs` (`navigate_history`, `can_go_*`, chip markers and list), a new small `trail_nav.rs` (or functions in
`tab_view.rs`) for the opened-from step (parent resolution via `mirror.parent`, the closed-root walk and `reopen_node`, the
level helper and selection for non-tab parents), `tabs.rs` (`spawn_tab_from` caller for promotion), `session_sync.rs`
(`hide_node` stashes and re-inserts the list, `flush_history`'s `keep` adds document liveness, `teardown_running_session`
clears the lists), `tabs/spawn_result.rs` (`view_reset` call), `debug_control.rs` (the second
`navigate_history` caller; EXPORTSTATE gains `trail` per tab: entry count, cursor depth, fork flags, `lin_at`, so a gate stage
can read it), gate stage `trail`, README.

## Out of scope / parked

A trail strip under the card; trails for Pdf, Json, Image and Download (a location field per kind first); syncing trails;
scroll and form restore on a sibling switch; keeping `left_to` across a restart; Alt+Left/Alt+Right and mouse buttons 4/5
(no handler today; routing them to the same function is a one-line follow-up once this lands); a redo stack.

## Tests

Existing `nav_history.rs` tests are rewritten or deleted with the code they cover, not kept beside it:
`follows_navigation_and_native_back_forward` (it asserts the forward entries drop), `a_single_page_tab_stays_native_after_a_reset`,
`a_hosted_load_that_redirects_replaces_the_entry` and `a_replaced_view_keeps_back_and_forward` (they assume `hosted`),
the round-trip and `the_list_is_capped` tests (old `Saved{entries: Vec<String>, index}` format), and
`a_saved_list_past_the_cap_is_cut_around_its_current_entry` (old window `capped`).

Unit (`nav_history.rs`): the fork rule reuses an equal child and otherwise adds a sibling; a redirect on an entry with
children adds a child (J5); the `lin` shadow allows native Back/Forward only when it matches the tree and falls back to a
`Navigate` after a sibling pick; `view_reset` rebuilds `lin`; eviction keeps the path and the `fwd` chain, drops the root
when the tree is a chain (cursor at the root moves), and remaps `lin`, `at` and `fwd`; persistence round trip, and a bad
index, a second root, or a `fwd` that is not a child drops the list; the dirty tuple changes when only a title or a `fwd`
changes; a repeated old URL after `GoBack` does not clear the pending move; a mismatching arrival after `GoBack` forks
instead of moving the cursor; a plain `Navigate` sibling pick does not push twice; an eviction that drops a `lin` slot
leaves a tombstone and native Back still lines up; an evicted `fwd` becomes `None`; `view_reset` with a differing URL
empties a restored tree and the next report makes a new root; a `left_to`-only history is not saved; a redirect resolution sets `lin[lin_at]` with no truncate or push; a stale
pending move leaves the tree and `lin` untouched; a plain move whose arrival is a redirect rewrites the target; a
session replace clears the stashed lists; a move whose target URL equals the origin's commits at `go()`; a fresh
navigation during a pending plain move leaves the target's URL untouched; the redirect report order (`T` loading, then
`T2`) on a target with children rewrites it; `view_reset` while a move is pending clears it; `fresh` is cleared by a settled report, a mid-trail reload redirect (not
fresh) pushes a child, Back onto an entry with children then a redirect rewrites it, and `view_reset` makes the entry fresh;
a plain equal-URL move leaves `lin.len()` unchanged and returns no action; an inferred Back
(script `history.back()`: URL equals the parent's, settled report with `can_go_forward` true) moves the cursor, while a
link click to the parent's URL (settled, `can_go_forward` false) forks, including with a fresh cursor entry (a held
loading report must not make step 5 rewrite it); loading A held then loading B forks B as a child of `at` with no entry for A; held loading A then a repeated or
title-only settled A; a candidate that expires while held, then settled A; an inferred Forward at the last slot (marked
fresh, unlike the fork path that reuses a child; harmless); `go()` while a candidate is held
clears it; a settled report with no prior loading report decides directly; the real engine order (a `loading` report with
stale flags, then a settled report with fresh flags) infers correctly in both directions, including a script back from the
last slot and a link click to the parent's URL at a non-last slot after a native Back; an inferred Forward, also to the
last slot, leaves `lin.len()` unchanged; `fwd` after Back, after a
sibling pick and after a Forward-chip pick; a repeated old URL after a native move does not drop it; evicting the root leaves Back at the new root non-native; `go()` is
ignored while a native move is pending and works again after the deadline; Back onto an entry whose URL redirects stays
on that entry; `lin` is capped. App level: hiding a tab keeps its list (and a restart keeps it),
deleting a shown tab drops it at once. App level: Back at the root focuses the parent (on another level too, and for a non-tab parent); with the parent
closed, Back reopens and focuses it, including a nested closed group; with it deleted, Back goes to the grandparent, and is disabled only when no live ancestor remains; Forward on the
parent returns to the child (reopening it if closed), the parent's own `fwd` wins over `left_to`, and a stale `left_to` is
cleared; a closed tab keeps its trail across Ctrl+Shift+T; promotion creates a tab with opened-from and leaves the trail
unchanged. Gate stage `trail`: open a tab from another, navigate twice, Back three times, check the parent is focused,
Forward returns; a fork produces a sibling in EXPORTSTATE.
