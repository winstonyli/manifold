---
status: done
---

# Search and categorization: one omnibar, a local page index, suggested groups — Design

## Question

Cards pile up and are hard to find or organize. The user wants this release to focus on **categorization and
searchability** (conversation 2026-10-23): find any card, page, note or setting from one box; have the app suggest how
the cards group; and lay cards out by topic on request. All of it ships under one tag.

Understanding, separated:

- **Said:** local search engine; layout by semantic similarity or latent topics; one search/address/filter bar shared by
  the Graph and web toolbars; the bar in every view (Graph, Item, Settings); settings are searchable; one tag; the
  canvas-style group rework is parked (`PARKED.md`).
- **Assumed:** everything stays on this device (no hosted API); a few thousand pages at most per profile; the user
  stays in control of structure (suggestions, not automatic moves).

Success: typing in the one bar finds open cards, page text, history and settings with the matching words shown; "Arrange
by topic" puts related cards near each other without moving cards the user pinned; "Suggest groups" proposes labelled
groups the user can accept; no network request is made for any of it.

## Decisions (settled 2026-10-23; D-numbers are the ids used in the conversation)

| Id | Decision |
|---|---|
| D1 | This spec, then a plan, then code; the user reviews both. |
| D2 | New `ViewToHost::PageText { text }`. |
| D3 | The page script truncates to 64 KB. |
| D4 | Extract once per navigation shortly after load, and again just before a tab sleeps. |
| D5 | All four hosts (WebView2, CEF, Servo, WebKitGTK) in this tag. |
| D6 | Manifold asks: new `HostToView::QueryPageText`; `PROTOCOL_VERSION` 5 → 6. |
| D7 | One `OmniBar` component in Graph, Item and Settings; Ctrl+K is the same component in a popup. |
| D8 | Graph view: Enter on a url or query row opens a new tab; on a card row it jumps to the card. |
| D9 | Settings: a row table that both draws the page and is searched. |
| D10 | Hand-rolled in-memory inverted index with BM25, rebuilt from the text log at start (saved as a file only if that is measured slow). |
| D11 | Every jar is indexed (results show the jar), with a Settings "Clear search index" button. Refinement in "Privacy": ephemeral jars are excluded. |
| D12 | TF-IDF plus LSA behind a trait (`Embedder`), so a model can replace it later. |
| D13 | Categorization produces suggested groups the user accepts. |
| D14 | "Arrange by topic" command; weighted edges on the existing solver; pinned cards stay put. A projection seed only if a prototype tangles. |
| D15 | Order inside the tag: extraction, settings rows, omnibar, index, categorization, layout. |

Also decided in this spec (rulings, cheap to reverse): see "Rulings" at the end.

## What exists today (verified 2026-10-23)

- Hosts report `NavigationStateChanged { url, title, ... }` only. `HostToView::QueryPageState` runs
  `PAGE_STATE_SCRIPT` and replies `ViewToHost::PageState`; WebView2, CEF, Servo and WebKitGTK each implement it
  (`manifold-view-*/src/main.rs`; CEF through the renderer process, `render_process_handler.rs`). Sleep sends it
  (`tabs/sleep.rs:56`) and gives up after a few seconds. `PAGE_KIND_SCRIPT` shows the push-on-load pattern (hosts run it at
  load end).
- `PROTOCOL_VERSION` is 5 (`manifold-view-protocol/src/lib.rs:30`); `wire_shape_tests` fails on a shape change without a
  bump.
- Three search surfaces: the Graph toolbar "Filter..." box (`graph_filter`, `graph_ui/toolbar.rs:305`; it dims cards that do
  not match through `graph_filter_dims`, `card.rs:1450`, it does not hide them); `AddressBar` (`tab_view.rs:9`; open-tab
  rows plus DuckDuckGo suggestions from `SuggestionFetcher`); Ctrl+K `QuickSwitcher` (`popups/quick_switcher.rs`).
- `ViewMode` is `Graph | Item | Settings`. `settings_page.rs` (872 lines) draws seven sections imperatively; there is no
  option registry.
- Layout: `graph_layout::step_sized(positions, velocities, edges: &[(NodeId, NodeId)], pinned, sizes, center, dt)`.
  `settle_current_level` (`graph_nav.rs:542`) runs 90 steps once when the visible set changes, and positions are static
  between settles. Physics writes only `App::layout`, never the document position. `pinned` there is the user's pin
  (`doc.mirror.pinned`). `spring_edges()` supplies the edges. Existing test
  `step_sized_matches_reference_bit_for_bit`.
- The profile today holds no page text. No search, vector or linear-algebra crate is in the workspace. The store journal's
  `FORMAT_VERSION` is 3.
- Jars can be `ephemeral` (`jar_defs.rs:20`).

## Architecture

### 1. Page text (`manifold-view-protocol`, four hosts, manifold)

- `PAGE_TEXT_SCRIPT`: returns `null` unless `document.contentType` is `text/html` or `text/plain`; otherwise
  `document.body.innerText` (which already omits script and style) with runs of whitespace collapsed, cut to 64 K UTF-16
  units; a cut that would end on a lone high surrogate drops it. The script lives in the protocol crate beside
  `PAGE_STATE_SCRIPT` and is shared by all hosts.
- `HostToView::QueryPageText` and `ViewToHost::PageText { text: String }`. An engine that cannot answer stays silent
  (as `QueryPageState` does). `PROTOCOL_VERSION` 6 with a doc line "6: `QueryPageText`, `PageText`"; the wire test is
  updated in the same commit.
- Each host answers by running the script and sending the string, the way it already answers `QueryPageState`
  (WebView2 `ExecuteScript`; Servo `evaluate_javascript`; WebKitGTK `run_javascript`; CEF through a new renderer-process
  message mirroring the page-state one).
- Manifold side (`tabs/page_text.rs`): when a tab's `loading` turns false for a new url, start a timer of 2 s
  (`TEXT_DELAY`, one constant); if the tab is still on that url and still alive, send `QueryPageText`. A navigation
  before the timer fires cancels it. Sleep sends `QueryPageText` immediately before `QueryPageState`, and only when the
  stored text was captured for a different url or is older than the last load (cheap check: a per-tab "text captured for
  url + load serial"). A `PageText` that arrives for a tab that no longer exists, or whose url changed since the query, is
  dropped. Sleep does not wait for the text (it waits on `PageState` as now); the text query goes first and hosts answer in
  request order in practice, but if a reply loses the race with the host's exit it is dropped, and the load-time text
  stays. A message is at most about 192 KB (64 K UTF-16 units as UTF-8), far under `MAX_MESSAGE_BYTES` (128 MB).
- Not extracted: `manifold://` pages, non-http(s) urls, tabs in ephemeral jars, and pages whose script returned null.
- Extraction timers and in-flight text queries do **not** hold `WAITIDLE` (a 2 s timer would add 2 s to every wait). A new
  headless verb `WAITINDEX` waits until the text queue and the indexing worker are empty.

### 2. The index (`crates/manifold/src/search/`)

- `text_store`: per-device, outside the synced journal. One file per profile, `%APPDATA%\manifold\search\text.log`
  (through `profile::dir()`, so tests never touch it), holding `(node id, url, title, text, captured_at)` records; written
  atomically (`store::atomic`). Notes, group names and file names are not stored there: they are read from the live
  document when (re)indexing.
- `index`: inverted index `term -> postings [(doc, tf)]` plus per-doc length, BM25 (`k1 = 1.2`, `b = 0.75`, constants).
  Tokenizer: lowercase; split on non-alphanumeric; CJK characters as single-character tokens; no stemming. Prefix match for
  the last typed word by binary search over the sorted term list. Snippet: the first window of the stored text containing a
  query term.
- Documents: one per card. A tab's document is title + url + extracted text; a note's is its text; a group's is its name;
  a download's is its file name; other kinds contribute their name only. History entries (`history.json`) are indexed by
  title + url, with no text.
- Worker: a thread owns writes (add, remove, rebuild, save); the UI thread reads under a short lock on an `Arc<Mutex>`
  holding the index. A query is synchronous (tens of thousands of postings; expected well under a frame, to be
  measured). The index is rebuilt on the worker from the text log at every start (the plan, Task 7, measures a 50 MB
  corpus); a saved `search/index.bin` is added only if that rebuild takes over 1 s, which removes a file format and its
  invalidation rules unless needed. The text log is an append-only `search/text.log` (one JSON line per record, last
  wins, compacted when wasteful). The index is derived state and is never synced or shipped.
- Updates: text arrival, note edit (debounced 1 s), rename, jar becoming ephemeral (removes that jar's records).
  A card that is closed stays in the document (Recently Closed can put it back), so its text record is kept; records
  are pruned when the document no longer holds the node at all (checked at startup and on idle), and closed cards are
  not offered as "Open cards" rows. "Clear search index" in Settings deletes both files and the in-memory state.
- Matching: a card matches a query when every query word is a prefix of one of its terms (the last word) or equals one
  (the others), or the query is a substring of its title or url. `matches(&App, &str) -> HashSet<CardId>` is uncapped
  and cached by (text, index generation); the list takes its top rows by score, and the Graph dimming uses the whole set.

### 3. The omnibar (`omnibar.rs`, replaces `AddressBar`'s dropdown, the Graph toolbar filter box and the Ctrl+K popup body)

- One `OmniBar` state struct: text, focus, highlighted row, and a `Mode`: `Query` (Graph, Settings, Ctrl+K: starts empty)
  or `Address` (Item view: starts as the tab's url, select-all on focus, as the address bar does now). One widget draws
  the field; one `rows(&App, &str) -> Vec<Row>` function produces the result list; one `activate(row)` handles Enter and
  click. The three old call sites become thin hosts of it.
- `Row` kinds, in fixed section order, each section capped (5 rows) and ranked within itself: **Open cards** (matched by
  title/url substring first, then BM25 text hits with a snippet and the jar name), **History**, **Settings** (section 4),
  **Typed** (the typed url or a search with the default engine), **Search suggestions** (DuckDuckGo, as now, from
  `SuggestionFetcher`, unchanged).
- Behaviour by view:
  - Graph: the box sits where the filter box is. While text is present, cards that match are shown and the rest dimmed,
    exactly as the filter does now (`graph_filter_dims` is replaced by "is this card in the result set", so dimming and the
    list agree). Enter: a card row jumps to the card (camera eases to it, opening its group level if needed); a typed or
    suggestion row opens a new tab (D8).
  - Item: the address bar. Enter on the typed row navigates the current tab, as today; a card row switches to it.
  - Settings: the same box in the page header; Enter on a settings row scrolls to and highlights it; other rows behave as in
    Graph.
  - Ctrl+K: same component in the popup placement, `Mode::Query`, Esc closes.
- Default highlight: `Query` mode preselects the first row (best card match, else the typed row), so Enter never
  opens a web search the user did not ask for while filtering; `Address` mode preselects nothing and Enter navigates the
  typed text, as today.
- Keyboard: Up/Down move the highlight, Enter activates, Esc clears then closes (the existing rules in `AddressBar` and
  the quick switcher are the reference; the plan lists each and its gate scenario).
- Headless verbs: `ADDRESSBAR`, `GRAPHFILTERTEXT` and the quick-switcher verbs keep their meaning, re-pointed at the one
  struct, so existing gate scenarios keep running; any rename is made once with the scenarios.

### 4. Settings rows (`settings_page.rs`)

- Replace the seven imperative section functions' bodies with a table of rows: `SettingRow { id, section, title,
  keywords, draw: fn(&mut App, &mut egui::Ui, &mut SettingsRequests) }`. The page iterates sections and calls each row's
  `draw`. A composite control (the lists, sessions, bookmarks, view-kinds editors) is one row with its own keywords.
- The omnibar's Settings rows are the table filtered on title + keywords; activating one sets a one-frame
  `scroll_to`/highlight request that the page honours (opening the page first if needed).
- A row's draw function is the old code moved, not rewritten; a unit test asserts every row id is unique and every row has
  a title.

### 5. Representation and categorization (`search/topics.rs`)

- `trait Embedder { fn embed(&self, docs: &Corpus) -> Vec<(NodeId, Vec<f32>)>; fn terms_of(&self, centroid: &[f32]) ->
  Vec<String>; }`. The one implementation, `LsaEmbedder`: TF-IDF over the index's matrix, vocabulary pruned to terms in at
  least 2 documents and the top 20 000 by document frequency; truncated SVD to `k = min(64, docs - 1)` by randomized
  subspace iteration with a fixed seed (documents processed in `NodeId` order, so the result is deterministic); vectors
  L2-normalized. Computed on the worker, cached until the corpus changes by more than 5 % of documents or the user asks.
  No linear-algebra crate: sparse matrix products and a small dense QR/eigen step are written for this (about 200 lines,
  covered by tests on a known matrix).
- Cards without text (images, empty notes) have no vector and are never clustered or pulled.
- **Suggested groups** (D13): a command "Suggest groups" (toolbar, and a row in the omnibar's commands) clusters the cards
  at the current level (group cards excluded) by average-linkage agglomerative clustering on cosine distance, cut at a
  fixed similarity threshold (`CLUSTER_MIN_SIMILARITY`), keeping clusters of at least 3 cards, at most 8 clusters. A label
  is the top 3 terms of the cluster's TF-IDF mass in the original term space. Each suggestion is drawn as a colored ring on
  each member card (a hull would overlap its neighbours when topics are interleaved) plus one label chip at the members'
  centroid with **Accept** and **Dismiss**; "Arrange by topic" first pulls the members together, but is not required. Nothing in the document changes until
  Accept, which calls one function, `accept_suggestion(cards, label)`. That function is the `GroupWith` arm of
  `apply_drop` (`graph_nav.rs`: `alloc_node_id`, `insert_group_at`, `edit_group` per member, `recheck_edge_scope`)
  extracted into `group_cards(members, at)` and used by both, followed by the same `Revert { moved, created }` undo
  record, so Accept is one undo step. Suggestions are session-local, not saved, and cleared when the level's card set
  changes.
- Accept stays one function so the parked region-style groups can reuse the suggestions later.

### 6. Layout by topic (`graph_layout.rs`, `graph_nav.rs`)

- `step_sized`'s `edges` becomes `&[Edge]` with `Edge { from, to, weight: f32 }`; the spring force is
  `SPRING_STRENGTH * weight * (dist - REST_LENGTH)`. `spring_edges()` (`graph_nav.rs:610`, today `(opener, opened)`
  pairs from edge-table rows with `affects_layout`) returns `Edge`s with weight 1.0, which multiplies exactly, so
  `step_sized_matches_reference_bit_for_bit` keeps passing unchanged apart from the type.
- "Arrange by topic" (toolbar and omnibar command): for each text-bearing card at the level, similarity edges to its top 3
  neighbours with cosine ≥ `SIMILARITY_EDGE_MIN`, de-duplicated, `weight = similarity`; one settle with structural plus
  similarity edges over `ARRANGE_ITERATIONS` (more than the 90 of a normal settle, one constant), user-pinned cards fixed
  (`pinned` is `doc.mirror.pinned`, as in `settle_current_level`). The similarity edges exist only for that call.
- Persistence and undo: a settle writes only `App::layout`, which is personal and recomputed; an arrangement the user
  asked for must survive restart and sync, so the command writes each moved card's position with `edit_pos` (what
  `release_drag` does) and records one `UndoKind::Revert` whose `Moved` entries change only `pos`, which `replay_revert`
  already restores (`undo.rs:142`). Plain drags stay off the undo stack by the 2026-10-19 decision; this command is a
  bulk edit, which is why it records one.
- Open question OQ1 (answered by the prototype): `settle_current_level` re-runs, without similarity edges, whenever the
  level's card set changes, and starts from the current positions. If that visibly relaxes an arrangement, the choices
  are (a) accept it, (b) keep the similarity edges in every settle of a level the user has arranged (a per-level flag,
  which makes the layout partly always-on), (c) pin arranged cards. The default is (a); the prototype measures how far
  cards move in one ordinary settle after an arrangement. *Decided 2026-10-23:* (a), accept the drift, with the LSA seed;
(b) is parked (PARKED.md "Keep similarity edges..."), since a bigger layout overhaul is planned.
- A prototype on a synthetic 200-card corpus (two to five topics plus noise) runs first. If the result is tangled, seed
  positions from the first two LSA components before settling (additive); if not, no seed.
- Cost: pairwise repulsion is O(n²) per step; at 300 cards and 300 iterations that is about 27 M pair evaluations
  (to be measured; the cap on cards arranged in one call, `ARRANGE_MAX_CARDS`, is set from the measurement).

## Privacy

- Nothing leaves the device: no embedding service, no telemetry, no network request for indexing or search. The existing
  DuckDuckGo suggestions already exist and are unchanged.
- Pages in **ephemeral jars** are never extracted or indexed, and a jar turning ephemeral removes its records (the jar's
  promise is that it leaves no trace). This refines D11 and is the one ruling the user should confirm.
- Page text is the user's browsing content stored in plain files under `%APPDATA%\manifold\search\`: the README and the
  Settings page say so, beside "Clear search index".
- Page text and the index are not part of the synced document, a shipped release, or an export.

## Open for the user

- R1 (ephemeral jars excluded, which narrows D11) needs a yes.
- OQ1 (does a later settle undo an arrangement) is decided: accept the drift (a), park (b).

## Docs this changes

README (a "Search" section: what is stored where, how to clear it; the protocol-version line), `CHANGELOG.md` at the bump
(the user bumps), `PARKED.md` (region-style groups already added; the PDF-text and topic-chip leftovers if cut), and the
doc comments on `PROTOCOL_VERSION`, `AddressBar` (removed) and `graph_filter` (removed).

## Not in this tag

PDF text through pdfium (a stretch task after the rest, only if time allows and called out in the plan), image text,
stemming or fuzzy matching, a real embedding model (the `Embedder` trait is the seam), topic chips on cards, automatic
grouping, always-on topical layout, and region-style groups (`PARKED.md`).

## Testing

- Protocol: wire test updated; `PAGE_TEXT_SCRIPT` unit-tested in the host crates' script tests where one exists, and a gate
  scenario per host that can run on this machine (WebView2, Servo and CEF as far as they build; WebKit through WSL) loading
  a local page and checking the stored text.
- Index: tokenizer, BM25 ranking on a fixed corpus, prefix, add/remove/rebuild equals fresh build, corrupt file rebuilds,
  ephemeral exclusion.
- Omnibar: row order and caps, per-view activation, dimming agrees with the result set, keyboard rules; the old quick
  switcher and address-bar unit tests move to the new struct.
- Settings: unique ids, search finds each section, scroll request honoured.
- Topics: SVD on a known matrix; determinism (same corpus, same suggestions); labels on a two-topic corpus.
- Layout: reference test unchanged; a two-topic corpus ends with each topic's mean intra-distance below the inter-distance;
  pinned cards do not move.
- Gate: smoke gains a search scenario (open pages, `WAITINDEX`, query, jump) and a suggest-and-accept scenario; the full
  gate runs on the final commit, as every tag.

## Rulings

- R1. Ephemeral jars are excluded from the index (Privacy).
- R2. The 2 s extraction delay, 64 KB cap, row cap of 5, `k = 64`, and the similarity constants are named constants set by
  the plan's measurements, not fixed here.
- R3. `WAITIDLE` does not wait for extraction; `WAITINDEX` does.

### Prototype result (2026-10-23, `graph_layout/prototype.rs`, release)

Synthetic cards: in-topic cosine 0.6 ± 0.15, cross-topic 0.1 ± 0.1, top-3 similarity edges at or above 0.30, 300 iterations of
`step_sized` from random positions on a 4000 x 4000 field; then 90 ordinary settle steps with no similarity edges.

| Cards | Seed | In/cross distance ratio | Mean drift in the settle | 300 iterations |
| --- | --- | --- | --- | --- |
| 120 | none | 0.695 | 97 (30 % of REST_LENGTH) | 23 ms |
| 200 | none | 0.694 | 110 (34 %) | 56 ms |
| 200 | corner seed | 0.149 | 143 (45 %) | 50 ms |
| 300 | none | 0.681 | 134 (42 %) | 108 ms |

Without a seed the ratio sits between the plan's "usable" (< 0.6) and "tangled" (> 0.8) lines; a corner seed (a stand-in
for the LSA seed) gives 0.15. OQ1: a later ordinary settle moves cards 30-45 % of REST_LENGTH on average, over the
plan's 25 % line, so the plan stops here for the user's choice.
