---
status: draft
---

# Search and categorization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (inline; tasks share interfaces and files) to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One omnibar over open cards, page text, history and settings; a local page-text index; suggested groups; "Arrange by topic" — all under one tag.

**Architecture:** Hosts return page text on `QueryPageText` (protocol 6). A per-device text log feeds an in-memory BM25 index built on a worker thread. One `OmniBar` component (rows + activate) replaces the address-bar dropdown, the Graph filter box and the Ctrl+K popup body. An `Embedder` trait (TF-IDF + LSA) feeds clustering (suggested groups) and similarity edges for `step_sized` (weighted edges).

**Tech Stack:** Rust, egui 0.36, existing crates only (`serde_json`, `getrandom`; no new dependencies).

**Spec:** `docs/superpowers/specs/2026-10-23-search-and-categorization-design.md` (approved 2026-10-23).

## Global Constraints

- No new dependencies; no network use by indexing or search; page text and the index never enter the journal, an export or a release.
- No test reads or writes the real `%APPDATA%\manifold`: paths start at `profile::dir()` (`None` under `cfg(test)`); tests that need files use `store::testutil::temp_dir` and pass the path in.
- `cargo fmt --all` before each commit; `cargo clippy -j 8 -p manifold -p manifold-view-protocol -p manifold-pages -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings` passes; the pre-commit hook is never bypassed; `#[allow]` only with `reason`.
- Commit trailer: `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`. Never tag, bump, push or publish; the user does.
- Cargo runs use `-j 8` (leave cores free); one cargo per target dir; check `%LOCALAPPDATA%\cpu-leases` before the gate; no lease or load logic in repo code.
- Python, if any, runs as `python -P f.py < /dev/null`.
- No migration or compatibility shims. Delete what a new mechanism supersedes (the address-bar dropdown, `graph_filter`, the quick switcher's matching) with its tests and docs.
- Named constants (`TEXT_DELAY`, `TEXT_CAP_UNITS`, `ROW_CAP`, `LSA_K`, `CLUSTER_MIN_SIMILARITY`, `SIMILARITY_EDGE_MIN`, `ARRANGE_ITERATIONS`, `ARRANGE_MAX_CARDS`) hold every tunable; values below are starting points set by the measuring steps.
- `PROTOCOL_VERSION` 5 → 6 once, in Task 1.

## Review Focus

1. A page whose text is huge, empty, binary (`contentType` not html/plain), or contains lone surrogates: the script returns null or clean text, the host never crashes, the index never panics on odd Unicode.
2. A tab navigating or sleeping while a text query is in flight: the reply for a stale url or a gone tab is dropped, never attached to the wrong card.
3. Ephemeral jars: no text stored, none indexed, and a jar turning ephemeral purges its records.
4. Omnibar Enter semantics: filtering in Graph never opens a web search unasked; Item-view address editing behaves exactly as before; Esc rules unchanged.
5. Arrange / Accept undo: one step each; undo after a card was moved again skips that card without corrupting others; pinned cards never move.

---

## File map

- Create `crates/manifold/src/search.rs` (module root) and `search/{tokenize,text_store,index,service,lsa,cluster}.rs`.
- Create `crates/manifold/src/omnibar.rs` (+ `omnibar/rows.rs`), `crates/manifold/src/tabs/page_text.rs`.
- Modify `crates/manifold-view-protocol/src/{messages,lib}.rs`; the four hosts' `main.rs` (CEF also `render_process_handler.rs`, `client.rs`).
- Modify `settings_page.rs` (row table), `tab_view.rs` (AddressBar → OmniBar), `popups/quick_switcher.rs`, `graph_ui/toolbar.rs`, `card.rs` (`graph_filter_dims`), `graph_layout.rs`, `graph_nav.rs`, `debug_control.rs`, `crates/gate/src/*.rs`, `README.md`.

---

### Task 1: Protocol 6 — `PAGE_TEXT_SCRIPT`, `QueryPageText`, `PageText`

**Files:** Modify `crates/manifold-view-protocol/src/messages.rs`, `lib.rs`.

**Interfaces:** Produces `pub const PAGE_TEXT_SCRIPT: &str`, `pub const TEXT_CAP_UNITS: usize = 65_536`, `HostToView::QueryPageText`, `ViewToHost::PageText { text: String }`, `pub fn clean_page_text(raw: &str) -> String` (the Rust-side guard every host applies to the reply).

- [ ] **Step 1: Write failing tests** (in `messages.rs` `mod tests`; create the module if the file has none besides `wire_shape_tests`)

```rust
#[test]
fn page_text_script_is_one_expression_returning_a_string_or_null() {
    assert!(PAGE_TEXT_SCRIPT.starts_with("(function(){"));
    assert!(PAGE_TEXT_SCRIPT.contains("text/html") && PAGE_TEXT_SCRIPT.contains("text/plain"));
    assert!(PAGE_TEXT_SCRIPT.contains("65536"));
}

#[test]
fn clean_page_text_collapses_whitespace_and_caps() {
    assert_eq!(clean_page_text("a \n\t b  c"), "a b c");
    let long = "x".repeat(TEXT_CAP_UNITS * 2);
    assert_eq!(clean_page_text(&long).chars().count(), TEXT_CAP_UNITS);
    assert_eq!(clean_page_text("\u{FFFD}ok"), "ok", "a replacement char from a cut surrogate is dropped at the edges");
}
```

- [ ] **Step 2:** `cargo test -j 8 -p manifold-view-protocol page_text` — Expected: FAIL (items undefined).
- [ ] **Step 3: Implement.** Beside `PAGE_STATE_SCRIPT`:

```rust
/// Largest page text kept, in UTF-16 units (the script's `slice` unit) and, after `clean_page_text`, in chars.
pub const TEXT_CAP_UNITS: usize = 65_536;

/// Returns the page's visible text, or null for a page that is not html or plain text. `innerText` omits script and
/// style; whitespace runs are collapsed; the cut is at `TEXT_CAP_UNITS`.
pub const PAGE_TEXT_SCRIPT: &str = "(function(){var t=document.contentType;if(t!=='text/html'&&t!=='text/plain'){return null;}var b=document.body;if(!b){return null;}return (b.innerText||'').replace(/\\s+/g,' ').trim().slice(0,65536);})();";

/// A host's reply, guarded: whitespace collapsed again, a replacement char left by a cut surrogate trimmed from the
/// ends, at most `TEXT_CAP_UNITS` chars. Every host runs this before sending `PageText`.
pub fn clean_page_text(raw: &str) -> String {
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim_matches('\u{FFFD}');
    trimmed.chars().take(TEXT_CAP_UNITS).collect()
}
```

Add to `HostToView` after `QueryPageState` (doc: "Asks for the page's visible text (`PAGE_TEXT_SCRIPT`); the engine answers `ViewToHost::PageText`, or stays silent when it cannot or the page has none.") `QueryPageText,` and to `ViewToHost` after `PageState(PageState),` `PageText { text: String },`. Update `lib.rs`: re-export `clean_page_text, PAGE_TEXT_SCRIPT, TEXT_CAP_UNITS`; `PROTOCOL_VERSION = 6` with doc line `/// 6: \`HostToView::QueryPageText\`, \`ViewToHost::PageText\`.`. In `wire_shape_tests`: add the two variants to `host_slot`/`view_slot` and `host_samples`/`view_samples`, bump `HOST_TO_VIEW_VARIANTS` to 22, `VIEW_TO_HOST_VARIANTS` to 17, `RECORDED_FOR_VERSION` to 6.
- [ ] **Step 4:** `cargo test -j 8 -p manifold-view-protocol` — Expected: the two new tests PASS; `wire_shape_matches_the_recorded_fingerprint` FAILS printing the new fingerprint; paste it into `RECORDED_FINGERPRINT`, rerun — PASS.
- [ ] **Step 5: No commit yet.** The pre-commit hook clippy-checks `manifold` and the WebView2 host, which stop compiling on the new variants until Tasks 2 and 3 add their arms. Tasks 1–3 are one commit group: leave the work in the tree and commit once at the end of Task 3 (Step 5 there). Ledger this.

---

### Task 2: Manifold side — query on load and before sleep, text log

**Files:** Create `search.rs`, `search/text_store.rs`, `tabs/page_text.rs`. Modify `main.rs` (`mod search;`, fields), `tabs/sleep.rs:56`, `event_loop/view_message.rs` (handle `PageText`; NavigationStateChanged load end), `tabs.rs`.

**Interfaces:** Produces
```rust
// search/text_store.rs
pub(crate) struct TextRecord { pub id: NodeId, pub url: String, pub title: String, pub text: String, pub at: u64 }
pub(crate) struct TextStore { /* path: Option<PathBuf>, live: HashMap<NodeId, TextRecord>, file_lines: usize */ }
impl TextStore {
    pub(crate) fn open(dir: Option<&Path>) -> Self;               // loads text.log; None dir = memory only
    pub(crate) fn put(&mut self, rec: TextRecord);                // appends a line, replaces live
    pub(crate) fn remove(&mut self, id: NodeId);                  // appends a tombstone line
    pub(crate) fn get(&self, id: NodeId) -> Option<&TextRecord>;
    pub(crate) fn ids(&self) -> impl Iterator<Item = NodeId> + '_;
    pub(crate) fn clear(&mut self);                               // deletes the file
    pub(crate) fn compact_if_wasteful(&mut self);                 // rewrites live records when file_lines > 2 * live + 100
}
// tabs/page_text.rs
pub(crate) const TEXT_DELAY: Duration = Duration::from_secs(2);
impl App {
    pub(crate) fn note_load_end(&mut self, id: NodeId, url: &str);      // arms a timer for (id, url)
    pub(crate) fn poll_text_timers(&mut self, now: Instant);            // sends QueryPageText for due timers
    pub(crate) fn accept_page_text(&mut self, id: NodeId, text: String);// stores if url unchanged and jar not ephemeral
    pub(crate) fn text_wanted(&self, id: NodeId) -> bool;               // http(s), not manifold://, jar not ephemeral
}
```
The log line format is one JSON object per line: `{"id":"<NodeId Display>","url":..,"title":..,"text":..,"at":..}` and a tombstone `{"id":..,"gone":true}`. `NodeId` already has `Display`; also parse it (find the existing `FromStr`/`parse` used by the journal; reuse it — grep `impl FromStr for NodeId` or `fn parse` in `store/model.rs` first; if none exists, store `actor` and `counter` as two fields instead).

- [ ] **Step 1: Failing tests for `TextStore`** (`search/text_store.rs` `mod tests`, using `crate::store::testutil::temp_dir`):

```rust
#[test] fn put_then_reopen_returns_the_record() { /* put, drop, open(Some(dir)) -> get(id).text == "hello" */ }
#[test] fn remove_is_remembered_after_reopen() { /* put, remove, reopen -> get is None */ }
#[test] fn a_later_put_wins() { /* two puts same id, reopen -> second text */ }
#[test] fn a_torn_last_line_is_ignored() { /* append `{"id":` without newline, reopen -> earlier records intact */ }
#[test] fn compaction_keeps_live_records_and_shrinks_the_file() { /* 300 puts of one id; compact_if_wasteful; file has 1 line; reopen ok */ }
#[test] fn memory_only_store_never_writes() { /* open(None) put/get work, no panic */ }
```
- [ ] **Step 2:** `cargo test -j 8 -p manifold text_store` — Expected: FAIL (module missing).
- [ ] **Step 3: Implement `TextStore`** (append via `OpenOptions::append`; `load` reads lines with `BufRead::lines`, `serde_json::from_str::<Value>` per line, skipping unparsable lines; compaction writes `text.log.tmp` then renames via `store::atomic::replace_private`). Add `mod search;` to `main.rs`; `search.rs` holds `pub(crate) mod text_store;` and later modules.
- [ ] **Step 4:** the six tests PASS.
- [ ] **Step 5: Failing tests for the flow** (`tabs/tests.rs` style, use `sleeping_tab` + a fake `Tab` sender if the existing tests have one — read `tabs/tests.rs` first for the pattern; if a send cannot be observed, record the query in a `#[cfg(test)] text_queries: Vec<NodeId>` field):

Each test builds `App::default()` with one tab node (`sleeping_tab`/`insert_placeholder_node`), calls the method named, and asserts with an explicit `now: Instant` argument (no real clock, no sleeping):

```rust
// note_load_end(id, "https://a.test/") then poll_text_timers(t0 + TEXT_DELAY - 1ms): text_queries empty;
// poll_text_timers(t0 + TEXT_DELAY): text_queries == [id], and a second poll queries nothing more.
#[test] fn load_end_arms_a_timer_and_it_fires_after_text_delay() {}
// note_load_end, then the tab's url changes to another url, poll after TEXT_DELAY: no query.
#[test] fn a_navigation_before_the_timer_cancels_it() {}
// query sent for url A, tab url becomes B, accept_page_text(id, "x"): texts.get(id) is None.
#[test] fn page_text_for_a_changed_url_is_dropped() {}
// text_wanted is false for "manifold://start", "file:///x", "about:blank", true for "https://a.test/".
#[test] fn manifold_pages_and_non_http_urls_are_not_wanted() {}
// the tab's jar made ephemeral: text_wanted false and accept_page_text stores nothing.
#[test] fn an_ephemeral_jar_tab_is_not_wanted() {}
// stored record url differs from tab url: request_sleep records a text query before QueryPageState; same url: none.
#[test] fn sleep_asks_for_text_first_when_the_stored_text_is_for_another_url() {}
```
- [ ] **Step 6: Implement** `tabs/page_text.rs`: `App.text_timers: HashMap<NodeId, (String, Instant)>`; `note_load_end` is called from the `NavigationStateChanged` arm in `view_message.rs:39` when `loading` flips true→false (compare with the node's previous `loading`); the timer key is the url at load end. `poll_text_timers` is called from the same per-frame place `sweep_idle_tabs` runs (grep its call site) and sends `HostToView::QueryPageText` when `now >= due`, the tab's current url equals the stored url and `text_wanted`. `accept_page_text` is the new `ViewToHost::PageText { text }` arm: it finds the tab node by `tab_id`, checks the url against `last_query_url` kept in the same map entry, builds `TextRecord`, `self.search.texts.put(..)`. In `tabs/sleep.rs` `request_sleep`, before `QueryPageState`, send `QueryPageText` when `text_wanted(id)` and the stored record's url differs from the tab's url. Ephemeral check: use the jar lookup the tab already has (grep `ephemeral` in `jar_defs.rs:20` and how a tab maps to its jar).
- [ ] **Step 7:** `cargo test -j 8 -p manifold page_text text_store` — PASS (the workspace may not compile until Task 3 adds host arms; if `cargo test -p manifold` fails only in host crates, build with `-p manifold` alone: hosts are separate crates).
- [ ] **Step 8:** no commit (see Task 1 Step 5); `WAITINDEX` comes in Task 7.

---

### Task 3: The four hosts answer `QueryPageText`

**Files:** `manifold-view-webview2/src/main.rs:376`, `manifold-view-servo/src/main.rs:291`, `manifold-view-webkitgtk/src/main.rs:353`, `manifold-view-cef/src/{main,render_process_handler,client}.rs:309`.

Each host copies its `QueryPageState` arm: run `manifold_view_protocol::PAGE_TEXT_SCRIPT`, decode the result string (WebView2: `serde_json::from_str::<Option<String>>(&result.to_string())` as `nav_state.rs:189`; Servo/WebKit: the string the callback gives), `Some(text)` → `clean_page_text(&text)`, skip when empty, send `ViewToHost::PageText { text }` through the same channel the state reply uses (a `pending_page_text: Rc<RefCell<Option<String>>>` drained next to `pending_page_state`). A null or failed result sends nothing. CEF: the renderer process evals the script and replies with a process message named `page-text`, handled in `client.rs` into a `SharedPageText` drained by `main.rs`, mirroring `SharedPageState` exactly.

- [ ] **Step 1: Write one failing unit test per host that can run on this machine** — the pure part only: a function `fn reply_to_text(raw: Option<&str>) -> Option<String>` in `manifold-view-protocol` (`pub fn page_text_reply(raw: Option<&str>) -> Option<String>`: `None`/empty after `clean_page_text` → `None`), tested in the protocol crate:
```rust
#[test] fn a_null_or_blank_reply_sends_nothing() { assert_eq!(page_text_reply(None), None); assert_eq!(page_text_reply(Some("  \n ")), None); }
#[test] fn a_reply_is_cleaned() { assert_eq!(page_text_reply(Some(" a  b ")).as_deref(), Some("a b")); }
```
  Run: `cargo test -j 8 -p manifold-view-protocol page_text_reply` — FAIL, then implement and PASS; the hosts call `page_text_reply`.
- [ ] **Step 2:** implement the WebView2 arm; `cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate` — PASS.
- [ ] **Step 3:** implement Servo, WebKitGTK and CEF arms. They cannot all be built here (Servo ~33 min, CEF needs the download, WebKit builds on Linux only): `cargo check` them only where the toolchain exists; run the WebKit one under WSL if present (README "Building"); otherwise commit them compile-unchecked and ledger exactly which were not built. Add each to `PARKED.md` "live-only behaviour never checked" if unbuilt.
- [ ] **Step 4: Gate scenario** `crates/gate/src/page_text.rs` (copy the shape of `sleep.rs`): serve a local page with known text over the gate's existing local server (grep how `sleep.rs` serves pages), `NEWTAB` it, wait for load, wait `TEXT_DELAY`+slack on an observable condition (a new debug verb `TEXTOF <tab>` returning the stored text or `none`; add it to `VERBS` in `debug_control.rs`), assert the text contains a known sentence and not script content. Run it for WebView2: `target/debug/gate.exe page_text` after the build — PASS.
- [ ] **Step 5:** `cargo fmt --all`, the Global Constraints clippy command clean, `cargo test -j 8 -p manifold-view-protocol -p manifold` PASS; commit Tasks 1–3 together: `Page text: protocol 6, hosts answer QueryPageText, manifold asks after load and before sleep`.

---

### Task 4: Settings page as a row table

**Files:** Modify `settings_page.rs`. Create `settings_rows.rs` (the table and `SettingRow`).

**Interfaces:** Produces
```rust
pub(crate) struct SettingRow {
    pub(crate) id: &'static str,
    pub(crate) section: &'static str,
    pub(crate) title: &'static str,
    pub(crate) keywords: &'static str,
    pub(crate) draw: fn(&mut App, &mut egui::Ui, &mut SettingsRequests),
}
pub(crate) fn rows() -> &'static [SettingRow];
impl App { pub(crate) fn settings_matching(&self, query: &str) -> Vec<&'static SettingRow>; }
// App field: pub(crate) settings_focus: Option<&'static str>  (row to scroll to and highlight, one frame)
```
- [ ] **Step 1: Failing tests** in `settings_rows.rs`:
```rust
#[test] fn row_ids_are_unique_and_titled() { /* collect ids into a HashSet, assert len == rows().len(), every title non-empty */ }
#[test] fn every_section_title_has_a_row_and_search_finds_it() { /* for each row, settings_matching(row.title.to_lowercase()) contains row.id */ }
#[test] fn keywords_match_but_unrelated_text_does_not() { /* "dark" finds the theme row if its keywords hold "dark"; "zzzz" finds nothing */ }
```
- [ ] **Step 2:** `cargo test -j 8 -p manifold settings_rows` — FAIL.
- [ ] **Step 3: Implement.** Convert each of the seven `settings_*_section` functions into one or more rows: the section function's body becomes the row's `draw` fn (move the code; do not rewrite it). A section that is one cohesive control group (lists, sessions, bookmarks, view-kinds editors) is a single row with a title naming it and keywords listing its controls' words. `build_settings_page` iterates `rows()` grouped by consecutive equal `section` (call `section_start` at each change) and calls `draw`; when `self.settings_focus == Some(row.id)` it calls `ui.scroll_to_rect` on the row's rect and paints a one-frame highlight (`ui.painter().rect_stroke`), then clears the field. `settings_matching` lowercases and requires every query word to be a substring of `title + " " + keywords`.
- [ ] **Step 4:** `cargo test -j 8 -p manifold settings` — the new tests and every existing settings test PASS; `cargo build` + `target/debug/gate.exe ui LOG` is run at Task 12, but run `target/debug/gate.exe tier` now and note the advised tier.
- [ ] **Step 5:** fmt, clippy, commit `Settings page draws from a row table`.

---

### Task 5: Omnibar over today's sources (substring matching)

**Files:** Create `omnibar.rs`, `omnibar/rows.rs`. Modify `tab_view.rs` (AddressBar), `graph_ui/toolbar.rs`, `popups/quick_switcher.rs`, `card.rs`, `debug_control.rs`, `main.rs`.

**Interfaces:** Produces
```rust
pub(crate) enum Mode { Query, Address }
pub(crate) enum Row {
    Card { id: NodeId, label: String, url: String, jar: Option<String>, snippet: Option<String> },
    History { url: String, title: String },
    Setting { id: &'static str, title: &'static str, section: &'static str },
    Typed { text: String },                 // the typed url, or a search with the default engine
    Suggestion { text: String },           // DuckDuckGo
}
pub(crate) struct OmniBar { pub text: String, pub focused: bool, pub highlighted: Option<usize>, pub mode: Mode, /* fetcher etc. moved from AddressBar */ }
impl App {
    pub(crate) fn omni_matches(&mut self, text: &str) -> &HashSet<CardId>;           // uncapped, cached by text+generation
    pub(crate) fn omni_rows(&mut self, bar_text: &str, mode: Mode) -> Vec<Row>;       // sections in fixed order, ROW_CAP each
    pub(crate) fn omni_activate(&mut self, row: &Row, view: ViewMode);                // D8 semantics
    pub(crate) fn omni_ui(&mut self, ui: &mut egui::Ui, host: Host) -> egui::Response; // the field + dropdown; Host = Graph|Item|Settings|Popup
}
```
**This is a refactor of working, tested code. The tests below are written first against the new API; the old tests of `AddressBar`, `graph_filter` and `QuickSwitcher` move onto it (same assertions, new calls) and the old types are deleted.**

- [ ] **Step 1: Inventory.** List every test touching the three old surfaces: `rg -n "address_bar|AddressBar|graph_filter|quick_switcher|QuickSwitcher" crates/manifold/src crates/gate/src > scratch`; record the list in the ledger. Each is either moved (assertion kept) or deleted with a stated reason.
- [ ] **Step 2: Failing tests for rows** (`omnibar/rows.rs`):
Tests over the pure `Corpus` fake (cards `(id, label, url, closed)`, history, the real `settings_rows::rows()`):

```rust
// corpus with a card "Rust book", a history entry "Rust blog", the settings row "Search engine"; query "e":
// the section order of the returned rows is Card, History, Setting, Typed (Suggestion rows come after Typed).
#[test] fn card_rows_come_before_history_before_settings_before_typed() {}
// ROW_CAP + 3 cards all containing "a": exactly ROW_CAP Card rows, and omni_matches still holds all ROW_CAP + 3.
#[test] fn each_section_is_capped_at_row_cap() {}
// omni_rows(.., Mode::Query).highlighted default == Some(0); Mode::Address == None.
#[test] fn query_mode_preselects_the_first_row_and_address_mode_preselects_none() {}
// card label "Rust Book", url "https://x.test/Guide": queries "rust", "BOOK", "guide" each match; "zebra" does not.
#[test] fn substring_of_title_or_url_matches_a_card_case_insensitively() {}
// a card with closed == true never appears as a Card row or in the match set.
#[test] fn closed_cards_are_not_offered() {}
// query "engine" yields a Setting row (the search-engine row); query "rust" yields none.
#[test] fn settings_rows_appear_for_a_settings_word_and_not_for_a_page_word() {}
```
- [ ] **Step 3: Failing tests for activation:**
Use the `Rig` pattern of `deselect.rs` (headless `build_graph_ui` frames) where a frame is needed, `App::default()` plus `omni_activate` otherwise:

```rust
// two cards far apart; omni_activate(Row::Card{far}, ViewMode::Graph): app.camera.reveal == Some(far) and the camera is moving.
#[test] fn enter_on_a_card_row_in_graph_jumps_to_the_card_and_sets_reveal() {}
// omni_activate(Row::Typed{"rust lang"}, Graph): node count +1 and the new tab's url is the default engine's search url.
#[test] fn enter_on_a_typed_row_in_graph_opens_a_new_tab() {}
// Item view, focused tab: omni_activate(Row::Typed{"https://b.test/"}, Item): node count unchanged, the tab navigated.
#[test] fn enter_on_a_typed_row_in_item_navigates_the_current_tab() {}
// omni_activate(Row::Setting{id}, Graph): view_mode == Settings and settings_focus == Some(id).
#[test] fn enter_on_a_settings_row_sets_settings_focus_and_opens_settings() {}
// frame with text "abc" + Esc: text empty and popup still open; second Esc: closed (copy the old quick-switcher test).
#[test] fn escape_clears_the_text_then_closes() {}
// three cards, text matching two: after a frame the dimmed set == all cards minus omni_matches.
#[test] fn dimming_set_equals_the_uncapped_match_set() {}
```
- [ ] **Step 4:** `cargo test -j 8 -p manifold omnibar` — FAIL.
- [ ] **Step 5: Implement** in this order, running the tests after each: (a) `rows.rs` pure functions over a small `Corpus` trait (cards: `(id, label, url, closed)`; history; settings via `settings_matching`) so tests need no `App`; (b) `omni_matches`/`omni_rows`/`omni_activate` on `App`; (c) `omni_ui`: lift the field and dropdown drawing from `tab_view.rs` `AddressBar` (suggestion fetcher, unfocused-frames blip rule, highlight and Up/Down handling — move, keep behaviour) and from `quick_switcher.rs` (popup placement and Esc); (d) replace the three call sites: Item view calls `omni_ui(Host::Item)` with `Mode::Address` (text seeded with the tab url on focus, select-all), Graph toolbar calls `omni_ui(Host::Graph)` where the filter box is, the Settings page header calls `omni_ui(Host::Settings)`, Ctrl+K opens the popup host; (e) `card.rs` dims by `omni_matches` membership; delete `graph_filter`, `graph_filter_dims`, `QuickSwitcher`'s matching and `AddressBar`'s dropdown code; (f) the debug verbs `ADDRESSBAR`, `GRAPHFILTERTEXT` and the quick-switcher verbs set the one `OmniBar` state (names kept).
- [ ] **Step 6:** `cargo test -j 8 -p manifold` (whole crate) — PASS; any moved test that cannot be moved is deleted only with a ledger reason.
- [ ] **Step 7:** build and run the UI-affecting scenarios: `cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate && target/debug/gate.exe ui LOG` (detached at BelowNormal, check `%LOCALAPPDATA%\cpu-leases` first) — PASS. Fix regressions before committing.
- [ ] **Step 8:** fmt, clippy, commit `One omnibar for the Graph, Item and Settings views and Ctrl+K`.

---

### Task 6: Tokenizer, BM25 index and prefix search

**Files:** Create `search/tokenize.rs`, `search/index.rs`.

**Interfaces:** Produces
```rust
pub(crate) fn tokens(text: &str) -> Vec<String>;
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub(crate) enum DocKey { Node(NodeId), History(u32) }
pub(crate) struct Index { /* postings: BTreeMap<String, Vec<(DocKey, u32)>>, len: HashMap<DocKey, u32>, total_len: u64 */ }
impl Index {
    pub(crate) fn new() -> Self;
    pub(crate) fn add(&mut self, key: DocKey, text: &str);        // replaces an existing doc
    pub(crate) fn remove(&mut self, key: DocKey);
    pub(crate) fn search(&self, query: &str) -> Vec<(DocKey, f32)>; // AND of words, last word prefix, BM25, best first
    pub(crate) fn snippet(text: &str, query: &str, width: usize) -> Option<String>;
    pub(crate) fn docs(&self) -> usize;
    pub(crate) fn term_doc_matrix(&self) -> Vec<(DocKey, Vec<(&str, u32)>)>; // for LSA
}
```
- [ ] **Step 1: Failing tests** (`tokenize.rs`, `index.rs`):
```rust
#[test] fn tokens_lowercase_and_split_on_punctuation() { assert_eq!(tokens("Hello, World-wide web!"), ["hello","world","wide","web"]); }
#[test] fn cjk_characters_are_single_tokens() { assert_eq!(tokens("日本語abc"), ["日","本","語","abc"]); }
#[test] fn odd_unicode_never_panics() { let _ = tokens("a\u{FFFD}\u{200B}\u{0}𝒳e\u{0301}"); }
#[test] fn bm25_prefers_the_doc_with_more_and_rarer_matches() { /* three docs; "rust" appears 3x in A, 1x in B, 0 in C; A before B, C absent */ }
#[test] fn last_word_matches_as_a_prefix_others_exactly() { /* "rus" finds "rust"; "rust prog" finds "rust programming"; "ru prog" finds none */ }
#[test] fn add_replaces_and_remove_forgets() {}
#[test] fn rebuild_equals_incremental() { /* index built by add-all vs add/remove/add sequence give equal search results */ }
#[test] fn snippet_is_a_window_around_the_first_hit() { /* contains the term, at most width chars, ends on char boundaries */ }
```
- [ ] **Step 2:** `cargo test -j 8 -p manifold search::` — FAIL.
- [ ] **Step 3: Implement.**

```rust
// tokenize.rs
fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF)
}
pub(crate) fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let flush = |cur: &mut String, out: &mut Vec<String>| {
        if !cur.is_empty() {
            out.push(std::mem::take(cur));
        }
    };
    for ch in text.chars() {
        if is_cjk(ch) {
            flush(&mut cur, &mut out);
            out.push(ch.to_string());
        } else if ch.is_alphanumeric() {
            cur.extend(ch.to_lowercase());
        } else {
            flush(&mut cur, &mut out);
        }
    }
    flush(&mut cur, &mut out);
    out
}
```
  BM25 (`K1 = 1.2`, `B = 0.75`): `idf = ln(1 + (N - n + 0.5) / (n + 0.5))`, `score += idf * tf * (K1 + 1) / (tf + K1 * (1 - B + B * len / avg_len))`. `search`: tokenize the query; for the last word collect terms via `postings.range(w.clone()..)` while `starts_with(w)`; each earlier word is an exact key; a doc must hit every word (intersection by `HashMap<DocKey, f32>` accumulating per word, keeping only docs present for all); sort by score then `DocKey` for determinism. `snippet`: find the first query token in `text.to_lowercase()` char offsets (do not slice by bytes from the lowercase copy; locate by `char_indices` on `text`), take `width` chars around it.
- [ ] **Step 4:** the eight tests PASS; add a test `search_is_fast_on_ten_thousand_docs` marked `#[ignore]` that prints elapsed time for 10 000 × 200-word docs (run once with `-- --ignored --nocapture`, record the number in the ledger; target well under 5 ms per query).
- [ ] **Step 5:** commit `Search: tokenizer and BM25 index`.

---

### Task 7: Index service — worker, updates, privacy, Settings button, `WAITINDEX`

**Files:** Create `search/service.rs`. Modify `search.rs`, `main.rs`, `tabs/page_text.rs`, `settings_rows.rs`, `debug_control.rs`, `omnibar/rows.rs`, `README.md`.

**Interfaces:** Produces
```rust
pub(crate) struct SearchService { /* index: Arc<Mutex<Index>>, tx: Sender<Job>, generation: Arc<AtomicU64>, pending: Arc<AtomicUsize> */ }
impl SearchService {
    pub(crate) fn start(texts: &TextStore) -> Self;               // spawns the worker, queues a full build
    pub(crate) fn queue_add(&self, key: DocKey, text: String);
    pub(crate) fn queue_remove(&self, key: DocKey);
    pub(crate) fn search(&self, query: &str) -> Vec<(DocKey, f32)>;   // locks briefly
    pub(crate) fn generation(&self) -> u64;                           // bumps after each job
    pub(crate) fn busy(&self) -> bool;                                // pending > 0
}
```
- [ ] **Step 1: Failing tests:**
```rust
#[test] fn queued_adds_become_searchable_and_generation_advances() { /* poll busy() with a deadline, not a sleep */ }
#[test] fn start_builds_from_the_text_store() {}
#[test] fn removing_a_node_removes_its_hits() {}
#[test] fn an_ephemeral_jar_turning_on_purges_its_records() {}
#[test] fn clear_empties_store_and_index() {}
#[test] fn a_note_edit_reindexes_after_the_debounce() { /* drive with an explicit `now` argument, no real clock */ }
```
- [ ] **Step 2:** FAIL. **Step 3: Implement.** Worker thread named `search-index`, `BelowNormal` priority is NOT set in code (priority is the launcher's business); jobs: Add/Remove/Rebuild; `pending` incremented on enqueue, decremented after the job; `generation` incremented after each job. `App` fields: `search: SearchState { texts: TextStore, service: SearchService, note_dirty: HashMap<NodeId, Instant> }`. Document text per kind: tab = title + url + stored text; note = note text; group = name; download = file name; others = name. Wire the updates: `accept_page_text` (Task 2) queues the add; note edit → `note_dirty` then flush after 1 s via `poll_text_timers`; rename → re-add; permanent delete (document no longer holds the node) → `texts.remove` + `queue_remove`; ephemeral toggle (`jar_defs.rs` `Ephemeral(NodeId)` request path) → purge all tabs of that jar. History entries: add `DocKey::History(i)` for each `history.json` entry at start and when history changes (grep `history.rs` for its mutation point). Startup prune: texts whose node the document does not hold at all are removed (once, after the document loads).
- [ ] **Step 4:** Omnibar rows use `service.search`: card rows now include BM25 hits (with `Index::snippet` over the stored text and the jar name); `omni_matches` = substring matches ∪ index hits; cache key = (text, `generation`). Add tests in `omnibar/rows.rs`: `a_page_text_word_finds_the_card_with_a_snippet`, `a_closed_card_text_hit_is_not_offered`.
- [ ] **Step 5:** `WAITINDEX` verb in `debug_control.rs` `VERBS`: replies when `!service.busy()` and no text query is in flight (use the existing `idle_waiters` mechanism with its own list; deadline via the verb's existing timeout). Settings row `Clear search index` (title, keywords "privacy history text forget delete") + a one-line note stating what is stored where; button calls `clear`.
- [ ] **Step 6: Measure startup build.** Create a 50 MB `text.log` fixture in the scratchpad (generated by a test helper behind `#[ignore]`), time `SearchService::start` to idle with `cargo test -p manifold --release -- --ignored --nocapture startup_build`; record milliseconds in the ledger. **Decision (P1): the index is rebuilt from `text.log` at startup and never saved (no `index.bin`) unless this exceeds 1 s on an idle machine; if it does, add a saved index as a follow-up task and say so.** (The spec's "saved as one file" is the fallback; this removes a file format and its invalidation rules.)
- [ ] **Step 7:** gate scenario `search` in `crates/gate`: serve two local pages with distinct words, open both, `WAITINDEX`, type a word via `ADDRESSBAR`/omnibar verb in Graph, assert the right card is in the match set (export state field `omni_matches`, add it to `debug_export.rs`), press Enter, assert the camera/focus reached the card. Run `target/debug/gate.exe search` — PASS.
- [ ] **Step 8:** README "Search" section (what is stored where, the Clear button, that nothing leaves the device, ephemeral jars excluded); fmt, clippy, commit `Local page index behind the omnibar`.

---

### Task 8: LSA embedder

**Files:** Create `search/lsa.rs`. Modify `search.rs`.

**Interfaces:** Produces
```rust
pub(crate) const LSA_K: usize = 64;
pub(crate) trait Embedder {
    fn embed(&self, docs: &[(DocKey, Vec<(&str, u32)>)]) -> Embedding;
}
pub(crate) struct Embedding { pub keys: Vec<DocKey>, pub vectors: Vec<Vec<f32>> /* L2-normalised */, pub terms: Vec<String>, pub weights: Vec<Vec<(u32, f32)>> /* tf-idf rows, term index */ }
impl Embedding { pub(crate) fn cosine(&self, a: usize, b: usize) -> f32; pub(crate) fn top_terms(&self, members: &[usize], n: usize) -> Vec<String>; }
pub(crate) struct LsaEmbedder { pub k: usize, pub seed: u64 }
```
- [ ] **Step 1: Failing tests:**
```rust
#[test] fn two_topics_separate() { /* 6 docs: 3 about cats/milk/fur, 3 about rust/cargo/crate; mean in-topic cosine > 0.5 and > mean cross-topic + 0.3 */ }
#[test] fn the_result_is_deterministic() { /* embed twice, vectors bit-equal */ }
#[test] fn input_order_does_not_matter() { /* shuffled docs give the same cosine for the same pair */ }
#[test] fn one_doc_and_empty_input_do_not_panic() {}
#[test] fn top_terms_name_the_topic() { /* members of the cat docs -> includes "cat" or "milk" */ }
#[test] fn svd_recovers_a_known_rank_one_matrix() { /* internal helper: A = u vT, top singular value == |u||v| within 1e-3 */ }
```
- [ ] **Step 2:** FAIL. **Step 3: Implement.** Pipeline: (1) sort docs by `DocKey`; (2) vocabulary = terms with df ≥ 2, top 20 000 by df (ties by term); (3) tf-idf row weight `w = (1 + ln tf) * ln(1 + N / df)`, rows L2-normalised; (4) randomized range finder: `l = min(k + 8, N, V)`, Ω entries from a seeded xorshift64* keyed by `(seed, term, col)` mapped to `[-1, 1]`; `Y = A Ω`; two power iterations, each `Z = Aᵀ Y`, Gram-Schmidt orthonormalise columns (twice-is-enough), `Y = A Z`, orthonormalise; `Q` = orthonormalised `Y` (N × l); (5) `B = Qᵀ A` computed as `(Aᵀ Q)ᵀ` (l × V, dense l×V f32 — at 20 000 × 72 that is 5.8 MB); (6) eigendecomposition of `B Bᵀ` (l × l) by cyclic Jacobi (max 60 sweeps, off-diagonal norm < 1e-9); singular values `σ = sqrt(max(λ, 0))`; (7) doc embedding = `Q · (U_B[:, top k] · diag(σ))`, rows L2-normalised (a zero row stays zero); `cosine` of a zero row is 0. `top_terms`: sum members' tf-idf weights per term, return the top `n` terms by mass, skipping terms in more than 60 % of all docs. All loops iterate in sorted-key order; no `HashMap` iteration feeds arithmetic.
- [ ] **Step 4:** the six tests PASS (`cargo test -j 8 -p manifold lsa`). Time `embed` on a 2 000-doc synthetic corpus (`#[ignore]` test, record ms; target < 2 s release).
- [ ] **Step 5:** commit `Search: LSA embedder behind a trait`.

---

### Task 9: Clustering, suggestions, Accept

**Files:** Create `search/cluster.rs`. Modify `graph_nav.rs` (extract `group_cards`), `graph_ui/{toolbar,overlays}.rs`, `omnibar/rows.rs` (commands), `main.rs` (`suggestions` state), `debug_control.rs`.

**Interfaces:** Produces
```rust
pub(crate) const CLUSTER_MIN_SIMILARITY: f32 = 0.35;   // starting value; Step 4 sets it
pub(crate) const CLUSTER_MIN_SIZE: usize = 3;
pub(crate) const CLUSTER_MAX: usize = 8;
pub(crate) const CLUSTER_MAX_CARDS: usize = 400;
pub(crate) fn cluster(emb: &Embedding, min_similarity: f32) -> Vec<Vec<usize>>;   // indices into emb.keys
pub(crate) struct Suggestion { pub label: String, pub cards: Vec<NodeId> }
impl App {
    pub(crate) fn suggest_groups(&mut self);                                   // fills self.suggestions for the current level
    pub(crate) fn accept_suggestion(&mut self, index: usize);                  // one undo step
    pub(crate) fn dismiss_suggestion(&mut self, index: usize);
    pub(crate) fn group_cards(&mut self, members: &[NodeId], at: egui::Vec2) -> NodeId;   // extracted from apply_drop GroupWith
}
```
- [ ] **Step 1: Failing tests (pure):**
```rust
#[test] fn average_linkage_splits_two_topics() { /* embedding from Task 8's two-topic corpus, cluster -> two clusters of 3 */ }
#[test] fn clusters_below_min_size_are_dropped_and_at_most_cluster_max_returned() {}
#[test] fn singletons_and_zero_vectors_are_never_clustered() {}
#[test] fn cluster_result_is_deterministic_and_independent_of_input_order() {}
```
  Implement naive average linkage: start with each non-zero-vector doc as a cluster; repeatedly merge the pair with the highest average cosine while it is ≥ `min_similarity`; tie-break by lowest indices; O(n³) is fine at `CLUSTER_MAX_CARDS`. Return clusters of ≥ `CLUSTER_MIN_SIZE`, largest first, at most `CLUSTER_MAX`.
- [ ] **Step 2: Failing tests (App):**
Fixture helper `two_topic_app()`: six tab nodes at one level (three about cats, three about Rust), a group card and an image card, texts put through `SearchService`, `wait until !busy()`.

```rust
// suggest_groups(): 2 suggestions of 3 cards each; labels contain "cat"/"rust"-topic terms; the group and image cards are in none.
#[test] fn suggest_groups_labels_clusters_with_top_terms_and_skips_group_cards_and_textless_cards() {}
// accept_suggestion(0): a new group exists, its members are exactly the 3 cards, undo history len == 1 with kind Revert{created: Some(g)}.
#[test] fn accepting_creates_the_group_moves_the_cards_and_records_one_revert() {}
// accept, then step_history(undo): the 3 cards are back at the level (group None), the made group is closed.
#[test] fn undo_after_accept_restores_membership_and_closes_the_empty_group() {}
// accept, move one member out by edit_group, undo: that member stays where it was put, the other two return.
#[test] fn undo_after_accept_skips_a_card_the_user_moved_again() {}
// suggestions non-empty; insert another card at the level; suggestions empty.
#[test] fn the_set_of_cards_changing_clears_suggestions() {}
// no new body: the existing apply_drop GroupWith tests must pass untouched after the extraction.
#[test] fn apply_drop_group_with_still_behaves_as_before() {}
```
- [ ] **Step 3: Implement.** Extract `group_cards(members, at)` from `apply_drop`'s `GroupWith` arm (`alloc_node_id`, `insert_group_at(group, Some(at))`, `edit_group` + `recheck_edge_scope` per member); `apply_drop`'s arm calls it, then run all existing drop/undo tests. `suggest_groups`: gather current-level non-group cards that have text (`SearchService` docs → `Embedder::embed` via the index's `term_doc_matrix` filtered to those keys; cap `CLUSTER_MAX_CARDS`, nearest by recency if more), cluster, label with `top_terms(…, 3).join(", ")`. `accept_suggestion`: snapshot `Moved` (group before/after, pos unchanged) like `apply_drop` does, call `group_cards` at the members' centroid, rename the group to the label (use the existing rename path with the name rules in `names.rs`), `record_undo(message, UndoKind::Revert { moved, created: Some(group) })`, `save_session`. Embedding is computed on demand off the UI thread? The measured time (Task 8) decides: if < 100 ms for the current level, run inline; else run on the service worker and show "Suggesting…" until the result arrives (add a `SuggestJob`). Record the choice in the ledger.
- [ ] **Step 4: UI.** Toolbar button "Suggest groups" (next to "+ New Group") and an omnibar command row (`Row::Command`, add to the `Row` enum and `rows.rs`: shown when the typed text is a prefix of "suggest groups" or "arrange by topic"); drawing in `graph_ui/overlays.rs`: a colored ring per member card (palette from the theme, one color per suggestion), one chip at the centroid with the label, **Accept**, **Dismiss** buttons. Add `SUGGEST`, `ACCEPTSUGGESTION n` debug verbs and `suggestions` to `export_state`.
- [ ] **Step 5: Tune `CLUSTER_MIN_SIMILARITY`** on a fixture of ~60 real-looking short documents in three topics written into a test (`#[ignore]` to print the cluster sizes at thresholds 0.2…0.6); pick the value that recovers the three topics; record the table in the ledger.
- [ ] **Step 6:** gate scenario `suggest` (open six local pages in two topics, `WAITINDEX`, `SUGGEST`, assert two suggestions with expected members, `ACCEPTSUGGESTION 0`, assert a group with three members, `UNDO`, assert restored). PASS.
- [ ] **Step 7:** fmt, clippy, commit `Suggested groups from page text`.

---

### Task 10: Weighted edges; prototype; "Arrange by topic"

**Files:** Modify `graph_layout.rs`, `graph_nav.rs`, `graph_ui/toolbar.rs`, `omnibar/rows.rs`, `debug_control.rs`; create `graph_layout/prototype.rs` as `#[cfg(test)]` only.

**Interfaces:** Produces
```rust
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edge { pub from: NodeId, pub to: NodeId, pub weight: f32 }
pub fn step_sized(positions, velocities, edges: &[Edge], pinned, sizes, center, dt);   // spring: SPRING_STRENGTH * weight * (dist - REST_LENGTH)
impl App {
    pub(crate) fn spring_edges(&self) -> Vec<Edge>;                  // weight 1.0
    pub(crate) fn similarity_edges(&self, emb: &Embedding, ids: &[NodeId]) -> Vec<Edge>;
    pub(crate) fn arrange_by_topic(&mut self);                       // one undo step
}
pub(crate) const ARRANGE_ITERATIONS: u32 = 300;  pub(crate) const ARRANGE_MAX_CARDS: usize = 300;  pub(crate) const SIMILARITY_EDGE_MIN: f32 = 0.30;
```
- [ ] **Step 1: Edge type change, tests first.** Change the existing tests in `graph_layout.rs` to build `Edge { weight: 1.0 }` (mechanical), run `cargo test -j 8 -p manifold graph_layout` — FAIL (type), implement the `Edge` struct and the spring line `let force = delta / dist * (SPRING_STRENGTH * edge.weight * (dist - REST_LENGTH));`, sort springs by `(from, to)` index as now. Expected: `step_sized_matches_reference_bit_for_bit` PASS **unchanged apart from the edge type** (the reference function in the test gets the same one-line change). Add `a_zero_weight_edge_exerts_no_force` and `a_heavier_edge_pulls_closer_than_a_lighter_one`. Update `spring_edges`, `settle_current_level` and the `#[cfg(test)] step` wrapper (`graph_layout.rs:70`, which takes `(NodeId, NodeId)` pairs: change it to `&[Edge]` too, and every test caller); `cargo test -p manifold` PASS. Commit `Weighted layout edges`.
- [ ] **Step 2: Prototype (throwaway, then record).** In `graph_layout/prototype.rs` (`#[cfg(test)]`, `#[ignore]`): synthetic 200 cards (4 topics × 40 + 40 noise), random start positions on a 4000×4000 field, a similarity matrix with in-topic cosine ≈ 0.6 ± 0.15 and cross-topic ≈ 0.1 ± 0.1, top-3 similarity edges, `ARRANGE_ITERATIONS` of `step_sized`. Print: mean in-topic / cross-topic center distance ratio; max displacement in a following ordinary 90-step settle with only structural edges (answers OQ1); time for 300 iterations at 200 and 300 cards. Run `cargo test -p manifold --release -- --ignored --nocapture prototype`.
  **Decision gate:** if in/cross ratio < 0.6 the result is usable → no seed. If tangled (ratio > 0.8) → add the LSA seed (initial positions from the first two embedding components scaled to the field) and re-run. If a 90-step settle moves cards > 25 % of `REST_LENGTH` on average → OQ1 needs the user (stop and report with the numbers); otherwise accept the drift (default (a)). Write the numbers into the spec's OQ1 paragraph and the ledger.
- [ ] **Step 3: Failing App tests:**
Same `two_topic_app()` fixture, cards spread randomly (fixed seed) first:

```rust
// arrange_by_topic(): mean distance within each topic < mean distance across topics.
#[test] fn arrange_moves_same_topic_cards_closer_than_different_topic_cards() {}
// pin one card (doc.mirror pin API), arrange: its layout_pos and doc pos are unchanged.
#[test] fn arrange_never_moves_a_pinned_card() {}
// after arrange, mirror.pos(id) == layout_pos(id) for moved cards; one undo entry; undo restores every pos(before).
#[test] fn arrange_writes_document_positions_and_one_undo_restores_them_all() {}
// arrange, drag one card elsewhere (edit_pos), undo: that card keeps its new pos, the others return.
#[test] fn undo_after_arrange_skips_a_card_moved_since() {}
// ARRANGE_MAX_CARDS + 1 textual cards: no positions change, no undo entry, a toast names the limit.
#[test] fn arrange_refuses_more_than_arrange_max_cards_with_a_toast() {}
```
- [ ] **Step 4: Implement `arrange_by_topic`:** build `Embedding` for the level's text-bearing non-group cards (as `suggest_groups`); `similarity_edges`: for each card its 3 highest cosine neighbours ≥ `SIMILARITY_EDGE_MIN`, dedupe unordered pairs keeping the larger weight; run `step_sized` for `ARRANGE_ITERATIONS` with `spring_edges()` plus similarity edges, `pinned` as in `settle_current_level`; for each card whose layout position changed, record `Moved { id, group: (g, g), parent: (p, p), pos: (before, after) }` using `self.doc.mirror.pos`, `edit_pos(id, Some(new))`, `snap_layout`; `record_undo("Arranged by topic", UndoKind::Revert { moved, created: None })`; `save_session()`. Button "Arrange by topic" on the toolbar and the omnibar command row.
- [ ] **Step 5:** gate scenario `arrange` (the six pages from `suggest`, plus `ARRANGE`, assert distances via `export_state` positions, `UNDO`, assert restored). PASS.
- [ ] **Step 6:** fmt, clippy, commit `Arrange by topic`.

---

### Task 11: Docs, gate and wrap-up

**Files:** `README.md`, `PARKED.md`, spec, this plan, `docs/runs.md`.

- [ ] **Step 1: Docs sweep.** `rg -n "AddressBar|graph_filter|QuickSwitcher|quick switcher|PROTOCOL_VERSION|Filter\.\.\." README.md docs crates/*/src --glob '!target'` — fix every stale statement (README keyboard help for Ctrl+K and the filter box, protocol version line, the module layout list gains `search/` and `omnibar`). Add to `PARKED.md`: the unbuilt-host live checks (Task 3), PDF text via pdfium, topic chips, and the saved-index follow-up only if Task 7 Step 6 triggered it.
- [ ] **Step 2: Gate.** `target/debug/gate.exe tier` advises; this change touches the protocol, store-adjacent files and UI, so run `full`: launch through `~/.claude/runs/run.ps1 -Timed -Kind gate -Name full-search -WaitSec 2700 -HoldMin 25 -Command 'cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate && target\debug\gate.exe all "%RUN_DIR%\gate-log.txt" full'` (as `launch-gate.ps1` in the scratchpad does), only when asked to run it; report CPU load and Defender state with the result; set the `docs/runs.md` headline.
- [ ] **Step 3:** set this plan and the spec to `status: done` after the last task lands; leftovers are in `PARKED.md` first.
- [ ] **Step 4:** final whole-branch review (one fresh reviewer, Opus only if the branch is over ~300 changed lines — it is; model `opus` per WORKING_STYLE), the ledger's Rulings and Deferred minors reported; then wait for the user to bump and tag.

---

## Self-review

Spec coverage: D2–D6 → Tasks 1–3; D9 → Task 4; D7/D8 → Task 5 (+7 for index rows); D10/D11 → Tasks 6–7 (P1 replaces the saved index file with a startup rebuild unless measured too slow; ephemeral exclusion in Tasks 2, 7); D12 → Task 8; D13 → Task 9; D14 and OQ1 → Task 10; D15 order → Tasks 1–3 extraction, 4 settings, 5 omnibar, 6–7 index, 8–9 categorization, 10 layout. Docs, gate → Task 11. R3 (`WAITINDEX`) → Task 7.

Rulings made while planning: P1 (startup rebuild, not a saved index, subject to the measurement in Task 7 Step 6); host verification limited to what builds here (Task 3 Step 3); `Row::Command` added to the omnibar for the two commands (spec said "a row in the omnibar's commands").
