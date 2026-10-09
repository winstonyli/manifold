# Wave 2: performance Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Measure, then speed up, the verified wave-2 hot paths, each change behind a pin that passes before and after it:
- H3: the settle layout's size lookups in its O(n²) pair loop
- JSON H1a: a revealed card paints and scans every row, not the visible ones
- JSON H1b (conditional): a far reveal builds every row's card; a window of pages instead
- Jar H3: one jar save per pulled tab per 30 s round; one per round instead
- M1: JSON search allocates per node
- H2 / P2: pick-target mode's O(N·L) link check, `edge_menu`'s full target list, two H4 one-liners
- Teardown: O(N·L) link scans when a session is replaced
- H5: a save with nothing queued still takes the lock and reads the file
- Mirror link index (conditional)

**Architecture:** Every optimization is two commits:
- a tests-only commit: a pin (golden, reference copy or counter) and an `#[ignore]` timing test;
- a change commit, made only when the first commit's numbers clear the spec's threshold, and kept only when its own numbers clear the keep rule.

A shared `#[cfg(test)] mod bench` (timing helpers and the 2000-node fixture) and a `TIME_ONLY` mode of `scripts/json-pan-profile.sh` are the harness. No store format, sync protocol or IPC change, except H1b's optional `VIEW` field `first` (conditional).

**Tech Stack:** Rust (edition 2021), egui 0.36.2, serde / serde_json 1.0.151 (`preserve_order`), the session store journal.

**Spec:** `docs/superpowers/specs/2026-10-07-wave-2-performance-design.md`. The scope and estimates come from `.superpowers/survey/verified-wave-2.md`. Every line number and symbol below was re-read at `c8b7485`. Paths are under `crates/manifold/src/` unless they start with `docs/`, `scripts/` or `README.md`. Waves 0 and 1 land first and edit some of the same code, so find code by the quoted text: the numbers drift.

## Global Constraints

**Branch and commits**
- Work on branch `wave-2-performance`, created from `main` after waves 0 and 1 have merged (`git switch -c wave-2-performance main`), with no worktree.
- Commit once per task, with the exact message given in its last step. A change task whose part misses its keep rule reverts that part before the commit (Decision 11).
- Merge into `main` only after the user's go-ahead (Task 16).
- Each implementer's commit message ends with that implementer's own `Co-Authored-By:` line, taken from its system reminder. The controller's is `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`.

**Formatting and lints**
- Run `cargo fmt --all` before each commit.
- `cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 --all-targets -- -D warnings` must pass.
- `.githooks/pre-commit` runs both and takes 2+ minutes, so commit in the background. Never use `--no-verify`.
- Don't silence a lint: fix it, or `#[allow]` it with a comment saying why. Wave 0 enables `allow_attributes_without_reason`: every new `#[allow]` takes `reason = "..."`.
- An item used only by a later task is a dead-code warning under `-D warnings`. Each task adds only what it uses.

**Builds and the machine**
- Use `-j 8` on every cargo command. Another session may be building: if cargo waits on the target-dir lock, wait for it rather than killing anything.
- C: is nearly full: check `df -h /c` before a release build (the benches and `TIME_ONLY` runs need one).
- Before the gate and after the wave, check for leftovers with `Get-Process python, manifold -ErrorAction SilentlyContinue`.

**Tests**
- No test may read or write the real `%APPDATA%\manifold`.
  - `App::default()` is safe: `profile::dir()` is `None` under `cfg(test)`.
  - A test that needs files uses `crate::store::testutil::temp_dir(name)` (a distinct directory per call).
  - Headless timing runs use `--profile-dir` under `target/`.
- A pin passes before and after its change. Where it can fail, prove it can: each task says how (a temporary mutation, reverted before the commit).
- A test that pins today's behavior and fails on today's code is a finding: stop, report its output, and change no production code to make it pass.

**Timing runs (benches)**
- Command: `cargo test --release -j 8 -p manifold --bins <name> -- --ignored --nocapture`.
- Before each run:
  - `(Get-Counter '\Processor(_Total)\% Processor Time' -SampleInterval 2 -MaxSamples 2).CounterSamples[-1].CookedValue` under 20;
  - `(Get-MpComputerStatus).RealTimeProtectionEnabled` recorded.
- Report best and median (the helpers print both), never a single run.
- A decision within 10% of its threshold is rerun once on a quiet machine.
- Implementers paste the raw bench output, the CPU reading and the Defender state into their report. The controller applies the proceed and keep rules and appends the numbers to `## Outcome`.

**Python**
- Never run `python` in any way that can read stdin: no `python -`, no heredoc python, no python call tacked on as a no-op.
- Write the script to a file and run `python -P f.py < /dev/null`. Prefer the Edit tool for edits.
- Every implementer prompt repeats this rule. Subagents never dispatch subagents.

**Rust**
- Edition 2021, so no let-chains. `Option::is_none_or` and `usize::is_multiple_of` are fine (both already used). Write `x.is_multiple_of(n)`, never `x % n == 0` or `!= 0` on an unsigned integer: `clippy::manual_is_multiple_of` rejects it under `-D warnings`. A loop or closure variable it is called on needs its type spelled out (`0..2000usize`, `|i: &usize|`), or the call on a still-ambiguous integer is error E0689. In a function returning `Option`, use `?`, not `let Some(x) = … else { return None }` (`clippy::question_mark`).

**Spec values**
- Thresholds (proceed / keep), all on best-of-N:

  | Item | Proceed if | Keep if |
  |---|---|---|
  | H3 settle | `settle_cost` g0 > 8 ms | g0 drops ≥ 25% |
  | H1a | `reveal-numbers` > 1.2 × pan baseline | `reveal-numbers` drops ≥ 30%, or within 20% of the pan baseline |
  | H1b | `reveal-objects` > 1.5 × pan baseline after H1a (Task 3's baseline if H1a did not land; see Task 14) | `reveal-objects` drops ≥ 50% |
  | Jar H3 | burst > 1 save per round and `jar_save_cost` > 3.2 ms | a burst saves exactly once |
  | M1 | either `json_search_cost` query > 8 ms | both drop ≥ 25% |
  | Hoist + `edge_menu` | (always measured) | `valid_targets` at L=100 ≥ 2× faster |
  | `pick_targets` binary search | (always measured) | pick-mode frame ≥ 5% faster |
  | Tab-slot map | (always measured) | idle top-level frame ≥ 5% faster |
  | Teardown | (always measured) | `teardown_cost` ≥ 2× faster |
  | H5 | no-op save at 2000 tabs > 0.3 ms | ≥ 2× faster and the counts drop to 1, 0, 1, 1 |
  | Mirror index | `valid_targets` at L=100 > 1 ms after the hoist | ≥ 2× faster |

- `JAR_FLUSH_AFTER` is 2 s. `REVEAL_GROW_PAGES` is 5. `REVEAL_N` defaults to 200 000.
- The README's crash-window text: the jar's file lags memory by at most 2 s plus one event-loop wake (at most 100 ms).

## Decisions resolved by reading the code

1. **The fixture moves to `bench.rs` with real store ids.** `graph_nav::tests::level_fixture` (`graph_nav.rs:532-602`, plus its doc comment just above; anchor on the quoted text) names nodes `g{g}` and `n{i}`, which don't parse as `NodeId`, so links and saves can't use them. `alloc_node_id` works on `App::default()` (`graph_ui.rs:2180`). `level_pass_cost`'s level becomes `f.groups[0]`. The fixture used `Kind` and `std::time` only in `graph_nav`'s tests, so those imports go with it. `spread` lands with its first caller (Task 1): an unused helper fails `-D warnings`.
2. **Struct literal sites (each new field is added at every one):**
   - `json_view::Card`: `card.rs:1823` (the preview), `json_view.rs:338` (`visible_cards`), Task 3's golden, and Task 4's `a_culled_card_paints_only_its_visible_rows`.
   - `CardPass`: `card.rs:2566` (a test) and `graph_ui.rs:1664`.
   - `CardLook`: `card.rs:1845` (the preview), `json_level_ui.rs:536`, and Task 3's golden.
3. **Hit rows are rebuilt only where the search result is written.** The only writes of `search.result` are `json_level.rs:291`, `:306` and `:333`. `JsonLevel::value` is never reassigned (a reload builds a new level), so the per-parent rows built there stay valid.
4. **egui focus is read where it can change.** `Memory::has_focus(id)` is `focused() == Some(id)`. The exact rules (egui 0.36.2 `memory/mod.rs`):
   - `request_focus` applies at once (918-921), with the default lock filter (`tab: false`).
   - egui's own Tab (`FocusDirection::Next`) moves focus inside `interact`, within the pass (660-667). Its Shift+Tab (`Previous`) goes through `id_next_frame`, so it applies at the next `begin_pass` (668-670, 573-575).
   - `set_focus_lock_filter` applies only to a widget that also had focus last frame (903-911). So a card focused by the app's Tab code gets its lock filter one frame late, and a Tab key in the very next frame also runs egui's own move and skips a card (Task 3's Tab walk puts an idle frame after each key; the skip is today's behavior and is parked, not fixed by this wave).
   - `Memory::end_pass` drops focus from a widget not drawn this pass, which is why a focused card is never culled (`json_level_ui.rs:513`).
5. **`changed_on_disk` is (len, mtime), as `poll_store` already relies on** (`journal.rs:911`). `file_seen` returns `None` for a missing file, so `quiet()` requires `seen.is_some()`. `catch_up` and `append` refresh `seen`.
6. **H5 skips only the lock and `catch_up` on an unchanged file.** `sync_if_due` is called on its own (`session_sync.rs:1762`), not from `begin`. The full path always calls `take_store_recovery`; the fast path requires no recovery pending, so skipping it changes nothing.
7. **`save_jar` settles the dirty mark itself.** Its only callers are `jar_defs.rs:414` (`switch_jar_ephemeral` off), `:716` (`clear_jar_cookies`) and `tabs.rs:213` (`apply_pulled_cookies`, which stops saving). It becomes `&mut self` and removes the jar's `jar_dirty` entry first, so every write settles the mark in one place. `forget_jar` removes it too.
8. **The quit flush has one site.** `final_cookie_pull` is called only from `exiting` (`event_loop.rs:107`), headless runs included, so `flush_dirty_jars(None)` goes right after it.
9. **Teardown's end state is a set.** `old` (`session_sync.rs:2021-2025`) is every live non-jar node of the document, links included. Forgetting every mirror link first and then queueing every link's delete before `old` gives the same `pending_deletes`, with every `links_naming` scan seeing no links.
10. **The save counts are by name.** `apply_graph_action` ends in `save_session` (`graph_ui.rs:390`):
    - a menu Delete saves in `close_node_and_save` (`node_menu.rs:977`);
    - a Join drop saves in `apply_drop` (`graph_nav.rs:138`);
    - a finished link saves in `finish_connect` (`links.rs:152`);
    - entering a group saves only if a note was being edited (`end_note_edits`).

    So today: Delete 2, Enter 1, Join 2, Finish 2; after H5: 1, 0, 1, 1.
11. **A part that misses its keep rule is reverted before its task's commit.** The implementer reports the numbers and stops before committing. The controller applies the rule and says which parts to keep. Reverted parts are recorded in `## Outcome`.
12. **Object skip is O(start).** `serde_json::map::Iter` doesn't forward `nth`, so `entries_from` on an object walks `start` cheap steps. An array slice starts at `start` directly.
13. **The C1 scan (wave 1) doesn't match the new App fields.** It matches `HashMap<String`, `HashSet<String`, `Option<String`, `Option<(String` and `Vec<String`. `jar_dirty: std::collections::BTreeMap<jar::JarKey, std::time::Instant>` and `jar_saves: std::sync::atomic::AtomicUsize` match none, so `NOT_FORGOTTEN` needs no entry.
14. **Task order and file overlap.**
    - Tasks 0, 1, 2 and 9, 10 all touch the fixture or `graph_ui.rs` tests, so they run in that order.
    - Tasks 3, 4 and 14 edit `json_level_ui.rs`, `json_view.rs` and `json_level.rs`, so they run in order.
    - Tasks 5 and 6 edit `main.rs` tests and `tabs.rs`; Tasks 12 and 13 edit `session_sync.rs` and `journal.rs`.
    - Run the tasks one at a time, in order.

## Per-task summary

| # | Item | Implementer | Gate if landed alone | Depends on | Proof |
|---|---|---|---|---|---|
| 0 | `bench.rs`, `level_pass_cost` uses it | haiku | light | none | `level_pass_cost` runs and prints both levels; clippy clean |
| 1 | H3 pins: reference, golden, `settle_cost` | sonnet | light | 0 | golden fails with the reference's `CARD_GAP + 0.001`, passes as copied |
| 2 | H3 size index | haiku | light | 1, proceed | golden still passes; `settle_cost` g0 numbers |
| 3 | H1a pins: painted golden, `reveal_bounds_rows`, Tab walk, `TIME_ONLY` and reveal modes, baselines | sonnet | light | none | goldens pass; three baselines recorded |
| 4 | H1a change | opus | light | 3, proceed | goldens and Tab walk pass; culled test; `reveal-numbers` after |
| 5 | Jar pins: `jar_saves`, burst, ephemeral-dirty, `jar_save_cost` | sonnet | light | none | burst counts 15 saves today |
| 6 | Jar H3 flush | opus | **full** | 5, proceed | burst saves 0 then 1; timing test; `jar-probe.sh` passes |
| 7 | M1 pins: `reference_search`, `search_matches_reference`, `json_search_cost` | sonnet | light | none | reference test passes; fails with a mutated reference |
| 8 | M1 DFS | opus | light | 7, proceed | reference test passes; both queries' numbers |
| 9 | H2 pins: property test, `link_query_cost`, `graph_frame_cost`, badge test, teardown pin, `teardown_cost` | sonnet | light | 0 | property test passes and makes ≥ 15 links |
| 10 | H2 change: hoist, `edge_menu`, binary search ×2, slot map | sonnet | light | 9 | property and badge tests pass; three keep rules |
| 11 | Teardown order | haiku | **full** | 9 | teardown pin passes; `teardown_cost` after |
| 12 | H5 pins: `begins` counter, `save_count_per_action`, `app_save_costs` at 2000 | sonnet | light | none | counts 2, 1, 2, 2 today |
| 13 | H5 fast path | opus | **full** | 12, proceed | counts 1, 0, 1, 1; remote-frame test; numbers |
| 14 | H1b windowed paging (conditional) | opus | **full** | 4 (or its Step 0), run rule | `reveal_bounds_rows` 22 cards; round trip; `reveal-objects` after |
| 15 | Mirror link index (conditional) | opus | **full** | 10, run rule | property test; `link_query_cost` after |
| 16 | Wave gate, docs, and the merge question | controller | **full** | 0-15 | full `cargo test`, `scripts/gate.sh LOG full`, CPU and Defender reported |

The haiku tasks are mechanical, with the complete code given here. Each gets a coverage review: the reviewer checks that every step's code landed, that the proof step was run, and that nothing else changed.

Decision points for the controller (after the task named, before the next):
- after 1: Task 2 runs only if `settle_cost` g0 best > 8 ms; otherwise skip it and record why;
- after 3: Task 4 runs only if `reveal-numbers` > 1.2 × the pan baseline;
- after 4: Task 14 runs only if `reveal-objects` > 1.5 × the pan baseline. If Task 4 was skipped or reverted, the trigger compares Task 3's `reveal-objects` baseline instead, and Task 14 then either runs with its Step 0 (land Task 4's helpers alone) or is parked with that number recorded in `## Outcome` (see Task 14);
- after 5: Task 6 runs only if the burst counted > 1 save and `jar_save_cost` best > 3.2 ms;
- after 7: Task 8 runs only if either query's best > 8 ms;
- after 10: Task 15 runs only if `valid_targets` at L=100 is still > 1 ms;
- after 12: Task 13 runs only if the no-op save at 2000 tabs is > 0.3 ms.

---

### Task 0: The shared bench module

**Model:** haiku. **Gate alone:** light. **Proof:** `level_pass_cost` runs over the moved fixture and prints both levels; clippy is clean.

**Files:**
- Create: `crates/manifold/src/bench.rs`
- Modify: `crates/manifold/src/main.rs:1` (module list)
- Modify: `crates/manifold/src/graph_nav.rs:515-697` (tests)

**Interfaces:**
- Consumes:
  - `App::alloc_node_id(&mut self) -> String`
  - `App::test_node(&mut self, id: &str, kind: Kind, runtime: Runtime)` (`node.rs:443`, `cfg(test)`)
  - `App::edit_group(&mut self, id: &str, group: Option<String>)`
  - `EngineTab::fake(EngineKind) -> (EngineTab, MessageChannel<TcpStream>)`
- Produces:
  - `crate::bench::best_median(Vec<Duration>) -> (Duration, Duration)`
  - `crate::bench::timed(n: usize, f: impl FnMut()) -> (Duration, Duration)`
  - `crate::bench::report(what: &str, (Duration, Duration))`
  - `crate::bench::Fixture { app, groups, nodes, _peers }`
  - `crate::bench::level_fixture(live_tabs: bool) -> Fixture`

- [ ] **Step 1: Create `bench.rs`.**

```rust
//! Timing helpers and fixtures shared by the `#[ignore]` timing tests (wave 2 "Measuring"):
//! `cargo test --release -j 8 -p manifold --bins <name> -- --ignored --nocapture`.

use std::time::{Duration, Instant};

use crate::node::{
    DownloadPayload, ImagePayload, JsonPayload, NotePayload, PdfPayload, Runtime, TabPayload, UnloadedState,
    UnloadedTabPayload,
};
use crate::process_manager::{EngineKind, EngineTab};
use crate::store::model::Kind;
use crate::App;

/// The best and the median of `times`.
pub(crate) fn best_median(mut times: Vec<Duration>) -> (Duration, Duration) {
    times.sort();
    (times[0], times[times.len() / 2])
}

/// Runs `f` `n` times: the best and the median.
pub(crate) fn timed(n: usize, mut f: impl FnMut()) -> (Duration, Duration) {
    best_median(
        (0..n)
            .map(|_| {
                let start = Instant::now();
                f();
                start.elapsed()
            })
            .collect(),
    )
}

pub(crate) fn report(what: &str, (best, median): (Duration, Duration)) {
    println!("  {what:<44} best {best:>10.1?}  median {median:>10.1?}");
}

/// `level_fixture`'s app, with its ids.
pub(crate) struct Fixture {
    pub(crate) app: App,
    /// The eight groups, in creation order.
    pub(crate) groups: Vec<String>,
    /// The 2000 nodes, in creation order.
    pub(crate) nodes: Vec<String>,
    /// The live tabs' peers, kept so their sockets stay open.
    _peers: Vec<ipc_protocol::MessageChannel<std::net::TcpStream>>,
}

/// 2000 nodes of every kind, with store ids (`alloc_node_id`): every third at the top level, the rest spread over
/// eight groups. Each node's kind cycles through the constructible payloads. With `live_tabs`, every 280th (8 of
/// 2000) is a live tab (`EngineTab::fake`); without, those are sleeping tabs too.
pub(crate) fn level_fixture(live_tabs: bool) -> Fixture {
    let mut app = App::default();
    let mut peers = Vec::new();
    let groups: Vec<String> = (0..8)
        .map(|_| {
            let id = app.alloc_node_id();
            app.test_node(&id, Kind::Group, Runtime::None);
            id
        })
        .collect();
    let mut nodes = Vec::new();
    // `usize` spelled out: a method call (`is_multiple_of`) on a still-ambiguous integer is error E0689.
    for i in 0..2000usize {
        let id = app.alloc_node_id();
        let (kind, payload) = match i % 7 {
            0 if live_tabs && i.is_multiple_of(40) => {
                let (tab, peer) = EngineTab::fake(EngineKind::Servo);
                peers.push(peer);
                let live = TabPayload {
                    tab,
                    url: "https://a/".into(),
                    title: String::new(),
                    can_go_back: false,
                    can_go_forward: false,
                    loading: false,
                    audio_playing: false,
                    muted: false,
                    zoom_factor: 1.0,
                    swap_target: None,
                };
                (Kind::Tab, Runtime::Tab(live))
            }
            0 => {
                let sleeping = UnloadedTabPayload {
                    url: "https://s/".into(),
                    title: String::new(),
                    muted: false,
                    zoom_factor: 1.0,
                    state: UnloadedState::Sleeping,
                };
                (Kind::Tab, Runtime::Unloaded(sleeping))
            }
            1 => {
                let download = DownloadPayload {
                    path: "f.bin".into(),
                    bytes_received: 0,
                    total_bytes: None,
                    completed: true,
                    failed: false,
                };
                (Kind::Download, Runtime::Download(download))
            }
            2 => (Kind::Note, Runtime::Note(NotePayload { text: "t".into(), editing: false })),
            3 => (Kind::Image, Runtime::Image(ImagePayload { path: "i.png".into(), width: 1, height: 1, error: None })),
            4 => (Kind::Pdf, Runtime::Pdf(PdfPayload { path: "d.pdf".into(), outcome: None })),
            5 => {
                let json = JsonPayload { path: "a.json".into(), value: Default::default(), leaf: false, error: None };
                (Kind::Json, Runtime::Json(json))
            }
            _ => (Kind::Group, Runtime::None),
        };
        app.test_node(&id, kind, payload);
        if !i.is_multiple_of(3) {
            app.edit_group(&id, Some(groups[i % 8].clone()));
        }
        nodes.push(id);
    }
    Fixture { app, groups, nodes, _peers: peers }
}
```

- [ ] **Step 2: Declare the module.** In `main.rs`, before the first line `mod bookmarks;`, add:

```rust
#[cfg(test)]
mod bench;
```

- [ ] **Step 3: Move `level_pass_cost` onto it.** In `graph_nav.rs`'s tests module:
  - Delete `use crate::store::model::Kind;` (line 518) and the whole `fn level_fixture()` with its doc comment (lines 520-593).
  - In `level_pass_cost`, delete `use std::time::{Duration, Instant};` and the local `fn timed` and `fn report` (lines 604-618), and add `use crate::bench::{report, timed};` as its first line.
  - Replace these two lines:

```rust
        let (mut app, _peers) = level_fixture();
        for level in [None, Some("g0".to_string())] {
```

  with:

```rust
        let f = crate::bench::level_fixture(true);
        assert_eq!((f.groups.len(), f.nodes.len()), (8, 2000));
        let levels = [None, Some(f.groups[0].clone())];
        let mut app = f.app; // `f`'s other fields (the live tabs' peers) live to the end of the test
        for level in levels {
```

- [ ] **Step 4: Check.** Run `cargo test -j 8 -p manifold --bins level_pass_cost -- --ignored --nocapture` (debug is enough here). Expect two `level …` lines (about 675 of 2008 nodes at the top, about 167 in the first group) and the test passing. Run `cargo test -j 8 -p manifold`, `cargo fmt --all` and the clippy line. Expect all pass and no warning.

- [ ] **Step 5: Commit.**

```bash
git add crates/manifold/src/bench.rs crates/manifold/src/main.rs crates/manifold/src/graph_nav.rs
git commit -m "Wave 2 (harness): shared bench helpers and the 2000-node fixture with store ids" -m "Co-Authored-By: <implementer's own line>"
```

---

### Task 1: H3 pins (reference, golden, `settle_cost`)

**Model:** sonnet. **Gate alone:** light. **Proof:** the golden fails when the reference's gap is `CARD_GAP + 0.001`, and passes with the reference as copied.

**Files:**
- Modify: `crates/manifold/src/graph_layout.rs` tests (after `use super::*;` at 199)
- Modify: `crates/manifold/src/bench.rs` (add `spread`)
- Modify: `crates/manifold/src/graph_nav.rs` tests

**Interfaces:**
- Consumes: `graph_layout::step_sized` (7 args, `graph_layout.rs:77`); `App::settle_current_level(&mut self)`; `App::snap_layout(&mut self, id: &str, pos: egui::Vec2)`; `Mirror::group(&self, id) -> Option<&str>`.
- Produces: `crate::bench::spread(f: &mut Fixture)`; tests `step_sized_matches_reference_bit_for_bit`, `settle_cost`.

- [ ] **Step 1: Copy the reference.** In `graph_layout.rs`'s tests module, add a copy of today's `step_sized` (lines 77-176, its body unchanged) named `step_sized_reference`, with this doc comment: `/// Today's \`step_sized\`, kept as the golden test's reference (wave 2, H3).` It has 7 arguments, so clippy's `too_many_arguments` (over 7) does not fire.

- [ ] **Step 2: The golden test.** Add after it:

```rust
    /// Wave 2 (H3): `step_sized` over 40 cards for 90 settle steps matches today's copy bit for bit: sized and unsized
    /// cards, two on one point (the `spawn_jitter` direction), springs, and one pinned card.
    #[test]
    fn step_sized_matches_reference_bit_for_bit() {
        let ids: Vec<String> = (0..40).map(|i| format!("c{i:02}")).collect();
        let mut positions: HashMap<String, egui::Vec2> = ids
            .iter()
            .enumerate()
            .map(|(i, id)| (id.clone(), egui::vec2((i % 8) as f32 * 150.0, (i / 8) as f32 * 90.0)))
            .collect();
        let c00 = positions["c00"];
        positions.insert("c01".into(), c00);
        let mut velocities: HashMap<String, egui::Vec2> = ids.iter().map(|id| (id.clone(), egui::Vec2::ZERO)).collect();
        let sizes: HashMap<String, egui::Vec2> =
            (0..40).filter(|i: &usize| !i.is_multiple_of(5)).map(|i| (ids[i].clone(), egui::vec2(160.0 + i as f32, 100.0))).collect();
        let edges: Vec<(String, String)> = (1..13).map(|i| ("c00".to_string(), ids[3 * i].clone())).collect();
        let pinned = HashSet::from(["c07".to_string()]);
        let (mut ref_positions, mut ref_velocities) = (positions.clone(), velocities.clone());
        for _ in 0..90 {
            step_sized(&mut positions, &mut velocities, &edges, &pinned, &sizes, egui::Vec2::ZERO, 1.0 / 60.0);
            step_sized_reference(
                &mut ref_positions,
                &mut ref_velocities,
                &edges,
                &pinned,
                &sizes,
                egui::Vec2::ZERO,
                1.0 / 60.0,
            );
        }
        let bits = |map: &HashMap<String, egui::Vec2>| {
            let mut all: Vec<(String, u32, u32)> =
                map.iter().map(|(id, v)| (id.clone(), v.x.to_bits(), v.y.to_bits())).collect();
            all.sort();
            all
        };
        assert_eq!(bits(&positions), bits(&ref_positions));
        assert_eq!(bits(&velocities), bits(&ref_velocities));
        assert_ne!(positions["c00"], positions["c01"], "the pair on one point pushed apart");
    }
```

- [ ] **Step 3: Prove it can fail.** In `step_sized_reference` only, change `egui::Vec2::splat(CARD_GAP)` to `egui::Vec2::splat(CARD_GAP + 0.001)`. Run `cargo test -j 8 -p manifold --bins step_sized_matches_reference`. Expect FAIL on the positions. Revert the mutation and rerun: expect PASS. (A commutative swap or `* 0.5` stays bit-identical, so it would not prove anything.)

- [ ] **Step 4: Add `spread` to `bench.rs`.** Add `use std::collections::HashMap;` to its imports, and:

```rust
/// Spreads each level's cards on a grid, 30 to a row, 300 by 200 apart, so a settle starts from distinct positions.
pub(crate) fn spread(f: &mut Fixture) {
    let mut next: HashMap<Option<String>, usize> = HashMap::new();
    for id in f.groups.iter().chain(&f.nodes) {
        let k = next.entry(f.app.mirror.group(id).map(str::to_owned)).or_default();
        let pos = egui::vec2((*k % 30) as f32 * 300.0, (*k / 30) as f32 * 200.0);
        *k += 1;
        f.app.snap_layout(id, pos);
    }
}
```

- [ ] **Step 5: `settle_cost`.** In `graph_nav.rs`'s tests module, after `level_pass_cost`:

```rust
    /// Wave 2 (H3): one `settle_current_level` (90 steps of the O(n²) pair loop) at the top level (about 675 cards)
    /// and in the first group (about 167), from a grid. Timing, not correctness, so ignored:
    /// `cargo test --release -j 8 -p manifold --bins settle_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn settle_cost() {
        use crate::bench::{level_fixture, report, spread, timed};
        let mut f = level_fixture(false);
        spread(&mut f);
        let levels = [None, Some(f.groups[0].clone())];
        let mut app = f.app;
        let start = app.layout.clone();
        for level in levels {
            app.current_group = level.clone();
            let cards = app.nodes.keys().filter(|id| app.in_level(id)).count();
            let what = format!("settle, {} ({cards} cards)", if level.is_some() { "g0" } else { "top" });
            report(
                &what,
                timed(10, || {
                    app.layout = start.clone();
                    app.settle_current_level();
                }),
            );
        }
    }
```

- [ ] **Step 6: Check and measure.** Run `cargo test -j 8 -p manifold`, `cargo fmt --all` and the clippy line. Then, after the CPU and Defender checks, run `cargo test --release -j 8 -p manifold --bins settle_cost -- --ignored --nocapture`. Report the raw output (best and median at both levels), the CPU reading and the Defender state.

- [ ] **Step 7: Commit.**

```bash
git add crates/manifold/src/graph_layout.rs crates/manifold/src/bench.rs crates/manifold/src/graph_nav.rs
git commit -m "Wave 2 (H3 pins): step_sized golden against today's copy, and settle_cost" -m "Co-Authored-By: <implementer's own line>"
```

---

### Task 2: H3 size index

**Model:** haiku. **Gate alone:** light. **Proof:** the golden still passes; `settle_cost`'s g0 best before and after.

Runs only if Task 1's g0 best was over 8 ms.

**Files:**
- Modify: `crates/manifold/src/graph_layout.rs:89-116` (`step_sized`)

**Interfaces:**
- Consumes: nothing new. Produces: nothing new (same signature).

- [ ] **Step 1: Build the index once per step.** In `step_sized` (not the reference), after the line `let pos: Vec<egui::Vec2> = ids.iter().map(|id| positions[id]).collect();`, add:

```rust
    // Each card's size, indexed like `pos`: looked up once per step, not twice per pair.
    let size: Vec<Option<egui::Vec2>> = ids.iter().map(|id| sizes.get(id).copied()).collect();
```

- [ ] **Step 2: Read it in the pair loop.** Replace:

```rust
            let (Some(a), Some(b)) = (sizes.get(&ids[i]), sizes.get(&ids[j])) else { continue };
            let reach = (*a + *b) / 2.0 + egui::Vec2::splat(CARD_GAP);
```

with:

```rust
            let (Some(a), Some(b)) = (size[i], size[j]) else { continue };
            let reach = (a + b) / 2.0 + egui::Vec2::splat(CARD_GAP);
```

- [ ] **Step 3: Check and measure.** Run `cargo test -j 8 -p manifold --bins step_sized_matches_reference` (expect PASS), the whole suite, `cargo fmt --all` and the clippy line. Then the CPU and Defender checks and `cargo test --release -j 8 -p manifold --bins settle_cost -- --ignored --nocapture`. Report the raw output and stop before committing (Decision 11). Keep rule: g0's best drops by 25% or more.

- [ ] **Step 4: Commit (if kept).**

```bash
git add crates/manifold/src/graph_layout.rs
git commit -m "Wave 2 (H3): settle reads card sizes from a per-step index" -m "Co-Authored-By: <implementer's own line>"
```

---

### Task 3: H1a pins and the JSON timing harness

**Model:** sonnet. **Gate alone:** light. **Proof:** the painted golden, `reveal_bounds_rows` and the Tab walk pass today; the three baselines are recorded.

**Files:**
- Modify: `crates/manifold/src/json_level_ui.rs` tests (after the imports at about 918)
- Modify: `crates/manifold/src/json_view.rs` tests (after `use serde_json::json;` at about 543)
- Modify: `crates/manifold/src/json_level.rs` tests
- Modify: `scripts/json-pan-profile.sh`, `scripts/json-pan-profile.py`

**Interfaces:**
- Consumes:
  - `paint_json_content(painter, overlays, rect, value, header: &HeaderParts, rows, more, look: &CardLook)` (`json_level_ui.rs:263`)
  - `header_parts(root: &Value, card: &Card, root_name: &str) -> HeaderParts` (`:162`)
  - `json_view::{reveal, visible_cards, default_expanded, JsonView}`
  - `App::build_graph_ui(&mut self, ui)`, `App::switch_group_level(Option<String>)`, `Camera::jump_to(Rect)`
- Produces: `painted_texts` (test helper), tests `a_card_paints_its_rows_texts`, `reveal_bounds_rows`, `tab_walks_the_json_cards_and_focus_survives_a_pan`; script modes `reveal-numbers`, `reveal-objects` and `TIME_ONLY=1`.

- [ ] **Step 1: The painted-text golden.** In `json_level_ui.rs`'s tests module:

```rust
    /// The texts one level card paints, in order: header, rows, `+ N more`.
    fn painted_texts(value: &Value, card: &Card) -> Vec<String> {
        let ctx = egui::Context::default();
        crate::theme::apply_fonts(&ctx);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let mut overlays: DeferredOverlays = Vec::new();
            let pal = palette(true);
            let look = CardLook {
                palette: &pal,
                text_color: egui::Color32::WHITE,
                inv_zoom: 1.0,
                text: true,
                selected_row: None,
                hit_rows: &[],
                ports: true,
                preview: false,
            };
            let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(json_view::CARD_WIDTH, card.height()));
            let header = header_parts(value, card, "f.json");
            paint_json_content(ui.painter(), &mut overlays, rect, value, &header, card.rows, card.more, &look);
            for overlay in overlays {
                overlay(ui.painter(), &|p| p);
            }
        });
        output.textures_delta.clear();
        output
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                _ => None,
            })
            .collect()
    }

    /// Wave 2 (H1a): a card's painted texts, pinned before rows are culled to the camera.
    #[test]
    fn a_card_paints_its_rows_texts() {
        let value = Value::Array((0..30).map(|i| Value::String(format!("v{i}"))).collect());
        let card = Card { pointer: String::new(), parent: None, depth: 0, collapsed: false, rows: 25, more: 5 };
        let texts = painted_texts(&value, &card);
        assert_eq!(texts.len(), 54, "{texts:?}");
        assert_eq!(texts[1..5], ["f.json", "[30]", "0", "\"v0\""], "{texts:?}");
        assert_eq!(texts[53], "+ 5 more");
    }
```

  If today's texts differ from these expectations (an icon or label rule read wrong here), pin today's actual texts instead and say so in the report. The test pins what is painted now.

- [ ] **Step 2: `reveal_bounds_rows`.** In `json_view.rs`'s tests module:

```rust
    /// Wave 2 (H1a): revealing the last of 20 000 objects pages its array to every row, so the level holds a card per
    /// object (H1b, if it lands, bounds this to a window).
    #[test]
    fn reveal_bounds_rows() {
        let root = json!({"items": (0..20_000).map(|n| json!({"n": n})).collect::<Vec<_>>()});
        let expanded = default_expanded(&root);
        let mut view = JsonView::default();
        assert!(reveal(&root, &mut view, &expanded, "/items/19999"));
        let cards = visible_cards(&root, &view, &expanded);
        let items = cards.iter().find(|c| c.pointer == "/items").expect("the array's card");
        assert_eq!((cards.len(), items.rows, items.more), (20_002, 20_000, 0));
    }
```

- [ ] **Step 3: The Tab walk.** In `json_level.rs`'s tests module:

```rust
    /// Wave 2 (H1a): Tab and Shift+Tab walk the JSON cards through real frames, and a focused card panned out of view
    /// keeps its focus. Pins the focus reads before they move out of the per-card loop.
    #[test]
    fn tab_walks_the_json_cards_and_focus_survives_a_pan() {
        let (mut app, j) = app_with_json("tab-walk", r#"{"a": {"x": 1}, "b": {"y": 2}, "c": {"z": 3}}"#);
        app.switch_group_level(Some(j));
        let ctx = egui::Context::default();
        crate::theme::apply_fonts(&ctx);
        let frame = |app: &mut App, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024.0, 768.0))),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| app.build_graph_ui(ui));
            output.textures_delta.clear();
        };
        let tab = |shift: bool| egui::Event::Key {
            key: egui::Key::Tab,
            physical_key: Some(egui::Key::Tab),
            pressed: true,
            repeat: false,
            modifiers: if shift { egui::Modifiers::SHIFT } else { egui::Modifiers::NONE },
        };
        let card = |app: &App, i: usize| {
            let level = app.json_level.as_ref().unwrap();
            egui::Id::new(("json_card", &level.node_id, &level.cards[i].pointer))
        };
        for _ in 0..3 {
            frame(&mut app, vec![]);
        }
        assert_eq!(app.json_level.as_ref().unwrap().cards.len(), 4, "root, a, b, c");
        ctx.memory_mut(|m| m.request_focus(card(&app, 0)));
        frame(&mut app, vec![]);
        let focused = || ctx.memory(|m| m.focused());
        assert_eq!(focused(), Some(card(&app, 0)));
        // An idle frame follows each key: egui applies a card's Tab lock filter only once it has held focus for a
        // frame (`set_focus_lock_filter` checks `had_focus_last_frame`), so a key in the very next frame would also
        // run egui's own Tab move and skip a card. That consecutive-frame skip is today's behavior, not pinned here.
        frame(&mut app, vec![tab(false)]);
        assert_eq!(focused(), Some(card(&app, 1)), "Tab: the next card");
        frame(&mut app, vec![]);
        assert_eq!(focused(), Some(card(&app, 1)), "an idle frame keeps it");
        frame(&mut app, vec![tab(false)]);
        assert_eq!(focused(), Some(card(&app, 2)));
        frame(&mut app, vec![]);
        frame(&mut app, vec![tab(true)]);
        assert_eq!(focused(), Some(card(&app, 1)), "Shift+Tab: the previous card");
        frame(&mut app, vec![]);
        assert_eq!(focused(), Some(card(&app, 1)));
        app.camera.jump_to(egui::Rect::from_min_size(egui::pos2(50_000.0, 50_000.0), egui::vec2(800.0, 600.0)));
        frame(&mut app, vec![]);
        frame(&mut app, vec![]);
        assert_eq!(focused(), Some(card(&app, 1)), "a focused card panned out of view keeps its focus");
    }
```

- [ ] **Step 4: Run the three pins.** `cargo test -j 8 -p manifold --bins <name>` takes one filter, so run it once for each of `a_card_paints_its_rows_texts`, `reveal_bounds_rows` and `tab_walks_the_json_cards`. Expect all PASS on today's code. A failure is a finding (Global Constraints, Tests): report it.

- [ ] **Step 5: Reveal fixtures in `json-pan-profile.py`.**
  - Docstring: `fixture <out.json> [mode]` and `script <fixture.json> <out.txt> <seconds> [pan|zoom|reveal-numbers|reveal-objects]`, with one paragraph: the reveal modes open `{"items": [...]}` of `REVEAL_N` (default 200000) numbers or `leaf` objects, select the last entry (which pages its array to every row), then pan as `pan` does.
  - Add `REVEAL_N = int(os.environ.get("REVEAL_N", "200000"))` under `BRANCH`.
  - `cmd_fixture`:

```python
def cmd_fixture(args):
    out = args[0]
    mode = args[1] if len(args) > 1 else "pan"
    if mode == "reveal-numbers":
        value = {"items": [n * 0.37 for n in range(REVEAL_N)]}
    elif mode == "reveal-objects":
        value = {"items": [leaf(n) for n in range(REVEAL_N)]}
    else:
        value = tree(3)
    with open(out, "w", encoding="utf-8") as f:
        json.dump(value, f)
```

  - `cmd_script`: replace the depth-2 loop with the block below. A JSONSELECT that finds no level does nothing and says nothing, so a reveal run writes `EXPORTSTATE` beside the fixture, and the `.sh` reads its `selected` back (Step 6). The first `WAITIDLE` gives the 200 000-entry file time to load (it waits on the focused tab's loading flag, which a Json card's file load may never raise: check on the first run that the double-click finds the card, else go back to `SLEEP 5`) before the double-click enters it; it is the same in both runs of a pair, so it cancels.

```python
    if mode.startswith("reveal-"):
        lines[2] = "WAITIDLE 60"  # after ADDFILE: a big file loads before the double-click
        lines.append(f"JSONSELECT /items/{REVEAL_N - 1}")  # pages the array to every row
        lines.append("WAITIDLE 60")
        lines.append("EXPORTSTATE " + os.path.join(os.path.dirname(fixture), "state.json"))
    else:
        for i in range(BRANCH):
            for j in range(BRANCH):
                lines.append(f"JSONSELECT /k{i}/k{j}/k0")  # reveals (opens) the depth-2 card
```

    and make the final branch `if mode == "zoom": … else:` cover `pan` and both reveal modes (the pan loop, unchanged).

- [ ] **Step 6: `TIME_ONLY` in `json-pan-profile.sh`.**
  - Usage line: `# Usage: [SECONDS_PANNING=20] [MODE=pan|zoom|reveal-numbers|reveal-objects] [REVEAL_N=200000] [TIME_ONLY=1] scripts/json-pan-profile.sh`.
  - A header paragraph:

```bash
# TIME_ONLY=1: no samply. Runs the copied exe once, in the foreground, and appends one line to
# target/json-time.log: `TIME_ONLY mode=… seconds=… commands=… ms=… exit=…`. The cost per command is the
# difference between two runs of different SECONDS_PANNING, divided by the difference in commands, so the
# setup cancels out. Launch detached at BelowNormal (children inherit it):
#   TIME_ONLY=1 MODE=pan SECONDS_PANNING=10 cmd //c start "" //b //belownormal bash scripts/json-pan-profile.sh
# and wait for the line before starting the next run (each run clears target/json-profile).
```

  - Pass the mode to the fixture: `python -P "$repo/scripts/json-pan-profile.py" fixture "$out/fixture.json" "${MODE:-pan}" </dev/null`.
  - After `cp "$exe" "$out/manifold.exe"`, before the `powershell` line:

```bash
if [ -n "${TIME_ONLY:-}" ]; then
    commands=$(wc -l <"$out/commands.txt")
    start=$(date +%s%N)
    status=0
    "$out/manifold.exe" --headless --profile-dir "$(cygpath -w "$out/appdata")" \
        --script "$(cygpath -w "$out/commands.txt")" >"$out/run.log" 2>&1 || status=$?
    ms=$(( ($(date +%s%N) - start) / 1000000 ))
    # A reveal run's selection, from its EXPORTSTATE: empty means the level never opened and the run timed nothing.
    selected=$(grep -o '"selected": *"[^"]*"' "$out/state.json" 2>/dev/null | sed 's/.*"\([^"]*\)"$/\1/' || true)
    echo "TIME_ONLY mode=${MODE:-pan} seconds=${SECONDS_PANNING:-20} commands=$commands ms=$ms exit=$status selected=$selected" \
        | tee -a "$repo/target/json-time.log"
    exit 0
fi
```

  - Run `bash -n scripts/json-pan-profile.sh`. Expect no output.

- [ ] **Step 7: Baselines.** `df -h /c`, then `cargo build --release -j 8 -p manifold`. Then, for each pair below, the CPU and Defender checks, and two runs, each started only after the previous line reached `target/json-time.log`:
  - `MODE=pan`, `SECONDS_PANNING=10` and `20`;
  - `MODE=reveal-numbers`, `SECONDS_PANNING=1` and `2`;
  - `MODE=reveal-objects`, `SECONDS_PANNING=1` and `2`.

  Each run is `TIME_ONLY=1 MODE=<m> SECONDS_PANNING=<s> cmd //c start "" //b //belownormal bash scripts/json-pan-profile.sh`. Every line must say `exit=0`, and every reveal line `selected=/items/199999` (with the default `REVEAL_N`). A reveal line with an empty `selected` timed an unopened level: raise the `SLEEP 5`, rerun, and report it. Per mode, cost per command = Δms / Δcommands. Report the six raw lines and the three costs. The pan cost is the baseline (about 33 ms on 2026-10-01).

- [ ] **Step 8: Check and commit.** Run `cargo test -j 8 -p manifold`, `cargo fmt --all` and the clippy line.

```bash
git add crates/manifold/src/json_level_ui.rs crates/manifold/src/json_view.rs crates/manifold/src/json_level.rs scripts/json-pan-profile.sh scripts/json-pan-profile.py
git commit -m "Wave 2 (H1a pins): painted-text golden, reveal size, Tab walk, and TIME_ONLY reveal timing" -m "Co-Authored-By: <implementer's own line>"
```

---

### Task 4: H1a. Rows culled to the camera

**Model:** opus. **Gate alone:** light. **Proof:**
- the golden, `reveal_bounds_rows`, the Tab walk and the existing reveal test (`/users/33` gives pages 2) pass;
- the new culled test and unit tests pass;
- `reveal-numbers` after.

Runs only if Task 3's `reveal-numbers` cost was over 1.2 times the pan baseline.

**Files:**
- Modify: `crates/manifold/src/json_view.rs` (`entries` at 244, new helpers, imports at 7, tests)
- Modify: `crates/manifold/src/json_level.rs` (`Search` at 51-63, the three `search.result =` writes, tests)
- Modify: `crates/manifold/src/json_level_ui.rs` (`CardLook` at 223-239, `paint_json_content` at 263-446, `draw_json_level` at 459-560, tests)
- Modify: `crates/manifold/src/card.rs:1845-1856` (the preview's `CardLook`)

**Interfaces:**
- Consumes: `egui::Memory::focused(&self) -> Option<egui::Id>`; `egui::Rect::EVERYTHING`.
- Produces:
  - `json_view::entries_from(value: &Value, start: usize) -> Box<dyn Iterator<Item = (String, &Value)> + '_>`
  - `json_view::visible_rows(rows_top: f32, rows: usize, visible: egui::Rect) -> std::ops::Range<usize>`
  - `json_view::row_of(value: &Value, card: &str, pointer: &str, window: std::ops::Range<usize>) -> Option<usize>`
  - `json_view::hit_rows_by_parent(root: &Value, hits: &HashSet<String>) -> HashMap<String, Vec<usize>>`
  - `Search::hit_rows: HashMap<String, Vec<usize>>`, `Search::set_result(&mut self, root: &Value, result: Option<SearchResult>)`
  - `CardLook::visible: egui::Rect`

- [ ] **Step 1: Write the failing tests.**
  - In `json_view.rs`'s tests module:

```rust
    #[test]
    fn visible_rows_cover_the_rows_the_rect_touches() {
        let r = |top, bottom| egui::Rect::from_min_max(egui::pos2(0.0, top), egui::pos2(220.0, bottom));
        assert_eq!(visible_rows(26.0, 25, r(137.0, 245.0)), 5..10);
        assert_eq!(visible_rows(26.0, 25, egui::Rect::EVERYTHING), 0..25, "the preview: every row");
        assert_eq!(visible_rows(26.0, 25, r(-500.0, 0.0)), 0..0, "above the rows");
        assert_eq!(visible_rows(26.0, 25, r(5000.0, 6000.0)), 25..25, "below them");
    }

    #[test]
    fn row_of_and_hit_rows_take_canonical_children_only() {
        let root = json!({"a~b": {"x/y": 1, "z": 2}, "arr": [10, 20, 30]});
        assert_eq!(row_of(&root["a~b"], "/a~0b", "/a~0b/x~1y", 0..2), Some(0));
        assert_eq!(row_of(&root["a~b"], "/a~0b", "/a~0b/z", 0..1), None, "outside the window");
        assert_eq!(row_of(&root["arr"], "/arr", "/arr/2", 0..3), Some(2));
        assert_eq!(row_of(&root["arr"], "/arr", "/arr/02", 0..3), None, "not canonical");
        assert_eq!(row_of(&root["arr"], "/arr", "/arr", 0..3), None, "the card itself");
        assert_eq!(row_of(&root, "", "/arr/2", 0..2), None, "a grandchild");
        let hits = HashSet::from(["/a~0b/z", "/arr/0", "/arr/2", "/arr/02"].map(String::from));
        let rows = hit_rows_by_parent(&root, &hits);
        assert_eq!(rows.get("/a~0b"), Some(&vec![1]));
        assert_eq!(rows.get("/arr"), Some(&vec![0, 2]));
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn entries_from_starts_at_an_entry() {
        let root = json!({"o": {"a": 1, "b": 2, "c": 3}, "arr": [10, 20, 30]});
        let keys = |v: &Value, start| entries_from(v, start).map(|(k, _)| k).collect::<Vec<_>>();
        assert_eq!(keys(&root["o"], 1), ["b", "c"]);
        assert_eq!(keys(&root["arr"], 2), ["2"]);
        assert!(keys(&root["arr"], 9).is_empty());
    }
```

  - In `json_level.rs`'s tests module:

```rust
    /// Wave 2 (H1a): a search's hit rows are built with its result, per parent card, and go with it.
    #[test]
    fn a_search_builds_its_hit_rows_per_card() {
        let (mut app, j) = app_with_json("hit-rows", r#"{"a": {"b": 1}, "c": [1, 2]}"#);
        app.switch_group_level(Some(j));
        app.set_json_search("1".into());
        let rows = |app: &App| app.json_level.as_ref().unwrap().search.hit_rows.clone();
        assert_eq!(rows(&app), HashMap::from([("/a".to_string(), vec![0]), ("/c".to_string(), vec![0])]));
        app.set_json_search(String::new());
        assert!(rows(&app).is_empty());
    }
```

  - In `json_level_ui.rs`'s tests module, give `painted_texts` a `visible: egui::Rect` parameter (set `visible` in its `CardLook`; Step 4 adds the field), pass `egui::Rect::EVERYTHING` from `a_card_paints_its_rows_texts`, and add:

```rust
    /// Wave 2 (H1a): only the rows the camera shows are painted; the header and `+ N more` always are.
    #[test]
    fn a_culled_card_paints_only_its_visible_rows() {
        let value = Value::Array((0..30).map(|i| Value::String(format!("v{i}"))).collect());
        let card = Card { pointer: String::new(), parent: None, depth: 0, collapsed: false, rows: 25, more: 5 };
        let full = painted_texts(&value, &card, egui::Rect::EVERYTHING);
        let visible = egui::Rect::from_min_max(egui::pos2(0.0, 137.0), egui::pos2(220.0, 245.0)); // rows 5..10
        assert_eq!(painted_texts(&value, &card, visible), [&full[..3], &full[13..23], &full[53..]].concat());
    }
```

  Run `cargo test -j 8 -p manifold --bins json`. Expect compile errors: the helpers, `hit_rows` and `visible` don't exist yet.

- [ ] **Step 2: `json_view.rs` helpers.** Change the import to `use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};`. Replace `entries` (lines 243-250) with:

```rust
/// A container's entries in order, array indices as keys.
pub(crate) fn entries(value: &Value) -> Box<dyn Iterator<Item = (String, &Value)> + '_> {
    entries_from(value, 0)
}

/// `entries` from entry `start` on. An array starts there directly; an object steps past `start` entries
/// (`serde_json`'s map iterator has no fast `nth`).
pub(crate) fn entries_from(value: &Value, start: usize) -> Box<dyn Iterator<Item = (String, &Value)> + '_> {
    match value {
        Value::Object(map) => Box::new(map.iter().skip(start).map(|(k, v)| (k.clone(), v))),
        Value::Array(items) => {
            let start = start.min(items.len());
            Box::new(items[start..].iter().enumerate().map(move |(i, v)| ((start + i).to_string(), v)))
        }
        _ => Box::new(std::iter::empty()),
    }
}

/// The rows of a card whose first row's top is `rows_top` that intersect `visible` (world), as a range of its
/// `rows` rows. `Rect::EVERYTHING` gives every row: float-to-int casts saturate.
pub(crate) fn visible_rows(rows_top: f32, rows: usize, visible: egui::Rect) -> std::ops::Range<usize> {
    let first = ((visible.top() - rows_top) / ROW_HEIGHT).floor().max(0.0) as usize;
    let end = ((visible.bottom() - rows_top) / ROW_HEIGHT).ceil().max(0.0) as usize;
    first.min(rows)..end.min(rows)
}

/// The entry index of `pointer` on the card at `card` (`value` is the card's value), when `pointer` is a canonical
/// child of it (its own pointer as `child_pointer` writes it) within `window`.
pub(crate) fn row_of(value: &Value, card: &str, pointer: &str, window: std::ops::Range<usize>) -> Option<usize> {
    let token = pointer.strip_prefix(card)?.strip_prefix('/')?;
    if token.contains('/') {
        return None;
    }
    let key = token.replace("~1", "/").replace("~0", "~");
    let row = entry_index(value, &key).filter(|row| window.contains(row))?;
    let canonical = if value.is_array() { row.to_string() } else { key };
    (child_pointer(card, &canonical) == pointer).then_some(row)
}

/// Each search hit's row on its parent's card: sorted entry indices per parent pointer. A hit that is not a
/// canonical child pointer (none from `search_with` is) or that names no entry is left out; the root has no row.
pub(crate) fn hit_rows_by_parent(root: &Value, hits: &HashSet<String>) -> HashMap<String, Vec<usize>> {
    let mut keys: HashMap<&str, HashSet<String>> = HashMap::new();
    for hit in hits {
        let (Some(parent), Some(key)) = (parent_pointer(hit), last_key(hit)) else { continue };
        if child_pointer(parent, &key) == *hit {
            keys.entry(parent).or_default().insert(key);
        }
    }
    keys.into_iter()
        .filter_map(|(parent, keys)| {
            let mut rows: Vec<usize> = match root.pointer(parent)? {
                Value::Object(map) => {
                    map.keys().enumerate().filter(|(_, k)| keys.contains(*k)).map(|(i, _)| i).collect()
                }
                Value::Array(items) => keys
                    .iter()
                    .filter_map(|k| k.parse::<usize>().ok().filter(|&i| i < items.len() && i.to_string() == *k))
                    .collect(),
                _ => return None,
            };
            rows.sort_unstable();
            Some((parent.to_string(), rows))
        })
        .collect()
}
```

- [ ] **Step 3: `Search::hit_rows`.** In `json_level.rs`, add to `Search` after `result`:

```rust
    /// The result's hits as rows (`json_view::hit_rows_by_parent`): sorted entry indices per parent card. Set with
    /// `result` (`set_result`), so a frame never scans a card's rows for hits.
    pub(crate) hit_rows: HashMap<String, Vec<usize>>,
```

  (`HashMap` comes in through `use super::*;`; if the compiler says otherwise, add `use std::collections::HashMap;`), and an `impl Search` (after the `Drop` impl):

```rust
impl Search {
    /// Sets the result and the hit rows built from it, together.
    pub(crate) fn set_result(&mut self, root: &serde_json::Value, result: Option<SearchResult>) {
        self.hit_rows = result.as_ref().map(|r| json_view::hit_rows_by_parent(root, &r.hits)).unwrap_or_default();
        self.result = result;
    }
}
```

  Replace the three writes:
  - `level.search.result = None; // the old query's …` becomes `level.search.set_result(&level.value, None); // the old query's matches mustn't show or step while this one runs`;
  - `level.search.result = Some(json_view::search(&level.value, &query));` becomes `let result = json_view::search(&level.value, &query);` then `level.search.set_result(&level.value, Some(result));`;
  - in `receive_json_search`, `level.search.result = Some(result);` becomes `level.search.set_result(&level.value, Some(result));`. If the borrow checker refuses it there (the `if let Some((query, rx)) = &level.search.pending` binding), compute `let current = *query == level.search.query;` before the call and test `current`.

  Check with `rg -n 'search\.result =' crates/manifold/src`: expect no match.

- [ ] **Step 4: Paint only the visible rows.** In `json_level_ui.rs`:
  - `CardLook` gets, after `hit_rows`:

```rust
    /// The world rect shown (the camera's): only entry rows intersecting it are painted. `Rect::EVERYTHING`
    /// paints them all (the Json card's preview).
    pub(crate) visible: egui::Rect,
```

  - Update `hit_rows`'s doc to `/// Sorted entry rows that match the search.`
  - In `paint_json_content`, after `let row_rect = …;`, add `let shown = json_view::visible_rows(rect.top() + header_height, rows, look.visible);`.
  - Replace `for &row in look.hit_rows {` with:

```rust
    let (lo, hi) = (
        look.hit_rows.partition_point(|&r| r < shown.start),
        look.hit_rows.partition_point(|&r| r < shown.end),
    );
    for &row in &look.hit_rows[lo..hi] {
```

  - Replace `for (row, (key, child)) in json_view::entries(value).take(rows).enumerate() {` with:

```rust
    let first = shown.start;
    for (row, (key, child)) in json_view::entries_from(value, first).take(shown.len()).enumerate().map(|(i, e)| (first + i, e)) {
```

  - Update the doc comment's first lines to: `/// A card's contents (the frame is the caller's): the header, then those of the first \`rows\` entries of \`value\` that \`look.visible\` shows, then \`+ N more\` when \`more > 0\`.`

- [ ] **Step 5: `draw_json_level`.**
  - Replace `if let Some(at) = (0..level.cards.len()).find(|&i| ui.memory(|m| m.has_focus(card_id(i)))) {` with:

```rust
        // The focused widget, read again only where it can change (the Tab request, each drawn card's interact),
        // not once per card: `has_focus(id)` is `focused() == Some(id)`, and egui applies focus moves at once.
        let mut focused = ui.memory(|m| m.focused());
        if let Some(at) = (0..level.cards.len()).find(|&i| focused == Some(card_id(i))) {
```

  - After `tab_target = Some(next);` add `focused = ui.memory(|m| m.focused());`.
  - The cull: replace `!ui.memory(|m| m.has_focus(card_id(i)))` with `focused != Some(card_id(i))`.
  - Replace the `shown`, `selected_row` and `hit_rows` lines (517-525) with:

```rust
            let selected_row =
                level.selected.as_deref().and_then(|s| json_view::row_of(value, &card.pointer, s, 0..card.rows));
            // This card's hit rows, cut to the rows it shows.
            let hit_rows: &[usize] = level
                .search
                .hit_rows
                .get(&card.pointer)
                .map_or(&[][..], |rows| &rows[..rows.partition_point(|&r| r < card.rows)]);
```

  - The border keeps `|| !hit_rows.is_empty()`. In `CardLook`, `hit_rows: &hit_rows,` becomes `hit_rows,` and add `visible: self.camera.rect,`.
  - After the root-focus `if i == 0 && …` block, add `focused = ui.memory(|m| m.focused());`.

- [ ] **Step 6: The preview.** In `card.rs`'s preview `CardLook` (after `hit_rows: &[],`), add `visible: egui::Rect::EVERYTHING,`.

- [ ] **Step 7: Check.** Run `cargo test -j 8 -p manifold`. Expect all PASS, including `a_card_paints_its_rows_texts`, `a_culled_card_paints_only_its_visible_rows`, `tab_walks_the_json_cards_and_focus_survives_a_pan`, `reveal_bounds_rows` and the existing reveal test. Then `cargo fmt --all` and the clippy line.

- [ ] **Step 8: Measure.** `cargo build --release -j 8 -p manifold`, then the CPU and Defender checks and the `reveal-numbers` and `reveal-objects` pairs (1 and 2 s) as in Task 3 Step 7. Report the raw lines and costs, and stop before committing. Keep rule: `reveal-numbers` drops by 30% or more, or comes within 20% of the pan baseline. The `reveal-objects` cost decides Task 14.

- [ ] **Step 9: Commit (if kept).**

```bash
git add crates/manifold/src/json_view.rs crates/manifold/src/json_level.rs crates/manifold/src/json_level_ui.rs crates/manifold/src/card.rs
git commit -m "Wave 2 (H1a): JSON cards paint only the rows the camera shows" -m "Hit rows are built once per search result; the focused id is read where it can change." -m "Co-Authored-By: <implementer's own line>"
```

---

### Task 5: Jar pins

**Model:** sonnet. **Gate alone:** light (tests and a `cfg(test)` counter only). **Proof:** the burst counts 15 saves today, and the file holds the last value.

**Files:**
- Modify: `crates/manifold/src/main.rs` (field after `jars_dir_override` at 1505-1506, its initializer at 1682-1683, tests after `ephemeral_on_asks_first_only_when_the_jar_has_a_saved_file`)
- Modify: `crates/manifold/src/jar_defs.rs:689-697` (`save_jar`)
- Modify: `crates/manifold/src/jar.rs` tests (after the `c` helper at 549)

**Interfaces:**
- Consumes: `App::{create_jar, ensure_jar_loaded, apply_pulled_cookies, request_jar_ephemeral}`; `jar::Jar::{load, save}`; `SID` (`main.rs:3030`).
- Produces: `App::jar_saves: AtomicUsize` (`cfg(test)`); tests `a_burst_of_pulls_saves_the_jar`, `ephemeral_on_asks_first_for_a_jar_whose_cookies_are_not_saved_yet`, `jar_save_cost`.

- [ ] **Step 1: The counter.** In `main.rs`, after `jars_dir_override: Option<std::path::PathBuf>,`:

```rust
    /// Jar files written (`save_jar`), per app so parallel tests don't share it.
    #[cfg(test)]
    jar_saves: std::sync::atomic::AtomicUsize,
```

  and after `jars_dir_override: None,` in the initializer: `#[cfg(test)]` then `jar_saves: std::sync::atomic::AtomicUsize::new(0),`. In `save_jar`, right before `if let Err(e) = jar.save(&dir, key) {`, add:

```rust
        #[cfg(test)]
        self.jar_saves.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
```

- [ ] **Step 2: The pins.** In `main.rs`'s tests module, after `ephemeral_on_asks_first_only_when_the_jar_has_a_saved_file`:

```rust
    fn jar_saves(app: &App) -> usize {
        app.jar_saves.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Wave 2 (Jar H3): fifteen tabs pull a rotating cookie into one jar. Each changed pull saves the file today.
    #[test]
    fn a_burst_of_pulls_saves_the_jar() {
        let dir = store::testutil::temp_dir("jar-burst");
        let mut app = App { jars_dir_override: Some(dir.clone()), ..App::default() };
        let work = app.create_jar("Work");
        let key = jar::JarKey::Named(work);
        app.ensure_jar_loaded(key);
        let before = jar_saves(&app);
        for t in 0..15 {
            let tab = format!("t{t}");
            app.jar_tabs.insert(tab.clone(), (key, jar::TabCookies::default()));
            let dump = SID.replace(r#""value":"1""#, &format!(r#""value":"v{t}""#));
            assert!(app.apply_pulled_cookies(&tab, &dump), "pull {t} changed the jar");
        }
        assert_eq!(jar_saves(&app) - before, 15, "one save per changed pull");
        assert_eq!(jar::Jar::load(&dir, key).cookies[0].value, "v14", "the file holds the last value");
    }

    /// Wave 2 (Jar H3): a jar whose first login is not on disk yet still asks before ephemeral deletes it.
    #[test]
    fn ephemeral_on_asks_first_for_a_jar_whose_cookies_are_not_saved_yet() {
        let dir = store::testutil::temp_dir("jar-ephemeral-dirty");
        let mut app = App { jars_dir_override: Some(dir.clone()), ..App::default() };
        let work = app.create_jar("Work");
        let key = jar::JarKey::Named(work);
        app.ensure_jar_loaded(key);
        app.jar_tabs.insert("a".into(), (key, jar::TabCookies::default()));
        assert!(app.apply_pulled_cookies("a", SID));
        app.request_jar_ephemeral(work, true);
        assert_eq!(app.pending_jar_confirm, Some(jar_defs::JarConfirm::Ephemeral(work)));
    }
```

- [ ] **Step 3: `jar_save_cost`.** In `jar.rs`'s tests module, after the `c` helper:

```rust
    /// Wave 2 (Jar H3): one save of a 3000-cookie jar (serialize, DPAPI on Windows, temp file, rename). Timing,
    /// not correctness, so ignored: `cargo test --release -j 8 -p manifold --bins jar_save_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn jar_save_cost() {
        use crate::bench::{report, timed};
        let dir = crate::store::testutil::temp_dir("jar-save-cost");
        let mut jar = Jar::default();
        jar.cookies = (0..3000).map(|i| c(&format!("n{i}"), &format!("value-{i}-0123456789abcdef"))).collect();
        report("save, 3000 cookies", timed(40, || jar.save(&dir, JarKey::Default).unwrap()));
    }
```

  If the tests module doesn't already import `JarKey` and `Jar` through `use super::*;`, add what it needs.

- [ ] **Step 4: Check and measure.** Run the two new tests (expect PASS, 15 saves), the whole suite, `cargo fmt --all` and the clippy line. Then the CPU and Defender checks and `cargo test --release -j 8 -p manifold --bins jar_save_cost -- --ignored --nocapture`. Report the raw output.

- [ ] **Step 5: Commit.**

```bash
git add crates/manifold/src/main.rs crates/manifold/src/jar_defs.rs crates/manifold/src/jar.rs
git commit -m "Wave 2 (Jar pins): count jar saves; a burst of pulls, an unsaved jar's ephemeral prompt, jar_save_cost" -m "Co-Authored-By: <implementer's own line>"
```

---

### Task 6: Jar H3. One save per pull round

**Model:** opus. **Gate alone:** **full**. **Proof:**
- the burst saves 0 times, then once after the flush;
- the flush-timing test passes;
- every existing jar test passes, one with a flush added;
- `scripts/jar-probe.sh` passes.

Runs only if Task 5 counted more than one save per round and `jar_save_cost`'s best was over 3.2 ms.

**Files:**
- Modify: `crates/manifold/src/tabs.rs` (`JAR_PULL_TIMEOUT` at 20, `apply_pulled_cookies` at 178-206, new `flush_dirty_jars`)
- Modify: `crates/manifold/src/jar_defs.rs` (`switch_jar_ephemeral` 398-420, `request_jar_ephemeral` 424-430, `forget_jar` 468-480, `save_jar` 687-697)
- Modify: `crates/manifold/src/event_loop.rs` (`exiting` 105-112, `about_to_wait` 157-163)
- Modify: `crates/manifold/src/main.rs` (field after `jar_silent` at 1013, initializer at 1572, tests)
- Modify: `README.md` (the "**Cookie jar**" paragraph)

**Interfaces:**
- Consumes: `tabs::pull_pending(&HashMap<String, Instant>, &str, Instant) -> bool`.
- Produces:
  - `App::jar_dirty: BTreeMap<jar::JarKey, Instant>`
  - `pub(crate) const tabs::JAR_FLUSH_AFTER: Duration` (2 s); `JAR_PULL_TIMEOUT` becomes `pub(crate)`
  - `App::flush_dirty_jars(&mut self, now: Option<Instant>)`
  - `App::save_jar(&mut self, key)` (was `&self`)

- [ ] **Step 1: Write the failing tests.** In `main.rs`'s tests module:
  - In `a_burst_of_pulls_saves_the_jar`, replace the line `assert_eq!(jar_saves(&app) - before, 15, "one save per changed pull");` with:

```rust
        assert_eq!(jar_saves(&app) - before, 0, "nothing is written until the round ends");
        app.flush_dirty_jars(None);
        assert_eq!(jar_saves(&app) - before, 1, "then the jar is saved once");
```

    and its doc comment's second sentence with `They are saved once, when the round ends.`
  - In `deleting_a_jar_deletes_its_file_and_clearing_empties_it`, after `app.apply_pulled_cookies("a", SID);` add `app.flush_dirty_jars(None); // the round ends`.
  - Add:

```rust
    /// Wave 2 (Jar H3): a dirty jar is saved once no pull is pending, or once it has waited `JAR_FLUSH_AFTER`. A pull
    /// that timed out unanswered keeps its entry but holds nothing back.
    #[test]
    fn a_dirty_jar_is_saved_when_its_pull_round_ends() {
        use std::time::{Duration, Instant};
        let dir = store::testutil::temp_dir("jar-flush-round");
        let mut app = App { jars_dir_override: Some(dir.clone()), ..App::default() };
        on_default(&mut app, "a");
        let t0 = Instant::now();
        app.jar_pulls.insert("b".into(), t0); // b's pull is still out
        assert!(app.apply_pulled_cookies("a", SID));
        // Deadlines from the jar's own dirty mark, not from `t0`: a loaded machine can take any time between the two.
        let dirty = *app.jar_dirty.values().next().expect("the pull marked the jar dirty");
        app.flush_dirty_jars(Some(dirty + Duration::from_millis(500)));
        assert_eq!(jar_saves(&app), 0, "b's pull is pending and the jar is not due yet");
        app.flush_dirty_jars(Some(dirty + tabs::JAR_FLUSH_AFTER));
        assert_eq!(jar_saves(&app), 1, "due: saved although b's pull is pending");
        app.flush_dirty_jars(Some(dirty + tabs::JAR_FLUSH_AFTER * 2));
        assert_eq!(jar_saves(&app), 1, "saved jars are clean");

        assert!(app.apply_pulled_cookies("a", "[]"));
        app.flush_dirty_jars(Some(t0 + tabs::JAR_PULL_TIMEOUT));
        assert!(app.jar_pulls.contains_key("b"), "a timed-out pull keeps its entry");
        assert_eq!(jar_saves(&app), 2, "but it is not pending: the round is over");
    }
```

  Run `cargo test -j 8 -p manifold --bins jar`. Expect compile errors (`flush_dirty_jars`, `JAR_FLUSH_AFTER`).

- [ ] **Step 2: The field.** In `main.rs`, after `jar_silent: HashMap<String, u8>,`:

```rust
    /// Jars a pull changed and not saved yet, each with when it first changed: saved once the pull round ends
    /// (`flush_dirty_jars`), not once per tab's reply. Memory and `jar_snapshots` are current; only the file lags.
    jar_dirty: std::collections::BTreeMap<jar::JarKey, std::time::Instant>,
```

  and after `jar_silent: HashMap::new(),`: `jar_dirty: std::collections::BTreeMap::new(),`.

- [ ] **Step 3: Mark instead of saving; flush.** In `tabs.rs`:
  - `const JAR_PULL_TIMEOUT` becomes `pub(crate) const JAR_PULL_TIMEOUT`, and below it:

```rust
/// The longest a jar a pull changed waits for its pull round to end before it is saved anyway (`flush_dirty_jars`).
pub(crate) const JAR_FLUSH_AFTER: std::time::Duration = std::time::Duration::from_secs(2);
```

  - In `apply_pulled_cookies`, replace `self.save_jar(key);` with:

```rust
            // Saved once the round's other replies are in (`flush_dirty_jars`): fifteen tabs that saw one rotating
            // cookie write the file once, not fifteen times.
            self.jar_dirty.entry(key).or_insert_with(std::time::Instant::now);
```

    and in its doc comment replace `and saves that jar's file when it changed` with `and marks that jar dirty when it changed (\`flush_dirty_jars\` saves it)`.
  - After `apply_pulled_cookies`:

```rust
    /// Saves each dirty jar (`jar_dirty`) once its pull round is over, or once it has waited `JAR_FLUSH_AFTER`;
    /// `None` (quit) saves every one. The round is over when no tab has a pull pending (`pull_pending`): one that
    /// timed out unanswered keeps its `jar_pulls` entry until that tab's next pull, so emptiness is not the test. A
    /// named jar deleted meanwhile is skipped, as `apply_pulled_cookies` skips one.
    pub(crate) fn flush_dirty_jars(&mut self, now: Option<std::time::Instant>) {
        if self.jar_dirty.is_empty() {
            return;
        }
        let round_over = now.is_none_or(|now| !self.jar_pulls.keys().any(|id| pull_pending(&self.jar_pulls, id, now)));
        let due: Vec<crate::jar::JarKey> = self
            .jar_dirty
            .iter()
            .filter(|(_, since)| {
                round_over || now.is_some_and(|now| now.saturating_duration_since(**since) >= JAR_FLUSH_AFTER)
            })
            .map(|(key, _)| *key)
            .collect();
        for key in due {
            if matches!(key, crate::jar::JarKey::Named(id) if !self.mirror.jar_defs().contains_key(&id)) {
                self.jar_dirty.remove(&key);
                continue;
            }
            self.save_jar(key);
        }
    }
```

- [ ] **Step 4: Every other writer settles the mark.** In `jar_defs.rs`:
  - `save_jar`: `pub(crate) fn save_jar(&self, key: JarKey) {` becomes `pub(crate) fn save_jar(&mut self, key: JarKey) {`, its first line is `self.jar_dirty.remove(&key); // written now, or never (ephemeral, no dir)`, and its doc gains: `Settles the jar's dirty mark (\`jar_dirty\`): every write goes through here.`
  - `forget_jar`: after `self.jars.remove(&key);` add `self.jar_dirty.remove(&key);`.
  - `request_jar_ephemeral`: as its first lines:

```rust
        // A jar whose cookies are not on disk yet has logins to lose too: save them, so it asks first.
        if on && self.jar_dirty.contains_key(&JarKey::Named(id)) {
            self.save_jar(JarKey::Named(id));
        }
```

  - `switch_jar_ephemeral` off already calls `save_jar` (settled there); `clear_jar_cookies` too. Run `rg -n 'save_jar\(' crates/manifold/src` and expect only `jar_defs.rs` (the definition, `switch_jar_ephemeral`, `clear_jar_cookies`, `request_jar_ephemeral`), `tabs.rs` (`flush_dirty_jars`), and no other caller.

- [ ] **Step 5: The loop.** In `event_loop.rs`:
  - `exiting`: after `self.final_cookie_pull();` add `self.flush_dirty_jars(None); // the last pull's changes, and any round still open`.
  - `about_to_wait`: after the `jar_pull_all` block (`self.jar_last_pull = Some(…); }`), add:

```rust
        // A pull round's changed jars, once its replies are in or `JAR_FLUSH_AFTER` has passed. The loop wakes at
        // least every `IDLE_FRAME` or `MINIMIZED_TICK` (100 ms), so a due jar is written within 100 ms.
        self.flush_dirty_jars(Some(std::time::Instant::now()));
```

- [ ] **Step 6: README.** In the paragraph starting `**Cookie jar** (spec`, after the sentence about pulls, add: `A pull round's changes are written once its replies are in, at most 2 s after the first change (plus one event-loop wake, at most 100 ms): a crash in that window loses those cookie changes. New tabs are seeded from memory, so they never see the lag.` Place it where the paragraph describes the 30 s pull; quote the neighbouring sentence in the report.

- [ ] **Step 7: Check.** Run `cargo test -j 8 -p manifold`. Expect all PASS. Then `cargo fmt --all` and the clippy line.

- [ ] **Step 8: Probe.** `cargo build -j 8 -p manifold`, then `bash scripts/jar-probe.sh` (it runs `target/debug`, the app at BelowNormal). Expect its runs to pass. If its first run checks the jar file right after the 30 s pull and the file is not there yet, extend that wait by `JAR_FLUSH_AFTER` plus a second in the probe, rerun, and report the change. Report the PASS/FAIL lines.

- [ ] **Step 9: Report and commit.** Report the test, probe and burst results and stop before committing. Keep rule: the burst saves exactly once.

```bash
git add crates/manifold/src/tabs.rs crates/manifold/src/jar_defs.rs crates/manifold/src/event_loop.rs crates/manifold/src/main.rs README.md
git commit -m "Wave 2 (Jar H3): save a jar once per pull round, not once per tab" -m "A dirty jar is saved when no pull is pending or after 2 s, and at quit; every other writer settles the mark." -m "Co-Authored-By: <implementer's own line>"
```

  Add `scripts/jar-probe.sh` to the `git add` if Step 8 changed it.

---

### Task 7: M1 pins

**Model:** sonnet. **Gate alone:** light. **Proof:** `search_matches_reference` passes, and fails with a mutated reference.

**Files:**
- Modify: `crates/manifold/src/json_view.rs` tests

**Interfaces:**
- Consumes: `json_view::{search, search_with, count_nodes, SearchResult, MAX_MATCHES, WORKER_SEARCH_NODES}`.
- Produces: `reference_search` (test copy), tests `search_matches_reference`, `json_search_cost`.

- [ ] **Step 1: Copy the reference.** In `json_view.rs`'s tests module, add a copy of today's `search_with` (lines 440-480, body unchanged) named `reference_search`, with the doc comment `/// Today's \`search_with\`, kept as M1's reference (wave 2).`

- [ ] **Step 2: The comparison.**

```rust
    /// Wave 2 (M1): the search's whole result equals today's, for queries chosen to hit its edge cases (case,
    /// Unicode lowercasing, escaped keys, numbers, booleans, null, the root), at the match cap, and its cancel
    /// checks come at the same visits.
    #[test]
    fn search_matches_reference() {
        let root = json!({
            "Name": "Kelvin \u{212A} and \u{130}stanbul",
            "kayak": "plain ascii",
            "a/b": {"c~d": "slash and tilde", "\u{DF}": "strasse ss"},
            "items": [1, 2.5, -3e10, true, false, null, "Item 9", {"item 9": ["x"]}],
            "~": "/",
            "": "empty key",
            "nested": {"deep": {"deeper": ["zzz?", 12_345_678_901_234_567_890u64]}},
        });
        let queries = [
            "", "a", "A", "k", "\u{212A}", "i\u{307}", "\u{130}", "\u{DF}", "ss", "~", "/", "~1", "c~d", "2.5", "-3",
            "e10", "true", "fal", "null", "item 9", "ITEM", "zzz", "nomatch", "1234567890", "empty",
        ];
        for q in queries {
            assert_eq!(search_with(&root, q, &|| false), reference_search(&root, q, &|| false), "{q:?}");
        }
        assert_eq!(search_with(&json!("root text"), "TEXT", &|| false), reference_search(&json!("root text"), "TEXT", &|| false));

        let many = Value::Array((0..10_005).map(|i| json!(format!("m{i}"))).collect());
        let new = search(&many, "m");
        assert!(new.truncated && new.matches.len() == MAX_MATCHES);
        assert_eq!(Some(new), reference_search(&many, "m", &|| false));
        type SearchFn = fn(&Value, &str, &dyn Fn() -> bool) -> Option<SearchResult>;
        let checks = |f: SearchFn, stop_at: usize| {
            let n = std::cell::Cell::new(0);
            let result = f(&many, "m", &|| {
                n.set(n.get() + 1);
                n.get() >= stop_at
            });
            (result, n.get())
        };
        assert_eq!(checks(search_with, usize::MAX), checks(reference_search, usize::MAX), "the same cancel checks");
        assert_eq!(checks(search_with, 2), (None, 2), "a cancel stops it");
        assert_eq!(checks(reference_search, 2), (None, 2));
    }
```

- [ ] **Step 3: Prove it can fail.** In `reference_search` only, change `result.keep.insert(pointer.clone());` to nothing (comment it out). Run `cargo test -j 8 -p manifold --bins search_matches_reference`. Expect FAIL. Restore it and rerun: expect PASS.

- [ ] **Step 4: `json_search_cost`.**

```rust
    /// Wave 2 (M1): a UI-thread search (just under `WORKER_SEARCH_NODES`) of 14 000 items of 7 nodes, with many
    /// matches and with none. Timing, not correctness, so ignored:
    /// `cargo test --release -j 8 -p manifold --bins json_search_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn json_search_cost() {
        use crate::bench::{report, timed};
        let root = Value::Array(
            (0..14_000)
                .map(|n| json!({"id": n, "name": format!("item {n}"), "score": n as f64 * 0.37, "tags": ["a", "b"]}))
                .collect(),
        );
        assert!(count_nodes(&root) < WORKER_SEARCH_NODES, "{} nodes: searched on the UI thread", count_nodes(&root));
        for q in ["item 9", "zzz"] {
            report(&format!("search {q:?}, 14 000 items"), timed(20, || drop(search(&root, q))));
        }
    }
```

- [ ] **Step 5: Check and measure.** The whole suite, `cargo fmt --all`, the clippy line. Then the CPU and Defender checks and `cargo test --release -j 8 -p manifold --bins json_search_cost -- --ignored --nocapture`. Report the raw output.

- [ ] **Step 6: Commit.**

```bash
git add crates/manifold/src/json_view.rs
git commit -m "Wave 2 (M1 pins): JSON search against today's copy, and json_search_cost" -m "Co-Authored-By: <implementer's own line>"
```

---

### Task 8: M1. Search without per-node allocation

**Model:** opus. **Gate alone:** light. **Proof:** `search_matches_reference` and every existing search test pass; both queries' numbers.

Runs only if either query's best in Task 7 was over 8 ms.

**Files:**
- Modify: `crates/manifold/src/json_view.rs:440-480` (`search_with`) and new private helpers beside it

**Interfaces:**
- Consumes: nothing new. Produces: the same `search_with` signature; private `Children`, `push_token`, `contains_lower`.

- [ ] **Step 1: The helpers.** Above `search_with`:

```rust
/// One open container's children in `search_with`'s walk.
enum Children<'a> {
    Object(serde_json::map::Iter<'a>),
    Array(std::iter::Enumerate<std::slice::Iter<'a, Value>>),
}

/// Appends `key` to `path` as one pointer token, escaping `~` and `/` as `child_pointer` does.
fn push_token(path: &mut String, key: &str) {
    path.push('/');
    for c in key.chars() {
        match c {
            '~' => path.push_str("~0"),
            '/' => path.push_str("~1"),
            c => path.push(c),
        }
    }
}

/// Whether `text` contains `needle` (already lowercased), ignoring case as `str::to_lowercase` does. When both
/// are ASCII, a byte compare with no allocation. Otherwise `to_lowercase().contains`, as before: a non-ASCII text
/// can lowercase to ASCII (the Kelvin sign is `k`), and lowercasing can change a text's length.
fn contains_lower(text: &str, needle: &str) -> bool {
    if text.is_ascii() && needle.is_ascii() {
        let (text, needle) = (text.as_bytes(), needle.as_bytes());
        needle.is_empty() || text.windows(needle.len()).any(|w| w.eq_ignore_ascii_case(needle))
    } else {
        text.to_lowercase().contains(needle)
    }
}
```

- [ ] **Step 2: The walk.** Replace `search_with`'s body after the empty-needle return:

```rust
    use std::fmt::Write;
    // Depth-first in document order with one path: a token is pushed going down and cut off coming back up
    // (`stack` holds each open container's children and its path length).
    let mut path = String::new();
    let mut number = String::new();
    let mut stack: Vec<(Children, usize)> = Vec::new();
    let mut node: Option<(Option<&str>, &Value)> = Some((None, root));
    let mut visited = 0usize;
    while let Some((key, value)) = node.take() {
        visited += 1;
        if visited.is_multiple_of(4096) && cancelled() {
            return None;
        }
        let key_hit = key.is_some_and(|k| contains_lower(k, &needle));
        let value_hit = match value {
            Value::String(s) => contains_lower(s, &needle),
            Value::Number(n) => {
                number.clear();
                let _ = write!(number, "{n}");
                contains_lower(&number, &needle)
            }
            Value::Bool(b) => contains_lower(if *b { "true" } else { "false" }, &needle),
            Value::Null => contains_lower("null", &needle),
            Value::Array(_) | Value::Object(_) => false,
        };
        if key_hit || value_hit {
            if result.matches.len() == MAX_MATCHES {
                result.truncated = true; // a match past the cap: there are more than it shows
                break;
            }
            for (i, _) in path.match_indices('/') {
                result.keep.insert(path[..i].to_string());
            }
            result.keep.insert(path.clone());
            result.hits.insert(path.clone());
            result.matches.push(path.clone());
        }
        match value {
            Value::Object(map) => stack.push((Children::Object(map.iter()), path.len())),
            Value::Array(items) => stack.push((Children::Array(items.iter().enumerate()), path.len())),
            _ => {}
        }
        // The next node: the next child of the innermost container that has one left.
        while let Some((children, len)) = stack.last_mut() {
            path.truncate(*len);
            let next = match children {
                Children::Object(iter) => iter.next().map(|(k, v)| {
                    push_token(&mut path, k);
                    (Some(k.as_str()), v)
                }),
                Children::Array(iter) => iter.next().map(|(i, v)| {
                    path.push('/');
                    let _ = write!(path, "{i}");
                    (None, v)
                }),
            };
            if next.is_some() {
                node = next;
                break;
            }
            stack.pop();
        }
    }
    Some(result)
```

  Update its comment `// Depth-first, children pushed in reverse …` (it goes with the old body).

- [ ] **Step 3: Check.** `cargo test -j 8 -p manifold`. Expect all PASS, `search_matches_reference` included. `cargo fmt --all`, the clippy line.

- [ ] **Step 4: Measure, report, commit (if kept).** The CPU and Defender checks, `cargo test --release -j 8 -p manifold --bins json_search_cost -- --ignored --nocapture`, report and stop. Keep rule: both queries drop by 25% or more.

```bash
git add crates/manifold/src/json_view.rs
git commit -m "Wave 2 (M1): JSON search walks with one path and no per-node allocation" -m "Co-Authored-By: <implementer's own line>"
```

---

### Task 9: H2 pins, the frame bench, and the teardown pin

**Model:** sonnet. **Gate alone:** light. **Proof:** the property test passes and makes at least 15 links; the badge and teardown pins pass; the benches print.

**Files:**
- Modify: `crates/manifold/src/links.rs` (new tests module at the end)
- Modify: `crates/manifold/src/graph_ui.rs` tests (after `a_click_finishes_or_cancels_pick_target_mode`)
- Modify: `crates/manifold/src/session_sync/tests.rs` (after `teardown_cancels_pending_pdf_renders`)

**Interfaces:**
- Consumes:
  - `App::{connect, create_link, edit_field, valid_targets, has_valid_target, check_connect (private, same module), links_naming, insert_group_at, card_tab_order, card_rect, graph_world_rect_to_screen, fit_all_nodes_scene_rect, build_graph_ui, teardown_running_session (private; the test is in its child module)}`
  - `crate::session_sync::tests::{add, sleeping_tab, app_on, a_note}` (`a_note` is private to `session_sync::tests`; the teardown tests are in that file)
  - `crate::store::testutil::{XorShift, temp_dir}`
- Produces: tests `valid_targets_match_check_connect`, `link_query_cost`, `graph_frame_cost`, `each_card_shows_its_tab_slot_badge`, `teardown_queues_every_node_and_link_delete`, `teardown_cost`.

- [ ] **Step 1: The property test.** At the end of `links.rs`:

```rust
#[cfg(test)]
mod tests {
    use crate::caps::{LINK, OPENED_FROM};
    use crate::node::{DownloadPayload, NotePayload, Runtime};
    use crate::session_sync::tests::{add, sleeping_tab};
    use crate::store::model::{field, FieldValue, Kind, NodeId};
    use crate::store::testutil::XorShift;
    use crate::App;

    /// Wave 2 (H2): `valid_targets` holds exactly the cards `check_connect` accepts, for every source and edge, over
    /// random cards (notes, sleeping tabs, completed and running downloads, two groups) in three levels, random
    /// connects, a half-made link and a dangling one; `has_valid_target` is its non-emptiness.
    #[test]
    fn valid_targets_match_check_connect() {
        let mut app = App::default();
        let groups: Vec<String> = (0..2)
            .map(|_| {
                let g = app.alloc_node_id();
                app.insert_group_at(g.clone(), None);
                g
            })
            .collect();
        let download = |completed| {
            Runtime::Download(DownloadPayload {
                path: "f.bin".into(),
                bytes_received: 1,
                total_bytes: Some(2),
                completed,
                failed: false,
            })
        };
        for i in 0..60usize {
            let id = match i % 4 {
                0 => add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false })),
                1 => add(&mut app, Kind::Tab, sleeping_tab("https://a.test/", "a")),
                2 => add(&mut app, Kind::Download, download(true)),
                _ => add(&mut app, Kind::Download, download(false)),
            };
            if !i.is_multiple_of(3) {
                app.edit_group(&id, Some(groups[i % 3 - 1].clone()));
            }
        }
        // Sorted: `HashMap` order is random, and the picks must repeat.
        let mut ids: Vec<String> = app.nodes.keys().cloned().collect();
        ids.sort();
        let mut rng = XorShift(0x5eed_1234);
        let mut links = 0;
        for _ in 0..300 {
            let (from, to) = (&ids[rng.below(ids.len() as u64) as usize], &ids[rng.below(ids.len() as u64) as usize]);
            let edge = if rng.below(2) == 0 { LINK } else { OPENED_FROM };
            if app.connect(edge, from, to).is_ok() && edge == LINK {
                links += 1;
            }
        }
        assert!(links >= 15, "{links} links made: raise the connect count");
        let (a, b, c) = (&ids[0], &ids[1], &ids[2]);
        let half = app.create_link(a, b);
        app.edit_field(&half, field::TO, FieldValue::Id(None));
        let ghost: NodeId = app.alloc_node_id().parse().unwrap(); // never inserted
        let dangling = app.create_link(a, c);
        app.edit_field(&dangling, field::TO, FieldValue::Id(Some(ghost)));
        for from in &ids {
            for edge in [LINK, OPENED_FROM] {
                let targets = app.valid_targets(edge, from);
                for to in &ids {
                    let accepted = app.check_connect(edge, from, to).is_ok();
                    assert_eq!(targets.binary_search(to).is_ok(), accepted, "{edge} {from} -> {to}");
                }
                assert_eq!(app.has_valid_target(edge, from), !targets.is_empty(), "{edge} {from}");
            }
        }
    }
}
```

  If the `use` lines name something `links.rs`'s parent already gives through another path (a compile error says so), adjust the paths only.

- [ ] **Step 2: `link_query_cost`.** In the same module:

```rust
    /// Wave 2 (H2): pick-target mode's per-frame queries at the first group of the 2000-node fixture, with 20, 100
    /// and 300 links among its cards. Timing, not correctness, so ignored:
    /// `cargo test --release -j 8 -p manifold --bins link_query_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn link_query_cost() {
        use crate::bench::{level_fixture, report, timed};
        let f = level_fixture(false);
        let g0 = f.groups[0].clone();
        let mut app = f.app;
        let at: Vec<String> = f.nodes.iter().filter(|id| app.mirror.group(id) == Some(g0.as_str())).cloned().collect();
        let n = at.len();
        let mut k = 0;
        for links in [20usize, 100, 300] {
            while app.mirror.link_ids().count() < links {
                let (a, b) = (&at[k % n], &at[(k * 7 + 1) % n]);
                if a != b {
                    app.create_link(a, b);
                }
                k += 1;
            }
            let from = &at[0];
            report(&format!("valid_targets(LINK), L={links}"), timed(50, || drop(app.valid_targets(LINK, from))));
            report(
                &format!("has_valid_target(LINK), L={links}"),
                timed(50, || drop(std::hint::black_box(app.has_valid_target(LINK, from)))),
            );
        }
    }
```

  (`f.nodes` is read after `f.app` moved: a partial move of a struct without `Drop` is fine.)

- [ ] **Step 3: `graph_frame_cost` and the badge test.** In `graph_ui.rs`'s tests module:

```rust
    /// Wave 2 (H2 / P2): whole `build_graph_ui` frames over the 2000-node fixture spread on a grid: idle at the top
    /// level, hovering a card, in pick-target mode, with the node menu open, filtered, and idle in the first group.
    /// Timing, not correctness, so ignored:
    /// `cargo test --release -j 8 -p manifold --bins graph_frame_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn graph_frame_cost() {
        use crate::bench::{level_fixture, report, spread, timed};
        let ctx = egui::Context::default();
        crate::theme::apply_fonts(&ctx);
        let mut f = level_fixture(false);
        spread(&mut f);
        let mut app = f.app;
        let frame = |app: &mut App, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024.0, 768.0))),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| app.build_graph_ui(ui));
            output.textures_delta.clear();
        };
        let run = |app: &mut App, what: &str, events: Vec<egui::Event>| {
            for _ in 0..5 {
                frame(app, events.clone());
            }
            report(what, timed(50, || frame(app, events.clone())));
        };
        let note = f
            .nodes
            .iter()
            .find(|id| app.mirror.group(id).is_none() && app.nodes[*id].as_note().is_some())
            .expect("a top-level note")
            .clone();
        app.camera.jump_to(app.card_rect(&note).unwrap().expand(300.0));
        for _ in 0..3 {
            frame(&mut app, vec![]);
        }
        run(&mut app, "idle, top level", vec![]);
        let on_note = app.graph_world_rect_to_screen(app.card_rect(&note).unwrap()).center();
        run(&mut app, "hovering a card", vec![egui::Event::PointerMoved(on_note)]);
        app.connecting = Some((note.clone(), crate::caps::LINK));
        run(&mut app, "pick-target mode (LINK)", vec![]);
        assert!(app.connecting.is_some(), "the mode stayed on");
        app.connecting = None;
        app.node_menu = Some((note.clone(), egui::pos2(500.0, 400.0)));
        run(&mut app, "node menu open", vec![]);
        assert!(app.node_menu.is_some(), "the menu stayed open");
        app.node_menu = None;
        app.graph_filter = "abc".into();
        run(&mut app, "filtered", vec![]);
        app.graph_filter.clear();
        app.switch_group_level(Some(f.groups[0].clone()));
        for _ in 0..3 {
            frame(&mut app, vec![]);
        }
        run(&mut app, "idle, first group", vec![]);
    }

    /// Wave 2 (H4): keyboard navigation shows each card's Tab slot (`card_tab_order`'s index plus one) at its
    /// top-left corner. Pins the slot lookup before it moves to a per-pass map.
    #[test]
    fn each_card_shows_its_tab_slot_badge() {
        let ctx = egui::Context::default();
        crate::theme::apply_fonts(&ctx);
        let mut app = App::default();
        for k in 0..3 {
            let id = app.alloc_node_id();
            let note = Runtime::Note(NotePayload { text: "n".into(), editing: false });
            app.insert_placeholder_node(id, egui::vec2(400.0 * k as f32, 0.0), crate::store::model::Kind::Note, note);
        }
        let frame = |app: &mut App| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024.0, 768.0))),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| app.build_graph_ui(ui));
            output.textures_delta.clear();
            output
        };
        for _ in 0..3 {
            frame(&mut app);
        }
        app.camera.jump_to(app.fit_all_nodes_scene_rect());
        frame(&mut app);
        app.card_nav_active = true; // read when the pass starts; the frame may clear it at its end
        let output = frame(&mut app);
        let badges: Vec<(String, egui::Pos2)> = output
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text().parse::<usize>().is_ok() => {
                    Some((t.galley.text().to_string(), t.galley.rect.translate(t.pos.to_vec2()).center()))
                }
                _ => None,
            })
            .collect();
        let order = app.card_tab_order();
        assert_eq!(order.len(), 3);
        for (k, id) in order.iter().enumerate() {
            // The rect the card is drawn at, as `a_click_finishes_or_cancels_pick_target_mode` reads it.
            let item = app.level_items().into_iter().find(|i| i.id == *id).expect("on the level");
            let corner = app.graph_world_rect_to_screen(item.rect).left_top();
            let number = (k + 1).to_string();
            assert!(
                badges.iter().any(|(text, at)| *text == number && at.distance(corner) < 15.0),
                "{id}: badge {number} near {corner:?}; badges {badges:?}"
            );
        }
    }
```

  `graph_frame_cost` reads `f.nodes` and `f.groups` after `f.app` moved (a partial move). The badge keycap is clamped 10 points inside the clip rect, hence the 15-point tolerance.

- [ ] **Step 4: The teardown pin and bench.** In `session_sync/tests.rs`, after `teardown_cancels_pending_pdf_renders`:

```rust
/// Wave 2 (teardown order): replacing the session queues a delete for every live non-jar node of the document and
/// for every link (saved or not), leaves nothing to create, and leaves the mirror no links.
#[test]
fn teardown_queues_every_node_and_link_delete() {
    let mut app = app_on(&temp_dir("teardown-queues").join("session.mnf"), 1);
    let notes: Vec<String> = (0..6).map(|_| a_note(&mut app)).collect();
    app.connect(LINK, &notes[0], &notes[1]).unwrap();
    app.connect(LINK, &notes[2], &notes[3]).unwrap();
    assert!(app.save_session());
    app.connect(LINK, &notes[4], &notes[5]).unwrap(); // in the mirror, not saved yet
    a_note(&mut app); // queued for create, not saved yet
    let mut want: BTreeSet<NodeId> = app
        .journal
        .as_ref()
        .unwrap()
        .doc()
        .live()
        .filter(|(_, n)| n.kind() != Kind::Jar)
        .map(|(id, _)| id)
        .collect();
    want.extend(app.mirror.link_ids());
    assert!(want.len() >= 9, "six notes and three links: {want:?}");
    app.teardown_running_session();
    assert_eq!(app.pending_deletes, want);
    assert!(app.pending_creates.is_empty());
    assert_eq!(app.mirror.link_ids().count(), 0);
}

/// Wave 2 (teardown order): replacing a session of 2000 notes and 300 links. Timing, not correctness, so ignored:
/// `cargo test --release -j 8 -p manifold --bins teardown_cost -- --ignored --nocapture`.
#[test]
#[ignore]
fn teardown_cost() {
    use crate::bench::{best_median, report};
    let mut times = Vec::new();
    for _ in 0..10 {
        let mut app = app_on(&temp_dir("teardown-cost").join("session.mnf"), 1);
        let notes: Vec<String> = (0..2000).map(|_| a_note(&mut app)).collect();
        for k in 0..300 {
            app.create_link(&notes[k], &notes[(k * 7 + 1) % 2000]);
        }
        assert!(app.save_session());
        let start = std::time::Instant::now();
        app.teardown_running_session();
        times.push(start.elapsed());
    }
    report("teardown, 2000 notes and 300 links", best_median(times));
}
```

  If `BTreeSet` or `NodeId` aren't in scope through `use super::*;`, add `use std::collections::BTreeSet;` or the `crate::store::model::NodeId` path inside the test.

- [ ] **Step 5: Check and measure.** Run the four non-ignored new tests (each by name; expect PASS), the whole suite, `cargo fmt --all` and the clippy line. Then the CPU and Defender checks and, in release, `link_query_cost`, `graph_frame_cost` and `teardown_cost`. Report the raw output of all three.

- [ ] **Step 6: Commit.**

```bash
git add crates/manifold/src/links.rs crates/manifold/src/graph_ui.rs crates/manifold/src/session_sync/tests.rs
git commit -m "Wave 2 (H2 pins): valid_targets property test, Tab-slot badges, teardown end state, and link, frame and teardown benches" -m "Co-Authored-By: <implementer's own line>"
```

---

### Task 10: H2 / P2. Hoist the link set; `edge_menu`; the two H4 one-liners

**Model:** sonnet. **Gate alone:** light. **Proof:** the property and badge tests pass; each part's keep rule is checked on its own bench.

**Files:**
- Modify: `crates/manifold/src/links.rs` (`check_connect` 39-63, `connect` 67, `valid_targets` 132-137, `has_valid_target` 140-142, the test)
- Modify: `crates/manifold/src/node_menu.rs:806` (`edge_menu`)
- Modify: `crates/manifold/src/graph_ui.rs` (`draw_graph_cards` 1641-1676, 1699)
- Modify: `crates/manifold/src/card.rs` (`CardPass` 613-636, 1008, 1113, test literal 2566-2578)

**Interfaces:**
- Produces:
  - `App::check_connect(&self, edge, from, to, linked: Option<&HashSet<NodeId>>)` (private)
  - private `App::linked_to(&self, from: &str) -> HashSet<NodeId>`, `App::linked_for(&self, edge: &str, from: &str) -> Option<HashSet<NodeId>>`
  - `CardPass::slots: &'a HashMap<&'a str, usize>`

Each part is measured on its own after it lands, against Task 9's numbers. A part that misses its rule is reverted before the commit (Decision 11).

- [ ] **Step 1: The hoist.** In `links.rs`, add `use std::collections::HashSet;`.
  - `check_connect`'s signature becomes `fn check_connect(&self, edge: &str, from: &str, to: &str, linked: Option<&HashSet<NodeId>>) -> Result<(), ConnectError> {`, with its doc gaining: `With \`linked\` (\`linked_for\`), whether a link already joins them is a set lookup; with \`None\`, a scan of the links (\`link_between\`).`
  - Its `Storage::Node(_)` arm becomes:

```rust
            Storage::Node(_) => match linked {
                Some(linked) => to.parse::<NodeId>().is_ok_and(|to| linked.contains(&to)),
                None => self.link_between(from, to).is_some(),
            },
```

  - `connect`: `self.check_connect(edge, from, to, None)?;`.
  - New, after `link_between`:

```rust
    /// The cards a link joins to `from`, either way: `check_connect`'s "already linked", built once per query
    /// instead of once per candidate. A half-made link (an end `None`) joins nothing, as in `link_between`.
    fn linked_to(&self, from: &str) -> HashSet<NodeId> {
        let Ok(me) = from.parse::<NodeId>() else { return HashSet::new() };
        self.links_naming(from)
            .iter()
            .filter_map(|link| match self.mirror.link_ends(link) {
                (Some(a), Some(b)) if a == me => Some(b),
                (Some(a), Some(b)) if b == me => Some(a),
                _ => None,
            })
            .collect()
    }

    /// `linked_to(from)` for an edge stored as a Link node; `None` for another edge, which never reads it.
    fn linked_for(&self, edge: &str, from: &str) -> Option<HashSet<NodeId>> {
        let node_stored = crate::caps::edges().row(edge).is_some_and(|r| matches!(r.storage(), Storage::Node(_)));
        node_stored.then(|| self.linked_to(from))
    }
```

  - `valid_targets` and `has_valid_target`:

```rust
    pub(crate) fn valid_targets(&self, edge: &str, from: &str) -> Vec<String> {
        let linked = self.linked_for(edge, from);
        let mut targets: Vec<String> = self
            .nodes
            .keys()
            .filter(|to| self.check_connect(edge, from, to, linked.as_ref()).is_ok())
            .cloned()
            .collect();
        targets.sort();
        targets
    }

    pub(crate) fn has_valid_target(&self, edge: &str, from: &str) -> bool {
        let linked = self.linked_for(edge, from);
        self.nodes.keys().any(|to| self.check_connect(edge, from, to, linked.as_ref()).is_ok())
    }
```

  - The property test's `app.check_connect(edge, from, to)` becomes `app.check_connect(edge, from, to, None)` (the unhoisted path), and its doc adds `(\`None\`: the unhoisted scan)`.
  - Run the property test (expect PASS), then `link_query_cost` in release (CPU and Defender first). Keep rule: `valid_targets` at L=100 twice as fast or more.

- [ ] **Step 2: `edge_menu`.** In `node_menu.rs`, replace `(!self.valid_targets(row.id(), id).is_empty()).then(|| EdgeMenuRow {` with `self.has_valid_target(row.id(), id).then(|| EdgeMenuRow {`. It goes with the hoist's keep decision.

- [ ] **Step 3: Binary search on `pick_targets`.** It is sorted (`valid_targets`).
  - `graph_ui.rs`: `pass.pick_targets.contains(id)` becomes `pass.pick_targets.binary_search(id).is_ok()`.
  - `card.rs`: `pass.pick_targets.iter().any(|t| t == id)` becomes `pass.pick_targets.binary_search_by(|t| t.as_str().cmp(id)).is_ok()`.
  - `CardPass::pick_targets`'s doc: `/// Pick-target mode's valid targets (\`App::valid_targets\`, sorted), each ringed; empty outside the mode.`
  - Run `graph_frame_cost` in release. Keep rule: the pick-target frame 5% faster or more.

- [ ] **Step 4: The Tab-slot map.**
  - `CardPass` gets, after `order`:

```rust
    /// Each id's first index in `order`, built once per pass: a card's Tab slot without a scan of `order`.
    pub(crate) slots: &'a HashMap<&'a str, usize>,
```

  - `graph_ui.rs`, after `let tab_order = self.card_tab_order();`:

```rust
        let mut slots: HashMap<&str, usize> = HashMap::with_capacity(tab_order.len());
        for (i, id) in tab_order.iter().enumerate() {
            slots.entry(id.as_str()).or_insert(i);
        }
```

    and `slots: &slots,` in the `CardPass` literal after `order: &tab_order,`. (The `self.handle_card_tab(ui, &tab_order)` calls between borrow `tab_order` immutably; `slots` borrows it immutably too.)
  - `card.rs:1008`: `let slot = pass.order.iter().position(|o| o == id);` becomes `let slot = pass.slots.get(id).copied();`.
  - `card.rs`'s test literal: `let slots = HashMap::new();` beside `let members = HashMap::new();`, and `slots: &slots,` after `order: &[],`.
  - Run the badge test (expect PASS), then `graph_frame_cost` in release. Keep rule: the idle top-level frame 5% faster or more.

- [ ] **Step 5: Check, report, commit.** `cargo test -j 8 -p manifold`, `cargo fmt --all`, the clippy line. Report the three benches' raw output with CPU and Defender, and stop before committing. Commit what the controller keeps:

```bash
git add crates/manifold/src/links.rs crates/manifold/src/node_menu.rs crates/manifold/src/graph_ui.rs crates/manifold/src/card.rs
git commit -m "Wave 2 (H2): link checks share one linked set per query; edge_menu stops at the first target; pick targets and Tab slots without scans" -m "Co-Authored-By: <implementer's own line>"
```

  Name only the kept parts in the message.

---

### Task 11: Teardown order

**Model:** haiku. **Gate alone:** **full** (session code). **Proof:** `teardown_queues_every_node_and_link_delete` passes; `teardown_cost` before and after.

**Files:**
- Modify: `crates/manifold/src/session_sync.rs:2026-2033` (`teardown_running_session`)

**Interfaces:**
- Consumes: `Mirror::{link_ids, forget_link}`, `App::queue_delete`. Produces: nothing new.

- [ ] **Step 1: Delete the links first.** Wave 1 left `teardown_running_session` (`session_sync.rs`, anchor on the quoted lines) with the link loop after the `for id in old {` loop. Move it, unchanged, to just before that loop:

```rust
        // A link is not a card, so no endpoint's cascade is certain to reach it (a dangling one has none): the
        // replaced session's links all go, as its cards do. Jars stay.
        for link in self.mirror.link_ids().map(|l| l.to_string()).collect::<Vec<_>>() {
            self.delete_link(&link);
        }
```

  Add above it: `// Links first: each queue_delete below scans the links naming its node (links_naming), so with every link already gone each scan is empty and the teardown is O(N + L), not O(N·L).` `delete_link` (`links.rs`) already does `forget_link` then `queue_delete`, so nothing else changes. Delete the old copy after the loop. The `forget_id` loop stays where it is, after the `pending_creates` clear.

- [ ] **Step 2: Check and measure.** `cargo test -j 8 -p manifold` (expect all PASS, the teardown pin included), `cargo fmt --all`, the clippy line. Then the CPU and Defender checks and `cargo test --release -j 8 -p manifold --bins teardown_cost -- --ignored --nocapture`. Report and stop. Keep rule: twice as fast or more.

- [ ] **Step 3: Commit (if kept).**

```bash
git add crates/manifold/src/session_sync.rs
git commit -m "Wave 2 (teardown): delete the links before the nodes, so no scan sees them" -m "Co-Authored-By: <implementer's own line>"
```

---

### Task 12: H5 pins

**Model:** sonnet. **Gate alone:** light (a test, a `cfg(test)` counter and a bench size). CLAUDE.md classes any store change as full; this edit to `store/journal.rs` is `cfg(test)`-only, so it is labelled a deviation (spec H5), and the wave's full gate (Task 16) covers it. **Proof:** the counts are 2, 1, 2, 2 today; `app_save_costs` prints 2000 tabs.

**Files:**
- Modify: `crates/manifold/src/store/journal.rs` (struct 724-762, `blank` 773-797, `begin` 903-907)
- Modify: `crates/manifold/src/session_sync/tests.rs` (`app_save_costs` 2888, new test)

**Interfaces:**
- Produces: `Journal::begins: usize` (`cfg(test)`, `pub(crate)`); test `save_count_per_action`.

- [ ] **Step 1: The counter.** In `Journal`, after `unsynced: bool,`:

```rust
    /// `begin` calls, for tests that count saves that reached the store.
    #[cfg(test)]
    pub(crate) begins: usize,
```

  in `blank` after `unsynced: false,`: `#[cfg(test)]` then `begins: 0,`; and as `begin`'s first line:

```rust
        #[cfg(test)]
        {
            self.begins += 1;
        }
```

- [ ] **Step 2: The test.** In `session_sync/tests.rs`, after `app_save_costs`:

```rust
/// Wave 2 (H5): the saves that reach the store (`Journal::begin`) per action through `apply_graph_action`, which
/// saves after the action's own save. With nothing left to write, the second one is redundant.
#[test]
fn save_count_per_action() {
    use crate::card::DropEffect;
    use crate::graph_ui::GraphAction;
    let mut app = app_on(&temp_dir("save-count").join("session.mnf"), 1);
    let ctx = egui::Context::default();
    let g = add_group(&mut app, "g");
    let (a, b, c, d) = (a_note(&mut app), a_note(&mut app), a_note(&mut app), a_note(&mut app));
    assert!(app.save_session());
    let begins = |app: &App| app.journal.as_ref().unwrap().begins;
    let count = |app: &mut App, action: GraphAction| {
        let before = begins(app);
        app.apply_graph_action(&ctx, action);
        begins(app) - before
    };
    let delete = count(&mut app, GraphAction::Node(a.clone(), NodeAction::Delete));
    let join = count(&mut app, GraphAction::Drop { id: b.clone(), target: Some((DropEffect::Join, Some(g.clone()))) });
    app.connecting = Some((c.clone(), crate::caps::LINK));
    let finish = count(&mut app, GraphAction::FinishConnect(d.clone()));
    let enter = count(&mut app, GraphAction::EnterGroup(g.clone()));
    assert!(!app.nodes.contains_key(&a), "deleted");
    assert_eq!(app.mirror.group(&b), Some(g.as_str()), "joined");
    assert_eq!(app.links_naming(&c).len(), 1, "linked");
    assert_eq!(app.current_group.as_deref(), Some(g.as_str()), "entered");
    assert_eq!(
        [("menu Delete", delete), ("enter a group", enter), ("Join drop", join), ("finish a link", finish)],
        [("menu Delete", 2), ("enter a group", 1), ("Join drop", 2), ("finish a link", 2)]
    );
}
```

  Run it. Expect PASS. If a count differs, pin today's actual count in the expected array, and report it: Task 13's expected counts are then today's minus the one redundant save per action.

- [ ] **Step 3: `app_save_costs` at 2000.** `for tabs in [300usize, 3000] {` becomes `for tabs in [300usize, 2000, 3000] {`.

- [ ] **Step 4: Check and measure.** The whole suite, `cargo fmt --all`, the clippy line. Then the CPU and Defender checks and `cargo test --release -j 8 -p manifold --bins app_save_costs -- --ignored --nocapture`. Report the raw output (the decision reads "save_session, nothing changed" at 2000 tabs).

- [ ] **Step 5: Commit.**

```bash
git add crates/manifold/src/store/journal.rs crates/manifold/src/session_sync/tests.rs
git commit -m "Wave 2 (H5 pins): count the saves that reach the store per action, and time a no-op save at 2000 tabs" -m "Co-Authored-By: <implementer's own line>"
```

---

### Task 13: H5. A save with nothing to save

**Model:** opus. **Gate alone:** **full**. **Proof:**
- `save_count_per_action` gives 1, 0, 1, 1;
- a no-op save still applies another replica's frame;
- every store and session test passes;
- `app_save_costs` "nothing changed" at 2000 tabs before and after.

Runs only if Task 12's no-op save at 2000 tabs was over 0.3 ms.

**Files:**
- Modify: `crates/manifold/src/store/journal.rs` (new `quiet` after `changed_on_disk` at 911)
- Modify: `crates/manifold/src/session_sync.rs:1173-1378` (`save_session`, new `nothing_to_save` and `after_save`)
- Modify: `crates/manifold/src/session_sync/tests.rs`

**Interfaces:**
- Produces: `Journal::quiet(&self) -> bool`; private `App::nothing_to_save(&self) -> bool`, `App::after_save(&mut self)`.

- [ ] **Step 1: Write the failing tests.**
  - In `save_count_per_action`, the expected array becomes `[("menu Delete", 1), ("enter a group", 0), ("Join drop", 1), ("finish a link", 1)]` (or Task 12's reported counts minus one each, Decision 10), and its doc's last sentence becomes `With nothing left to write, the second one stops before the store (\`nothing_to_save\`).`
  - Add after it:

```rust
/// Wave 2 (H5): a save with nothing queued still applies another replica's frames when the file changed.
#[test]
fn a_no_op_save_still_applies_another_replicas_frames() {
    let path = temp_dir("noop-save-remote").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = a_note(&mut app);
    assert!(app.save_session());
    let quiet = app.journal.as_ref().unwrap().begins;
    assert!(app.save_session(), "nothing to write: done");
    assert_eq!(app.journal.as_ref().unwrap().begins, quiet, "without the store lock");
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, &note, field::COLOR, red());
    assert!(app.save_session());
    assert_eq!(app.mirror.get(&note, field::COLOR), red(), "the changed file took the full path");
}
```

  Run both. Expect FAIL (2 and 2; the begins count grows).

- [ ] **Step 2: `Journal::quiet`.** After `changed_on_disk`:

```rust
    /// Whether a `begin` now would find nothing but the lock: this replica's state matches its file (not
    /// `stale`), it may commit (not read-only), it has no repair to report, and the file is unchanged since this
    /// replica last read or wrote it. `seen` is unset before the first read or when the file can't be stat'ed.
    pub fn quiet(&self) -> bool {
        !self.stale && !self.read_only && self.recovery.is_none() && self.seen.is_some() && !self.changed_on_disk()
    }
```

- [ ] **Step 3: `after_save` and the fast path.** In `session_sync.rs`:
  - New private methods beside `save_session`:

```rust
    /// Nothing for a save to write or read: no queued create, move, field or delete, no retry scheduled, and the
    /// store quiet (`Journal::quiet`). A save then needs neither the lock nor the file's tail.
    fn nothing_to_save(&self) -> bool {
        self.pending_creates.is_empty()
            && self.pending_moves.is_empty()
            && self.pending.is_empty()
            && self.pending_deletes.is_empty()
            && self.save_retry_at.is_none()
            && self.journal.as_ref().is_some_and(|j| j.quiet())
    }

    /// How a save that left the store holding every local edit ends, written or not (`nothing_to_save`).
    fn after_save(&mut self) {
        // A tab still spawning keeps its entry, and its layout: a respawn has removed it from `nodes`
        // until it lands, and `Mirror::init` then leaves an existing entry alone.
        let (nodes, spawning) = (&self.nodes, &self.spawning);
        self.mirror.retain(|id| nodes.contains_key(id) || spawning.contains_key(id));
        self.layout.retain(|id, _| nodes.contains_key(id) || spawning.contains_key(id));
        self.held_text.retain(|id, _| nodes.get(id).and_then(|n| n.as_note()).is_some_and(|n| n.editing));
        self.store_notices.clear();
        self.last_session_save = Some(std::time::Instant::now());
        self.save_retry_at = None;
        self.flush_history();
        #[cfg(debug_assertions)]
        self.check_graph_matches_document();
    }
```

  - In `save_session`, after `self.commit_note_buffers();`:

```rust
        // Nothing queued and the file unchanged: the store already holds every local edit (H5).
        if self.nothing_to_save() {
            self.after_save();
            return true;
        }
```

  - In the `Ok(())` arm, replace the lines from `// A tab still spawning keeps its entry, and its layout:` through `self.check_graph_matches_document();` (the housekeeping, 1353-1364) with `self.after_save();`, keeping `return true;`.
  - The doc comment of `save_session` gains: `A save with nothing to write and an unchanged file returns \`true\` without the lock (\`nothing_to_save\`).`

- [ ] **Step 4: Check.** `cargo test -j 8 -p manifold`. Expect all PASS, the two tests and `a_read_only_store_saves_nothing` included. `cargo fmt --all`, the clippy line.

- [ ] **Step 5: Measure, report, commit (if kept).** The CPU and Defender checks, `app_save_costs` in release, report and stop. Keep rule: "save_session, nothing changed" at 2000 tabs twice as fast or more, and the counts as stated.

```bash
git add crates/manifold/src/store/journal.rs crates/manifold/src/session_sync.rs crates/manifold/src/session_sync/tests.rs
git commit -m "Wave 2 (H5): a save with nothing to write skips the store lock when the file is unchanged" -m "Co-Authored-By: <implementer's own line>"
```

---

### Task 14: H1b. Windowed paging (conditional)

**Model:** opus. **Gate alone:** **full** (a stored field). **Proof:**
- `reveal_bounds_rows` gives 22 cards, rows 20, skip 19 980;
- an old view's text round-trips unchanged;
- every JSON test passes;
- `reveal-objects` after.

Runs only if `reveal-objects` was over 1.5 times the pan baseline after Task 4 (Task 3's number if Task 4 did not land: see below).

**Dependency on Task 4.** The steps below build on Task 4's code: `entries_from`, `visible_rows`, `row_of`, `Search::hit_rows`, `CardLook::visible`, the hit-row slicing in `draw_json_level` and `paint_json_content`, and Task 4's `Card` literal in `a_culled_card_paints_only_its_visible_rows`. If Task 4 was skipped (its proceed rule failed) or reverted (its keep rule failed), `reveal-objects` can still be slow, since it is the 200 000-card case H1a does not address. Then:
- the run rule uses Task 3's `reveal-objects` baseline (> 1.5 × the pan baseline);
- the controller picks one of two, and records which in `## Outcome`:
  1. **Park H1b** (recommended when Task 4 failed its keep rule: the row culling it would sit on was judged not worth its cost), with the Task 3 `reveal-objects` number recorded.
  2. **Run with Step 0.** Before Step 1, land Task 4's Step 2 `entries_from` (with `entries` delegating to it) and its `entries_from_starts_at_an_entry` test, alone: not Step 2's import change or other helpers, which would be unused and fail `-D warnings`. Then follow the steps below with these substitutions: no `visible_rows`, `row_of`, `hit_rows` or `CardLook::visible` exists, so Step 5 changes only today's code paths: `draw_json_level` builds its `shown` pointers (`json_level_ui.rs:517-520`) from `entries_from(value, card.skip).take(card.rows)`, so `selected_row` and `hit_rows` stay window rows as today; `paint_json_content` keeps its full row loop (`:376`) over `entries_from(value, skip).take(rows)`, and draws window row `r` (entries, hits and selection alike) at `row_rect(lead + r)`; and there is no culled-test literal to update. Everything else (the data, `reveal`, `show_earlier`, hit-testing) is unchanged.

**Files:**
- Modify: `crates/manifold/src/json_view.rs` (`CardOverride`, `JsonView`, `Card`, `visible_cards`, `reveal`, new `show_earlier`, `CARD_OVERRIDE_KEYS` from wave 1, tests)
- Modify: `crates/manifold/src/json_level.rs` (`JsonEdit`, `apply_json_edit`, `row_at`, `entry_rect`, `more_row_at`, new `earlier_row_at`, the debug export at 440-447)
- Modify: `crates/manifold/src/json_level_ui.rs` (`paint_json_content`, `draw_json_level`, the golden's `Card` literals)
- Modify: `crates/manifold/src/card.rs` (the preview's `Card` literal and `paint_json_content` call)
- Modify: `docs/superpowers/specs/2026-09-27-json-node-rework-design.md` (line 53 and "Outline / reveal", about 82-95)

**Interfaces:**
- Produces:
  - `CardOverride::first: Option<u32>` (serde default, skipped when `None`), after `pages`
  - `JsonView::first(&self, pointer) -> usize` (0 when unset)
  - `pub(crate) const REVEAL_GROW_PAGES: usize = 5`
  - `Card::skip: usize` (entries before the window) and `Card::lead(&self) -> usize` (`usize::from(self.skip > 0)`)
  - `json_view::show_earlier(root: &Value, view: &mut JsonView, pointer: &str)`
  - `JsonEdit::Earlier(String)`, `JsonLevel::earlier_row_at(&self, card: usize, world: Pos2) -> bool`
  - `paint_json_content(…, skip: usize, rows, more, look)`

- [ ] **Step 1: Write the failing tests.**
  - `reveal_bounds_rows`: its doc becomes `/// Wave 2 (H1b): revealing the last of 20 000 objects moves the array's window to it: one page of rows, so one page of cards.`, and its assertion `assert_eq!((cards.len(), items.rows, items.skip, items.more), (22, 20, 19_980, 0));`.
  - Add to `json_view.rs`'s tests:

```rust
    /// Wave 2 (H1b): a view saved before `first` existed reads, lays out and writes back exactly as before.
    #[test]
    fn an_old_view_round_trips_without_first() {
        let text = r#"{"/items":{"collapsed":false,"pages":3}}"#;
        let view = JsonView::from_field(text);
        assert_eq!(view.first("/items"), 0);
        assert_eq!(view.pages("/items"), 3);
        assert_eq!(view.to_field(), text);
    }

    /// Wave 2 (H1b): a reveal near the window grows it as before; a far one moves it; `+ N earlier` grows it up.
    #[test]
    fn reveal_moves_the_window_and_earlier_grows_it_up() {
        let root = json!({"items": (0..1000).collect::<Vec<_>>()});
        let expanded = default_expanded(&root);
        let mut view = JsonView::default();
        reveal(&root, &mut view, &expanded, "/items/90"); // page 4: within 5 of the window's end
        assert_eq!((view.first("/items"), view.pages("/items")), (0, 5));
        reveal(&root, &mut view, &expanded, "/items/900"); // page 45: far
        assert_eq!((view.first("/items"), view.pages("/items")), (45, 1));
        show_earlier(&root, &mut view, "/items");
        assert_eq!((view.first("/items"), view.pages("/items")), (44, 2));
        let items = visible_cards(&root, &view, &expanded).into_iter().find(|c| c.pointer == "/items").unwrap();
        assert_eq!((items.skip, items.rows, items.more, items.lead()), (880, 40, 80, 1));
    }
```

  - `CARD_OVERRIDE_KEYS` (wave 1, M5) becomes `[&str; 5]` with `"first"`, and its guard test's `CardOverride` literal sets `first: Some(3)`.

  Run `cargo test -j 8 -p manifold --bins json`. Expect compile errors.

- [ ] **Step 2: The data.** In `json_view.rs`:
  - `CardOverride` gets, after `pages`:

```rust
    /// The first page shown (H1b): a far reveal moves the window here instead of paging every row in; `+ N
    /// earlier` grows it up. 0 when unset, so a view saved before it reads unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) first: Option<u32>,
```

  - `JsonView::first`: `self.get(pointer).and_then(|o| o.first).map_or(0, |first| first as usize)`.
  - `pub(crate) const REVEAL_GROW_PAGES: usize = 5;` beside `PAGE_SIZE`, doc: `/// A reveal this many pages past a card's window grows the window; farther, it moves it.`
  - `Card` gets `skip: usize` after `collapsed` (doc: `/// Entries before the window: shown as one \`+ N earlier\` row when over 0.`), `rows`' doc becomes `the window's pages`, and:

```rust
    /// The `+ N earlier` row: 1 when the window starts past the first entry.
    pub(crate) fn lead(&self) -> usize {
        usize::from(self.skip > 0)
    }
```

    with `height` counting it: `HEADER_HEIGHT + ROW_HEIGHT * (self.lead() + self.rows + usize::from(self.more > 0)) as f32`.

- [ ] **Step 3: `visible_cards`, `reveal`, `show_earlier`.**

```rust
        let collapsed = is_collapsed(view, expanded, &pointer);
        let len = entry_count(value);
        // The window starts on a page boundary, at the last page at most.
        let skip = if collapsed { 0 } else { (view.first(&pointer) * PAGE_SIZE).min(len.saturating_sub(1) / PAGE_SIZE * PAGE_SIZE) };
        let rows = if collapsed { 0 } else { (len - skip).min(view.pages(&pointer) * PAGE_SIZE) };
        let lead = usize::from(skip > 0);
        let index = cards.len();
        for (row, (key, child)) in entries_from(value, skip).take(rows).enumerate() {
            if is_card(child) {
                // The row as drawn: below the `+ N earlier` row when there is one.
                queue.push_back((child_pointer(&pointer, &key), child, Some((index, lead + row)), depth + 1));
            }
        }
        let more = if collapsed { 0 } else { len - skip - rows };
        cards.push(Card { pointer, parent, depth, collapsed, skip, rows, more });
```

  In `reveal`, replace the `if index >= view.pages(&ancestor) * PAGE_SIZE { … }` block with:

```rust
        let (page, first, pages) = (index / PAGE_SIZE, view.first(&ancestor), view.pages(&ancestor));
        let window = if page < first && first - page <= REVEAL_GROW_PAGES {
            Some((page, pages + (first - page))) // just above: grow up
        } else if page >= first + pages && page < first + pages + REVEAL_GROW_PAGES {
            Some((first, page - first + 1)) // just below: grow down, as before
        } else if page < first || page >= first + pages {
            Some((page, 1)) // far: move the window to it
        } else {
            None
        };
        if let Some((first, pages)) = window {
            let o = view.entry(&ancestor);
            o.first = (first > 0).then_some(first as u32);
            o.pages = (pages > 1).then_some(pages as u32);
        }
```

  and update its doc: `every ancestor expanded and its window holding the next step down (grown when within REVEAL_GROW_PAGES pages, moved otherwise)`. Then:

```rust
/// The `+ N earlier` row: one more page above the window, so it grows up.
pub(crate) fn show_earlier(root: &Value, view: &mut JsonView, pointer: &str) {
    let len = root.pointer(pointer).map_or(0, entry_count);
    let first = view.first(pointer).min(len.saturating_sub(1) / PAGE_SIZE);
    if first == 0 {
        return;
    }
    let pages = view.pages(pointer) + 1;
    let o = view.entry(pointer);
    o.first = (first > 1).then(|| (first - 1) as u32);
    o.pages = Some(pages as u32);
}
```

  `toggle`'s `view.0.retain(…)` and wave 1's `JsonView` shape: follow the code as it is after wave 1 (anchor by quoted text).

- [ ] **Step 4: Hit-testing.** In `json_level.rs`:
  - `JsonEdit` gets `/// The \`+ N earlier\` row.` `Earlier(String),` after `More`, and `apply_json_edit` an arm beside `More`'s: `JsonEdit::Earlier(pointer) => self.edit_json_view(|view, value, _| json_view::show_earlier(value, view, &pointer)),` (follow `More`'s arm's exact shape there).
  - `row_at`: after `let row = …;`:

```rust
        let c = &self.cards[card];
        let row = row.checked_sub(c.lead())?; // the `+ N earlier` row (`?`, not `let … else`: clippy::question_mark)
        let pointer = &c.pointer;
        let value = self.value.pointer(pointer)?;
        let (key, _) = json_view::entries_from(value, c.skip).take(c.rows).nth(row)?;
```

  - `earlier_row_at`:

```rust
    /// Whether `world` is on card `card`'s `+ N earlier` row.
    pub(crate) fn earlier_row_at(&self, card: usize, world: egui::Pos2) -> bool {
        let (Some(rect), Some(c)) = (self.rects.get(card), self.cards.get(card)) else { return false };
        let top = rect.top() + json_view::HEADER_HEIGHT;
        c.skip > 0 && rect.contains(world) && world.y >= top && world.y < top + json_view::ROW_HEIGHT
    }
```

  - `more_row_at`: `json_view::ROW_HEIGHT * (c.lead() + c.rows) as f32`.
  - `entry_rect`: `if row < c.skip || row >= c.skip + c.rows { return None; }` with `let c = &self.cards[card];`, and the drawn row `row - c.skip + c.lead()` in the `top` line.
  - The debug export adds `"skip": card.skip,` after `"rows"`.

- [ ] **Step 5: Paint and click.** In `json_level_ui.rs`:
  - `paint_json_content` takes `skip: usize` before `rows`. Inside: `let lead = usize::from(skip > 0);`; the divider test counts `lead`; `shown` is `json_view::visible_rows(rect.top() + header_height + lead as f32 * ROW_HEIGHT, rows, look.visible)` (window rows); entry `e`'s rect is `row_rect(lead + e - skip)`; the hit slice uses `partition_point` over `skip + shown.start .. skip + shown.end`; `selected_row` (an entry index) draws at `lead + row - skip`; the row loop is `entries_from(value, skip + shown.start).take(shown.len())`, drawn at `lead + shown.start + i`; when `skip > 0`, row 0 paints `format!("+ {skip} earlier")` like the `+ N more` row; the `+ N more` row is `row_rect(lead + rows)`.
  - `draw_json_level`: `row_of(value, &card.pointer, s, card.skip..card.skip + card.rows)`; the hit slice is `&rows[rows.partition_point(|&r| r < card.skip)..rows.partition_point(|&r| r < card.skip + card.rows)]`; the call passes `card.skip`; a click on `level.earlier_row_at(i, p)` pushes `JsonEdit::Earlier(card.pointer.clone())` before the `more_row_at` test.
  - Every `Card` literal gets `skip: 0` (Decision 2): Task 3's golden (`a_card_paints_its_rows_texts`), Task 4's `a_culled_card_paints_only_its_visible_rows`, and `card.rs`'s preview; the preview's call passes `0`. Grep `Card {` in the three files to confirm none is missed.

- [ ] **Step 6: The spec.** In `docs/superpowers/specs/2026-09-27-json-node-rework-design.md`, line 53 (the saved overrides list) gains `first` (the window's first page), and "Outline / reveal" (about 82-95) says a reveal within 5 pages of a card's window grows it, a farther one moves it, and `+ N earlier` grows it up one page. Quote the old and new text in the report.

- [ ] **Step 7: Check, measure, report, commit (if kept).** `cargo test -j 8 -p manifold` (all PASS), `cargo fmt --all`, the clippy line. Release build, CPU and Defender checks, the `reveal-objects` pair (1 and 2 s). Report and stop. Keep rule: `reveal-objects` drops by 50% or more.

```bash
git add crates/manifold/src/json_view.rs crates/manifold/src/json_level.rs crates/manifold/src/json_level_ui.rs crates/manifold/src/card.rs docs/superpowers/specs/2026-09-27-json-node-rework-design.md
git commit -m "Wave 2 (H1b): a far reveal moves a JSON card's window instead of paging every row in" -m "A view's optional first page is saved with its pages; + N earlier grows the window up." -m "Co-Authored-By: <implementer's own line>"
```

---

### Task 15: Mirror link index (conditional)

**Model:** opus. **Gate alone:** **full** (mirror). **Proof:** the index property test and every link test pass; `link_query_cost` before and after.

Runs only if `link_query_cost`'s `valid_targets` at L=100 is still over 1 ms after Task 10.

**Files:**
- Modify: `crates/manifold/src/mirror.rs` (struct 34-43, `set` 63-83, `add_link` 108-113, `forget_link` 116-122, tests)
- Modify: `crates/manifold/src/links.rs:226-237` (`App::links_naming`)
- Modify: the debug check `check_graph_matches_document` (`session_sync.rs`, after 1380)

**Interfaces:**
- Produces: `Mirror::links_at: HashMap<NodeId, BTreeSet<NodeId>>`; private `Mirror::index_link(&mut self, link: NodeId, add: bool)`; `Mirror::links_naming(&self, node: NodeId) -> impl Iterator<Item = NodeId> + '_`; `#[cfg(debug_assertions)] Mirror::check_link_index(&self)`.

- [ ] **Step 1: Write the failing test.** In `mirror.rs`'s tests:

```rust
    /// Wave 2 (mirror index): after random link adds, end writes and forgets, the index names each end's links
    /// exactly as a scan of every link does.
    #[test]
    fn the_link_index_matches_a_scan() {
        use crate::store::testutil::XorShift;
        let pool: Vec<NodeId> = (0..12).map(|c| NodeId { actor: crate::store::model::ActorId(1), counter: c }).collect();
        let (links, ends) = (&pool[..6], &pool[6..]);
        let mut mirror = Mirror::default();
        let mut rng = XorShift(0x11_2233);
        let pick = |rng: &mut XorShift, from: &[NodeId]| from[rng.below(from.len() as u64) as usize];
        for _ in 0..500 {
            let link = pick(&mut rng, links);
            match rng.below(4) {
                0 => mirror.add_link(link),
                1 => mirror.forget_link(link),
                k => {
                    let end = if rng.below(5) == 0 { None } else { Some(pick(&mut rng, ends)) };
                    mirror.set(&link.to_string(), if k == 2 { field::FROM } else { field::TO }, FieldValue::Id(end));
                }
            }
            for &node in ends {
                let scan: Vec<NodeId> = mirror
                    .link_ids()
                    .filter(|l| {
                        let (from, to) = mirror.link_ends(&l.to_string());
                        from == Some(node) || to == Some(node)
                    })
                    .collect();
                assert_eq!(mirror.links_naming(node).collect::<Vec<_>>(), scan, "{node}");
            }
        }
    }
```

  (If `NodeId`'s fields or `ActorId`'s path differ, build the pool with `"0000000000000001-<n>".parse()` instead.) Expect a compile error.

- [ ] **Step 2: The index.** In `Mirror`:

```rust
    /// The links naming each end (`links_naming`), kept by `set` (a known link's `FROM`/`TO`), `add_link` and
    /// `forget_link`, so a query reads one entry instead of scanning every link.
    links_at: HashMap<NodeId, BTreeSet<NodeId>>,
```

```rust
    /// Adds (or removes) `link` under each of its current ends.
    fn index_link(&mut self, link: NodeId, add: bool) {
        let (from, to) = self.link_ends(&link.to_string());
        for end in [from, to].into_iter().flatten() {
            if add {
                self.links_at.entry(end).or_default().insert(link);
            } else if let Some(at) = self.links_at.get_mut(&end) {
                at.remove(&link);
                if at.is_empty() {
                    self.links_at.remove(&end);
                }
            }
        }
    }

    /// The links whose `from` or `to` is `node`, in id order (as `link_ids` lists them).
    pub(crate) fn links_naming(&self, node: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        self.links_at.get(&node).into_iter().flatten().copied()
    }
```

  - `set`: before the `self.nodes.entry(…)` line,

```rust
        let link = matches!(no, field::FROM | field::TO)
            .then(|| id.parse::<NodeId>().ok())
            .flatten()
            .filter(|link| self.link_nodes.contains(link));
        if let Some(link) = link {
            self.index_link(link, false);
        }
```

    and after it `if let Some(link) = link { self.index_link(link, true); }`.
  - `add_link`: inside `if self.link_nodes.insert(id) {`, after the `init_field`, `self.index_link(id, true);` (ends written before the link was known are indexed now).
  - `forget_link`: first line `self.index_link(id, false);`.
  - `#[cfg(debug_assertions)] pub(crate) fn check_link_index(&self)`: rebuilds the map from `link_nodes` and `link_ends` and `assert_eq!`s it with `links_at`. Call it from `check_graph_matches_document`.

- [ ] **Step 3: `App::links_naming` reads it.**

```rust
    /// The Link nodes whose `from` or `to` is `node`, in id order (`Mirror::links_naming`).
    pub(crate) fn links_naming(&self, node: &str) -> Vec<String> {
        let Ok(node) = node.parse::<NodeId>() else { return Vec::new() };
        self.mirror.links_naming(node).map(|link| link.to_string()).collect()
    }
```

- [ ] **Step 4: Check, measure, report, commit (if kept).** `cargo test -j 8 -p manifold` (all PASS), `cargo fmt --all`, the clippy line. `link_query_cost` in release with the CPU and Defender checks. Report and stop. Keep rule: `valid_targets` at L=100 twice as fast or more.

```bash
git add crates/manifold/src/mirror.rs crates/manifold/src/links.rs crates/manifold/src/session_sync.rs
git commit -m "Wave 2 (mirror index): links_naming reads a per-end index kept by the mirror's writers" -m "Co-Authored-By: <implementer's own line>"
```

---

### Task 16: Wave gate, docs, and the merge question

**Model:** controller (the session running the plan). **Gate:** **full**, once for the whole wave. **Proof:**
- the full test suite passes;
- `scripts/gate.sh` reports every run `exit=0` and the smoke test `fail=0`;
- CPU load and Defender state are reported with it.

**Files:**
- Modify: `docs/superpowers/specs/2026-10-07-wave-2-performance-design.md` (Status line)
- Modify: `docs/superpowers/plans/2026-10-07-wave-2-performance.md` (fill in `## Outcome`)

**Interfaces:**
- Consumes: the commits of Tasks 0-15 on `wave-2-performance`, and every bench report.
- Produces: a branch ready to merge, and an Outcome record.

- [ ] **Step 1: Run the whole suite.**

Run: `cargo test -j 8 -p manifold`
Expected: all pass. Record the count.

Run clippy once more, with the CLAUDE.md line.

- [ ] **Step 2: Prechecks.**
  - `df -h /c`
  - `Get-Process python, manifold -ErrorAction SilentlyContinue` (no leftovers)
  - `(Get-MpComputerStatus).RealTimeProtectionEnabled`
  - `(Get-Counter '\Processor(_Total)\% Processor Time' -SampleInterval 2 -MaxSamples 2).CounterSamples[-1].CookedValue`

- [ ] **Step 3: Build, then run the full gate detached.**

```bash
cargo build -j 8 -p manifold
cmd //c start "" //b //belownormal bash scripts/gate.sh "<scratchpad>/wave2-gate.log" full
```

`start //b` returns at once. Watch `<scratchpad>/wave2-gate.log` for the `GATE done` line (Monitor with an until-loop), not the launcher. It takes about 6 minutes on a quiet machine since the wave 1 gate tooling (`WAITIDLE` instead of fixed sleeps; measured 2026-10-08), and more under load.

Expected:
- `GATE two-instance run 1..3 exit=0`;
- `GATE smoke exit=0 … fail=0`.

On a failure, read `<log>.two-N.log` or `<log>.smoke.log`. Under load, rerun the failed part once before calling it a regression, and name what else was running.

- [ ] **Step 4: Docs.**
  - Set the spec's Status to "Implemented 2026-10-0x on `wave-2-performance` (commits …); see the plan's Outcome for the numbers and deviations."
  - Fill in `## Outcome` below:
    - a table of tasks, commits and review findings, with each skipped or reverted part and why;
    - every bench: before and after, best and median, CPU load and Defender state, and each proceed and keep decision;
    - the test count;
    - the gate result with CPU load and Defender state;
    - deviations from this plan;
    - what stays parked.
  - Commit "Wave 2: outcome and spec status" with the controller's own `Co-Authored-By:` line.

- [ ] **Step 5: Ask before merging.** Report the gate result and the bench summary, and ask the user whether to merge `wave-2-performance` into `main`. Don't merge or tag without that go-ahead. Afterwards, check `Get-Process python, manifold` for leftovers.

---

## Self-review against the spec

| Spec item | Where it is covered |
|---|---|
| Decision 1: measure, change, measure; tests-only commit then change commit | Architecture; every pin task and its change task; the decision points |
| Decision 1: harness as its own task | Task 0 (bench), Task 3 Steps 5-6 (`TIME_ONLY`, reveal modes) |
| Decision 1: numbers into the Outcome with CPU and Defender | Global Constraints "Timing runs"; Task 16 Step 4 |
| Decision 2: the split, the conditional index, H5 as a fast path, debounce and M2 dropped, H4 trimmed | Tasks 10, 13, 15; Not covered on purpose: debounce, M2, `CardView` (parked list in Task 16) |
| Decision 3: one branch, per-task tests, one full gate, per-task gate class | Global Constraints; the summary table; Task 16 |
| Decision 4: pins pass before and after; the three timing exceptions | Every pin step; Task 6 (jar), Task 13 (H5), Task 14 (H1b) |
| Decision 5: no real profile; `--profile-dir` under `target/` | Global Constraints "Tests"; Task 3 Step 6 |
| Measuring: `bench.rs`, fixture with `alloc_node_id`, best and median | Task 0; Decision 1 |
| Measuring: `TIME_ONLY` difference of two runs | Task 3 Steps 6-7 |
| H3 size index; reference golden bit-identical; `settle_cost` top and g0 | Tasks 1-2 |
| H3 deviation: no hoist (2b) | Task 2 changes only the index |
| H1a 1-4: cull rows, `entries_from`, `visible_rows`, `row_of`, `hit_rows`, focused reads, preview `EVERYTHING` | Task 4 Steps 2-6 |
| H1a pins: golden, culled, reveal test, Tab walk; `reveal_bounds_rows`; reveal modes | Task 3 Steps 1-3; Task 4 Step 1 |
| H1b: `first`, `REVEAL_GROW_PAGES`, earlier row, `lead + row`, compatibility, `CARD_OVERRIDE_KEYS`, round trip, spec text | Task 14 |
| Jar H3: `jar_dirty`, flush rule (`pull_pending`), quit flush, every writer settles, ephemeral prompt, deleted jar skipped, README | Task 6 Steps 2-6; Decisions 7-8 |
| Jar pins: burst 15 then 0+1, flush timing, existing tests, per-app counter, `jar_save_cost` | Task 5; Task 6 Step 1 |
| Jar deviation: tests-only commit light | Summary table row 5 |
| M1: explicit DFS, one path, number scratch, `contains_lower` both-ASCII rule; kept behaviors | Task 8 |
| M1 pins and `json_search_cost` (14 000 × 7 nodes) | Task 7 |
| H2: hoist with `Option<&HashSet>`, `connect` passes `None`, `edge_menu`, binary search ×2, slot map | Task 10 Steps 1-4 |
| H2 pins: property test (60 cards, 3 levels, 300 connects, ≥ 15 links, half-made and dangling, sorted ids), badge test, `link_query_cost`, `graph_frame_cost` | Task 9 Steps 1-3 |
| H2 keep rules, per part, revert on miss | Task 10; Decision 11 |
| Teardown: forget first, then link deletes, then nodes; end state; pin; `teardown_cost`; full | Task 9 Step 4; Task 11; Decision 9 |
| H5: fast path conditions, `quiet()`, `after_save` from both paths, full path otherwise | Task 13 Steps 2-3; Decisions 5-6 |
| H5 pin: `begins` counter, four actions, 2,1,2,2 then 1,0,1,1; `app_save_costs` at 2000 | Task 12; Task 13 Step 1; Decision 10 |
| H5 deviations: EnterGroup save kept; `destroy_group` comment is wave 1's; tests-only light | Decision 10; nothing in Task 13 touches either; summary table row 12 |
| Mirror index: `links_at`, maintained in `set`/`add_link`/`forget_link`, order, debug check, property test | Task 15 |
| Not in scope | Task 16 Outcome lists what stays parked |
| Risks: wave 1 overlap by quoted text; C1 scan | Spec line in the header; Decision 13; Tasks 6, 11, 14 anchor notes |
| Risks: egui focus semantics | Decision 4; Task 3 Step 3 |
| Risks: timing noise, rerun within 10% | Global Constraints "Timing runs" |
| Risks: jar crash window | Task 6 Steps 5-6 |
| Testing: manual proof (`reveal-*`, `jar-probe.sh`) | Task 3 Step 7, Task 4 Step 8, Task 6 Step 8 |

Placeholder scan: every code step has its code. The only `<…>` left are the implementer's own `Co-Authored-By:` line, the bench name in the generic command, the scratchpad path, and the commit list in Task 16's Status line, each filled in at run time. Task 14 (conditional, opus) gives the data, `visible_cards`, `reveal`, `show_earlier` and hit-testing code in full and the painting change as exact rules, since its exact text depends on Task 4's final code; its "Dependency on Task 4" paragraph gives the rules for when Task 4 did not land.

Type consistency:
- `bench::{best_median, timed, report, Fixture, level_fixture, spread}`: Tasks 0-1, used in 1, 5, 7, 9, 12 (local helpers stay in `app_save_costs`).
- `entries_from(&Value, usize)`, `visible_rows(f32, usize, Rect) -> Range<usize>`, `row_of(&Value, &str, &str, Range<usize>) -> Option<usize>`, `hit_rows_by_parent(&Value, &HashSet<String>) -> HashMap<String, Vec<usize>>`: Task 4, reused in Task 14 (if Task 4 did not land, Task 14's Step 0 lands `entries_from` alone and the others are not used).
- `Search::{hit_rows, set_result}`, `CardLook::visible`: Task 4.
- `App::jar_saves` (Task 5), `App::jar_dirty`, `flush_dirty_jars(Option<Instant>)`, `JAR_FLUSH_AFTER`, `save_jar(&mut self)` (Task 6).
- `check_connect(.., Option<&HashSet<NodeId>>)`, `linked_to`, `linked_for`, `CardPass::slots` (Task 10).
- `Journal::begins` (Task 12), `Journal::quiet`, `nothing_to_save`, `after_save` (Task 13).
- `CardOverride::first`, `Card::{skip, lead}`, `show_earlier`, `JsonEdit::Earlier`, `earlier_row_at` (Task 14).
- `Mirror::{links_at, index_link, links_naming, check_link_index}` (Task 15).

Stress test (adversarial passes over both documents, re-reading the code each time). Seven rounds; the fourth and the seventh found nothing material (rounds 5-7 followed an independent cross-check).

- Round 1 (spec, while drafting the plan):
  - `row_of` and the hit rows check the canonical pointer, or `/arr/02` would light row 2;
  - `set_result`'s call inside `receive_json_search` borrows `pending` (NLL ends it, as the existing `pending = None` shows; fallback given);
  - the property test asserts at least 15 links, or it could pass while testing nothing;
  - teardown forgets links first: moving the link loop alone leaves O(L²);
  - the jar round test is `pull_pending`, not an empty `jar_pulls` (timed-out pulls keep their entry);
  - `quiet()` needs `seen`, which is unset before the first read;
  - the search bench's items are 7 nodes each (14 000 stays under the worker threshold);
  - EnterGroup keeps its save (Decision 10);
  - tests-only commits are light;
  - `row_of`'s argument order matches its callers;
  - the save counts are asserted by name;
  - moving the fixture leaves unused imports in `graph_nav`;
  - the frame bench's `camera.jump_to(app.…)` relies on a two-phase borrow, as the existing harness test does;
  - the golden's fail-proof uses a non-commutative mutation (`+ 0.001`);
  - `app_on` takes the session file's path;
  - a `TIME_ONLY` run inherits BelowNormal from `start`.
- Round 2 (plan against the code):
  - `spread` moves to Task 1 and Task 0 reads `f.nodes`, so nothing is dead code under `-D warnings`;
  - the card border uses the card's whole hit slice, not just the painted rows;
  - `card_nav_active` is set right before the measured badge frame: the pass reads it at its start and resets it at its end (`graph_ui.rs:1672`, `:1751`);
  - `save_jar` becomes `&mut self`, so the dirty mark is settled in one place;
  - wave 0's `allow_attributes_without_reason` means any new `#[allow]` needs a reason (added to the spec's Risks);
  - the flush test's dirty mark is at or after `t0`, so the due check uses `JAR_FLUSH_AFTER + 100 ms`;
  - `final_cookie_pull` is called only from `exiting`, so the quit flush goes there.
- Round 3 (plan against the code, every API the steps call):
  - the badge test reads the drawn rect from `level_items()`, as the existing pick-mode test does, not `card_rect`;
  - `map_or(&[], …)` would infer an array type, so it is `&[][..]`;
  - a JSONSELECT with no level open is silent, and a 200 000-entry file may not load within the existing one-second sleep. Reveal runs now sleep 5 s, write `EXPORTSTATE`, and log `selected=`; an empty value means the run timed nothing;
  - `json_level.rs` gets `HashMap` through `use super::*;` (fallback noted).
  - Confirmed with no change needed:
    - `mirror.set` is the only writer of `nodes`, so the index hook in `set` covers `init` and `init_field`;
    - `create_link` adds the link before it writes its ends;
    - `CardOverride::is_empty` compares with `Default`, so `first` needs no change there;
    - `queue_delete` always inserts into `pending_deletes`, so the teardown pin's `want` includes the unsaved link;
    - settling queues no edits, so Enter goes from 1 save to 0;
    - `SearchResult` derives `PartialEq`.
- Round 4: a full pass found nothing material. Every step's code, anchors, counts and expectations were rechecked against the files, and the spec's thresholds, deviations and risks against the plan's table and tasks.
- Round 5 (an independent cross-check, `.superpowers/survey/xcheck-wave-2.md`, each finding re-verified against the code and egui 0.36.2's source before fixing). Rounds 1-4 had missed these:
  - Task 3's Tab walk sent keys in consecutive frames. `set_focus_lock_filter` needs `had_focus_last_frame`, so the newly focused card's filter is set a frame late and the second Tab would also run egui's own move (card 1 to card 3). An idle frame now follows each key; Decision 4 and the spec's focus text state the exact rules, with the consecutive-frame skip parked as today's behavior;
  - `i % n == 0` / `!= 0` on `usize` in Tasks 0, 1 and 9 fail `clippy::manual_is_multiple_of`: now `is_multiple_of`, with the loop and closure types spelled out (E0689), and a Global Constraints rule;
  - Task 14's `let Some(row) = … else { return None }` fails `clippy::question_mark`: now `?`;
  - Task 14 relied on Task 4's helpers without saying what happens if Task 4 is skipped or reverted: a "Dependency on Task 4" paragraph (park, or Step 0 landing `entries_from` alone), the decision point, the table row and the spec's run rule;
  - minors: Decision 2 and Task 14 Step 5 name Task 4's `Card` literal; Task 12's light label says why it differs from CLAUDE.md's store class; the spec's H1b compatibility notes that a wave-1 build's `pages` from page 0 widens an H1b window; the jar flush test takes its deadlines from the recorded dirty `Instant` (superseding Round 2's `+ 100 ms` margin, which could fail under load on the "due" check). The cross-check's line-drift note (Decision 1) was rejected: `fn level_fixture` is at 523 as stated; the doc comment above it is now mentioned.
- Round 6: a full pass over both revised documents. Caught: Task 14's Step 0 must not take Task 4's import change (unused imports under `-D warnings`); the no-Task-4 paint rule had to match today's `shown` code (`json_level_ui.rs:517-520`, `:376`), with hits and selection as window rows. Also checked: egui's `EventFilter::matches` ignores modifiers for Tab, so the idle frame's `tab: true` filter also stops egui's Shift+Tab move.
- Round 7: a full pass found nothing material.

---

## Outcome

Implemented 2026-10-08 on `wave-2-performance`, base `64c56fc` (the plan refresh).

| Task | Item | Commit | Kept? | Review findings |
|---|---|---|---|---|
| 0 | bench module and 2000-node fixture with store ids | `2ae8f76` | pins | none; minor: report column width 44 vs 40 |
| 1 | H3 pins: `step_sized` golden, `settle_cost` | `b6542bf` | pins | none; minor: golden skips `dt <= 0` |
| 2 | H3 size index in `step_sized` | `36338f7` | kept | none; minor: committed where the brief said stop (see Deviations) |
| 3 | H1a pins, `reveal-*` modes in `json-pan-profile` | `9f7018c` | pins | minor: script header comment omits `selected=`; the pan baseline was taken under load |
| 4 | H1a rows culled to the camera | `6ff1740` | kept | minor: hit-row fill culling and the selected-row path have no paint-level test |
| 5 | Jar pins: `jar_saves`, burst, `jar_save_cost` | `2da1360` | pins | none (batched review) |
| 6 | Jar H3: one save per pull round | `285eadf`, `6d6b759` | kept | Important test gap (the round-end save before the jar was due was never exercised), fixed in `6d6b759` along with the minors |
| 7 | M1 pins: `search_matches_reference`, `json_search_cost` | `01eb4f0` | pins | none (batched review) |
| 8 | M1 search walks with one path, no per-node allocation | `9444b65` | kept | none |
| 9 | H2 pins: `valid_targets` property test, badge test, benches | `31f9829` | pins | none (batched review) |
| 10 | H2 hoisted linked set, `edge_menu` stops at the first target | `6fd74fc`, `c1554a5` | steps 1-2 kept; steps 3-4 reverted | low: a test doc comment over 120 columns, rewrapped in `c1554a5` |
| 11 | teardown deletes links first | none | not kept | n/a (reverted before review) |
| 12 | H5 pins: `Journal::begins`, `app_save_costs` at 2000 | `e95be81` | pins | none (batched review) |
| 13 | H5 no-op save fast path | none | not kept | n/a (reverted before review) |
| 14 | H1b: a far reveal moves the card's window; `+ N earlier` | `bf11332`, `8b2060d` | kept | Low x3: `reveal` reads the unclamped `first`; a 130-column comment; no tests with `skip > 0`. `8b2060d` added the tests, the rewrap and the README paging line |
| 15 | mirror `links_at` index | none | skipped | n/a |
| 16 | gate, docs | this record | n/a | n/a |

Skipped and reverted parts, with why:

- **Task 10 step 3 (binary search in `pick_targets`)**: pick-target frame best 1.4-1.5 ms before and after, under the 5% rule. Reverted.
- **Task 10 step 4 (Tab-slot map)**: idle top-level frame best 0.88 ms (twice) before, 0.93-0.96 ms after, under the 5% rule. Reverted.
- **Task 11 (teardown links first)**: 1.69-1.78x over two alternating A/B rounds (best 23.1 to 13.0 ms), below the 2x rule. Reverted, no commit.
- **Task 13 (no-op save fast path)**: 1.71-1.81x at 2000 tabs over four alternating pairs (241-267 to 136-147 us), below the 2x rule. Reverted, no commit. While the change was in, the full suite passed, and the red and mutation checks failed the new tests as they should.
- **Task 15 (mirror link index)**: skipped. After Task 10, `valid_targets` at L=100 is 0.50 ms, under the 1 ms trigger.

Benches (release). "Contended" means the CPU was busy with other sessions' jobs, so read the number as an upper bound. Defender real-time protection was off (`False`) at every check. Numbers are best / median where the report gave both.

| Bench | Before | After | CPU | Decision |
|---|---|---|---|---|
| `settle_cost`, g0 (166 cards) | 48.6 / 50.0 ms (Task 1 baseline); 56.6 / 58.6 ms (Task 2, same session) | 10.8 / 11.4 ms | 5% (Task 1); 10-50%, noisy (Task 2) | Task 1: over 8 ms, so Task 2 proceeds. Task 2: g0 best down about 81% (rule 25%), kept |
| `settle_cost`, top (675 cards) | 782.0 / 797.2 ms (Task 1); 904.3 / 975.8 ms (Task 2, same session) | 131.1 / 281.0 ms | as above | about 86% down, kept |
| `json-pan-profile` pan, ms per command | 31.7 (Task 3); 37.9 (Task 4 before) | 34.8 (Task 4 after) | 88% and 37% (Task 3); 92% before and 24% after (Task 4) | baseline |
| `reveal-numbers`, ms per command | 893 (Task 3); 1079 (Task 4 before) | about 49 (three-point; 36.3 for the 1 to 2 s pair) | 18% (Task 3); 48% before, 22-65% after (Task 4) | Task 3: over 1.2x pan, so Task 4 proceeds. Task 4: about 95% down (rule 30%), kept |
| `reveal-objects`, ms per command | 700 (Task 3); 795 (Task 4 before); 537 and 782 (Task 14 before) | 453 (Task 4 after, 13x pan); 22.5 and 23.3, noisy (Task 14 after) | 11-95% (Task 3); 38% before, 51% after (Task 4); 27-93% (Task 14) | After Task 4 still over 1.5x pan, so Task 14 proceeds. Task 14: over 95% down (rule 50%), kept |
| `jar_save_cost` (3000 cookies) | 42.2 / 44.6 ms (Task 5); 55.1 / 67.4 ms (Task 6 A) | 65.5 / 71.2 ms (Task 6 B) | 88-100% (Task 5); about 75% and about 92% (Task 6), contended | the per-save cost is unchanged; the change cuts the count: a 15-pull burst goes from 15 saves to 1 (rule: exactly one), kept |
| `json_search_cost`, "item 9" (14 000 items) | 31.2 / 34.0 ms (Task 7); 29.5 / 37.5 ms (Task 8 before) | 4.5 / 6.4 ms | 31-100%, contended (Task 7); 20% before, 35% after (Task 8) | Task 7: over 8 ms, so Task 8 proceeds. Task 8: about 85% down (rule 25%), kept |
| `json_search_cost`, "zzz" | 32.0 / 37.4 ms (Task 7); 31.0 / 38.4 ms (Task 8 before) | 4.3 / 5.4 ms | as above | kept |
| `link_query_cost`, `valid_targets` L=20 / 100 / 300 | 3.7 / 7.7 / 20.4 ms best (Task 9, contended); 1.1 / 3.3 / 11.4 ms best (Task 10 A/B) | 510 / 502 / 557 us best | 40-100% (Task 9); 10-23% (Task 10) | Task 9: L=100 over 1 ms, so Task 10 proceeds. L=100 6.6x (rule 2x), hoist kept. Afterwards L=100 is 0.50 ms, under the 1 ms trigger, so Task 15 is skipped |
| `link_query_cost`, `has_valid_target` L=20 / 100 / 300 | 7.3 / 19.8 / 70.2 us best (Task 10 A/B) | 4.5 / 16.8 / 46.7 us best | 10-23% | kept |
| `graph_frame_cost`, node menu open | 12.0 / 23.4 ms (Task 9, contended); 4.3 ms best (Task 10 A/B) | 1.3 ms best | 40-100%; 10-23% | `edge_menu` kept. The ratio overstates: the first baseline run was under heavier load (all frames 1.3-2.9 ms vs about 0.9 ms later). The change is a strict reduction (first hit instead of a full scan, called per frame) |
| `graph_frame_cost`, other rows (Task 9 baseline, contended) | idle top 4.9 / 7.7 ms; hovering a card 5.9 / 9.7; pick-target 7.7 / 9.6; filtered 4.2 / 6.3; idle first group 3.1 / 5.5 | n/a | 40-100% | baseline |
| Task 10 steps 3 and 4 (best) | pick-target 1.4-1.5 ms; idle top 0.88 ms (twice) | 1.4-1.5 ms; 0.93-0.96 ms | not recorded | both reverted (rule 5%) |
| `teardown_cost` (2000 notes, 300 links) | 51 / 72 ms (Task 9, contended); 25.6 / 29.5 ms (Task 11 round 1) | 14.5 / 17.5 ms (round 1); round 2 alternating, best: before 23.1 and 23.4 ms, after 13.0 and 14.3 ms | about 31% before and 70% after (round 1); 21.9-33.2% on accepted round 2 runs | 1.69-1.78x, rule 2x not met, not kept |
| `app_save_costs`, "save_session, nothing changed", 2000 tabs | 677.6 / 767.4 us (Task 12, contended); 241.0-267.2 us best (Task 13, alternating base) | 135.6-147.3 us best (Task 13, alternating new) | 90-100% (Task 12); 23-26% (Task 13) | 1.71-1.81x, rule 2x not met, not kept |
| `app_save_costs`, other sizes | 300 tabs 124 us, 3000 tabs 355 us best (Task 13 base) | 31.5 us (about 3.9x), 210 us (about 1.7x) | as above | not kept |
| `app_save_costs`, other rows (Task 12 baseline) | snapshot of every node 4.4 / 7.3 ms; one url changed 895.3 us / 1.1 ms; `local_state` 2.6 / 3.1 ms; `maybe_write_local` 3.6 / 4.7 ms | n/a | 90-100% | baseline |

Saves per action (Task 12, `begins`): menu Delete 2, enter a group 1, Join drop 2, finish a link 2. Task 13 got these to `[1, 0, 1, 1]`, but that change was reverted, so the pins stay at these counts.

**Tests:** 1034 passed, 0 failed, 11 ignored (`manifold`).

**Gate (full, run 2026-10-08 18:22-18:28):** three two-instance runs passed (exit=0 each) and the smoke test passed (exit=0, pass=48 fail=0). CPU 24-57% during the run, 46.3% at the end. Defender real-time protection off (`False`). Two earlier full gates on the same branch also passed: after Task 6 (3x ok, smoke 48/0, CPU 18-70%, Defender off) and after Task 14 (3x ok, smoke 48/0, CPU 66.8% at the start and 25.6% at the end, Defender off).

**Deviations from the plan:**
- Decision 11 was simplified by the controller on 2026-10-08: an implementer commits only when the keep rule is met, and otherwise reverts and reports. The plan had implementers stop before the commit.
- Task 2 was committed by its implementer past a stop point: its brief said to stop before committing; the keep rule was met, so the review found it harmless.
- Tasks 4 and 14 wrote their tests together with the implementation, not compiled red first.
- Task 0 kept `use crate::store::model::Kind;` because other tests still use it. Fixture ids are random store ids, so tests take them from `Fixture`, never `"g0"` or `"n{i}"`.
- Task 9 times `has_valid_target` with `{ black_box(...); }` instead of `drop(black_box(...))` (rustc `dropping_copy_types`). Task 5's `jar_save_cost` uses `Jar { cookies, ..Jar::default() }` (clippy `field_reassign_with_default`).
- Co-author lines differ from the dispatch text: Task 0 used `Claude Haiku 5.5`, and Task 14 used `Claude Opus 5.5` (the model that did the work) where the dispatch said `Sonnet 5.5`.
- Task 14 changed `docs/superpowers/specs/2026-09-27-json-node-rework-design.md` at line 14 (the overrides list) as well as the Paging bullet near line 53, because the brief's "line 53, the saved overrides list" was ambiguous. It also updated that spec's Outline and "Saved view state" notes.
- Task 11 and Task 13 were re-measured as alternating A/B runs gated on CPU under 35% after the first back-to-back figures were unclean; the verdicts come from the alternating runs. Task 11 had only two accepted runs per side because the machine was loaded.

**Parked:**
- Debounce, M2 and `CardView`, per the plan (not in scope).
- Task 11, teardown links first: 1.7-1.8x, a real gain below the 2x rule. Revisit only if teardown shows up in a profile.
- Task 13, no-op save fast path: the store part (`begin`) went to zero, but about 140 us at 2000 tabs remains in `after_save` housekeeping (`mirror.retain`, `layout.retain`, `held_text.retain`, `flush_history`). Extending the fast path to skip it needs an audit of whether any node leaves `nodes` without going through `queue_delete`. The absolute cost is about 245 us per no-op save at 2000 tabs.
- Task 14 (H1b): a click on `+ N earlier` was never tried in the running app. It is covered by unit tests of `show_earlier` and the hit-test arithmetic and by the gate's smoke test, and no headless command clicks the row.
- Task 14, Low: `reveal` compares against the unclamped stored `first`, so after the array shrinks, a reveal in the clamped window can move the window to the same page alone. Harmless; clamp in `reveal` if wanted.
- Timing caveat: most remaining timings were taken under CPU contention from other sessions (the CPU column above), so treat them as upper bounds.
