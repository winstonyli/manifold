---
status: done
---

# Wave 2: performance (design)

Status: Implemented 2026-10-08 on `wave-2-performance` (commits `2ae8f76`, `b6542bf`, `36338f7`, `9f7018c`, `6ff1740`,
`2da1360`, `285eadf`, `6d6b759`, `01eb4f0`, `9444b65`, `31f9829`, `6fd74fc`, `c1554a5`, `e95be81`, `bf11332`, `8b2060d`); see
the plan's Outcome for the numbers and deviations. Designed 2026-10-07. The plan is `docs/superpowers/plans/2026-10-07-wave-2-performance.md`. The scope
and the estimates come from the read-only fact check `.superpowers/survey/verified-wave-2.md` (at `c8b7485`), which ran no
code: every cost in it is an estimate. This spec re-read every line and symbol it relies on at `c8b7485`. Where it departs
from the verified file, it says so ("Deviation").

Paths are under `crates/manifold/src/`. Line numbers are as of `c8b7485`. Wave 1 (`wave-1-integrity`) lands first and edits
some of the same functions (`teardown_running_session`, `apply_pulled_cookies`, `jar::from_json`, `destroy_group`,
`JsonView`), so find code by the quoted text, not by the number.

## Decisions

1. **Measure first, then change, then measure again.** Every optimization is two commits:
   - a commit that adds only tests: a correctness pin (golden or reference) and an `#[ignore]` timing test;
   - a change commit, made only when the first commit's numbers clear a stated threshold, and kept only when the numbers
     after it clear a stated keep rule.

   Where no harness exists, building it is its own task (the shared bench helpers, the JSON reveal mode of
   `scripts/json-pan-profile.sh`). Every number goes into the plan's Outcome, with best and median, CPU load and the
   Defender real-time state (LONG_RUNS.md).
2. **The verified file's 15-commit split, plus a harness commit in front.** The order and the verdicts are kept:
   - the Mirror link-by-endpoint index is dropped unless the link benches show it is needed (conditional task);
   - H5 is a no-op fast path in `save_session`, not a dirty flag over 55 call sites, and only if a no-op save costs over
     0.3 ms;
   - search debounce and a thread per keystroke are dropped;
   - M2 (lazy outline rows) is deferred;
   - of H4, only the `pick_targets` binary search and the Tab-slot map are kept.
3. **One branch, one full gate.** The wave is built on `wave-2-performance`, branched from `main` after wave 1 merges
   (CLAUDE.md "Branches"). Each task runs `cargo test`, `cargo fmt --all` and the clippy line. One
   `scripts/gate.sh LOG full` covers the whole wave before the merge. This overrides the verified file's "gate after
   commits 6, 11, 13, 14 and 15". Each task still records its own gate class, for a task landed alone later.
4. **No behavior change.** Every change keeps what the user sees and what is stored. Each one is pinned by a test that
   passes before and after the change:
   - a golden (bit-identical layout positions; the painted texts of a card);
   - a reference (the old search kept as a test copy; `valid_targets` against `check_connect`);
   - a counter that states the intended difference exactly (jar saves, `Journal::begin` calls).

   Three exceptions say so:
   - Jar H3 changes timing, not results: jars are written once per pull round instead of once per pull.
   - H5 changes timing, not results: a save with nothing to write skips the store lock.
   - H1b (conditional) changes what a far reveal shows. The card shows a window of pages around the revealed entry,
     not every page from the top. A `+ N earlier` row grows the window up one page at a time.
5. **No test touches the real `%APPDATA%\manifold`.** Tests use `App::default()` and `store::testutil::temp_dir`.
   Headless timing runs use `--profile-dir` under `target/`.

## Measuring

- **Timing tests** are `#[ignore]` tests run as `cargo test --release -j 8 -p manifold --bins <name> -- --ignored --nocapture`.
  They report best and median of N runs. They share one helper module, `bench.rs` (`#[cfg(test)]`): `timed`, `report`,
  and the 2000-node fixture that `graph_nav::tests::level_pass_cost` builds today, moved there and given real store ids
  (`alloc_node_id`), so link and save code can use it.
- **Before every timing run:** CPU under about 20% (`\Processor(_Total)\% Processor Time`), and the Defender real-time
  state recorded. Report best-of-N, never a single run.
- **Whole-app timing** for the JSON level uses `scripts/json-pan-profile.sh` with `TIME_ONLY=1` (no samply). The cost per
  frame is the difference between two runs of different length, divided by the difference in command count, so the
  setup cancels out.

## H3. Settle layout (`graph_layout.rs:77-176`)

`step_sized` looks up both cards' sizes in a `HashMap<String, Vec2>` inside the O(n²) pair loop (line 115), twice per pair,
90 times per settle. The change builds `size: Vec<Option<Vec2>>` once per step, indexed like `pos`, and reads `size[i]`,
`size[j]`.

- **Pin:** a test copy of today's `step_sized` (`step_sized_reference`). A golden test runs both over 40 cards for 90 steps
  and requires bit-identical positions and velocities (`to_bits`). The cards are sized and unsized, two start on one
  point (the `spawn_jitter` path), some have springs, and one is pinned. The arithmetic is unchanged (`(a + b) / 2.0`
  with the same operands), so bits must match.
- **Measure:** `settle_cost`, `settle_current_level` over the fixture spread on a grid, at the top level (about 675 cards: 667
  ungrouped nodes and the 8 groups) and in group g0 (about 167). Proceed if g0's best is over 8 ms. Keep if g0's best drops by 25% or more.
- Deviation: the verified file's "optional hoist (2b)" is not planned. The size index is the only change.

## JSON H1a. Rows culled to the camera (`json_level_ui.rs`, `json_view.rs`, `json_level.rs`, `card.rs`)

Revealing entry 199 999 of an array sets that card's `pages` to 10 000, so the card has 200 000 rows. Every frame:

- `paint_json_content` lays out every row's text;
- `draw_json_level` builds a `shown` Vec of every row's pointer (lines 517-520) to find the selected row and the hit rows;
- it asks egui's memory `has_focus` once per card in the cull (line 513) and once per card in the Tab scan (line 484).

The change:

1. `paint_json_content` paints the header, the frame's divider, the `+ N more` row, and only the entry rows that intersect
   the visible world rect (`CardLook::visible`). `json_view::visible_rows(rows_top, rows, visible)` gives the range.
   `json_view::entries_from(value, start)` starts the walk at the first visible row (`entries` becomes
   `entries_from(value, 0)`).
2. The `shown` Vec goes:
   - The selected row is `json_view::row_of(value, card, selected, window)`. This is the entry index of the selected
     pointer's last token, when that pointer is a canonical child of the card within its rows.
   - The hit rows come from `Search::hit_rows`, built once per search result by `json_view::hit_rows_by_parent`: sorted
     row indices per parent pointer.
3. The focused id is read from egui's memory (`Memory::focused`) only where it can change, not once per card: once
   before the Tab scan, again after the Tab request, and again after each drawn card's `interact` and root-focus
   request. The scan and the cull compare a card's id with that local copy. `has_focus(id)` is
   `focused() == Some(id)`. egui applies `request_focus` and its own Tab (`Next`) move inside `interact` at once within
   the pass, so this equals today's per-card memory read. (Its Shift+Tab move, `Previous`, applies next frame through
   `id_next_frame`; it fires only when the app's lock filter is not yet set, see Risks.) Deviation: the verified file
   says `request_focus` applies "next frame"; egui 0.36.2's `Memory::request_focus` sets the focused widget at once.
4. The Json card's preview at its parent's level passes `visible: Rect::EVERYTHING`. Float-to-int casts saturate, so
   that gives every row, as today.

- **Pins:**
  - a painted-text golden of one card, the same 54 texts before and after;
  - a culled-card test, only the visible rows' texts;
  - the existing reveal test (`/users/33` gives pages 2);
  - a Tab-walk test through `build_graph_ui`: Tab and Shift+Tab move between cards (an idle frame after each key; see
    Risks "egui focus semantics"), and a focused card panned out of view keeps focus.
- **Measure:**
  - `reveal_bounds_rows` (`json_view` test) states the size: revealing `/items/19999` of 20 000 objects gives 20 002 cards,
    the array card with 20 000 rows;
  - `json-pan-profile.sh` modes `reveal-numbers` and `reveal-objects` (200 000 entries; `REVEAL_N`) time the frame after a
    reveal, against today's pan baseline of about 33 ms per command.
- **Proceed** if `reveal-numbers` is over 1.2 times the pan baseline. **Keep** if `reveal-numbers` drops by 30% or more,
  or comes within 20% of the pan baseline.

## JSON H1b. Windowed paging (conditional)

H1a leaves arrays of objects slow: each revealed object row is its own card, and `visible_cards` builds 200 000 of them.
H1b bounds a card's rows to a window: a new `CardOverride.first` page (saved with `pages` in the node's `VIEW`).

- `reveal` of an entry far below the shown pages moves the window to it, instead of growing `pages` to reach it. It grows
  `pages` only when the entry is within `REVEAL_GROW_PAGES` (5) pages of the window's end.
- A `+ N earlier` row above the window (`JsonEdit::Earlier`) shows one more page above: `first` goes down one and
  `pages` up one, so the window grows up.
- A card's parent row index stays the row as drawn (`lead + row`, where `lead` is 1 when the earlier row is shown), so
  `level.rs:137` is unchanged.
- **Compatibility:** an old `VIEW` (no `first`) reads as `first: 0`, so it shows exactly what it showed. A `VIEW` written
  with `first` and read by an older build:
  - a build with wave 1's M5 keeps `first` in `Unread::extra` and writes it back on its next edit. But its `reveal` and
    `+ N more` still count `pages` from page 0 (`pages = index / PAGE_SIZE + 1`), so an H1b build reading that view back
    shows `first .. first + pages`, a much larger window than intended (up to every page from `first`). Nothing is lost,
    and the window is bounded again at the next reveal that falls outside it; accepted as a known effect of mixing
    builds;
  - a build before wave 1 drops `first` on its next edit of that view, and shows the first pages, as it does for any view.

  `CARD_OVERRIDE_KEYS` (wave 1) gains `"first"`, and its guard test's literal sets it. A round-trip test pins an old
  view's text unchanged.
- **Run only if** `reveal-objects` is over 1.5 times the pan baseline after H1a. Keep if `reveal-objects` drops by 50%
  or more. H1b builds on H1a's `entries_from` and its culled painting. If H1a was skipped or reverted, the trigger uses
  the pre-H1a `reveal-objects` baseline, and H1b is either parked (with that number recorded) or runs after first
  landing `entries_from` alone, painting its window without H1a's culling (plan Task 14). It is the riskiest JSON change (stored field, outline and spec text), so it runs last of the JSON tasks.
- The spec text "Outline / reveal" in `docs/superpowers/specs/2026-09-27-json-node-rework-design.md` is updated by the
  task that lands it.

## Jar H3. One jar save per pull round (`tabs.rs`, `jar_defs.rs`, `event_loop.rs`)

Every 30 s `jar_pull_all` asks each WebView2 and CEF tab for its cookies. Each reply that changes the jar calls
`save_jar`: serialize, DPAPI-encrypt, write a temp file and rename it. Fifteen tabs that saw one rotating cookie save the
same jar 15 times per round.

The change:

- `apply_pulled_cookies` marks the jar dirty (`App::jar_dirty: BTreeMap<JarKey, Instant>`, the first change's time)
  instead of saving it.
- `flush_dirty_jars(Some(now))` runs in `about_to_wait`. It saves a dirty jar once no pull is pending (no `jar_pulls`
  entry sent less than the 10 s `JAR_PULL_TIMEOUT` ago, the `tabs::pull_pending` rule; a pull that timed out unanswered
  keeps its entry until that tab's next pull, so emptiness is not the test), or once it has been dirty for `JAR_FLUSH_AFTER` (2 s).
- `flush_dirty_jars(None)` saves every dirty jar at quit, in `exiting`, after `final_cookie_pull`.
- Every other path that writes or drops the jar's file settles the dirty mark first:
  - `switch_jar_ephemeral` off saves, so its mark is removed;
  - `clear_jar_cookies` saves the emptied jar, so its mark is removed;
  - `forget_jar` deletes the files, so its mark is removed;
  - `request_jar_ephemeral` with `on` saves a dirty jar before `has_saved_file`. A jar whose first cookies are still
    unsaved therefore still asks before its logins are deleted, exactly as today.
- A dirty named jar deleted meanwhile is skipped at flush (its definition is gone), as `apply_pulled_cookies` already
  skips one.
- **What changes:** the jar's file lags memory by at most `JAR_FLUSH_AFTER` (2 s) plus
  one event-loop wake (at most 100 ms, `IDLE_FRAME` or `MINIMIZED_TICK`). A crash in that
  window loses those cookie changes. New hosts seed from `jar_snapshots` (memory), which is still updated at once, so
  tabs never see the lag. The README's cookie-jar paragraph says so.
- **Pins:**
  - `a_burst_of_pulls_saves_the_jar`: 15 tabs pull a rotating cookie into one jar. Today that is 15 saves, and the file
    holds the last value. After the change it is 0 saves until the flush, then 1, and the file holds the last value.
  - A flush-timing test covers the round rule.
  - The existing jar tests keep passing. One of them, which checked the file right after a pull, flushes first.
  - The saves are counted by a `#[cfg(test)]` counter on `App` (`jar_saves`), so parallel tests don't share it.
- **Measure:** `jar_save_cost`, one 3000-cookie jar's `save`. Proceed if the burst test shows more than one save per
  round and `jar_save_cost`'s best is over 3.2 ms (15 saves would then cost over 48 ms, about three frames, every 30 s).
  Keep if a burst saves exactly once.
- Deviation: the verified file calls the tests-only commit "Full". It adds tests and a `cfg(test)` counter only, so its
  own class is light. The change commit is full.

## M1. JSON search without per-node allocation (`json_view.rs:442-480`)

Below `WORKER_SEARCH_NODES` (100 000) the search runs on the UI thread, per keystroke. For every node it:

- builds the pointer `String` and every child's pointer (`child_pointer`, two `replace` calls);
- builds a `Vec` of children;
- lowercases the key and the scalar text into new strings.

The change is an explicit depth-first walk:

- a stack of child iterators (`Children::Object` / `Children::Array`);
- one reused path `String`, truncated on pop, with `~` and `/` escaped in place;
- a scratch `String` for numbers (`write!`);
- `contains_lower`: when both the needle and the text are ASCII, a byte-window compare with `eq_ignore_ascii_case`;
  otherwise `to_lowercase().contains`, as today. A non-ASCII text can lowercase to ASCII (the Kelvin sign), so the fast
  path needs both ASCII.

Kept exactly:

- pre-order document order;
- array indices are never matched, only object keys and scalars;
- the root has no key;
- `MAX_MATCHES` and `truncated`;
- the `keep` ancestors;
- the cancel check every 4096 nodes visited.

- **Pin:** a test copy of today's `search_with` (`reference_search`). `search_matches_reference` compares whole
  `SearchResult`s for queries chosen to hit the edge cases: empty, case, Kelvin sign, dotted capital I, `ß` and `ss`, `~`,
  `/`, escaped keys, numbers, `true`, `null`, no match. It also checks a 10 005-match value (truncation) and a cancel.
- **Measure:** `json_search_cost` over 14 000 items of 7 nodes (just under the worker threshold), queries `item 9` (many
  matches) and `zzz` (none). Proceed if either best is over 8 ms. Keep if both drop by 25% or more.
- Debounce and a thread per keystroke: dropped (verified file: a stale worker stops within 4096 nodes, and `value` is an
  `Arc` clone).

## H2 / P2. Link queries, and the two H4 one-liners (`links.rs`, `node_menu.rs`, `card.rs`, `graph_ui.rs`)

In pick-target mode every frame calls `valid_targets(edge, from)`. For a link edge, its `check_connect` per candidate
calls `link_between`, which scans every link (`links_naming`) with a `to_string` and a field read per link. That is
O(N·L) per frame. `edge_menu` builds the whole sorted target list just to test it is non-empty.

The change:

1. **Hoist the "already linked" set.**
   - `linked_to(from)` builds once the set of node ids `from` is linked to (from `links_naming`).
   - `linked_for(edge, from)` builds it only for a node-stored edge.
   - `check_connect` takes `linked: Option<&HashSet<NodeId>>`: with a set, "exists" is a set lookup; with `None`, it is
     `link_between` as today. `valid_targets` and `has_valid_target` pass the set; `connect` passes `None`.
2. **`edge_menu`** tests `has_valid_target` (stops at the first) instead of `!valid_targets(..).is_empty()`.
3. **`pick_targets`** (sorted by `valid_targets`) is searched with `binary_search`, in `graph_ui.rs` and in the card's ring.
4. **The Tab slot** comes from a map built once per pass (`CardPass::slots`: first index of each id in `tab_order`). Today
   every card does `order.iter().position`.

- **Pins:**
  - `valid_targets_match_check_connect`: a property test over 60 random cards (notes, sleeping tabs, completed and
    running downloads) in three levels, with 300 random connects (at least 15 links made), half-made and dangling links. The ids are
    sorted before the seeded picks, since `HashMap` order is random. For every from, to and
    edge, membership in `valid_targets` equals `check_connect(.., None)` (the unhoisted path), and `has_valid_target`
    equals non-emptiness.
  - A badge test pins the Tab-slot numbers drawn on cards.
- **Measure:**
  - `link_query_cost`: `valid_targets` and `has_valid_target` at g0 with 20, 100 and 300 links;
  - `graph_frame_cost`: `build_graph_ui` frames over the 2000-node fixture, idle, hovering, in pick mode, with the node
    menu open, and filtered.
- **Keep:**
  - the hoist and `edge_menu` if `valid_targets` at L=100 gets twice as fast or more;
  - the binary search if the pick-mode frame gets 5% faster or more;
  - the slot map if the idle frame at the top level gets 5% faster or more.

  A part that misses its rule is reverted before the commit and recorded.
- **Dropped:** the borrowed `CardView`, `ArrayVec` controls, lazy tooltips and a `Kind` cache (verified file: a few small
  allocations per visible card).

## Teardown order (`session_sync.rs:2017-2060`)

`teardown_running_session` queues a delete for every live node, and each `queue_delete` scans every link
(`links_naming`). Only then does it delete the links. Moving the link loop first is not enough: each link's own
`queue_delete` would still scan the links left, O(L²). The change forgets every link in the mirror first
(`Mirror::forget_link`), then queues each link's delete, then the nodes'. Every scan then sees no links: O(N·L) becomes
O(N + L).

The end state is the same set:

- `pending_deletes` holds every live non-jar node and every link;
- `pending_creates` is empty;
- the mirror holds no links.

A new test pins it before the change: `teardown_queues_every_node_and_link_delete`. Measure: `teardown_cost`, 2000 notes
and 300 links. Keep if it gets twice as fast or more. Gate: full (session code).

## H5. A save with nothing to save (`session_sync.rs:1173-1378`, `store/journal.rs`)

Every user action ends in `save_session`, often twice: the action saves itself, then `apply_graph_action` saves again.
A save with nothing queued still does all of this:

- takes the profile lock;
- runs `catch_up` (reads the file's tail);
- builds the empty transaction;
- runs the housekeeping.

The change is a fast path after `commit_note_buffers`. It applies when:

- all four queues are empty (`pending_creates`, `pending_moves`, `pending`, `pending_deletes`);
- no retry is scheduled (`save_retry_at` is `None`);
- the journal is quiet: `Journal::quiet()` is not stale, not read-only, has no recovery to report, has seen the file
  at least once (`seen` is set), and the file has not changed on disk since this replica last read or wrote it
  (`changed_on_disk`, a metadata read).

The fast path runs the same housekeeping as a successful save and returns `true`. The housekeeping moves into
`after_save()`, called from both paths:

- the mirror, layout and held-text retains;
- `store_notices.clear()` and `last_session_save`;
- `save_retry_at = None` and `flush_history`;
- the debug graph check.

A changed file, a stale or read-only journal, or a pending recovery takes the full path as today. So another replica's
frames are still applied by the first save after they land, and `poll_store` still applies them between saves.

- **Pin:** `save_count_per_action` counts `Journal::begin` calls (a `#[cfg(test)]` counter on the journal) for four
  actions through `apply_graph_action`: a menu Delete of a note, entering a group, a Join drop, and finishing a link.
  Today: 2, 1, 2, 2. After: 1, 0, 1, 1.
- **Measure:** `app_save_costs` already has "save_session, nothing changed". It is extended to 2000 tabs. Proceed if its
  best at 2000 tabs is over 0.3 ms. Keep if it gets twice as fast or more and the counts drop as stated.
- **Deviations:**
  - The verified file's "drop the EnterGroup/LeaveGroup save" is not planned. With the fast path that save costs one
    metadata read, and `switch_group_level` calls `end_note_edits`, which saves on its own when a note was being edited.
    Dropping the save would only remove an apply-remote opportunity.
  - The stale `destroy_group` comment ("there's no save on exit") is not this wave's: wave 1's P1 rewrites that comment.
  - The tests-only commit is light (the verified file says full): it adds a test and a `cfg(test)` counter.

## Mirror link index (conditional; `mirror.rs`)

`links_naming(node)` scans every link. After the hoist and the teardown reorder, the remaining callers run once per action
or per menu (`edges_of`, `queue_delete`, `retarget_links`, the Disconnect check), not per candidate per frame.

**Run only if** `link_query_cost`'s `valid_targets` at L=100 is still over 1 ms after the hoist. If it runs:

- `Mirror` keeps `links_at: HashMap<NodeId, BTreeSet<NodeId>>`, the links naming each end;
- it is maintained in `set` (for `FROM`/`TO` of a known link: unindex the old ends, index the new), `add_link` and
  `forget_link`;
- `links_naming` reads it, in link-id order as today;
- a `#[cfg(debug_assertions)] check_link_index` runs in `check_graph_matches_document`;
- a property test runs random set, add and forget sequences against a rebuilt index.

Keep if `valid_targets` at L=100 gets twice as fast or more. Gate: full (mirror).

## Not in scope / parked

- M2: lazy outline rows. Rebuilt only on open or search change, not per frame; deferred until measured.
- A dirty flag over every `save_session` call site (55 sites). The fast path covers the no-op case.
- Search debounce, a thread per keystroke.
- H4's borrowed `CardView`, `ArrayVec` controls, lazy tooltips, `Kind` cache.
- The settle's optional hoist (2b).
- Data-layer P1/P3 and C1/G1, which touch the same functions as H5 and the mirror index: their waves coordinate with this
  one by quoted-text anchors.

## Risks

- **Wave 1 overlap.** Wave 1 rewrites `jar::from_json` (Result), `teardown_running_session` (`forget_id`, `spawning`),
  `destroy_group` and parts of `JsonView`. Tasks anchor by quoted text and say which wave-1 change they sit beside. Wave 1's
  App-field scan (C1) matches `HashMap<String`, `HashSet<String`, `Option<String`, `Option<(String` and `Vec<String`.
  `jar_dirty: BTreeMap<jar::JarKey, Instant>` and `jar_saves: AtomicUsize` match none, so its `NOT_FORGOTTEN` list needs
  no entry.
- **Wave 0 overlap.** Wave 0 enables `allow_attributes_without_reason`, so any `#[allow]` this wave adds takes
  `reason = "..."`. Its other changes (toolchain pin, lints) touch none of this wave's code paths.
- **egui focus semantics.** The `focused` tracking relies on `request_focus` and `interact` changing focus within the pass.
  The Tab-walk test drives real frames to pin it. `set_focus_lock_filter` applies only to a widget that also had focus
  last frame, so a card the app's Tab code just focused gets its lock filter one frame late. A Tab in the very next frame
  then also runs egui's own move and skips a card. The test puts an idle frame after each key so it does not encode that
  edge case; the skip is today's behavior and is parked, not changed by this wave.
- **Timing noise.** Other sessions load the CPU and the eGPU. Each bench is best-of-N with a CPU check. A keep decision
  that sits within 10% of its threshold is rerun once on a quiet machine before it is recorded.
- **Jar crash window.** Stated above and in the README. The flush runs in `about_to_wait`, at least every `IDLE_FRAME`
  (100 ms) while the window is open and every `MINIMIZED_TICK` (100 ms) while minimized, so a due jar is written within
  100 ms of its 2 s mark.
- **H1b stored format.** A new optional field in `VIEW`. Old views read unchanged. An old build reading a new view shows
  the first pages, which is what it shows for any view today; one with wave 1's M5 keeps the field on its edits.
  Conditional and last.

## Testing

- Each task: its tests (written first, and seen to fail where a test can fail first), `cargo test -j 8 -p manifold`,
  `cargo fmt --all`, the clippy line.
- Each timing test: run in release with the CPU check. Its numbers go into the plan's Outcome.
- The wave: one `scripts/gate.sh LOG full` after `cargo build -j 8 -p manifold`, run detached at BelowNormal, with CPU
  load and Defender state reported (CLAUDE.md "Gate").
- Manual or probe proof where a unit test can't reach:
  - the `reveal-*` timings;
  - `scripts/jar-probe.sh` after Jar H3, which drives a real WebView2 login and restart.
