---
status: draft
---

# Servo 0.8 and the hybrid load path: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** a url, Reload, Back or Forward sent to a Servo tab is never silently dropped or overridden, and the page keeps its own session history whenever no load may be pending in it.

**Architecture:** decision w1 (2026-11-10), from the spike on `spike/servo-0.8` (hybrid 5eadb328, v2 7382bc59, v3 9e512e40; devlog entries from "A pending-view swap fixes every host-sent load" to "The hybrid's race is not seen, and a Back needs its own rule"). The Servo host moves to Servo 0.8. A new `views.rs` owns a shown `View` and at most one hidden pending `View`, each a `WebView` with its own `Delegate`. Each delegate keeps a `LoadState`: whether a load may be pending in its view (set when the host sends one or the page requests one, cleared at `HeadParsed`) and how many host traversals are still open. A `Navigate` or `Reload` goes to the shown view only when no load may be pending there, no traversal is open and no view is pending; otherwise it builds a hidden pending view at the target, which replaces the shown one at its first history change. A Back or Forward goes to the shown view (dropping any pending view) unless a load may be pending there, since Servo would commit that load over the Back; then, if there is an entry to go to, it swaps to that entry instead. `take_start_url` is deleted: an early `Navigate` builds a pending view like any other.

**Tech Stack:** Rust, `servo` 0.8.0 (crates.io), the gate's local `Site` server.

**Spec:** this Architecture paragraph, PARKED "Servo drops a load sent before the previous one commits", and the 2026-11-10 Servo devlog entries. The spike is the design record; there is no separate spec.

## Global Constraints

- Commit this plan to `main` first. Then work in a new worktree `.claude/worktrees/servo-hybrid` on branch `servo-hybrid` from `main`, with `target-servo` a junction to `%LOCALAPPDATA%\Temp\s8s` (a Servo 0.8 tree, so builds are incremental) and `target` a junction to `%LOCALAPPDATA%\Temp\s8t`. Both, the spike worktree, `s8x` and both branches are deleted after the merge (y3).
- `s8s` holds the spike's host until Task 1 rebuilds it. Before any gate run, the Servo host exe's mtime must be newer than the last host source edit, and `grep -c SPIKE` on it must print 0 (memory "Shared target keeps a stale exe").
- Every build or gate run over about a minute goes through `~/.claude/runs/run.ps1`. Build the gate, the app and the Servo host in separate wrapper calls; never chain them with `&&` in one `-Command` (run `2026-11-10-race-build-2` built the gate into `s8s` after a failed host build).
- App and gate: `cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate`. Servo host: `set "CARGO_TARGET_DIR=target-servo"& cargo build -j 6 -p manifold-view-servo`, then, in a separate call (so `CARGO_TARGET_DIR` is unset), `target\debug\gate.exe sync-runtime-deps`.
- `cargo lint` and `cargo unit` exclude the Servo crate (`.cargo/config.toml`). After every Servo host change also run `set "CARGO_TARGET_DIR=target-servo"& cargo clippy -j 6 -p manifold-view-servo -- -D warnings` and `... cargo test -j 6 -p manifold-view-servo`. Run `cargo fmt --all` before each commit.
- Commits end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`; stage files by path. No push, tag or bump.
- Python never reads stdin (`python -P f.py < /dev/null`).
- No `SPIKE` text, `SPIKE_*` switches or timing lines reach `main`.

## End-state fit

| Change | Target location | Replaces | Deletes |
|---|---|---|---|
| Servo 0.8 | workspace `Cargo.toml` (`servo`, `icu_properties`), `Cargo.lock` | 0.5.0 | the 0.5-specific comments in `translate_mouse_button` and above the `Find*` arm |
| Views and the load rules | new `crates/manifold-view-servo/src/views.rs`: `Delegate` (moved), `LoadState`, `View`, `Views`; its tests in `views/tests.rs` | the single `webview`/`delegate` pair in `main.rs` | `take_start_url`, its two tests, the early-message drain |
| Load checks | new gate stage `loads`, `crates/gate/src/loads.rs`; routes `/slow`, `/color`, `/hop`, `/go` in `probes.rs` `Site`; README names the stage | the spike's throwaway `gate spike` cases | nothing on `main` |
| The parked load-drop entry | PARKED.md | "Servo drops a load sent before the previous one commits" | that entry; one new entry for the accepted limits (Task 5) |

`main.rs` keeps process setup, the message loop and input translation; `views.rs` decides which `WebView` a message reaches. The split comes before the hybrid as its own commit (tripwire: `main.rs` would otherwise gain a second responsibility). `main.rs` shrinks from 576 lines; `views.rs` starts near 300; both stay far under the 1,200-line limit.

## Review Focus

1. **A flag that never clears.** A load that never parses a head (a 204, a download, a non-HTML document), or a navigation an iframe or a page script starts (Servo's `request_navigation` does not say which frame), leaves the flag set. The next typed url and the next Back or Forward each swap: a fresh load of the target, the page's in-view history lost (seen on Microsoft's sign-in request, 1 of 19 clicked sites, run `-hyb-clicked`). Accepted, in the PARKED entry. Tests: `live204` (a later typed url still reaches the server), `back-swap`.
2. **A dead pending view.** A swap whose load ends in a 204 never commits, so Servo sends it nothing; the tab is not marked loading for it (spike run `-n1-live204`) and the app's trail deadline reverts the move. It stays until the next `Navigate`, Reload or Back replaces it. Test: `live204` second step.
3. **Size and zoom of a pending view.** It is built at `main`'s tracked window size and the shown `WebView`'s `page_zoom()`, and `Resize` and `SetZoom` reach both views. Test: `zoom-swap`.
4. **Messages during a swap.** `GetState`, `QueryPageState`, `QueryPageText`, `RestoreScroll` and input reach the shown (old) view, and promote drops that view, so a script reply pending at promote never comes. The app's own timeouts cover the sleep path (`tabs/sleep/state.rs`, 5 s). Accepted, in the PARKED entry; no test.
5. **The early start.** The first view loads `initial_url` (from the host's environment), and a `Navigate` or `Reload` that arrives before its first commit swaps; the newest wins, since each replaces the last. The app sends `Navigate` only for a user or trail load (`viewport.rs:208`) and a remote replica's url change (`session_sync/remote.rs:138`, absent from the single-instance gate), not at spawn, so no start-up refetch of `initial_url` follows. `sleep-servo` history covers a Navigate sent at about view-build time; `reload-early` covers a Reload before the first commit.

---

### Task 1: Servo 0.8

**Files:**
- Modify: `Cargo.toml` (`servo = "0.8.0"`, `icu_properties = "2.1.1"`), `Cargo.lock`, `crates/manifold-view-servo/src/main.rs`

- [ ] **Step 1:** Commit the plan on `main`; create the worktree and junctions; set the plan's `status: active` in the worktree. Expected: `git -C .claude/worktrees/servo-hybrid status` shows only the plan's status edit, on `servo-hybrid`.
- [ ] **Step 2:** Set both versions. Treat 0.8's new `LoadStatus::Stopped` (sent for `window.stop()`) like `Complete` for `nav.loading`, so a stopped load doesn't leave the tab loading. Map `Left/Middle/Right` to `ServoMouseButton::Primary/Auxiliary/Secondary` and delete the comment above the match; reword "the pinned 0.5.0 `WebView`" above the `Find*` arm to "Servo's `WebView`".
- [ ] **Step 3:** Build the app and gate, then the host, then `sync-runtime-deps` (wrapper, one call each). Expected: exit 0; the host exe has no `SPIKE` text.
- [ ] **Step 4:** `cargo unit`, and the Servo clippy and test commands. Expected: pass (`names.rs` on `icu_properties` 2.1.1; `every_host_key_has_a_servo_code`).
- [ ] **Step 5:** `VIEWS=servo gate trail` and `VIEWS=servo CASES="history crash restart" gate sleep` (wrapper). Expected: `trail` passes; `sleep` passes or fails only `history` (about 18 of 22 passed on plain 0.8).
- [ ] **Step 6:** Commit `servo host: Servo 0.8` with the lockfile and the plan's status.

### Task 2: the `loads` gate stage

**Files:**
- Create: `crates/gate/src/loads.rs`
- Modify: `crates/gate/src/probes.rs` (`Site` routes), `crates/gate/src/main.rs` (`STAGES`), `crates/gate/src/all.rs` (`STEPS` and the test `ui_mode_keeps_the_store_free_steps_in_order`), `README.md` (the gate stage list and the engine-stage paragraph)

**Interfaces:**
- Consumes: `probes::{seed, tab_node, quit, views, guarded, host_logs, Site}`, `instance::{poll, Instance}` (`launch_env`, `send`, `step`, `barrier`, `export_until`, `export_state`), `check::check`, `shots_img::load`.
- Produces: `pub(crate) fn loads(ctx: &mut Ctx) -> io::Result<()>`; `CASES` selects cases (default all); `VIEWS` as every probe stage. Each case launches its instance with its own `TEMP`/`TMP` directory (as `bad_url` does, `probes.rs:512-526`) and reads the host's per-tab log from it with `probes::host_logs`, so no other process's log can satisfy a check. A helper `decisions(tmp) -> Vec<String>` returns that log's `[views]` decision lines (`load in place`, `swap`, `history in place`, `history swap`), and `settled(tmp) -> bool` says no view is pending: the last of the log's `swap`, `history swap`, `history in place` and `[views] promoted` lines, if any, is neither `swap` nor `history swap` (an in-place Back drops a pending view too).

Routes added to `Site`: `/slow` (headers after 4 s, then the plain page), `/color?c=RRGGBB` (a full-window colour page) with a new `&w=N` (an N CSS px square of the colour at the top left, on white), `/hop` (an iframe at `/color?c=00ff00` that the page moves to `/color?c=ff00ff` after 1 s), `/go` (a page that sets `location.href = '/slow'` 1 s after load). All but `w=` are ported from the spike's `probes.rs`.

Typing is `ADDRESSBAR <url>`, `SLEEP 1`, `ENTER` (the spike's `type_url`: no `WAITIDLE`, which would wait out the load being raced); "then" below means after `WAITIDLE` unless a case says otherwise, and on Servo, at each "then" and at the case's end, the case waits for `settled` (20 s, else a failed check `loads servo: <case>: the swapped-in view is shown`), except at `live204`'s end, where the second `/k` swap never commits (Review Focus 2) and `settled` must stay false: a pending view is never marked loading, so `WAITIDLE` alone can return before promote, and the app sets the tab's url when the load is typed (`viewport.rs:207`), so "ends on" checks the trail, not what is shown; `settled` and the server hits are what show a swap landed. `GOBACK` and `GOFORWARD` are each sent once, then `export_until` waits for the move ("until" below), as the spike's `backtype` does: a resend would add a decision line. The width of a colour in a screenshot is the number of pixel columns holding at least one pixel within 16 of it on every channel. Each check is named `loads <view>: <claim>`. The last column is checked on Servo only: the `[views]` decisions the case must log, in this order, with nothing else between them.

| Case | Steps | Checks | Servo decisions |
|---|---|---|---|
| `dead204` | first load `/k` (204); wait 3 s; type `/p2` | `/p2` reaches the server | `swap` |
| `double` | type `/slow`; once requested (no idle wait) type `/p2` | `/p2` reaches the server; the tab ends focused on `/p2` | `load in place`, `swap` |
| `live204` | on `/`, type `/k`; then type `/p2`; then type `/slow` and once requested type `/k` | the first `/k` goes idle within 14 s and the tab stays off `/k`; `/p2` reaches the server; after the second `/k`, `/k` has 2 hits and the tab ends off `/k`, idle within 14 s | `load in place`, `swap`, `load in place`, `swap` |
| `reload` | Reload; then type `/slow` and once requested Reload | the first Reload refetches `/`; the second refetches `/` and the tab ends on `/` | `load in place`, `load in place`, `swap` |
| `reload-early` | first load `/slow`; once requested (no idle wait) Reload; wait 10 s | `/slow` is requested twice; the tab ends on `/slow`, idle | `swap` |
| `paint` | red, typed blue, then `/slow` | the spike's four pixel checks | `load in place`, `load in place` |
| `iframe` | first load `/hop`; wait for `/color?c=ff00ff`; type `/p2` | `/p2` reaches the server | `swap` |
| `back-type` | on `/`, type `/p2`; then `GOBACK` until the tab is off `/p2`; wait 3 s; type `/p3` | `/p3` reaches the server | `load in place`, `history in place`, `load in place` |
| `forward` | on `/`, type `/p2`; then `GOBACK` until off `/p2`; then `GOFORWARD` until on `/p2` | the tab ends on `/p2` | `load in place`, `history in place`, `history in place` |
| `back-pending` | on `/`, type `/p2`; then type `/slow` and once requested `GOBACK`; wait 8 s | the tab ends on `/` (one entry back from the committed `/p2`) | `load in place`, `load in place`, `history swap` |
| `back-page` | on `/`, type `/go`; once `/slow` is requested (the page's own load) `GOBACK`; wait 8 s | the tab ends on `/` | `load in place`, `history swap` |
| `back-swap` | on `/`, type `/hop`; wait for the frame's move; type `/slow`; once requested `GOBACK`; wait 8 s | `/slow` never shows; the tab ends on `/hop` (a Back after the frame's own move stays on `/hop` in both engines; Servo's joint history is `/`, `/hop`, `/hop`) | `load in place`, `swap`, `history swap` |
| `zoom-swap` | first load `/color?c=ff0000&w=100`; `WAITIDLE`, `SLEEP 1`, screenshot (red width `w0`); `ZOOMIN`; screenshot until the red width differs from `w0` (20 s; that width is `wr`); type `/slow`, once requested type `/color?c=0000ff&w=100`; screenshot until blue shows (20 s, else the check fails), then `WAITIDLE`, `SLEEP 1`, screenshot (blue width `wb`) | `wr > w0`; `wb` within 2 px of `wr` | `load in place`, `swap` |

`zoom-swap`, `forward`, `reload-early` and `back-swap` are new designs; `back-page` is the spike's `backpage` (run `-hyb3-back`); the rest are the spike's cases.

- [ ] **Step 1:** Write `loads.rs`, the routes and the stage entry. Add to `STEPS`, after `sleep-servo`: `Step { stage: "loads", env: &[("VIEWS", "webview2")], ..step("loads", Mode::Light) }` and `Step { stage: "loads", env: &[("VIEWS", "servo")], engine: Some("servo"), ..step("loads-servo", Mode::Ui) }` (`loads-servo` is `Ui`, as `trail-servo`, so a Servo host change under `gate all ... ui` runs it; the WebView2 run checks behaviour this work doesn't change, so it stays out of the ~2-minute `ui` tier); add `loads-servo` to `ui_mode_keeps_the_store_free_steps_in_order` in its order. Record the stage's duration in README once Task 3's run measures it. Name both in README (the test `the_readme_names_every_step` requires it), `loads-servo` beside `trail-servo` in the engine-stage paragraph.
- [ ] **Step 2:** `cargo unit`. Expected: pass.
- [ ] **Step 3:** Build the gate. Run `VIEWS=webview2 gate loads` (wrapper, `-Name loads-wv2`). Expected: every check passes. This is the first run of these cases on WebView2, so a failure here is a finding about the case or about WebView2: report it and stop before Step 4.
- [ ] **Step 4:** Run `VIEWS=servo gate loads` on Task 1's host (`-Name loads-red`). Expected: every Servo decision check fails (no `[views]` lines yet). Of the other checks, expected fails: `dead204`, `double`, the second `reload` check, `reload-early`, `zoom-swap` (each a load sent over a pending one, dropped), and `back-pending`, `back-page` and `back-swap` (the pending load commits over the Back; runs `-hyb-back`, `-hyb2-backpage`). May fail or pass: `live204`'s later steps (the dropped `/k` never shows). Expected green: `paint`, `iframe`, `back-type`, `forward`, the first `reload` check. Record every result; any other outcome is a finding to report before Task 3.
- [ ] **Step 5:** Commit `gate: a loads stage checks that no host-sent load or Back is dropped` (red on Servo until Task 3; `all` is not run in between).

### Task 3: the hybrid

**Files:**
- Create: `crates/manifold-view-servo/src/views.rs`, `crates/manifold-view-servo/src/views/tests.rs`
- Modify: `crates/manifold-view-servo/src/main.rs`

**Interfaces:**
- Produces, in `views.rs`:
  - `#[derive(Default)] pub(crate) struct LoadState { maybe_pending: bool, traversals: u32 }` with `load_sent(&mut self)` (set), `head_parsed(&mut self)` (clear), `traversal_started(&mut self)`, `traversal_complete(&mut self)` (saturating), `load_in_place(&self) -> bool` (clear and no open traversal), `history_in_place(&self) -> bool` (clear).
  - `Delegate` (moved from `main.rs`, with a `Delegate::new(url: String)` (seeding `NavState::new(url, false)`) replacing `main`'s struct literal and `pub(crate)` on the fields `main` reads) gains `load: RefCell<LoadState>` and `entries: RefCell<(Vec<Url>, usize)>` (the last `notify_history_changed`; the view has committed once it is non-empty). `notify_load_status_changed` calls `head_parsed` on `LoadStatus::HeadParsed`; `request_navigation` calls `load_sent` and leaves the request approved (the default); `notify_traversal_complete` calls `traversal_complete`.
  - `pub(crate) struct View { pub webview: WebView, pub delegate: Rc<Delegate> }`; `View::build(servo: &Servo, ctx: &Rc<SoftwareRenderingContext>, url: Url, size: PhysicalSize<u32>, zoom: f32) -> View` builds a shown view with its load marked sent; a caller that wants it pending calls `hide()`.
  - `pub(crate) enum Load { Url(Url), Reload }`.
  - `pub(crate) struct Views { shown: View, pending: Option<View>, size: PhysicalSize<u32> }` with `new(servo, ctx, url, size) -> Views`, `shown(&self) -> &View`, `load(&mut self, servo, ctx, load: Load)`, `traverse(&mut self, servo, ctx, back: bool)`, `resize(&mut self, size)`, `set_zoom(&self, factor: f32)`, `promote(&mut self) -> bool`.
- Rules:
  - `load`: the target is the url, or for `Reload` the shown view's current entry (`entries[current]`, which moves at a traversal's start; else `webview.url()`; else, before the first commit, the url the view was built at, `nav.url`), so a Reload always has a target. When `pending` is `None` and the shown `load_in_place()`, call `load_sent` and `load`/`reload` on the shown view, logging `[views] load in place`. Otherwise build a hidden pending view at the target, replacing any older pending one, logging `[views] swap`. So a Reload while a pending view exists reloads the page the user sees, not the pending one.
  - `traverse`: if the shown `history_in_place()`, drop the pending view, call `traversal_started` and `go_back(1)`/`go_forward(1)` (no `entries` lookup: Servo completes an out-of-range traversal at once, so the count can't stick), logging `[views] history in place`. Otherwise the target is `entries[current - 1]` or `entries[current + 1]`; with none, do nothing; else drop the pending view and build one at the target, logging `[views] history swap`.
  - `promote`: once the pending view's `entries` is non-empty, show it, hide and drop the old one, mark the new delegate's `frame_ready` and `nav.dirty`, and log `[views] promoted`. A pending view is never marked loading (Review Focus 2).
  - `resize` stores the size and reaches both views (`main.rs` keeps its guard that skips a zero width or height before calling it); `set_zoom` reaches both views; a pending view takes the shown view's `page_zoom()`.

- [ ] **Step 1:** Move `Delegate` and its impl into `views.rs`, adding only `Delegate::new` and the `pub(crate)` field markers; `mod views;` in `main.rs`. Servo build and clippy. Commit `servo host: move the view delegate to views.rs`.
- [ ] **Step 2:** Add `#[cfg(test)] mod tests;` to `views.rs` and write `views/tests.rs` for `LoadState`: `a_sent_load_is_not_in_place`, `head_parsed_clears_a_load`, `a_pending_load_blocks_a_back_in_place`, `an_open_traversal_blocks_a_load_until_complete`, `an_open_traversal_does_not_block_a_back`, `traversal_complete_never_goes_below_zero`. Run `cargo test -p manifold-view-servo` (Servo target). Expected: fails to compile (no `LoadState`).
- [ ] **Step 3:** Implement `LoadState`, then `View`, `Views` and the rules. In `main.rs`, call `views.promote()` after each `spin_event_loop`, route Navigate, Reload, GoBack, GoForward, Resize and SetZoom through `Views`, and read every other message and the drained cells from `views.shown()`. Delete `take_start_url`, its tests and the early drain.
- [ ] **Step 4:** Servo test and clippy. Expected: pass. Build the host and sync (wrapper).
- [ ] **Step 5:** `VIEWS="servo webview2" gate loads` (`-Name loads-green`). Expected: every check passes on both.
- [ ] **Step 6:** `VIEWS=servo gate trail`, and `VIEWS=servo CASES=history gate sleep` 10 times in one wrapper call (`-Name hybrid-sleep-10`). Expected: `trail` passes; `sleep` 10 of 10 (v2 passed 5 of 5, v3 1 of 1).
- [ ] **Step 7:** Commit `servo host: a load or Back goes to the shown view unless a load may be pending there, else to a new view`.

### Task 4: real sites

- [ ] **Step 1:** Throwaway, not committed: on a scratch copy of `loads.rs`, a `sites` case running the spike's `stuck` loop over its 20 sites, launched with `Instance::launch_online` as the spike's is (`launch_env` is offline) (no click; `CLICK` needs the spike's host switch, so clicked pages stay measured by run `-hyb-clicked` only). Per site: type the url, poll `export_state` (its own 30 s deadline; `export_until` waits 90 s) for the tab to be on that site and not loading, going on at the deadline (GitHub's page never completes), dwell 10 s, type the next; read `decisions`. Run it once (`-Name hybrid-sites`). Expected: about 20 of 20 typed urls reach the server, and every one except at most Microsoft's loads in place. Revert the scratch case (`git checkout -- crates/gate/src/loads.rs`, then check `git status` is clean); no `cargo unit` or commit before the revert (`limits.rs`'s `the_gate_launches_the_app_offline` fails on it).

### Task 5: docs, gate, merge

**Files:**
- Modify: `docs/devlog.md`, `PARKED.md`, `docs/runs.md` (wrapper rows and headlines)

- [ ] **Step 1:** Devlog entry: what shipped, the run ids, the clear signal chosen and the accepted limits (Review Focus 1, 2, 4; a load the page starts while the host's is pending is dropped, the host winning as in WebView2 and CEF; a pending view shows no loading state; a `window.stop()` leaves the flag set; the `HeadParsed` race inferred earlier was not seen in 4 × 200 chained loads, run `-hyb-race-2`).
- [ ] **Step 2:** PARKED: delete "Servo drops a load sent before the previous one commits". Add "A Servo tab swaps views where it can't tell a load ended": the limits above; why parked: Servo signals neither a 204, a download or a non-HTML end, nor which frame a `NavigationRequest` is for, and `WebView` has no stop; trigger: a report of a lost Back or Forward entry or a missing spinner on a Servo tab, or a Servo release that adds one of those signals; evidence: runs `-hyb-iframe-3`, `-hyb-clicked`, `-hyb3-back`, `constellation.rs:4109-4147`. Grep the docs for the old entry's title, `take_start_url`, `servo 0.5` and `0.5.0` and fix each live reference (PARKED's "Servo 0.5.0 has none" in the redirect entry: check it against 0.8 and say which version it was checked on); dated entries and decision files stay.
- [ ] **Step 3:** Build per CLAUDE.md "Gate", then the Servo host and `sync-runtime-deps` (Global Constraints), and run `set "WITH_ENGINES=servo"& target\debug\gate.exe all "%RUN_DIR%\gate-log.txt" full`, detached through the wrapper (`-Kind gate`), with a watcher (`loads-servo`, `trail-servo` and `sleep-servo` run only with `WITH_ENGINES=servo`, `all.rs:231-269`). Expected: pass; report CPU load and Defender state.
- [ ] **Step 4:** Commit the docs; merge `servo-hybrid` into `main` with `git merge --no-ff`; rerun `cargo unit` on `main`. No push.
- [ ] **Step 5:** Plan `status: done`; then y3: list what will be deleted (both worktrees, `s8s`, `s8t`, `s8x`, branches `spike/servo-0.8` and `servo-hybrid`), confirm with the user, delete.
