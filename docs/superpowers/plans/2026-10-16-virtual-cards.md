---
status: done
---

# Virtual cards Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The Graph view draws, hits, selects, drags and drops cards that have no document node, through the same code as stored cards, with each card's abilities read from the `Caps` registry.

**Architecture:** `Caps` (the MIME-keyed capability registry in `caps/rows.rs`) gains ability fields; a `*/*` row sets them true so every stored kind keeps today's behaviour, and a `application/vnd.manifold.virtual.*` prefix row turns them off for virtual kinds. `LevelItem` becomes `CardEntry` carrying a `CardKind` and an `Origin`; the card loop, tab order, drag, selection, right-click and edges read entries instead of `self.nodes`. A `VirtualSource` trait supplies a source's entries, edges and handlers. A test-only fake source exercises every ability. The trail provider is piece 2, not here.

**Tech Stack:** Rust, egui 0.36, the existing `caps`, `level.rs`, `card.rs`, `graph_ui.rs`, `graph_nav.rs`.

**Spec:** `docs/superpowers/specs/2026-10-16-virtual-cards-design.md`

## Global Constraints

- Work on `main`; `cargo fmt --all` before each commit; `cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings` must pass; never `--no-verify`; an `#[allow]` carries a `reason`.
- No test reads or writes the real `%APPDATA%\manifold` (`profile::dir()` is `None` under `cfg(test)`).
- Stored-card behaviour must not change: every existing test stays green and unedited unless a task says why.
- No compatibility shims: when `LevelItem` becomes `CardEntry`, rename every user at once; delete what the change supersedes.
- A virtual id is `virtual:<source>:<key>` and never reaches `edit_pos`, `edit_field`, the store, sync, `EXPORTSESSION`, undo, Recently Closed or `tab-history.json`.
- Commit trailer: `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`.
- Gate before merging: `target/debug/gate.exe all LOG light`, then `ui`, after `cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate`, detached at BelowNormal; report CPU load and Defender state.

## Review Focus

- A virtual id given to every graph write path (`edit_pos`, `release_drag`, `apply_drop`, Close Selected, Pin, group, delete) writes nothing and does not panic (Task 7 test).
- `level_edges` with an edge to or from a virtual card does not index `self.nodes[id]` (Task 3 test).
- A drag that starts on a virtual card never moves any rect and is not cancelled by `end_stale_drag` (Task 6 test).
- A mixed selection (stored + virtual) acted on by Close Selected closes only the stored cards (Task 5 test).
- Right-click on a virtual card does not call `on_jar_chip` or open the node menu (Task 4 test).
- `fit`/Reset View, the minimap and `EXPORTSTATE` include a virtual card (Task 7 tests).

---

### Task 1: Abilities in `Caps`

**Files:**
- Modify: `crates/manifold/src/caps/rows.rs` (Row, Caps, Registry::caps, builtin_rows, tests)
- Modify: `crates/manifold/src/store/model.rs` (add `TYPE_VIRTUAL_PREFIX`)

**Interfaces:**
- Produces: `Caps { restorable, draw_layer, selectable, drag_source, movable, drop_target, menu, linkable, stored_controls }` (new fields `bool`); `Row::selectable(bool)`, `drag_source`, `movable`, `drop_target`, `menu`, `linkable`, `stored_controls`; const `TYPE_VIRTUAL_PREFIX = "application/vnd.manifold.virtual."`; `DRAW_LAYERS = 9`.

- [ ] **Step 1: Write the failing tests** in `caps/rows.rs` `mod tests`:

```rust
    #[test]
    fn every_builtin_stored_kind_has_every_ability() {
        use crate::store::model::{TYPE_DOWNLOAD, TYPE_GROUP, TYPE_JAR, TYPE_JSON, TYPE_NOTE, TYPE_PDF, TYPE_TAB, TYPE_UNKNOWN};
        let registry = registry(builtin_rows());
        for ty in [TYPE_TAB, TYPE_GROUP, TYPE_JAR, TYPE_JSON, TYPE_NOTE, TYPE_PDF, TYPE_DOWNLOAD, TYPE_UNKNOWN, "image/png"] {
            let caps = registry.caps(ty, TagSet::EMPTY);
            assert!(
                caps.selectable && caps.drag_source && caps.movable && caps.drop_target && caps.menu && caps.linkable
                    && caps.stored_controls,
                "{ty}: {caps:?}"
            );
        }
    }

    #[test]
    fn a_virtual_kind_has_no_ability_until_its_own_row_grants_one() {
        let registry = registry(builtin_rows());
        let bare = registry.caps("application/vnd.manifold.virtual.test", TagSet::EMPTY);
        assert!(!bare.selectable && !bare.drag_source && !bare.movable && !bare.drop_target);
        assert!(!bare.menu && !bare.linkable && !bare.stored_controls);
        assert_eq!(bare.draw_layer, 8, "a virtual card draws above every stored kind");
    }
```

- [ ] **Step 2: Run to verify failure.** `cargo test -j 8 -p manifold caps::rows` — Expected: compile error, no field `selectable`.

- [ ] **Step 3: Implement.** In `Row` add `selectable: Option<bool>` and the six others (`drag_source`, `movable`, `drop_target`, `menu`, `linkable`, `stored_controls`), `None` in `Row::new`; a builder per field in the style of `restorable`. In `Caps` add the seven `bool` fields with a doc line each (`selectable`: may be in the rectangle selection; `drag_source`: may be picked up; `movable`: a drag repositions it and stores the position; `drop_target`: a dragged card may land on it; `menu`: has a node menu; `linkable`: shows link handles; `stored_controls`: Swatch, Pin, Close and Eject act on a stored node). In `Registry::caps`, inside the `for row in matching` loop, copy each `Some` field as `restorable` is copied. Set `pub(crate) const DRAW_LAYERS: usize = 9;`. In `store/model.rs` next to the `TYPE_*` consts add:

```rust
/// Every virtual card's type starts with this: a card drawn from other state, with no node (spec 2026-10-16-virtual-cards-design.md).
pub const TYPE_VIRTUAL_PREFIX: &str = "application/vnd.manifold.virtual.";
```

In `builtin_rows()` insert first:

```rust
        // Every stored kind can do everything; a virtual kind's own row, below, turns off what it cannot.
        Row::new("*/*")
            .selectable(true)
            .drag_source(true)
            .movable(true)
            .drop_target(true)
            .menu(true)
            .linkable(true)
            .stored_controls(true),
        // A virtual kind starts with nothing and draws on top (layer 8); a source's exact row grants what it needs.
        Row::new("application/vnd.manifold.virtual.*")
            .selectable(false)
            .drag_source(false)
            .movable(false)
            .drop_target(false)
            .menu(false)
            .linkable(false)
            .stored_controls(false)
            .draw_layer(8),
```

- [ ] **Step 4: Run to verify pass.** `cargo test -j 8 -p manifold caps` — Expected: PASS, including the existing `every_builtin_draw_layer_fits_the_bucket_count`.
- [ ] **Step 5: Commit.** `cargo fmt --all`, then `git add crates/manifold/src/caps crates/manifold/src/store/model.rs && git commit -m "Caps: per-kind abilities; a virtual kind starts with none"` (with the trailer).

---

### Task 2: `CardEntry` and the `VirtualSource` trait

**Files:**
- Modify: `crates/manifold/src/level.rs` (`LevelItem` → `CardEntry`, `LevelItemKind` → `Origin`; users: grep `LevelItem`)
- Create: `crates/manifold/src/virtual_source.rs` (trait, registry field, fake source under `#[cfg(test)]`)
- Modify: `crates/manifold/src/main.rs` or the `App` definition file (a field `virtual_sources: Vec<Box<dyn VirtualSource>>`, empty by default)

**Interfaces:**
- Lint rule: the pre-commit clippy rejects dead code, so the trait grows by task: Task 2 declares only `name` and `entries`; Task 3 adds `edges`; Task 4 adds `paint_body`, `on_click`, `on_menu`; Task 6 adds `on_drop`; Task 7 adds `searchable_text`. The block below is the end state.
- Produces:

```rust
pub(crate) enum Origin { Stored, Json, Virtual(&'static str) }
pub(crate) struct CardEntry { pub id: String, pub kind: CardKind, pub origin: Origin, pub rect: egui::Rect }
impl CardEntry { pub(crate) fn caps(&self) -> Caps { self.kind.caps() } }
pub(crate) trait VirtualSource {
    fn name(&self) -> &'static str;
    fn entries(&self, app: &App) -> Vec<CardEntry>;
    fn edges(&self, app: &App, rect_of: &dyn Fn(&str) -> Option<egui::Rect>) -> Vec<LevelEdge>;
    fn paint_body(&self, app: &App, ui: &mut egui::Ui, entry: &CardEntry) -> CardBody;
    fn on_click(&self, app: &mut App, entry: &CardEntry) -> Option<GraphAction>;
    fn on_menu(&self, app: &mut App, entry: &CardEntry, at: egui::Pos2);
    fn on_drop(&self, app: &mut App, entry: &CardEntry, target: Option<(DropEffect, Option<String>)>) -> Option<GraphAction>;
}
```

- The JSON card's kind: `CardKind { ty: TYPE_JSON, tags: EMPTY }` (a JSON card behaves as the JSON row says; `Origin::Json` marks that it has no node).

- [ ] **Step 1: Write the failing test** in `virtual_source.rs`:

```rust
#[cfg(test)]
pub(crate) mod fake {
    // FakeSource: yields three entries "virtual:fake:a|b|c" of ty "application/vnd.manifold.virtual.fake",
    // rects at (0,0)-(100,60), (200,0)-(300,60), (400,0)-(500,60); one edge a→b; records every handler call in a RefCell<Vec<String>>.
}
#[test]
fn level_items_include_a_registered_virtual_source_after_the_stored_nodes() {
    let mut app = App::default();
    app.virtual_sources.push(Box::new(fake::FakeSource::default()));
    let items = app.level_items();
    assert!(items.iter().any(|i| i.id == "virtual:fake:a" && matches!(i.origin, Origin::Virtual("fake"))));
    assert_eq!(items.last().unwrap().id, "virtual:fake:c", "virtual entries draw last, so on top");
}
```

Also in `fake`: `pub(crate) struct Rig { ctx: egui::Context, app: App }` modelled on `deselect::tests::Rig` (read it first: `crates/manifold/src/deselect.rs:40-125`). `Rig::new()` makes an `egui::Context` (`crate::theme::apply_fonts`) and an `App::default()` with the `FakeSource` pushed, inserts one sleeping tab with `insert_placeholder_node(id, vec2(0.0, 300.0), Kind::Tab, sleeping_tab(url, "t"))` (`crate::session_sync::tests::sleeping_tab`), runs 3 frames, calls `app.camera.jump_to(app.fit_all_nodes_scene_rect())` and runs one more. `frame(events, modifiers)` runs `app.build_graph_ui` in a 1024x768 `RawInput`; `click(at)`, `right_click(at)` and `drag(from, to)` copy that file's versions (press, 8 move steps, release, settle frame). `screen(id) -> egui::Rect` is `app.graph_world_rect_to_screen` of the entry with that id from `app.level_items()`. Tasks 4-7 use this `Rig`. `caps::registry()` is a `OnceLock`, so a test cannot add a row to it; instead `builtin_rows()` gets, under `#[cfg(test)]`, one exact row per fake kind: `application/vnd.manifold.virtual.fake-none` (no abilities, inherited from the prefix row), `...fake-all` (every ability true), and one per ability (`fake-selectable`, `fake-drag`, `fake-drop`, `fake-menu`). `FakeSource::with_kind(ty)` picks which kind its entries carry.

(The fake is written out in full in this step: three `CardEntry` literals with `kind: CardKind { ty: "application/vnd.manifold.virtual.fake", tags: TagSet::EMPTY }`, `edges` returns one `LevelEdge` between the rect midpoints with `EdgeKind::Opened`, `curve: EdgeCurve::Bow`, handlers push their name to the log and return `None`.)

- [ ] **Step 2: Run** `cargo test -j 8 -p manifold level_items_include` — Expected: compile error.
- [ ] **Step 3: Implement.** Rename `LevelItem` to `CardEntry` and `LevelItemKind` to `Origin` everywhere (`grep -rn "LevelItem\|LevelItemKind" crates/manifold/src` — `level.rs`, `graph_nav.rs`, `graph_ui.rs`, `debug_control.rs`, `link_gestures.rs`, and their tests). `Node(CardKind)` becomes `Origin::Stored` with the `CardKind` in the entry's `kind`; `JsonCard` becomes `Origin::Json` with `kind` = the JSON kind above. Fix each `match` on the old enum. In `level_items_from`, after the stored/JSON entries, append `self.virtual_sources.iter().flat_map(|s| s.entries(self))`. Add the field to `App` (default empty) and the module to `main.rs`.
- [ ] **Step 4: Run** `cargo test -j 8 -p manifold` — Expected: all existing tests PASS (behaviour unchanged) plus the new one.
- [ ] **Step 5: Commit** "CardEntry: a level item carries its kind and origin; VirtualSource trait".

---

### Task 3: Edges read entries, not `self.nodes[id]`

**Files:** Modify `crates/manifold/src/level.rs` (`level_edges`, line with `rect_of`), tests there.

- [ ] **Step 1: Failing test:** with the fake source registered, `app.level_edges()` contains an edge with `from_id == "virtual:fake:a"` and `to_id == "virtual:fake:b"`, and does not panic when a stored Link node's end is the id `virtual:fake:a` (build a link node whose `from` is that id via the test helpers already used in `level.rs` tests).
- [ ] **Step 2: Run** — Expected: FAIL (no virtual edge) or panic at `self.nodes[id]`.
- [ ] **Step 3: Implement.** Replace `let rect_of = |id: &str| node_card_rect(self.layout_pos(id), &self.nodes[id].runtime);` with a closure over the entry list built once at the top of the function:

```rust
        let rects: HashMap<String, egui::Rect> = self.level_items().into_iter().map(|e| (e.id, e.rect)).collect();
        let rect_of = |id: &str| rects.get(id).copied();
```

and make the three edge sources (`opened`, `Link`, `jar`) skip an edge when `rect_of` returns `None` for either end. After them, append `self.virtual_sources.iter().flat_map(|s| s.edges(self, &rect_of))`.
- [ ] **Step 4: Run** `cargo test -j 8 -p manifold level` — Expected: PASS.
- [ ] **Step 5: Commit** "level_edges: rects come from the level's entries; virtual sources add edges".

---

### Task 4: The card loop draws entries (stored, virtual) and routes click, menu, hover by ability

**Files:** Modify `crates/manifold/src/card.rs` (`draw_graph_card`, `card_view`, `paint_card_body`, `card_controls`), `crates/manifold/src/graph_ui.rs` (`draw_graph_cards`, `card_tab_order`, the right-click handler near the `level_items` hit test).

**Interfaces:** `draw_graph_card(&mut self, ui, pass, entry: &CardEntry) -> Option<CardOutput>`; `card_view(&self, entry: &CardEntry, members) -> Option<CardView>` (for a non-`Stored` origin, a view with `color: None, in_group: false, members: empty, pinned/muted/audio false, closable: false`, `selected` from `interaction.selected`, `focused` false).

- [ ] **Step 1: Failing tests** (in `virtual_source.rs` tests, using `fake::Rig`): (a) with the `fake-all` kind registered, one frame draws the three virtual cards: after a pointer move onto `rig.screen("virtual:fake:b").center()` and a frame, `app.interaction.hovered == Some("virtual:fake:b")`; (b) a click on a virtual card calls `FakeSource::on_click` once and no `GraphAction::Node` is pushed; (c) a right-click on a virtual card with `menu` off calls neither `on_jar_chip` nor opens `menus.node_menu`; with the fake's exact row granting `menu(true)` it calls `on_menu`.
- [ ] **Step 2: Run** — Expected: FAIL.
- [ ] **Step 3: Implement.**
  1. `draw_graph_cards` iterates `self.level_items_from(at_level)` (stored then virtual; JSON levels return before the loop as today). For each entry: `Origin::Stored` takes the existing path unchanged (it only needs `entry.id`); `Origin::Virtual(name)` calls `draw_graph_card(ui, &mut pass, &entry)`, then on `card.response.clicked()` calls the source's `on_click` and pushes its returned action; on a secondary click with `entry.caps().menu` calls `on_menu`.
  2. `draw_graph_card` takes `&CardEntry`; replace `self.card_rect(id)?` with `entry.rect` and `self.card_view(id, …)` with `self.card_view(entry, …)`. The node-only lines (`self.nodes.contains_key(id)` guard on `widget_info`, `if let Some(node) = self.nodes.get(id)` filter dim, `edit` of `layout`) stay guarded as they are and are skipped for a virtual entry. `paint_card_body`: a `Origin::Virtual(name)` entry calls `source.paint_body(self, ui, entry)`; the stored match is unchanged.
  3. `card_controls(view)`: add `if !view.kind.caps().stored_controls { return Vec::new(); }` as the first line, so a virtual kind shows no Swatch, Pin, Eject or Close; `card_style(kind, visuals)` gets a final arm for `ty.starts_with(TYPE_VIRTUAL_PREFIX)` using the unknown-type style.
  4. The right-click handler (`graph_ui.rs`, the `level_items` hit test): branch on `entry.origin`; call `on_jar_chip` and open the node menu only for `Origin::Stored`; for `Virtual` consult `caps().menu`.
  5. `card_tab_order` builds from `level_items()` ids (stored in `node_order` order, then virtual), not `nodes.contains_key`.
- [ ] **Step 4: Run** `cargo test -j 8 -p manifold` — Expected: all PASS.
- [ ] **Step 5: Commit** "The card loop draws level entries; click, menu and controls follow the kind's abilities".

---

### Task 5: Selection by ability

**Files:** Modify `crates/manifold/src/graph_nav.rs` (`end_stale_selection`), `crates/manifold/src/graph_ui.rs` (`handle_rect_select`, `toggle_selected`), and the selection actions (Close Selected, `pin_shortcut`, Group Selected: `grep -rn "interaction.selected" crates/manifold/src`).

- [ ] **Step 1: Failing tests:** (a) with the fake's exact row granting `selectable(true)`, rectangle select over the three virtual cards puts their ids in `interaction.selected`, and `end_stale_selection` keeps them; with `selectable` off it selects none. (b) with a stored tab and a selected virtual id in `selected`, Close Selected closes the tab and leaves the virtual id untouched; `pin_shortcut` does not call `edit_field` for the virtual id.
- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Implement.** `end_stale_selection`: retain an id if it is a level entry whose `caps().selectable` is true (`self.level_items()`), else drop it (a stored id passes through the same test, since the `*/*` row grants it). `handle_rect_select`: add every entry with `caps().selectable` whose rect intersects, not only `Node(_)`. Each selection action filters its id list with `self.nodes.contains_key(id)` before acting (they already `filter(|id| self.nodes.contains_key(id))` in `pin_shortcut`; add the same to the others found by the grep).
- [ ] **Step 4: Run** `cargo test -j 8 -p manifold` — PASS.
- [ ] **Step 5: Commit** "Selection follows the selectable ability; node actions skip ids with no node".

---

### Task 6: Drag source, ghost and drop routing

**Files:** Modify `crates/manifold/src/card.rs` (the `response.dragged()` block), `crates/manifold/src/graph_nav.rs` (`live_drag`, `end_stale_drag`, `drop_target`, `release_drag`, `apply_drop`), `crates/manifold/src/graph_ui.rs` (`draw_graph_cards`'s `drop_preview`).

First read, then decide (record in the ledger): (1) whether the graph already draws a drag ghost (`grep -n "ghost" crates/manifold/src/*.rs`); if not, the ghost is the virtual card's body painted at `pointer - grab offset` into `deferred_overlays`. (2) What `drop_target` returns for a release on empty canvas (today it returns `None`); a virtual `on_drop` receives `None` and decides.

- [ ] **Step 1: Failing tests:** (a) dragging a virtual card (fake exact row `drag_source(true)`, `movable` false) by a pointer delta leaves every rect unchanged and `self.layout` has no entry for its id; `interaction.dragging` stays `Some(virtual id)` while held (not cancelled by `end_stale_drag`). (b) releasing over empty canvas calls `FakeSource::on_drop(entry, None)` once; releasing over a stored group with `drop_target` calls it with `Some((effect, Some(group)))`. (c) `release_drag` for a virtual id writes nothing (no `edit_pos`).
- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Implement.**
  1. `live_drag`/`release_drag`/`end_stale_drag`: `live_drag` accepts an id that is a level entry with `caps().drag_source` (stored ids pass by the `*/*` row).
  2. `card.rs` drag block: when `entry.caps().movable`, keep the existing layout move; otherwise (a `drag_source` only) do not touch `self.layout`, set `self.interaction.dragging = Some(id)` and `drag_moved = vec![]`, and paint the ghost.
  3. `drop_target(id, …)`: take the dragged entry's `kind` from the entry list; for a non-movable source use the pointer position as the probe point instead of `layout_pos(id)`; keep the BackButton test; skip candidates whose `caps().drop_target` is false. The searches iterate `self.level_items()` instead of `node_order`.
  4. The release site pushes `GraphAction::Drop { id, target }` as today; `apply_drop` begins with: if `id` is a virtual entry, call its source's `on_drop` and return its action (apply it with the same `apply_graph_action`), else the existing match.
- [ ] **Step 4: Run** `cargo test -j 8 -p manifold` — PASS.
- [ ] **Step 5: Commit** "Virtual cards can be dragged as sources and dropped through their provider".

---

### Task 7: Persistence guard, camera, EXPORTSTATE, search

**Files:** Modify `crates/manifold/src/graph_nav.rs` (`bounds_for_level`), `crates/manifold/src/debug_control.rs` (EXPORTSTATE `cards`), `crates/manifold/src/card.rs` (filter dim), tests.

- [ ] **Step 1: Failing tests:** (a) *Persistence guard:* call `edit_pos`, `edit_field`, `release_drag`, `apply_drop`, Close Selected, `pin_shortcut` with `"virtual:fake:a"` and assert `store` document and `layout` are byte-identical before and after and nothing panics. (b) `bounds_for_level` and fit/Reset View include a virtual card far from the stored ones. (c) `EXPORTSTATE`'s `cards` lists `virtual:fake:a` with `"virtual": "fake"` and its abilities. (d) a filter text dims a virtual card only when the source provides `searchable_text` (add `fn searchable_text(&self, entry: &CardEntry) -> Option<String> { None }` to the trait, default none).
- [ ] **Step 2: Run** — FAIL where unimplemented.
- [ ] **Step 3: Implement** the guard by adding `if !self.nodes.contains_key(id) { return; }` at the top of `edit_pos` and `edit_field` only if the tests show a write happens (the survey says they already guard; the test settles it). `bounds_for_level`: build from `level_items()`. EXPORTSTATE: add the `virtual` branch beside the existing `json_card` one. Filter dim: call the source's `searchable_text`.
- [ ] **Step 4: Run** `cargo test -j 8 -p manifold` — PASS.
- [ ] **Step 5: Commit** "Virtual cards in camera bounds, EXPORTSTATE and filter; persistence guard test".

---

### Task 8: Gate, docs, cleanup

**Files:** Modify `README.md` (a "Virtual cards" paragraph under the graph notes: what the seam is, the `Caps` abilities, `VirtualSource`, the persistence rule; update the spec's status line to "implemented"), `crates/gate` only if a stage needs a virtual-card probe (not in this piece: the fake exists only under `cfg(test)`).

- [ ] **Step 1:** `grep -rn "LevelItem\b\|LevelItemKind" crates README.md docs` — Expected: only history in old plans/specs; fix any live reference.
- [ ] **Step 2:** `cargo fmt --all --check` and the full clippy command from Global Constraints — Expected: clean.
- [ ] **Step 3:** `cargo test -j 8 -p manifold -p ipc-protocol -p manifold-view-webview2 -p gate` — Expected: PASS.
- [ ] **Step 4:** Build and run `gate all LOG light` then `ui` as in Global Constraints — Expected: every stage exit 0 (`gate layout` and `gate smoke` are the regression check that stored cards did not change).
- [ ] **Step 5: Commit** "Virtual cards: README and cleanup". Stop and report; do not push or tag without the user's go-ahead.

## Self-review (round 1)

- Spec coverage: decisions 1-2 → Tasks 1-2; 3 → Task 2; 4 → Task 6; 5 → Task 5; 6 → Task 3; 7 and 8 and 9 → Task 7; tests → every task; gate → Task 8.
- Placeholders: Task 2's fake is described by its exact contents rather than printed. Observation helpers are named (`fake::Rig`, copied from `deselect::tests::Rig`).
- Round 3: trait methods are introduced by the task that first calls them (dead-code lint at each commit); `DRAW_LAYERS = 9` is safe (`level.rs:68` and `graph_nav.rs:691` size their arrays from the const; `level_scan` clamps).
- Round 2 (adversarial) replaced the vague "existing test helpers" with `fake::Rig`, and fixed a hole: `caps::registry()` is a `OnceLock`, so the fake kinds' exact rows live in `builtin_rows()` under `#[cfg(test)]`.
- Type consistency: `CardEntry`/`Origin`/`VirtualSource` are defined in Task 2 and used unchanged in Tasks 3-7; `Caps` field names are fixed in Task 1.
- Unverified in the plan: the drag ghost and empty-canvas `drop_target` (Task 6 reads first), `DRAW_LAYERS = 9` against code that indexes by `draw_layer` (the Task 1 test covers the bucket count; `level_scan` clamps), and the line-level shape of `handle_rect_select` and the right-click handler (read at execution).
