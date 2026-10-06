# Node model 2b-6: live-state enums go, the card is its type Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the live-state enums `NodePayload` and `ItemNodeKind` (and the inner kind of `LevelItemKind`) with the
document node's type (`Mirror::ty`) plus a runtime table, delete the caps bridge, change EXPORTSTATE `cards[].kind` and
the scripts that read it, and add a grep guard that fails if any source names the deleted names again.

**Architecture:** A card's identity is `CardKind { ty, tags }`, computed from `Mirror::ty` and the runtime entry, so every
per-kind decision reads the capability and edge tables (`caps/`) and never an enum. The per-kind live data (webview,
edit buffer, decoded image, ...) stays in `App::nodes` as `enum Runtime`: the old payload variants minus the document
fields, minus `Group`. The accessors (`as_tab` and friends) keep their names, so about 240 call sites barely change.
`Kind` (the store's type classifier) and `ContentKind` stay (Rulings 1, 4).

**Tech Stack:** Rust, egui, the `store` module (`Kind`, `TYPES`), `Mirror`, `caps/` tables; bash and python probe scripts.

**Spec:** `docs/superpowers/specs/2026-10-04-node-model-design.md` (sections 2, 6, "Rename plan and order": the "2b" bullet).
Survey (git-ignored, regenerate if lost): `.superpowers/scratch-2b6-map.md`. Previous plan and its Outcome:
`docs/superpowers/plans/2026-10-05-node-model-2b-5-sync-completion.md`. Sync map: `docs/superpowers/specs/2026-10-05-sync-flow-map.md`
("After 2b-5").

## Global Constraints

- Work on `dev`; never push, never merge to `main`. Run `cargo fmt --all` before each commit; the pre-commit hook runs
  `cargo fmt --all --check` and `cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 --all-targets -- -D warnings`. Never `--no-verify`; do not silence a lint, fix it or `#[allow]` it with a comment saying why.
- No test may read or write the real `%APPDATA%\manifold` (`profile::dir()` is `None` under `cfg(test)`); tests that need files use `store::testutil::temp_dir`.
- Every task leaves the app compiling, clippy clean and `cargo test -j 8 -p manifold --bins` green; run `-p ipc-protocol` separately (43 tests). Baseline at the start: 869 passed, 4 ignored.
- Run long commands in the background (120 s foreground limit); `cargo build -j 8 -p manifold` before any probe script (the scripts run the existing exe).
- Unit tests cannot construct a live `EngineTab`; tests use sleeping tabs.
- Behavior-neutral until Task 5: no card, menu, drop, draw-order or save/restore behavior changes. Task 5 changes only the debug export and the scripts.
- Parked, not in this plan (Rulings 1, 4, 6, 7, 8): store `Kind` replacement, `ContentKind` replacement, raw TYPE, JsonLeaf/FROM_JSON/KEY/VALUE retirement, the unknown-type card.

---

## Rulings (controller, 2026-10-06; the user chose 1, 2, 4, 6 in chat)

1. **`Kind` (store) stays.** 2b-6 replaces `NodePayload`, `ItemNodeKind` and `LevelItemKind`'s inner kind only. `Kind` is
   already a thin view over the `TYPES` table (`from_type`, `type_str`, `fields`). Rule for all new code: readers use
   `Mirror::ty` and `caps(type)`, never a new `Kind::` match, so the later swap to type strings (sub-project D2) stays
   mechanical. Cost if wrong: the grep guard cannot ban `Kind`, and the open-type goal for plugins is only partly met until D2.
2. **Runtime is one enum.** `NodePayload` becomes `enum Runtime { Tab(TabPayload), Unloaded(UnloadedTabPayload),
   Download(DownloadPayload), Note(NotePayload), Image(ImagePayload), Pdf(PdfPayload), Json(JsonPayload), None }`, held by
   `GraphNode { runtime }` in `App::nodes` (which stays the "shown in the graph" set). A group gets `Runtime::None`.
   `GroupPayload` is deleted. Payload struct names stay. `as_group(id)` call sites (22) become an edge-table container
   check (`is_container(id)`). Cost if wrong: splitting the enum into per-family maps later (see Deferred) rewrites the
   about 40 data-access `match` sites and the Tab/Unloaded swap code, not the accessor call sites.
3. **A card's identity is `CardKind { ty: &'static str, tags: TagSet }`**, `Copy`, computed by `App::card_kind(id)` from
   `Mirror::ty` canonicalised through `Kind::from_type(..).type_str()` plus `Tag::Loaded` when the runtime is `Tab`. This is
   the bridge's `(mime, TagSet)` output (`caps/bridge.rs` `item_node`) without the enum. `Tag::Loaded` gets its producer here
   (otherwise clippy flags it never-constructed once the bridge is gone). A leaf JSON card has `ty = TYPE_JSON_LEAF`; where
   the old code treated Json and JsonLeaf alike (`ItemNodeKind::Json`), the golden test (Task 1) pins that.
4. **`ContentKind` stays** (content dispatch on fetched mime, not node identity); it is exempt from the guard until
   sub-project D (views registry). Cost if wrong: 73 more sites in a later plan.
5. **Decisions move to the tables; data access stays.** Moves onto rows/edges: draw order (`Caps.draw_layer`, replaces
   `GRAPH_DRAW_ORDER`), container/close/drop decisions (edge table `CONTAINS`, already used by Recently Closed), the
   per-type menu and verb decisions that are pure per-type `match`es. Stays a `match` on `Runtime` (it reads payload data):
   `node_card_size` (image dimensions, pdf, json preview), `card_filter_text`, error marking, file-path and download
   handling. Correction to the earlier chat estimate: card size and filter text are data reads, not table decisions; only
   the base size per type could be a row field, and that is not worth a field today.
6. **TYPE stays canonical in the mirror** (`image/jpeg` reads as `image/png` until sniffed types arrive with D).
   Parked explicitly; `CardKind.ty` uses the canonical string.
7. **JsonLeaf, FROM_JSON/KEY/VALUE and `JsonPayload.leaf` stay.** `leaf` remains a runtime flag copied from the type.
   Retiring the leaf mechanism is its own change. Cost if wrong: one more small plan.
8. **No unknown-type card in 2b-6.** `Runtime::None` makes it possible (an unknown node is shown with no runtime); the
   `*/*` card itself is a later change. Spec 6.9's "is 2c" note is amended in Task 6 to point at this.
9. **EXPORTSTATE (provisional, Task 5).** Each `cards[]` entry gets `"type"` (the mime from `CardKind.ty`), `"loaded"`
   (bool; true only for a live tab) and `"json_card"` (bool); the `"kind"` key is removed. Scripts that said
   `kind == "Tab"` now check `type == TYPE_TAB and loaded`, `"Unloaded"` is `type == TYPE_TAB and not loaded`, `"JsonCard"`
   is `json_card`, and `"Json"` is the JSON type with `not json_card`. Cost if wrong: another edit of the same script lines.
10. **Per-frame path.** `graph_ui` scans `nodes_at_current_level` eight times per frame (one pass per kind). Replace it with
    one pass that buckets by `draw_layer`. `card_kind(id)` is computed per call from the mirror (one hash lookup, no cache);
    Task 3 measures the pass with an `#[ignore]`d unit benchmark (2000 test nodes, 200 passes, old eight-pass scan versus the new single pass; `scale-test.sh` measures restore time, not frame time, so it cannot be used) and records the numbers. Cost if wrong: a
    cached `CardKind` field on `GraphNode`, set wherever the runtime is replaced.
11. **Tests build nodes through one helper.** `App::test_node(id, kind: Kind, runtime: Runtime)` (cfg(test)) places a node with
    its TYPE seeded in the mirror; the ~130 test sites that construct `NodePayload` literals use it, so Tasks 2-4 touch tests once.
12. **Guard needles.** The guard (Task 6) bans the identifiers `NodePayload`, `ItemNodeKind`, `LevelItemKind`,
    `item_node_kind`, `GRAPH_DRAW_ORDER`, `GroupPayload` and the module `caps::bridge` in `crates/manifold/src` (comments
    count; docs, README and scripts are outside `src`). It exempts only its own file.

## Deferred: homogeneity directions this plan deliberately leaves open

Recorded so a new session does not re-derive them (also in the spec, "Deferred from 2b-6").

- **`Kind` to type strings** (D2, external plugins): `Node::kind()`/`is_live`/`OpBody::create`/JSON import-export keyed by
  type; the `TYPES` table's `Kind` column goes. Mechanical if Ruling 1's rule holds.
- **`ContentKind` to the views/caps registry** (D): fetched content picks its node type through registered rows, not a
  closed enum.
- **`Runtime` as per-family maps or a plugin runtime slot** (D2): a plugin type's live state is an additive variant (a boxed
  `dyn Any`) now; per-family maps are a local rewrite behind the accessors if the enum ever hurts.
- **Raw TYPE** (D): store the real mime, match `image/*` through the caps matcher; needs sniffing.
- **Unknown-type card, JsonLeaf/FROM_JSON/KEY/VALUE retirement** (Rulings 7, 8).
- **Selection state** (`selected_tabs`) and runtime maps keyed by id (`layout`, `held_text`, ...) stay as they are; unifying
  them into the runtime table is not part of this work.

---

## File structure

- `crates/manifold/src/card_kind.rs` (new): `CardKind`, `App::card_kind`, `legacy_kind_name` (Task 5 only), `is_container`.
- `crates/manifold/src/caps/rows.rs`, `caps/pattern.rs`: `Caps.draw_layer`; `Tag::Loaded` documented as produced by `card_kind`.
- `crates/manifold/src/node.rs`: `NodePayload` becomes `Runtime`; `GroupPayload`, `ItemNodeKind`, `item_node_kind`, `GRAPH_DRAW_ORDER`, `store_kind` go.
- `crates/manifold/src/card.rs`, `node_menu.rs`, `graph_ui.rs`, `tab_view.rs`, `graph_nav.rs`, `tabs.rs`, `level.rs`, `main.rs`, `debug_control.rs`, `popups.rs`: readers migrate.
- `crates/manifold/src/caps/bridge.rs`: deleted (Task 2).
- `crates/manifold/src/fetched.rs`, `session_sync.rs`, `closed.rs`, `jar_defs.rs`: `place_node` and `insert_materialized` take the type explicitly.
- `crates/manifold/src/session_sync/tests.rs` and per-file test modules: `test_node` builder.
- `scripts/chrome-shots.sh`, `headless-smoke-test.sh`, `json-level-probe.py`, `page-kind-probe.sh`, `sleep-probe.sh`, `scale-test.sh` (Task 5).
- `crates/manifold/src/no_old_enums.rs` or a test module in `profile.rs` style (Task 6): the grep guard.
- Docs: spec 2b note and "Deferred from 2b-6", sync map, README current-architecture lines, this plan's Outcome.

---

### Task 1: CardKind, draw layer, test builder (additive)

**Files:**
- Create: `crates/manifold/src/card_kind.rs`
- Modify: `crates/manifold/src/main.rs` (module declaration), `caps/rows.rs`, `caps/pattern.rs`, `node.rs` (test helper only)
- Test: `crates/manifold/src/card_kind.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `pub(crate) struct CardKind { pub ty: &'static str, pub tags: TagSet }` (`Copy, Clone, PartialEq, Eq, Debug`);
  `impl App { pub(crate) fn card_kind(&self, id: &str) -> Option<CardKind> }` (None when the id is not in `self.nodes`);
  `Caps.draw_layer: u8` (0 draws first; same order as `GRAPH_DRAW_ORDER`: Tab, Unloaded, Download, Note, Image, Pdf, Json, Group);
  `registry().caps(ty, tags).draw_layer`; `#[cfg(test)] impl App { pub(crate) fn test_node(&mut self, id: &str, kind: Kind, runtime: NodePayload) }`.
  Until Task 4 renames the enum, `test_node` takes `NodePayload`; Task 4 changes only the type name.

- [ ] **Step 1: Failing golden test.** For every constructible payload (sleeping tab, download, note, image, pdf, json, json leaf, group) place it with a type and assert `card_kind(id)` equals `caps::bridge::item_node(item_node_kind(&payload))` for the non-leaf kinds, and that a Tab runtime has `Tag::Loaded` and Unloaded does not. A second test asserts `draw_layer` ordering equals the index in `GRAPH_DRAW_ORDER` for each `ItemNodeKind`, so the table cannot drift.
- [ ] **Step 2: Run, expect compile failure** (`card_kind`, `draw_layer`, `test_node` undefined): `cargo test -j 8 -p manifold --bins card_kind`.
- [ ] **Step 3: Implement.** `card_kind` reads `self.mirror.ty(id)`, canonicalises with `Kind::from_type(ty).type_str()` (falls back to the unknown row string for `Kind::Unknown`), and adds `Tag::Loaded` iff `self.nodes[id].payload.as_tab().is_some()`. `Caps` gets `draw_layer` set in each of the nine rows of `caps/rows.rs`, default for the `*/*` row = 0. `test_node` calls `place_node`-equivalent seeding (`mirror.init_field(TYPE, ..)`) plus inserts the node, exactly as `insert_placeholder_node` does today, so tests keep their behavior.
- [ ] **Step 4: Run focused tests, then the full suite** (background): 869 + the new tests green, `-p ipc-protocol` 43.
- [ ] **Step 5: Commit** `CardKind: a card's identity from its type and runtime`.

### Task 2: Non-test readers move to CardKind; the bridge goes

**Files:** Modify `card.rs` (43 `ItemNodeKind` sites, 4 bridge calls at the `CardControl::action`, `drop_action` sites), `node_menu.rs` (11), `graph_ui.rs` (18 + 4), `main.rs` (16 + 1), `tab_view.rs` (9), `graph_nav.rs` (4), `tabs.rs` (3), `level.rs` (4), `debug_control.rs` (2); Delete `caps/bridge.rs` and its `mod` line in `caps.rs`.

**Interfaces:**
- Consumes: Task 1's `CardKind`, `App::card_kind`, `Caps.draw_layer`, `test_node`.
- Produces: `fn is_container(ty: &str) -> bool` (`edges().target_accepts(CONTAINS, ty)`), `fn can_contain(member: CardKind, group: CardKind) -> bool` in `card_kind.rs` replacing the bridge pair; type positions that held `ItemNodeKind` hold `CardKind` (`CardView.kind`, `DropTarget::Card(CardKind)`, `NodeMenuConfig.kind`, `card_hint: Option<(CardKind, Rect)>`, `LevelItemKind::Node(CardKind)`).

- [ ] **Step 1: Per-file, compiler-led.** Convert type positions first (they fail to compile), then each `match kind { ItemNodeKind::X => .. }`: decisions on container, close verb, drop target and draw order use `is_container`, `can_contain`, `caps(..).draw_layer`; per-type menu entries match on `card_kind.ty` against the `TYPE_*` constants (`store::model`), with Tab vs Unloaded decided by `Tag::Loaded`. Keep `ItemNodeKind` and `item_node_kind` alive until Task 3 for the test sites; add `impl From<ItemNodeKind> for CardKind` only if the tests still need it, and delete it in Task 3.
- [ ] **Step 2: Container rule.** The carry-over sites that hard-code `Group` (`card.rs` `del_verb`, `graph_ui.rs` click and hint tables, `main.rs` `destroy_node`, `session_sync.rs` load order) use `is_container`/the edge table. Do not change `session_sync.rs` load order logic beyond reading the type from the mirror (it is `Kind`-based and stays, Ruling 1).
- [ ] **Step 3: Delete the bridge** (`caps/bridge.rs`, its three tests move into `card_kind.rs` tests over the same `KINDS` list: `is_container`/`can_contain` agree with the old results for every pair).
- [ ] **Step 4: Verify.** Full suite; `cargo clippy` clean (a `Tag::Loaded` never-constructed warning means Task 1's producer was not wired); `headless-smoke-test.sh` (48 pass) after `cargo build -j 8 -p manifold`.
- [ ] **Step 5: Commit** `Cards read their type; the caps bridge is gone`.

### Task 3: Delete ItemNodeKind; one pass per frame; test sites

**Files:** Modify `node.rs` (delete `ItemNodeKind`, `item_node_kind`, `GRAPH_DRAW_ORDER`), `level.rs` (`LevelItemKind::Node` has no inner kind or holds `CardKind`: choose `Node` with readers looking the card up by id; update `debug_control.rs:1125-1137` accordingly), `graph_ui.rs:~422` (one bucketed pass), test modules of `card.rs`, `node_menu.rs`, `graph_nav.rs`, `level.rs`, `tab_view.rs`.

**Interfaces:** Consumes Tasks 1-2. Produces: no `ItemNodeKind`, `item_node_kind`, `GRAPH_DRAW_ORDER` anywhere in `src` (Task 6 pins it); `fn nodes_by_layer(&self) -> impl Iterator<Item = (u8, &str)>` or equivalent single pass used by `graph_ui` and `graph_nav` search order.

- [ ] **Step 1: Measure before.** Add `#[test] #[ignore] fn level_pass_cost` in `graph_nav.rs` tests: place 2000 `test_node`s (mixed kinds), time 200 runs of the current eight-pass scan; run it with `cargo test -p manifold --bins level_pass_cost -- --ignored --nocapture` and record the numbers with CPU load and Defender state.
- [ ] **Step 2: Migrate the 69 test uses** of `ItemNodeKind` (table tests over all kinds use a `KINDS: [CardKind; N]` fixture built from `TYPES` plus the loaded/unloaded tab pair) and delete the enum and helpers.
- [ ] **Step 3: One pass per frame.** Replace the eight `nodes_at_current_level(kind)` passes with one pass bucketed by `draw_layer`; keep `nodes_at_current_level` for its other callers if any remain (grep).
- [ ] **Step 4: Measure after** with the same benchmark on the single pass; the new pass slower than the old by more than 5% is a finding to resolve (cache `CardKind` per Ruling 10) before commit.
- [ ] **Step 5: Verify and commit** `ItemNodeKind is gone`. Full suite, smoke 48/0, two-instance once.

### Task 4: Runtime replaces NodePayload

**Files:** Modify `node.rs` (`NodePayload` to `Runtime`, `Group` variant to `None`, delete `GroupPayload`, `store_kind`), `fetched.rs` (`place_node`, `place_group`, `insert_*` take the type explicitly: `place_node(id, pos, kind: Kind, runtime: Runtime)`; ~11 sites), `session_sync.rs` (`insert_materialized` `:541-637`, `tab_common`, personal-state paths), `tabs.rs` (spawn landing, swaps: unchanged logic, renamed type), `main.rs` (first-launch demo, `destroy_node`, mute, file-path match), `closed.rs`, `jar_defs.rs`, `card.rs` (`node_card_size`, `card_filter_text`), `session_sync/tests.rs` (helpers `failed_tab`, `sleeping_tab`, `add`) and the other test modules.

**Interfaces:**
- Consumes: `App::test_node` (Task 1), `is_container` (Task 2).
- Produces: `enum Runtime` (Ruling 2); the accessors `as_tab/as_tab_mut/as_unloaded/as_download(_mut)/as_note(_mut)/as_image/as_pdf(_mut)/as_json` keep their signatures on `Runtime` and `GraphNode`; `GraphNode { runtime: Runtime }` with `.payload` field renamed `.runtime` (89 uses); `as_group` removed (22 uses become `is_container(id)` or `card_kind(id)`); `Mirror::ty` is the only source of a node's type (`store_kind` gone).

- [ ] **Step 1: Rename first** (`NodePayload` to `Runtime`, field `payload` to `runtime`) as a mechanical commit-sized change with the `Group` variant still present, compile and test (a pure rename).
- [ ] **Step 2: Remove `Group`.** Groups are placed with `Runtime::None`; delete `GroupPayload` (`fetched.rs:98`, `session_sync.rs:578`, tests); replace `as_group` call sites. `mirror_names` in `debug_control.rs` filters by `is_container`.
- [ ] **Step 3: Type travels explicitly.** `place_node` and `insert_materialized` take the `Kind`; delete `store_kind`; the first-launch demo, `new_note_shortcut`, `insert_content`, reload and download paths pass the kind they already know.
- [ ] **Step 4: Tests.** Every test constructing a payload literal uses `test_node(id, Kind::X, Runtime::..)`; keep the two guard tests of 2b-5 (`check_graph_matches_document`) green.
- [ ] **Step 5: Verify.** Full suite, `-p ipc-protocol`, clippy, then (background) two-instance x3 and smoke x1 per the 2b-5 gate (`.superpowers/sdd/.../scratchpad` style: a detached BelowNormal bash, record CPU and Defender). Tab/Unloaded swap paths (`tabs.rs` landing, sleep, wake, crash respawn, engine swap) are the risk: run `scripts/sleep-probe.sh` if WebView2 is available and say so if skipped.
- [ ] **Step 6: Commit** `Runtime replaces NodePayload; groups have none`.

### Task 5: EXPORTSTATE and the scripts

**Files:** Modify `debug_control.rs:~1101-1147` (`ExportState`), `scripts/chrome-shots.sh` (:106, :295, :310 readers; literals :205, :301, :304, :318, :340, :343, :347, :350, :369, :425, :489), `scripts/headless-smoke-test.sh` (:253-259, :442, :458, :477, :479, :499, :515, :520, :540, :542-549), `scripts/json-level-probe.py:39`, `scripts/page-kind-probe.sh` (:37, :62), `scripts/sleep-probe.sh` (:54, :208, :234), `scripts/scale-test.sh` (:85-86; it greps compact JSON, update the patterns); README lines naming the old strings (`:391` page-kind probe text).

**Interfaces:** Produces EXPORTSTATE `cards[]` entries with `type`, `loaded`, `json_card` (Ruling 9) and no `kind`. A shared helper in the scripts (a small python function or bash `card_is TYPE LOADED` wrapper) holds the mapping so each script line changes once.

- [ ] **Step 1: Failing check.** Add a Rust test for the export shape (build a level with a live-tab stand-in is impossible, so use a sleeping tab, a note, a group, a json card) asserting the keys and that `kind` is absent.
- [ ] **Step 2: Implement the export,** using `card_kind` and `LevelItemKind`.
- [ ] **Step 3: Update the scripts** mapping `Tab` to `type == "application/vnd.manifold.tab" and loaded`, `Unloaded` to the same type with `not loaded`, `Note` to `text/markdown`, `Image` to `image/png`, `Pdf`, `Json`, `Group`, `Download` to their constants from `store/model.rs:116-125`, `JsonCard` to `json_card`.
- [ ] **Step 4: Run live,** after `cargo build -j 8 -p manifold`: `headless-smoke-test.sh` (48/0), `chrome-shots.sh` if its prerequisites exist, `page-kind-probe.sh`, `sleep-probe.sh`, `scale-test.sh`, `json-level-probe.py`; any not runnable here is listed as unverified in the report with the reason.
- [ ] **Step 5: Commit** `EXPORTSTATE names a card by type, not by enum`.

### Task 6: Grep guard, docs, final gate

**Files:** Create the guard test (copy `profile.rs:132-156`, `app_default_touches_no_profile_file`: needles are plain substrings; recurse `CARGO_MANIFEST_DIR/src` with `read_dir`; exempt only its own file by `path.ends_with`); Modify spec (2b note with real hashes; "Deferred from 2b-6" section from this plan; amend 6.9 "is 2c"), sync map ("After 2b-6": card identity, runtime table, no enum), README current-architecture lines (`:20`, `:80`, `:85`), this plan's Outcome.

- [ ] **Step 1: Write the guard** with the Ruling 12 needles; run it, expect it to pass only if Tasks 1-4 left nothing; remove any leftover mention (comments included).
- [ ] **Step 2: Docs.** Spec note: "2b-6 done: ..." with hashes and "Next: B (links), E, C, D, D2"; Deferred section; sync map; README; Outcome (deviations, parked, gate results with machine load and Defender state).
- [ ] **Step 3: Gate.** `cargo build -j 8 -p manifold`; two-instance x3, smoke x1; full suite; fmt; clippy. Record results.
- [ ] **Step 4: Commit** `Docs: 2b-6 note; grep guard for the deleted enums`.

---

## Self-review (run 2026-10-06)

- **Spec coverage:** the 2b bullet's list is `NodePayload`, `ItemNodeKind`, `ContentKind`, `LevelItemKind`, caps bridge, fixtures, grep guard: `ContentKind` is deliberately out (Ruling 4) and recorded in the spec's Deferred section; `Kind` is out (Ruling 1). EXPORTSTATE and scripts are Task 5; first-launch demo is in Task 4; `seed.json` and probe seeds already use type strings (no change). The spec's `presentation.draw_layer` is delivered as `Caps.draw_layer`; the rest of `presentation` (icon, card style) is not needed by any reader here and stays unbuilt.
- **Placeholders:** none intended; per-site conversions are compiler-led and listed by file with counts from the survey (re-grep at execution: counts drift).
- **Type consistency:** `CardKind`, `card_kind`, `is_container`, `can_contain`, `Runtime`, `test_node`, `draw_layer` are defined in Tasks 1-2 and used later with the same signatures; `test_node` takes `NodePayload` in Tasks 1-3 and `Runtime` from Task 4.
- **Risks:** Task 4 is the riskiest (Tab/Unloaded swap under one id, test helpers, `payload` to `runtime` rename touching 89 sites); Task 3's frame-time measurement can force the cached-`CardKind` fallback (Ruling 10). Counts in this plan came from a survey at 4a58427: 879 enum occurrences (47% tests).
- **Unverified at writing:** whether `CardControl::action` and `drop_action` have exactly the four bridge calls after 2b-5 (survey says three sites, four calls); whether `LevelItemKind::Node` readers can look the card up by id without a borrow conflict in `graph_ui`.

## Outcome (2026-10-06)

Done on `dev`: b115bd1 (Task 1: `CardKind { ty, tags }`, `card_kind`, `Caps.draw_layer`, a tenth row for TYPE_DOWNLOAD),
7c4c4f8 (Task 2: cards read their type; the caps bridge is gone), 747015d (Task 3: `ItemNodeKind` and `GRAPH_DRAW_ORDER`
gone, the level is scanned in one pass, `Registry::caps` allocation-free), 326f73f and 438b7eb (Task 4: `NodePayload` is
`Runtime`, `payload` is `runtime`; groups have no runtime and the type travels explicitly), 78bff97 (Task 5: EXPORTSTATE
`cards[]` names a card by `type`, `loaded`, `json_card`; scripts follow); plus the Task 6 commit (this Outcome, the docs, the
guard `crates/manifold/src/deleted_names.rs`). Plan commits: e82a6fb, 6ede40d. Tests: 879 at Task 5.

Gate results: filled by the controller after the final gate.

### Frame cost of the level pass (Task 3 unit benchmark `level_pass_cost`, release build, load about 9%, Defender real-time off)

| Level (2000 test nodes) | old enum scan (8 passes) | Task 2 (nine passes) | new one pass |
|---|---|---|---|
| top level | 432 µs | 1100 µs | about 170 µs |
| g0 (a group level) | about 400 µs | about 532 µs | about 72 µs |

The first one-pass version cost 553/496/535 µs at the top level (over Ruling 10's 5% limit): about 70% of it was
`Registry::caps` allocating and lower-casing per row. The root cause was fixed (allocation-free `Registry::caps` and
`Pattern::matches`, rows tests as goldens) instead of caching `CardKind` on `GraphNode`. The numbers move between runs
under the same code (the old scan read 426 and 532 µs), so read them as ratios.

### Rulings made during execution and deviations

- **Ruling 12 deviation: `LevelItemKind` is not a guard needle.** Task 3 keeps it as a live type (`LevelItemKind::Node(CardKind)`
  and `JsonCard`, `level.rs`) and the plan only retires its inner kind; banning the name would delete working code. The guard
  bans the other six needles and `LevelItemKind` is documented in the guard's doc comment.
- **Guard placement:** its own small test module `deleted_names.rs` (not a profile.rs test), so the needles it names exempt only
  that file, as Ruling 12 requires. Its only offender check was a planted `// NodePayload` comment in `rgba.rs` (failed, then
  removed).
- **Ruling 10 changed:** `scale-test.sh` measures restore time, not frame time, so Task 3 used an `#[ignore]`d unit
  benchmark (plan fixed at 6ede40d). When it showed the one-pass scan over the 5% limit, the controller ruled to fix
  `Registry::caps` rather than cache `CardKind` (a cache would have recovered only 36 µs).
- **Task 2 scaffolding:** `ItemNodeKind`, `GRAPH_DRAW_ORDER`, `item_node_kind` and `legacy_kind_name` lived on as
  `cfg(test)`/temporary code until Task 3 and Task 5 removed them; Task 2's review caught a wrong concern (a node
  lacking TYPE reads as `*/*`, `card_kind` is `None` only for an absent node) and moved those fixtures to `test_node`.
- **Ruling 3 detail:** `card_kind` returns `Some(*/*)` for a node whose TYPE is missing.
- **Ruling 8 consequence:** six data-read arms treat `Runtime::None` as a group; they must switch to `is_container` when the
  unknown-type card arrives. `snapshot()` returns `None` for an unknown type (unreachable today).
- **Ruling 9 interpretation:** a JSON level's card exports `type: application/json` with `json_card: true`; the export's
  `label` appends `;loaded` for a live tab (vocabulary used by page-kind-probe and sleep-probe).
- **Test helper:** `test_node` uses `Mirror::seed` semantics (TYPE set only when absent) since Task 4.
- **Trailers:** Task 5's commit trailer names Fable 5.1 (the implementer's own model); accepted, not amended.

### Parked (not fixed)

- Task 3: `drop_target` hits the first card in `node_order`, the bottom-most of overlapping same-kind cards, not the topmost
  (pre-existing, user-visible); a fix could search `node_order` in reverse.
- Task 4: `every_creation_path_seeds_the_node_type` is circular for Note/Json/Pdf/Image/Download (tests.rs near 4441-4455).
- Task 4: stale `payload` names remain in comments or locals (`card.rs` near 852 and 873, `fetched.rs` near 325, `tabs.rs`
  near 576, `node.rs` near 485).
- Task 4: reloading with a differing content kind leaves the mirror type unchanged (`fetched.rs` near 325-344, pre-existing).
- Task 4: add one `// Ruling 8` marker on the `Runtime::None`-as-group arms.
- Task 5: smoke check messages still say "Pdf card" and so on; the `ExportState` doc comment (`debug_control.rs` near 157-170)
  says `json_card` has only screen/rect besides (it also has `type` and `loaded`).
- Task 5: `scripts/json-level-probe.txt` does not enter the JSON level on this machine (pre-existing); a follow-up could seed
  `has_shown_first_launch_demo` or poll two exports before computing the double-click.
- Deferred directions (spec "Deferred from 2b-6"): `Kind` to type strings (D2), `ContentKind` to the views registry (D),
  `Runtime` per-family maps or a plugin slot (D2), raw TYPE (D), the unknown-type card and JsonLeaf retirement.
