# Jars as Cards Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A cookie jar becomes an ordinary card (position, group, select, rename, duplicate, close, reopen), joined to tabs and groups by one `jar` edge, and every mechanism that existed only because jars were not cards is deleted.

**Architecture:** Jars become `App::nodes` entries with a unit `Runtime::Jar` and a draw layer, so the generic store, sync, Recently Closed and menu paths carry them. The built-in Default jar becomes a card with `IS_DEFAULT`, kept by one startup invariant, and `JarKey` collapses to `NodeId`. One `jar` edge row (`Field(JAR)`, scope `SameLevelOrAncestor`) replaces `JAR_SET`/`DEFAULT_JAR`; `effective_jar` replaces the cascade machinery. Jar cookie files and fetched content files share one keep set, one prune and one release point.

**Tech Stack:** Rust 2021, egui, the manifold store (`store::model`, `session_sync`, `mirror`), the caps tables (`caps::rows`, `caps::edges`), the `gate` crate (headless smoke).

**Spec:** `docs/superpowers/specs/2026-10-13-jars-as-cards-design.md`

## Global Constraints

- Work on `main`. One commit per task. Every commit message ends with the line `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`.
- Every task leaves the tree building, `cargo fmt --all --check` clean and this passing: `cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings`.
- Never `--no-verify`. The pre-commit hook runs fmt and clippy; if it fails, fix the cause.
- No lint silenced without `#[allow(..., reason = "...")]`.
- No migration code and no shims (spec "Existing sessions: no migration"). Old fields 16 and 17 stay raw on old nodes and go in `RESERVED`.
- Replace, don't layer: each task deletes what it supersedes, with its tests and docs, in the same commit.
- No test reads or writes the real `%APPDATA%\manifold`. Files go under `store::testutil::temp_dir(..)`; jar files under `App { jars_dir_override: Some(dir), .. }`.
- Each piece of non-trivial logic gets one unit test.
- Never let `python` read stdin; this plan needs no python.
- Unit tests: `cargo test -j 8 -p manifold <filter>`; run them detached at BelowNormal when the machine is busy (LONG_RUNS.md).
- New field 22 is `IS_DEFAULT`, `Bool`, name `"is_default"`. Fields 16 (`JAR_SET`) and 17 (`DEFAULT_JAR`) are never reused.
- Edge verb: `"Use jar\u{2026}"`. Default card name: `"Default"`; a losing duplicate is renamed `"Default (old)"`.

## Resolved open items

- **(a) A card with no `POS`.** A missing field cannot be told from `Pos(0, 0)`: `encode_field` writes nothing for an absent value and `Node::get` decodes a missing field as `FieldValue::absent` (`Pos(0.0, 0.0)`, store/model.rs:293, 465), and `fields_of` goes through `Node::get`. `Mirror::pos` already reads the exact origin as "no pos" (2b-4 Ruling 5). So Task 1 changes `pos_of(id, fields)` to treat the exact origin as unplaced and return `GRAPH_WORLD_CENTER + spawn_jitter(id)` (`GRAPH_WORLD_CENTER` is `Vec2::ZERO`), the rule `App::placement` uses for a new node with no position. One rule, every kind. A card really dropped on the exact origin lands a jitter away on another replica (or on a restore without local state), which is harmless and already how `Mirror::pos` reads it.
- **(b) How `reopen_closed` rebinds files.** Path-derived files (content) need nothing: `PATH` is copied verbatim. Id-derived files (jars) are rebound in `reopen_closed` by zipping `entry.doc.nodes` with `imported` (the same zip the method already does for zoom and title): for each jar, when the old id is not a live node, `<old>.json` is **copied** to `<new>.json`. Duplicate reuses `reopen_closed` with the original live, so nothing is copied and the copy starts logged out. Copy, not move: a named session or a sibling instance's Recently Closed list may still name the old id; the startup prune deletes the old file once nothing holds it. Deviation from the spec's "moved", flagged in the hand-back.
- **(c) Runtime match sites.** Exhaustive `match` on `Runtime` (each gains a `Runtime::Jar` arm, and the compiler names any this survey missed): `node.rs` `saved_tab` (77), `file_path` (93), `listed_tab` (105), `accessible_name` (395) and the test at 527; `reveal.rs` `node_file_path` (44); `session_sync.rs` `runtime_has_type` (91), `update_live_node` (574); `card.rs` `node_card_size` (890), `card_filter_text` (917); `graph_nav.rs` bench `old_layer` (652). Non-exhaustive sites the compiler will not flag, each changed on purpose: `session_sync.rs:940` `own_text` (its `_` arm would read a jar as a note; a jar takes the name arm, see Task 1), and `card.rs:1282` `paint_card_body`, which dispatches on `view.kind.ty` (a `TYPE_*` string), not on `Runtime`, so it gains a `TYPE_JAR` arm. `graph_nav.rs:290` has a `_` arm that is right for a jar and is left alone.

## Decisions this plan applies (pending the user's confirmation)

Two calls the coordinator made on review. The plan builds them; the user is asked to confirm before Task 4 (scope) and Task 5 (release) run. If the user overrules either, only that task's marked steps change.

- **Release on delete (Task 5).** A jar or content card removed with no Recently Closed entry recording it (a remote delete) has its backing files deleted at once, unless a live node, a Recently Closed entry or a named session still holds them. This is the startup prune's keep rule, limited to the removed node's files and with no grace. Only leaving Recently Closed (eviction past `MAX_CLOSED`) waits for the startup prune and its grace. A local close records its entry first, so its files are always held then. Accepted risk: a sibling instance on this machine whose own Recently Closed list holds the jar loses its cookies (the case `PRUNE_GRACE` covers for the prune).
- **A group's jar edge reaches inside the group (Task 4).** For a group source, its own contents count as its level: its `jar` edge may name a jar inside that group, at the group's own level, or in any enclosing group. A tab's edge reaches its own level and above, as the spec says. The spec's Scope section needs the matching edit (listed in the hand-back; this plan does not edit that spec section).

## Leftover-reference check

Every task's check step greps for the names it deletes, over all of `crates/` (tests included), and expects no hits outside comments that explain the history:

```bash
git grep -nE "<names>" -- crates
```

Each task lists its `<names>`. A hit is fixed before the commit, never left for a later task.

## Task order

1. Jars are cards (store, `Runtime::Jar`, sync, placement, close and reopen).
2. The jar card's body and menu; Settings loses named jars.
3. The Default jar is a card; `JarKey` becomes `NodeId`.
4. One `jar` edge with a scope column replaces `JAR_SET`/`DEFAULT_JAR` and the cascade.
5. Backing files: one keep set, one prune, one release, one rebind.
6. Debug commands and gate smoke.
7. Full gate run and docs.

---

### Task 1: Jars are cards

**Files:**
- Modify: `crates/manifold/src/store/model.rs:150` (TYPES jar row), tests at 600-660
- Modify: `crates/manifold/src/node.rs:15` (enum), 77, 93, 105, 395, 527
- Modify: `crates/manifold/src/caps/rows.rs:98-108` (new row)
- Modify: `crates/manifold/src/caps/edges.rs:27-32` (`CARD_TYPES`, `OPENER_TARGETS`), tests 183-229
- Modify: `crates/manifold/src/card_kind.rs:81-92` (`kinds()`), tests `kinds_cover_every_node_type` (107-118), `draw_layer_follows_the_graph_draw_order` (190-206), `payloads()` (150-161)
- Modify: `crates/manifold/src/mirror.rs:38-39, 62-105, 145-157, 204-211` (cache and retain), tests at 444, 577
- Modify: `crates/manifold/src/session_sync.rs:91, 217, 380-404, 475-518, 574, 616-709, 733-737, 764-768, 940, 1430-1449, 1476-1481, 1929, 2049`
- Modify: `crates/manifold/src/store/json.rs:121, 207, 256`, test at 463
- Modify: `crates/manifold/src/closed.rs:39`, test row at 516
- Modify: `crates/manifold/src/jar_defs.rs:338-350` (`create_jar`), 452-493 (`delete_jar`, `forget_jar`), 501-510 (`prune_jar_files`), 740-754 (`materialize_jar`)
- Modify: `crates/manifold/src/node_store.rs:119` (`forget_id`)
- Modify: `crates/manifold/src/reveal.rs:44`, `crates/manifold/src/card.rs:890, 917, 1272-1296`, `crates/manifold/src/graph_nav.rs:652`
- Modify: `crates/manifold/src/links.rs:1-2` (module doc)
- Test: `crates/manifold/src/session_sync/tests.rs`, `crates/manifold/src/jar_defs/tests.rs`, `crates/manifold/src/main/tests.rs`, `crates/manifold/src/node_menu/tests.rs` (every `create_jar("X")` call), `crates/manifold/src/mirror.rs` tests (444, 577)

**Interfaces:**
- Consumes: nothing new.
- Produces:
  - `Runtime::Jar` (unit variant).
  - `session_sync::pos_of(id: &str, fields: &Fields) -> egui::Vec2` (was `pos_of(fields)`).
  - `App::create_jar(&mut self, name: &str, at: crate::fetched::Placement) -> NodeId`.
  - `App::jar_defs(&self) -> BTreeMap<NodeId, JarDef>`: the live jar cards, in id order.
  - `Mirror::jar_def(&self, id: &str) -> JarDef` (now `pub(crate)`).
  - `App::jar_exists(&self, id: NodeId) -> bool`: a live `Runtime::Jar` node.
  - `caps::edges` private const `OPENER_TARGETS: [&str; 7]` (the old `CARD_TYPES`); `CARD_TYPES: [&str; 8]` gains `TYPE_JAR`.

The mirror's `jars` cache, `Mirror::add_jar`, `Mirror::forget_jar`, `Mirror::jar_defs`, `App::materialize_jar` and every Jar arm in sync are deleted. A jar's liveness is `App::nodes`, as for every card.

- [ ] **Step 1: Write the failing tests**

In `crates/manifold/src/session_sync/tests.rs`, add:

```rust
#[test]
fn a_jar_is_a_card_that_round_trips_with_its_position_and_group() {
    let path = temp_dir("jar-card").join("session.mnf");
    let mut app = app_on(&path, 1);
    let group = add_group(&mut app, "G");
    let at = crate::fetched::Placement { group: Some(group.clone()), pos: egui::vec2(120.0, -40.0) };
    let jar = app.create_jar("Work", at).to_string();
    assert!(app.save_session());
    // A tab too, so the restore is not a fresh start.
    add(&mut app, Kind::Tab, sleeping_tab("https://a.com/", "A"));
    assert!(app.save_session());
    let mut back = app_on(&path, 2);
    assert!(back.restore_session());
    assert!(matches!(back.nodes[&jar].runtime, Runtime::Jar), "a jar is a live card");
    assert_eq!(back.mirror.group(&jar), Some(group.as_str()));
    assert_eq!(back.mirror.pos(&jar), Some(egui::vec2(120.0, -40.0)));
    assert_eq!(back.jar_defs()[&jar.parse().unwrap()].name, "Work");
}

#[test]
fn a_stored_card_without_a_position_lands_jittered_around_the_world_center() {
    use crate::graph_layout::{spawn_jitter, GRAPH_WORLD_CENTER};
    // What `fields_of` hands over for a node whose POS was never written: the absent value, (0, 0).
    let unplaced = fields_of(&Node::typed(Kind::Jar));
    assert_eq!(unplaced.get(&field::POS), Some(&FieldValue::Pos(0.0, 0.0)));
    let (a, b) = (pos_of("1-1", &unplaced), pos_of("1-2", &unplaced));
    assert_eq!(a, GRAPH_WORLD_CENTER + spawn_jitter("1-1"));
    assert_ne!(a, b, "two old jars do not stack");
    let placed = Fields::from([(field::POS, FieldValue::Pos(120.0, -40.0))]);
    assert_eq!(pos_of("1-1", &placed), egui::vec2(120.0, -40.0), "a stored pos is used as is");
}

#[test]
fn closing_a_jar_records_it_and_reopen_brings_the_card_back() {
    let path = temp_dir("jar-close").join("session.mnf");
    let mut app = app_on(&path, 1);
    let jar = app.create_jar("Work", crate::fetched::Placement { group: None, pos: egui::vec2(5.0, 5.0) }).to_string();
    assert!(app.save_session());
    app.close_node_and_save(&jar);
    assert!(!app.nodes.contains_key(&jar) && app.jar_defs().is_empty());
    let entry = app.closed.pop().expect("a jar is worth reopening");
    app.reopen_closed(entry);
    let defs = app.jar_defs();
    assert_eq!(defs.len(), 1);
    assert_eq!(defs.values().next().unwrap().name, "Work");
}
```

If `pos_of` is private to `session_sync`, the test module already sees it through `use super::*`.

In `crates/manifold/src/caps/edges.rs` tests, replace `builtin_contains_takes_every_card_into_a_group_only` and `link_takes_any_card_to_any_card_and_opened_from_any_but_a_group_never_a_jar_or_a_link_node` with:

```rust
    #[test]
    fn builtin_contains_takes_every_card_a_jar_included_into_a_group_only() {
        use crate::store::model::{TYPE_GROUP, TYPE_JAR, TYPE_TAB};
        let t = edges();
        for source in [
            TYPE_TAB,
            TYPE_GROUP,
            TYPE_JAR,
            "text/markdown",
            "image/png",
            "application/pdf",
            "application/json",
            "application/octet-stream",
        ] {
            assert!(t.can_connect(CONTAINS, source, TYPE_GROUP), "{source}");
            assert!(!t.can_connect(CONTAINS, source, TYPE_TAB), "{source}");
        }
    }

    #[test]
    fn link_takes_any_card_a_jar_included_and_opened_from_any_but_a_group_never_a_jar_or_a_link_node() {
        use crate::store::model::TYPE_JAR;
        let t = edges();
        for a in [
            TYPE_TAB,
            TYPE_GROUP,
            "text/markdown",
            "image/png",
            "application/pdf",
            "application/json",
            "application/octet-stream",
        ] {
            for b in [TYPE_TAB, TYPE_GROUP, "image/png"] {
                assert!(t.can_connect(LINK, a, b), "{a} -> {b}");
                assert_eq!(t.can_connect(OPENED_FROM, a, b), a != TYPE_GROUP, "{a} -> {b}");
            }
        }
        assert!(t.can_connect(LINK, TYPE_JAR, TYPE_TAB) && t.can_connect(LINK, TYPE_TAB, TYPE_JAR));
        assert!(!t.can_connect(LINK, TYPE_TAB, "application/vnd.manifold.link"));
    }
```

Keep `opened_from_excludes_a_jar` unchanged: it must still pass.

In `crates/manifold/src/card_kind.rs` tests, change `kinds_cover_every_node_type` to exempt only links:

```rust
    /// The fixture lists one card per node type (a link is never a card) and the loaded tab, in draw order.
    #[test]
    fn kinds_cover_every_node_type() {
        use crate::store::model::{TYPES, TYPE_LINK};
        let types: Vec<&str> = TYPES.iter().map(|t| t.1).filter(|&ty| ty != TYPE_LINK).collect();
```

(the rest of the body unchanged; the test name's doc loses "a jar is never a card"). In `draw_layer_follows_the_graph_draw_order`, add `(plain(TYPE_JAR), 3),` after `(plain(TYPE_NOTE), 3),` and import `TYPE_JAR` in the test module. `only_a_group_card_is_a_container` and `every_card_kind_can_go_in_a_group_and_nothing_goes_in_anything_else` iterate `kinds()` and need no change: they now cover the jar. No caps test counts rows or `CARD_TYPES` (checked: `git grep -n "len()" crates/manifold/src/caps` finds only `pattern.rs` tag-set counts).

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -j 8 -p manifold a_jar_is_a_card_that_round_trips`
Expected: compile errors: `create_jar` takes 1 argument, no method `jar_defs` on `App`, no variant `Runtime::Jar`, `pos_of` takes 1 argument.

- [ ] **Step 3: Store, node and caps**

`store/model.rs:150`, the jar row of `TYPES`:

```rust
    (Kind::Jar, TYPE_JAR, &[field::POS, field::COLOR, field::NAME, field::EPHEMERAL]),
```

Update the model tests (600-660) that list the jar's fields to include `POS` first.

`node.rs:15`: add the variant after `Json(JsonPayload),`:

```rust
    /// A cookie jar's card. Everything it shows is the mirror's (name, colour, ephemeral flag); its cookies are
    /// `App::jars`, keyed by its id.
    Jar,
```

Add `| Runtime::Jar` to the `None`-returning arms of `saved_tab` (77), `file_path` (93) and `listed_tab` (105). In `accessible_name` (395) give it the arm `Runtime::Jar => "Cookie jar".to_string()` (match the arm shape of the `Note` arm there; use the mirror name if that function already reads names for notes). In the test at 527, add `Runtime::Jar` with the same expectation as `Runtime::None`.

`caps/rows.rs`, after the note rows (106-107):

```rust
        Row::new(TYPE_JAR).restorable(true).draw_layer(3), // a jar card, drawn with notes
```

and import `TYPE_JAR` beside the other type constants.

`caps/edges.rs:27-32`:

```rust
/// Every card type: what may sit inside a group and be linked.
const CARD_TYPES: [&str; 8] = [
    TYPE_TAB,
    TYPE_GROUP,
    TYPE_NOTE,
    TYPE_JAR,
    "image/*",
    "application/pdf",
    "application/json",
    "application/octet-stream",
];
/// What a card may be opened from: every card but a jar (a jar opens nothing).
const OPENER_TARGETS: [&str; 7] =
    [TYPE_TAB, TYPE_GROUP, TYPE_NOTE, "image/*", "application/pdf", "application/json", "application/octet-stream"];
```

and in `builtin_edges` change `EdgeRow::new(OPENED_FROM, &OPENER_SOURCES, &CARD_TYPES)` to `&OPENER_TARGETS`. Import `TYPE_JAR`. `contains` and `link` keep `CARD_TYPES`, so a jar may sit in a group and be linked (homogeneity "Links reach jars").

`card_kind.rs:81`: `kinds()` returns `[CardKind; 9]`, with `plain(TYPE_JAR),` after `plain(TYPE_NOTE),`. Add `(Kind::Jar, Runtime::Jar, plain(TYPE_JAR))` to `payloads()`.

`graph_nav.rs:652` (bench `old_layer`): add `Runtime::Jar => 3,`.

`reveal.rs:44`, `card.rs:917` (`card_filter_text`): add `Runtime::Jar` to the arm `Runtime::None` takes (no file; filtered by its own text). `session_sync.rs:91` `runtime_has_type` maps `Runtime::Jar` to `Kind::Jar`.

`session_sync.rs:940` `own_text` has a `_` arm, so the compiler will not flag it; a jar would read as a note's text (""). A jar, like a group, is known by its stored name and nothing else, so both take the name arm. Name the shared rule once, beside `own_text`:

```rust
    /// Whether node `id` is known by its stored name alone (a group, a jar): no derived label to fall back on, so
    /// the name is its own text and an empty rename changes nothing (`commit_rename`).
    pub(crate) fn named_by_name(&self, id: &str) -> bool {
        matches!(self.nodes.get(id).map(|n| &n.runtime), Some(Runtime::None | Runtime::Jar))
    }

    pub(crate) fn own_text(&self, id: &str) -> &str {
        if self.named_by_name(id) {
            // name rule: raw, for `accessible_name` (display_name) and the filter (matches the stored text)
            return self.mirror.name(id);
        }
        self.note_text(id)
    }
```

(keep `own_text`'s existing doc comment, adding "a jar's" beside "a group's raw name").

`card.rs:890` (`node_card_size`):

```rust
        Runtime::Jar => JAR_NODE_SIZE,
```

with, beside `GRAPH_NODE_SIZE` (card.rs:25):

```rust
/// A jar card's fixed size: a header and nothing under it (spec "The jar card"). Exactly
/// `TAB_HEADER_MIN_CARD_POINTS` tall, so `tab_has_header` draws the header rather than the bottom strip from zoom 1 up.
pub(crate) const JAR_NODE_SIZE: egui::Vec2 = egui::vec2(160.0, TAB_HEADER_MIN_CARD_POINTS);
```

(`TAB_HEADER_MIN_CARD_POINTS` is 80.0 at card.rs:84 and private to `card.rs`, where this constant lives too.)

`card.rs:1282` (`paint_card_body`) dispatches on `view.kind.ty`, a type string, not on `Runtime`: add `TYPE_JAR => self.paint_jar_body(ui, deferred_overlays, id, rect, inv_zoom, zoom_scale),` after the `TYPE_NOTE` arm and import `TYPE_JAR` in card.rs:5. Then, beside `paint_note_body` (1510):

```rust
    /// A jar card (spec "The jar card"): a cookie glyph in the jar's colour, its name through the display rule, and
    /// "Ephemeral" when it keeps no file. No body.
    pub(crate) fn paint_jar_body(
        &mut self,
        ui: &mut egui::Ui,
        deferred_overlays: &mut DeferredOverlays,
        id: &str,
        rect: egui::Rect,
        inv_zoom: f32,
        zoom_scale: f32,
    ) -> CardBody {
        let body = CardBody { interact_rect: rect, tooltip: None };
        if self.graph_lod_simplified {
            return body;
        }
        let def = self.mirror.jar_def(id);
        let segs = crate::names::display_name(&def.name, crate::names::UNTITLED);
        let header = CardHeader {
            glyph: Some(egui_phosphor::light::COOKIE),
            glyph_color: Some(def.color.unwrap_or(crate::theme::TAG_COLOR_PALETTE[0])),
            icon_image: None,
            name: "Jar".to_owned(),
            user_name: Some(&segs),
            source: if def.ephemeral { "Ephemeral".to_owned() } else { String::new() },
            source_is_path: false,
            scrim: false,
            right_margin: 0.0,
        };
        self.paint_card_header(ui, deferred_overlays, id, rect, inv_zoom, zoom_scale, header);
        body
    }
```

If `egui_phosphor::light::COOKIE` does not exist in the pinned version (grep the crate source under `~/.cargo/registry`), use the closest glyph there (`light::JAR` or `light::DATABASE`) and name it in the commit message.

- [ ] **Step 4: Mirror and sync carry a jar like a note**

`mirror.rs`:
- Delete the `jars` field (38-39), the jar branch in `set` (76-81), `add_jar` (83-95), `forget_jar` (97-104), `jar_defs` (143-147). Make `jar_def` `pub(crate)`.
- `retain` (202-211) exempts links only:

```rust
    /// Drops the entries `keep` rejects, except a link's: a link is not a graph node, and only `forget_link` drops it.
    pub(crate) fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        let link_nodes = &self.link_nodes;
        let keep = |id: &str| keep(id) || id.parse::<NodeId>().is_ok_and(|n| link_nodes.contains(&n));
        self.nodes.retain(|id, _| keep(id));
        self.links.retain(|id, _| keep(id));
        self.views.retain(|id, _| keep(id));
    }
```

- Delete the test `a_jars_fields_refresh_its_def_and_retain_keeps_it` (577) and the jar half of the test at 444; keep its link half.

`jar_defs.rs`, replacing the mirror cache:

```rust
impl App {
    /// The live jar cards' definitions, in id order (the order labels and pickers use).
    pub(crate) fn jar_defs(&self) -> BTreeMap<NodeId, JarDef> {
        self.nodes
            .iter()
            .filter(|(_, n)| matches!(n.runtime, Runtime::Jar))
            .filter_map(|(id, _)| Some((id.parse().ok()?, self.mirror.jar_def(id))))
            .collect()
    }

    /// Whether `id` is a live jar card.
    pub(crate) fn jar_exists(&self, id: NodeId) -> bool {
        self.nodes.get(&id.to_string()).is_some_and(|n| matches!(n.runtime, Runtime::Jar))
    }
}
```

Replace every `self.mirror.jar_defs()` in `jar_defs.rs`, `settings_page.rs`, `node_menu.rs`, `tabs/jar_pull.rs`, `debug_control.rs` with `self.jar_defs()` (bind it once to a local where the old code called it in a loop: `let defs = self.jar_defs();`), and each `jar_defs().contains_key(&id)` with `self.jar_exists(id)`. `named_jar_label(id, &defs)` and `jar_fragment(id, &defs)` keep their signatures.

`create_jar` (338) places a card:

```rust
    /// A new jar card under `name`'s clean form (name rules) at `at`, with the first palette colour no jar shows
    /// (with every colour taken, they cycle by count). The caller's `save_session` creates it.
    pub(crate) fn create_jar(&mut self, name: &str, at: crate::fetched::Placement) -> NodeId {
        let defs = self.jar_defs();
        let shown = |c: &egui::Color32| defs.values().any(|d| d.color.unwrap_or(TAG_COLOR_PALETTE[0]) == *c);
        let free = TAG_COLOR_PALETTE.iter().find(|c| !shown(c));
        let color = *free.unwrap_or(&TAG_COLOR_PALETTE[defs.len() % TAG_COLOR_PALETTE.len()]);
        let key = self.place_node(None, Kind::Jar, Runtime::Jar, at);
        self.edit_field(&key, field::NAME, FieldValue::Str(crate::names::clean_name(name).into_owned()));
        self.edit_field(&key, field::COLOR, color_value(Some(color)));
        key.parse().expect("place_node returns a store id")
    }
```

Callers: `NodeAction::NewJar` in `node_menu.rs` `apply_node_action` and `set_focused_tab_jar_named` place the jar at the tab's level: `crate::fetched::Placement { group: self.mirror.group(tab).map(str::to_owned), pos: self.layout_pos(tab) + egui::vec2(0.0, -80.0) }`. Tests that called `create_jar("X")` use the helper below; add to `session_sync/tests.rs`

```rust
/// A jar card named `name` at the top level, off the origin (which reads as unplaced, `pos_of`).
pub(crate) fn add_jar(app: &mut App, name: &str) -> NodeId {
    app.create_jar(name, crate::fetched::Placement { group: None, pos: egui::vec2(40.0, 40.0) })
}
```

and use it in every existing jar test (`jar_defs/tests.rs`, `node_menu/tests.rs`, `main/tests.rs`, `session_sync/tests.rs`).

`delete_jar` (452): replaced by the generic close. Delete the function; its callers (`popups.rs:629` `JarConfirm::Delete`, the debug control, Settings) call `self.close_node_and_save(&id.to_string())`. The fall-back-to-default of tabs that named it is now the read rule: `resolved_jar` already maps a jar that does not exist to `(JarKey::Default, true)`. After the close, sweep: `close_node` gains, after `destroy_node`, `self.sweep_jar_switches();` (cheap when nothing changed).

`forget_jar` (480): the memory half moves into `forget_id` (node_store.rs:119), which every removal runs:

```rust
        if let Ok(jar) = id.parse::<NodeId>() {
            self.forget_jar_memory(jar);
        }
```

with, in `jar_defs.rs`:

```rust
    /// A removed jar's in-memory state: its cookies saved if dirty (a Recently Closed entry may bring it back, and
    /// its file is what comes back with it), then dropped. Files are not touched here: closing never wipes
    /// credentials (spec "Backing files"); the startup prune deletes a file nothing holds.
    fn forget_jar_memory(&mut self, id: NodeId) {
        let key = JarKey::Named(id);
        if self.cookies.jar_dirty.contains_key(&key) {
            self.save_jar(key);
        }
        self.jars.remove(&key);
        if let Ok(mut snapshots) = self.cookies.jar_snapshots.lock() {
            snapshots.remove(&key);
        }
    }
```

`save_jar` must run before the node leaves `self.nodes` for `is_ephemeral` to read its flag: `forget_id` runs after `self.nodes.remove` in `drop_node`, so `is_ephemeral` reads the mirror (`self.mirror.jar_def(&id.to_string()).ephemeral`), which still holds the entry until the next save's `retain`. Change `is_ephemeral` to read `self.mirror.jar_def(..)` rather than `jar_defs()` for that reason.

`prune_jar_files` (501): its keep set becomes `self.jar_defs().keys()`, the document's live jars, and the ids of jars in Recently Closed entries (`self.closed.iter().flat_map(|e| &e.doc.nodes).filter(|n| n.kind() == Some(Kind::Jar)).filter_map(|n| n.id.parse().ok())`), so a closed jar keeps its cookies until Task 5 replaces this function.

`materialize_jar` (740): deleted. The ephemeral side effect of a remote change moves into `update_live_node` (below).

`session_sync.rs`:
- `pos_of` (217):

```rust
/// A stored node's document pos, or, for one that has none (an old jar from before jars were cards), a jittered
/// spawn around the world center: the rule `App::placement` uses for a new node with no position. A missing `POS`
/// decodes as (0, 0) (`FieldValue::absent`), so the exact origin reads as unplaced, as `Mirror::pos` reads it.
pub(crate) fn pos_of(id: &str, fields: &Fields) -> egui::Vec2 {
    match fields.get(&field::POS) {
        Some(FieldValue::Pos(x, y)) if (*x, *y) != (0.0, 0.0) => egui::vec2(*x, *y),
        _ => crate::graph_layout::GRAPH_WORLD_CENTER + crate::graph_layout::spawn_jitter(id),
    }
}
```

  Its two callers (session_sync.rs:513 and 617) pass the id. An existing test that stores a card at exactly `Vec2::ZERO` and asserts it comes back there after a restore without local state now fails: move that fixture's pos off the origin (it was asserting an accident of the encoding). `add_jar` below places at `(40, 40)` for the same reason.
- `snapshot` (380): delete the jar branch (401-404); the generic node loop writes a jar.
- `materialize` (475-518): delete the jar branch (490-495).
- `insert_materialized` (616-709): `Kind::Jar` gets its own arm before the catch-all:

```rust
            Kind::Jar => {
                self.place_node(Some(id), Kind::Jar, Runtime::Jar, Placement { group, pos });
            }
```

  (use the same local names for `group` and `pos` as the note arm does) and the catch-all at 701 becomes `Kind::Link | Kind::Unknown => return`.
- `update_live_node` (525): `materialize` calls `mirror_remote` (508) before `update_live_node` (514), so by then the mirror already holds the new flag, and `changed` (527) is a closure `|no: u32| -> bool` that says only whether field `no` changed and is not held. So the ephemeral side effects cannot compare against the mirror there, and must not need to. Split `switch_jar_ephemeral` (jar_defs.rs:406) into the local request and the effects of a flag that has already changed:

```rust
    /// Jar `id`'s ephemeral flag has just changed to `on`, locally or from another replica (the mirror already
    /// holds it). Turning it on: the jar's cookies are kept in memory for the running tabs (loaded from its file
    /// first if they were not, whatever the flag now says), then the file is deleted unless the jar is locked.
    /// Turning it off: the jar's cookies are written to a new file.
    fn after_jar_ephemeral_changed(&mut self, id: NodeId, on: bool) {
        let key = JarKey::Named(id);
        if on {
            if let (false, Some(dir)) = (self.jars.contains_key(&key), self.jars_dir()) {
                let jar = Jar::load(&dir, key);
                self.publish_jar(key, jar);
            }
            if let (Some(dir), Some(false)) = (self.jars_dir(), self.jars.get(&key).map(Jar::is_locked)) {
                if let Err(e) = Jar::delete_files(&dir, key) {
                    eprintln!("jar: could not delete {}: {e}", key.file_stem());
                }
            }
        } else {
            self.ensure_jar_loaded(key);
            self.save_jar(key);
        }
    }

    /// A local Make/Stop Ephemeral (`set_jar_ephemeral`, after any confirm): writes the flag, then its effects.
    fn switch_jar_ephemeral(&mut self, id: NodeId, on: bool) {
        if !self.jar_exists(id) || self.mirror.jar_def(&id.to_string()).ephemeral == on {
            return;
        }
        self.edit_field(&id.to_string(), field::EPHEMERAL, FieldValue::Bool(on));
        self.after_jar_ephemeral_changed(id, on);
    }
```

  (`JarKey::Named(id)` becomes `id` in Task 3 with every other `JarKey` use.) The old `local: bool` parameter goes: the remote path no longer calls `switch_jar_ephemeral`, and its one other caller, `set_jar_ephemeral` (jar_defs.rs:397), becomes `self.switch_jar_ephemeral(id, on)`. In `update_live_node`, after the POS snap (531-533) and before `let Some(node) = self.nodes.get_mut(id)`, where `self` is not borrowed:

```rust
        // Only a jar has `ephemeral`. The mirror already holds the new flag (`mirror_remote`), so this applies its
        // effects without comparing (`after_jar_ephemeral_changed`).
        if changed(field::EPHEMERAL) {
            if let Ok(jar) = id.parse::<NodeId>() {
                let on = matches!(fields.get(&field::EPHEMERAL), Some(FieldValue::Bool(true)));
                self.after_jar_ephemeral_changed(jar, on);
            }
        }
```

  and the `match &mut node.runtime` gains `Runtime::Jar => {}`. Make `after_jar_ephemeral_changed` `pub(crate)`. Add to `jar_defs/tests.rs`:

```rust
#[test]
fn a_remote_ephemeral_switch_keeps_the_cookies_in_memory_and_deletes_the_file() {
    let dir = crate::store::testutil::temp_dir("jar-remote-eph");
    let mut app = App { jars_dir_override: Some(dir.clone()), ..App::default() };
    let jar = crate::session_sync::tests::add_jar(&mut app, "Work");
    let file = dir.join(format!("{jar}.json"));
    app.ensure_jar_loaded(JarKey::Named(jar));
    app.save_jar(JarKey::Named(jar));
    assert!(file.exists());
    // What `materialize` does with a remote change: the mirror first, then `update_live_node`.
    let mut fields = app.snapshot(jar).expect("a live jar").fields;
    fields.insert(crate::store::model::field::EPHEMERAL, FieldValue::Bool(true));
    app.materialize(jar.to_string(), Kind::Jar, &fields, None, None);
    assert!(app.mirror.jar_def(&jar.to_string()).ephemeral);
    assert!(!file.exists(), "the file went");
    assert!(app.jars.contains_key(&JarKey::Named(jar)), "the cookies stay in memory");
}
```

  (`App::snapshot(NodeId) -> Option<Snapshot>` is what `closed.rs:153` uses; the jar's `NAME` and `COLOR` are held as pending local edits and skipped by `mirror_remote`, `EPHEMERAL` is not.) Task 3 rewrites the `JarKey::Named(jar)` in this test to `jar`.
- `apply_remote` (733-737) and `materialize_imported` (1929): delete the "jars first" ordering; links still sort last.
- `remove_remotely_deleted` (764-768): delete the jar branch; a jar falls through to the generic `drop_node`.
- `check_graph_matches_document` (1430-1449): delete the jar loops; jars are checked with the other nodes.
- `restore_session` pre-pass (1476-1481): links only. The fresh-start test changes from "nothing but jars and links" to "nothing but jars", and a fresh start still shows its jars (they are cards now):

```rust
        // Links ride along: stored, never graph nodes.
        for (id, kind, fields, _) in nodes.iter().filter(|(_, kind, ..)| *kind == Kind::Link) {
            self.materialize(id.to_string(), *kind, fields, None, None);
        }
        nodes.retain(|(_, kind, ..)| *kind != Kind::Link);
        // Only jars (with Task 3, the Default at least): a fresh start, which keeps them where they are.
        if nodes.iter().all(|(_, kind, ..)| *kind == Kind::Jar) {
            for (id, kind, fields, group) in &nodes {
                self.materialize(id.to_string(), *kind, fields, group.map(|g| g.to_string()), None);
            }
            self.restore_closed(local.closed);
            self.arm_local(unread, true);
            return false;
        }
```

  Add the test `a_session_of_only_jars_is_a_fresh_start_that_keeps_them` beside the round-trip test: save a session holding one jar, `restore_session` on a second app returns `false` and `nodes` holds the jar.
- `teardown_running_session` (2049): drop the `n.kind() != Kind::Jar` filter; jar cards are torn down with the rest.

`store/json.rs`: delete the jar exemptions at 121 (`is_live_card`), 207 (FROM/TO label filter) and 256 (`creates` card set) so a jar exports, imports and is linked like any card; links stay excluded. Update `a_foreign_import_drops_download_paths_and_outside_jars` (463) to its remaining assertion on outside jars: an imported tab's `JAR` naming a jar outside the file is still dropped under Foreign (the filter at 200 is unchanged).

`closed.rs:39` `worth_reopening`: a jar is worth reopening through its caps row (restorable); delete any jar special case there. The test row at 516 becomes `(Kind::Jar, &none, true)`.

`links.rs:1-2` module doc: "Links: stored `Kind::Link` nodes joining two cards by `from` and `to`. A link is mirror and store state, never a graph card; it goes with either end (`queue_delete`, `remove_remotely_deleted`)."

- [ ] **Step 5: Build, then run the new and touched tests**

Run: `cargo build -j 8 -p manifold`
Expected: builds. Fix each non-exhaustive `match` the compiler names with a no-view `Runtime::Jar` arm.

Run: `cargo test -j 8 -p manifold jar`
Expected: PASS, including `a_jar_is_a_card_that_round_trips_with_its_position_and_group`, `closing_a_jar_records_it_and_reopen_brings_the_card_back`, `opened_from_excludes_a_jar`.

Run: `cargo test -j 8 -p manifold`
Expected: PASS. A failing old jar test that asserted "a jar is not a card" is deleted with a line in the commit message; one that asserted cookie files are deleted on delete is deleted (Task 5 owns file lifetime).

Run: `git grep -nE "mirror\.jar_defs|add_jar\(&?mut self|fn forget_jar\b|\.forget_jar\(|materialize_jar|delete_jar|create_jar\(\"[^\"]*\"\)" -- crates`
Expected: no hits (the test helper `add_jar(app, ..)` takes `app`, not `self`, and does not match).

- [ ] **Step 6: fmt, clippy, commit**

Run: `cargo fmt --all` then `cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings`
Expected: no output from fmt; clippy finishes with no warnings.

```bash
git add -A crates/manifold
git commit -m "Jars are cards: Runtime::Jar, POS, and no jar special cases in sync

A jar is an App::nodes entry placed like a note. Deleted: Mirror::jars,
add_jar, forget_jar, jar_defs, App::materialize_jar, delete_jar, and the
jar arms in snapshot, materialize, apply_remote, remove_remotely_deleted,
restore_session, teardown and store/json. A stored card without POS lands
jittered around the world center.

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 2: The jar card's body and menu

**Files:**
- Modify: `crates/manifold/src/card.rs:1510` area (`paint_jar_body`), `CardView` (983)
- Modify: `crates/manifold/src/node_menu.rs:521-666` (`node_menu_config`), 982-997 (`commit_rename`), 999+ (`apply_node_action`, whose jar arms sit at 1084-1102)
- Modify: `crates/manifold/src/jar_defs.rs:67-74` (`JarConfirm`), 301-332 (`jar_confirm_text`), 296-299 (`jar_tab_count`)
- Modify: `crates/manifold/src/settings_page.rs:5-6, 50-55, 114, 500-590, 620-630`
- Modify: `crates/manifold/src/popups.rs:615-640`
- Test: `crates/manifold/src/node_menu/tests.rs`, `crates/manifold/src/settings_page.rs` tests

**Interfaces:**
- Consumes: `Runtime::Jar`, `App::jar_defs`, `App::jar_exists`, `App::close_node_and_save` (Task 1).
- Produces:
  - `NodeAction::ClearJarCookies`, `NodeAction::SetJarEphemeral(bool)` (menu actions on a jar card, acting on the menu's node).
  - `JarConfirm { Clear(JarKey), Ephemeral(NodeId) }` (`Delete` removed).

- [ ] **Step 1: Write the failing tests**

In `crates/manifold/src/node_menu/tests.rs`:

```rust
#[test]
fn a_jar_cards_menu_has_the_generic_items_and_its_cookie_items() {
    let labels = |app: &App, id: &str| -> Vec<&'static str> {
        node_menu_config(&app.node_menu_view(id).unwrap(), false)
            .items
            .iter()
            .filter_map(|item| match item {
                NodeMenuItem::Button(label, _) => Some(*label),
                _ => None,
            })
            .collect()
    };
    let mut app = App::default();
    let jar = crate::session_sync::tests::add_jar(&mut app, "Work").to_string();
    let shown = labels(&app, &jar);
    for want in ["Clear Cookies...", "Make Ephemeral", "Rename", "Duplicate", "Pin", "Close"] {
        assert!(shown.contains(&want), "{want} missing from {shown:?}");
    }
    assert!(!shown.iter().any(|l| l.starts_with("Delete")), "{shown:?}");
    app.set_jar_ephemeral(jar.parse().unwrap(), true);
    assert!(labels(&app, &jar).contains(&"Stop Ephemeral"));
}

#[test]
fn make_ephemeral_on_a_jar_card_with_a_saved_file_asks_first() {
    let dir = crate::store::testutil::temp_dir("jar-menu-eph");
    let mut app = App { jars_dir_override: Some(dir.clone()), ..App::default() };
    let jar = crate::session_sync::tests::add_jar(&mut app, "Work");
    std::fs::write(dir.join(format!("{jar}.json")), b"x").unwrap();
    app.apply_node_action(&egui::Context::default(), &jar.to_string(), NodeAction::SetJarEphemeral(true));
    assert_eq!(app.pending_jar_confirm, Some(JarConfirm::Ephemeral(jar)));
}

#[test]
fn an_empty_rename_leaves_a_jar_its_name() {
    let mut app = App::default();
    let jar = crate::session_sync::tests::add_jar(&mut app, "Work").to_string();
    app.commit_rename(&jar, "   ");
    assert_eq!(app.mirror.name(&jar), "Work", "a jar is known by its name alone, like a group");
    app.commit_rename(&jar, "Home");
    assert_eq!(app.mirror.name(&jar), "Home");
}
```

`apply_node_action` takes `ctx: &egui::Context` first (node_menu.rs:999); tests pass `&egui::Context::default()` as `node_menu/tests.rs:24` does. Match the real name of the pending-confirm field (grep `fn offer_jar_confirm`).

Menu labels in this codebase use three ASCII dots ("New Jar...", "Color..."), so "Clear Cookies..." does too. Rename, Duplicate and Pin come from `finish`'s shared tail; check they are pushed as `Button`s there (if `finish` pushes them as another variant, filter on that variant too).

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -j 8 -p manifold a_jar_cards_menu`
Expected: compile error: no variant `SetJarEphemeral` on `NodeAction`.

- [ ] **Step 3: Menu**

In `node_menu.rs`, add to `NodeAction`:

```rust
    /// A jar card's "Clear Cookies…": asks first (`JarConfirm::Clear`).
    ClearJarCookies,
    /// A jar card's "Make Ephemeral" / "Stop Ephemeral".
    SetJarEphemeral(bool),
```

`NodeMenuView` (675) gains a field, set in `node_menu_view` (856):

```rust
    /// A jar card's ephemeral flag (`JarDef::ephemeral`); `false` for every other card.
    ephemeral: bool,
```

```rust
            ephemeral: matches!(node.runtime, Runtime::Jar) && self.mirror.jar_def(id).ephemeral,
```

(and `ephemeral: false` in every test constructor of `NodeMenuView`, grep `NodeMenuView {`).

In `node_menu_config` (591), a jar arm before the `_` arm (import `TYPE_JAR`):

```rust
        TYPE_JAR => {
            let ephemeral = if view.ephemeral {
                Button("Stop Ephemeral", A::SetJarEphemeral(false))
            } else {
                Button("Make Ephemeral", A::SetJarEphemeral(true))
            };
            let items = vec![Button("Clear Cookies...", A::ClearJarCookies), ephemeral];
            finish("jar_context_menu", 150.0, items, delete("Close"), Vec::new())
        }
```

In `apply_node_action` (1084-1102):

```rust
            NodeAction::ClearJarCookies => {
                if let Ok(jar) = id.parse() {
                    self.offer_jar_confirm(JarConfirm::Clear(JarKey::Named(jar)));
                }
            }
            NodeAction::SetJarEphemeral(on) => {
                if let Ok(jar) = id.parse() {
                    self.request_jar_ephemeral(jar, on);
                }
            }
```

`NodeAction::key_hint` and any exhaustive match on `NodeAction` gets the two variants (no key hint).

`commit_rename` (node_menu.rs:982): the group-only empty-name rule becomes the `named_by_name` rule (Task 1), so a jar keeps its name on an empty draft, as a group does:

```rust
        if self.named_by_name(id) {
            if !clean.is_empty() {
                self.edit_name(id, &clean);
            }
        } else if self.nodes.contains_key(id) {
            self.edit_name(id, &clean);
        }
```

and its doc's "a group takes the clean name" becomes "a group or jar takes the clean name; an empty one changes nothing (`named_by_name`)".

- [ ] **Step 4: Settings keeps only the built-in jar's row**

In `settings_page.rs` `settings_jars_section` (500-590): delete the named-jar loop (545-587), the empty hint (588-590), `SettingsRequests::rename_jar` and `jar_ephemeral` (50-53) and their handling (620-622 and the ephemeral handler). Keep the built-in row with Clear Cookies; Task 3 removes the section. Delete `jar_name_draft`, `jar_name_edit_id` and their tests if nothing else uses them (grep). A jar card is renamed by the generic Rename (`commit_rename` writes `NAME` on any node), so `App::rename_jar` goes too once Settings no longer calls it (grep for other callers first; port a debug caller to `commit_rename`). Delete the settings tests that drive rename, ephemeral or delete through Settings; port any assertion that is about behaviour (not Settings) to a menu test above.

`JarConfirm::Delete` (jar_defs.rs:71) and its arms in `jar_confirm_text` (313-322) and `popups.rs:629-632` are deleted; `jar_tab_count` (296) is deleted if its only caller was the delete confirm and Settings rows (grep).

- [ ] **Step 5: Run the tests**

Run: `cargo test -j 8 -p manifold jar`
Expected: PASS.

Run: `cargo test -j 8 -p manifold`
Expected: PASS.

Run: `git grep -nE "JarConfirm::Delete|rename_jar\b" -- crates`
Expected: no hits. (`jar_name_draft` and `jar_name_edit_id` go only if nothing else used them; say which in the commit.)

- [ ] **Step 6: fmt, clippy, commit**

Run: `cargo fmt --all` then the clippy command from Global Constraints.
Expected: clean.

```bash
git add -A crates/manifold
git commit -m "Jar card: cookie glyph, name, ephemeral marker; Clear Cookies and Ephemeral in its menu

Settings loses named jars (rename, ephemeral, delete); a jar card is
renamed and closed like any card. JarConfirm::Delete is gone.

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 3: The Default jar is a card; `JarKey` becomes `NodeId`

**Files:**
- Modify: `crates/manifold/src/store/model.rs` (field 22, `FIELDS` at 246, `TYPES` jar row, model tests)
- Modify: `crates/manifold/src/jar.rs:22-41` (`JarKey` deleted), 287-315 (`named_jar_of`, `prune_orphans`), 333-420 (`Jar::path`, `delete_files`, `load`, `save`), the `Snapshots` type, `CookieState`
- Modify: `crates/manifold/src/jar_defs.rs` (all `JarKey` uses, `JarDef`, `JarLabel`, `jar_label`, `jar_color`, `resolved_jar`, new `default_jar`, `ensure_default_jar`)
- Modify: `crates/manifold/src/main.rs:248, 295-297, 548-549`
- Modify: `crates/manifold/src/tabs.rs:166, 355-356, 752-800`, `crates/manifold/src/tabs/jar_pull.rs:80, 120, 181`, `crates/manifold/src/tabs/spawn_result.rs`, `crates/manifold/src/sleep_state.rs:53`
- Modify: `crates/manifold/src/settings_page.rs` (the Jars section deleted), `crates/manifold/src/popups.rs:615-640, 1913`
- Modify: `crates/manifold/src/node_ops.rs:188-217` (guards), `crates/manifold/src/groups.rs` (`destroy_group` member loop), `crates/manifold/src/session_sync.rs:1054` (`edit_group`), 2049 (`teardown_running_session`)
- Modify: `crates/manifold/src/closed.rs:49` (`members_under`), 103-135 (`live_fields`), `crates/manifold/src/session_sync.rs` (`materialize_imported`, `apply_remote`, `replace_running_session` 1988)
- Modify: `crates/manifold/src/card.rs:589` (`card_controls`), 983 (`card_view`), `crates/manifold/src/node_menu.rs` (Default has no Close)
- Modify: `crates/manifold/src/event_loop.rs:175-186` (startup)
- Modify: `crates/manifold/src/debug_control.rs:1429-1437` (`CLEARJAR default`)
- Modify: `crates/manifold/src/mirror.rs` (the `defaults` index, `default_jar_ids`)
- Modify: `crates/gate/src/jar.rs:339-341` (`default_jar`: no `default.json`), 346, 361, 372, 502 (its callers)
- Test (every file that names `JarKey`, from `git grep -l JarKey -- crates`): `crates/manifold/src/jar_defs/tests.rs`, `crates/manifold/src/jar/tests.rs` (407: `Jar::path(&dir, JarKey::Default)`), `crates/manifold/src/main/tests.rs`, `crates/manifold/src/node_menu/tests.rs`, `crates/manifold/src/session_sync/tests.rs`, `crates/manifold/src/popups.rs` tests (1913), `crates/manifold/src/settings_page.rs` tests, `crates/manifold/src/debug_control.rs` tests. The literal `"default.json"` names in `jar/tests.rs` (temp and aside handling) and `store/atomic.rs` tests are arbitrary file names and stay.

**Interfaces:**
- Consumes: Task 1 (`create_jar(name, at)`, `jar_defs()`, `jar_exists`), Task 2 (`JarConfirm`).
- Produces:
  - `field::IS_DEFAULT: u32 = 22`.
  - `JarDef { name, color, ephemeral, is_default: bool }`.
  - `Mirror::default_jar_ids(&self) -> impl Iterator<Item = NodeId> + '_`: the ids whose mirrored `IS_DEFAULT` is true, lowest first (an index `set` maintains, like `views`).
  - `App::default_jar(&self) -> Option<NodeId>`: the lowest-id live jar with `IS_DEFAULT`.
  - `App::is_default_jar(&self, id: &str) -> bool`.
  - `App::ensure_default_jar(&mut self) -> bool`: true when it changed anything (the caller saves).
  - `App::resolved_jar(&self, choice: &JarChoice) -> Option<(NodeId, bool)>`: `None` only when no Default exists (tests that never call `ensure_default_jar`).
  - `Jar::path(dir: &Path, id: NodeId) -> PathBuf` (`<dir>/<id>.json`), `Jar::load(dir, id)`, `Jar::save(&self, dir, id)`, `Jar::delete_files(dir, id)`.
  - `App::jars: BTreeMap<NodeId, Jar>`; `CookieState::jar_tabs: HashMap<String, (NodeId, TabCookies)>`; `jar_dirty: BTreeMap<NodeId, Instant>`; `Snapshots` keyed by `NodeId`.
  - `TabSpawnOutcome::jar: Option<NodeId>`.
  - `sleep_state::jar_switch(spawned_with: Option<NodeId>, wanted: NodeId, reseed: bool, busy: bool, focused: bool) -> JarSwitch`.
  - `JarConfirm { Clear(NodeId), Ephemeral(NodeId) }`.
  - `JarLabel::built_in()` is deleted; the Default card's label is its stored name with `JarMarker::BuiltIn`.
  - `CardView::closable: bool`.

- [ ] **Step 1: Write the failing tests**

In `crates/manifold/src/jar_defs/tests.rs`:

```rust
#[test]
fn the_default_jar_is_made_once_at_the_top_level() {
    let mut app = App::default();
    assert_eq!(app.default_jar(), None);
    assert!(app.ensure_default_jar(), "made");
    let default = app.default_jar().expect("a Default");
    assert_eq!(app.mirror.group(&default.to_string()), None);
    assert_eq!(app.jar_defs()[&default].name, "Default");
    assert!(!app.ensure_default_jar(), "already there: nothing to do");
    assert_eq!(app.jar_defs().len(), 1);
}

#[test]
fn two_defaults_converge_on_the_lowest_id() {
    let mut app = App::default();
    let (a, b) = (add_jar(&mut app, "Default"), add_jar(&mut app, "Default"));
    for jar in [a, b] {
        app.edit_field(&jar.to_string(), crate::store::model::field::IS_DEFAULT, FieldValue::Bool(true));
    }
    let (low, high) = (a.min(b), a.max(b));
    assert!(app.ensure_default_jar());
    assert_eq!(app.default_jar(), Some(low));
    let defs = app.jar_defs();
    assert!(!defs[&high].is_default);
    assert_eq!(defs[&high].name, "Default (old)");
}

#[test]
fn the_default_jar_cannot_be_closed_and_an_unset_tab_resolves_to_it() {
    let mut app = App::default();
    app.ensure_default_jar();
    let default = app.default_jar().unwrap();
    app.close_node(&default.to_string());
    assert!(app.jar_exists(default), "still there");
    let tab = tab_in(&mut app, None);
    assert_eq!(app.resolved_jar(&jar_of(&app, &tab)), Some((default, false)));
}

#[test]
fn duplicating_the_default_makes_an_ordinary_jar() {
    let mut app = App::default();
    app.ensure_default_jar();
    let default = app.default_jar().unwrap();
    app.duplicate_nodes(&[default.to_string()]);
    let defs = app.jar_defs();
    assert_eq!(defs.len(), 2);
    assert_eq!(defs.values().filter(|d| d.is_default).count(), 1);
    assert_eq!(app.default_jar(), Some(default));
}
```

with `add_jar` imported from `crate::session_sync::tests` and `FieldValue` from `crate::store::model`.

In `crates/manifold/src/session_sync/tests.rs`:

```rust
#[test]
fn replacing_the_running_session_keeps_the_default_jar() {
    let dir = temp_dir("default-replace");
    let mut app = app_on(&dir.join("session.mnf"), 1);
    app.ensure_default_jar();
    let default = app.default_jar().unwrap();
    add(&mut app, Kind::Tab, sleeping_tab("https://a.com/", "A"));
    assert!(app.save_session());
    app.teardown_running_session();
    assert!(app.jar_exists(default));
    assert_eq!(app.nodes.len(), 1, "only the Default survives a teardown");
}

```

```rust
#[test]
fn an_imported_default_jar_is_the_running_one() {
    use crate::store::json::{import, JsonDoc, JsonNode, Trust};
    let mut app = App::default();
    app.ensure_default_jar();
    let running = app.default_jar().unwrap();
    let node = |id: &str, fields: serde_json::Value| JsonNode { id: id.into(), fields: fields.as_object().unwrap().clone() };
    let doc = JsonDoc {
        nodes: vec![
            node("a", serde_json::json!({ "type": crate::store::model::TYPE_JAR, "name": "Default", "is_default": true })),
            node("t", serde_json::json!({ "type": crate::store::model::TYPE_TAB, "url": "https://a/", "jar": "a" })),
        ],
        local: None,
    };
    let next = || app.alloc_node_id().parse().unwrap();
    let imported = import(&doc, next, Trust::Own).unwrap();
    let tab = imported[1].id.to_string();
    app.materialize_imported(imported, None, false);
    assert_eq!(app.jar_defs().len(), 1, "no second Default");
    let jar = app.mirror.get(&tab, crate::store::model::field::JAR);
    assert_eq!(jar, FieldValue::Id(Some(running)), "an edge to the file's Default names the running one");
}
```

Write the `"jar": "a"` value the way `doc_to_json` writes an `Id` field (check a neighbouring `store/json.rs` test; labels are strings). If `next` cannot borrow `app` mutably while `import` runs, allocate ids from a local counter as the neighbouring import tests do.

In `crates/manifold/src/jar/tests.rs`, change every `JarKey::Default` / `JarKey::Named(id)` to a `NodeId`, and add:

```rust
#[test]
fn a_jar_file_is_named_by_its_node_id() {
    let id: NodeId = "3-7".parse().unwrap();
    assert_eq!(Jar::path(std::path::Path::new("d"), id), std::path::Path::new("d").join("3-7.json"));
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -j 8 -p manifold the_default_jar_is_made_once`
Expected: compile error: no method `default_jar` / `ensure_default_jar`, no field `IS_DEFAULT`.

- [ ] **Step 3: Field 22 and `JarDef::is_default`**

`store/model.rs`: in `mod field`, `pub const IS_DEFAULT: u32 = 22;`. In `FIELDS` (246), a `FieldDef` for 22 named `"is_default"`, type `Bool`, in the same shape as `EPHEMERAL`'s entry; `FIELDS` grows to `[FieldDef; 19]`. The jar row of `TYPES` becomes `&[field::POS, field::COLOR, field::NAME, field::EPHEMERAL, field::IS_DEFAULT]`. Update the model tests that count fields or list the jar's.

`mirror.rs` `jar_def`: `is_default: matches!(self.get(id, field::IS_DEFAULT), FieldValue::Bool(true)),`. `JarDef` gains `pub(crate) is_default: bool` with a doc line: "The built-in Default jar (`field::IS_DEFAULT`); exactly one live jar has it once `ensure_default_jar` has run."

The Default is read per card per frame (`jar_chip` at card.rs:1338 and 2140, `will_switch`, `sweep_jar_switches`), so finding it must not scan every node. `Mirror` keeps an index, the way it keeps `views` for `VIEW`:

```rust
    /// The ids whose `IS_DEFAULT` is true (`default_jar_ids`), kept by `set` and `retain` like `views`. It may hold
    /// a removed jar until the next `retain`; `App::default_jar` checks liveness.
    defaults: BTreeSet<NodeId>,
```

In `set`, beside the `VIEW` branch:

```rust
        if no == field::IS_DEFAULT {
            if let Ok(node) = id.parse::<NodeId>() {
                if value == FieldValue::Bool(true) {
                    self.defaults.insert(node);
                } else {
                    self.defaults.remove(&node);
                }
            }
        }
```

In `retain`, `self.defaults.retain(|n| keep(&n.to_string()));` (it runs on a save, not per frame). And

```rust
    /// The ids whose mirrored `IS_DEFAULT` is true, lowest first: what `App::default_jar` picks from.
    pub(crate) fn default_jar_ids(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.defaults.iter().copied()
    }
```

Every write of a mirrored field goes through `set` (`init`, `init_field`, `mirror_remote` and `edit_field` all call it; the doc on `set` says so), so the index cannot miss a write. Add to `mirror.rs` tests:

```rust
    #[test]
    fn the_default_index_follows_is_default_writes_and_retain() {
        let mut m = Mirror::default();
        let (a, b): (NodeId, NodeId) = ("1-1".parse().unwrap(), "1-2".parse().unwrap());
        m.set("1-2", field::IS_DEFAULT, FieldValue::Bool(true));
        m.set("1-1", field::IS_DEFAULT, FieldValue::Bool(true));
        assert_eq!(m.default_jar_ids().collect::<Vec<_>>(), [a, b], "lowest first");
        m.set("1-1", field::IS_DEFAULT, FieldValue::Bool(false));
        m.retain(|id| id != "1-2");
        assert_eq!(m.default_jar_ids().count(), 0);
    }
```

- [ ] **Step 4: The Default card**

In `jar_defs.rs`:

```rust
/// The Default jar's name when it is made, and the name a losing duplicate takes (spec "The Default jar").
const DEFAULT_NAME: &str = "Default";
const OLD_DEFAULT_NAME: &str = "Default (old)";

impl App {
    /// The Default jar: the lowest-id live jar with `IS_DEFAULT` (`ensure_default_jar` leaves exactly one). Cheap
    /// enough per card per frame: the mirror's index holds one id, two for a moment after two replicas' first
    /// launches, plus a closed one until the next save; each costs a `to_string` and a hash lookup.
    pub(crate) fn default_jar(&self) -> Option<NodeId> {
        self.mirror.default_jar_ids().find(|&id| self.jar_exists(id))
    }

    /// Whether node `id` is the Default jar.
    pub(crate) fn is_default_jar(&self, id: &str) -> bool {
        id.parse().ok().is_some_and(|id| self.default_jar() == Some(id))
    }

    /// The startup and sync invariant (spec "The Default jar"): with no Default, one is made at the top level; with
    /// several (two replicas' concurrent first launches), the lowest id keeps the flag and each other is cleared and
    /// renamed, a deterministic repair every replica makes alike. Returns whether it wrote anything; the caller
    /// saves.
    pub(crate) fn ensure_default_jar(&mut self) -> bool {
        let defaults: Vec<NodeId> = self.mirror.default_jar_ids().filter(|&id| self.jar_exists(id)).collect();
        let Some((_, losers)) = defaults.split_first() else {
            let pos = crate::graph_layout::GRAPH_WORLD_CENTER + crate::graph_layout::spawn_jitter(DEFAULT_NAME);
            let id = self.create_jar(DEFAULT_NAME, crate::fetched::Placement { group: None, pos });
            self.edit_field(&id.to_string(), field::IS_DEFAULT, FieldValue::Bool(true));
            return true;
        };
        for loser in losers {
            let key = loser.to_string();
            self.edit_field(&key, field::IS_DEFAULT, FieldValue::Bool(false));
            self.edit_field(&key, field::NAME, FieldValue::Str(OLD_DEFAULT_NAME.to_owned()));
        }
        !losers.is_empty()
    }
}
```

The index iterates in id order, so `split_first` keeps the lowest.

Call sites:
- `event_loop.rs:175-186` startup:

```rust
        self.open_store();
        let restored = self.restore_session();
        if self.ensure_default_jar() {
            self.save_session();
        }
        if !restored {
            if self.settings.has_shown_first_launch_demo || !self.demo_available() {
                self.spawn_root_tab();
            } else {
                self.spawn_first_launch_demo();
                self.settings.has_shown_first_launch_demo = true;
                self.save_settings();
            }
        }
        self.prune_content_dir();
        self.prune_jar_files();
```

- `apply_remote` (session_sync.rs): read `let had_default = self.default_jar().is_some();` before the batch, and after it is applied, before its sweep, `if had_default && self.ensure_default_jar() { self.save_session(); }`. This converges two Defaults that arrive from replicas' concurrent first launches, and repairs a remote delete of the Default. The `had_default` guard keeps windowless sync tests that never made a Default free of a surprise jar card.
- `replace_running_session` (1988), after the import: `self.ensure_default_jar();` (its own save follows).

- [ ] **Step 5: `JarKey` becomes `NodeId`**

Delete `JarKey` (jar.rs:22-41). The rewrite rule at every use the compiler names (the 15 files listed by `grep -rln JarKey crates`):
- `JarKey::Named(id)` becomes `id`; a pattern `JarKey::Named(id)` becomes a binding `id`.
- `JarKey::Default` becomes `self.default_jar()`; where the code needs a jar and gets `None`, it does nothing (only tests run without a Default).
- `key.file_stem()` in log lines becomes `id` (`NodeId: Display`).
- `Jar::path(dir, id)` is `dir.join(format!("{id}.json"))`; `delete_files`, `load`, `save` take `id: NodeId` and build names from `format!("{id}.json")`.
- `named_jar_of` (287) no longer special-cases `default.json`; its doc loses that sentence.
- `main.rs:548-549`: delete the eager load of the Default jar; the Default loads lazily like any jar (`ensure_jar_loaded` at its first spawn).
- `main.rs:295`: `jars: BTreeMap<NodeId, Jar>`.

`resolved_jar` (jar_defs.rs:523):

```rust
    /// The only place a tab's jar choice becomes a jar (spec "Resolution"), and whether it dangles: a choice naming a
    /// jar that is not a live card resolves to the Default with `true` (the chip warns). `None` only with no Default.
    pub(crate) fn resolved_jar(&self, choice: &JarChoice) -> Option<(NodeId, bool)> {
        match choice.jar {
            Some(id) if self.jar_exists(id) => Some((id, false)),
            Some(_) => self.default_jar().map(|d| (d, true)),
            None => self.default_jar().map(|d| (d, false)),
        }
    }
```

Callers (`jar_chip`, `jar_tab_count` if it survived Task 2, `will_switch`, `sweep_jar_switches`, `spawn_tab_async`) take `let Some((jar, missing)) = self.resolved_jar(..) else { return .. }` with the function's neutral value (`None`, `false`, or skipping the tab). `TabSpawnOutcome::jar` becomes `Option<NodeId>`; a spawn with `None` seeds no cookies and pulls none (`note_spawned_jar` takes `Option<NodeId>` and returns early on `None`).

`jar_label(id: NodeId) -> JarLabel`:

```rust
    /// Jar `id`'s label: its stored name, marked `BuiltIn` for the Default and with a fragment when another jar
    /// shares the name (`named_jar_label`). A jar that is gone reads as the Default's label, as that is what its tabs
    /// use.
    pub(crate) fn jar_label(&self, id: NodeId) -> JarLabel {
        let defs = self.jar_defs();
        let shown = if defs.contains_key(&id) { Some(id) } else { self.default_jar() };
        let Some(shown) = shown else { return JarLabel { name: DEFAULT_NAME.into(), marker: Some(JarMarker::BuiltIn) } };
        let mut label = named_jar_label(shown, &defs).expect("a live jar");
        if defs[&shown].is_default {
            label.marker = Some(JarMarker::BuiltIn);
        }
        label
    }
```

`jar_color(id: NodeId)`: the jar's colour, the first palette colour if it has none, grey when it is not a live jar. The Default card gets a colour at creation like any jar (`create_jar`), so it no longer reads grey.

Delete `JarLabel::built_in` and its uses (popups and settings tests port to a Default card).

`JarConfirm::Clear(JarKey)` becomes `Clear(NodeId)`; `jar_confirm_text` checks `self.jar_exists(id)`. `popups.rs:1913` test builds `JarConfirm::Clear(default)` after `ensure_default_jar`. `node_menu.rs` `NodeAction::ClearJarCookies` passes `JarConfirm::Clear(jar)`.

`debug_control.rs:1429-1437` (`CLEARJAR`): `"default"` maps to `self.default_jar()`, any other name to `self.jar_named(&name)`.

`gate/src/jar.rs:339-341` reads `default.json`, which no build writes after this task. The gate is fixed here, not in Task 6, so at every commit the gate matches the app. Two things change for the gate:

1. The Default's file is `<its id>.json`.
2. Each `run` (jar.rs:274) starts from a freshly imported session (`probes::import_session` deletes `session.mnf`), so each run's app makes a new Default card with a new id. The login run's cookies would then sit in a file the next run's Default does not name. The old `default.json` outlived the document; a Default card does not (flagged to the user).

The gate therefore carries the Default's cookies across its own imports, the way `rebind_jar_files` carries a reopened jar's:

- Every `run` seed includes the Default card: `json!({"id": "d", "type": JAR, "name": "Default", "is_default": true, "pos": [300.0, 0.0]})` beside the tab. The tab has no `jar`, so it resolves to the Default.
- After `import_session`, read the new Default's id from `exe --dump <profile>/session.mnf`. Its stdout is header lines and then the pretty JSON document; parse from the first line that starts with `{`, and take the `JAR` node whose `is_default` is true.
- If `<id>.json` does not exist, copy into it the newest-modified `<n>-<m>.json` in `jars_dir()`, skipping asides and temps (`file_name` ends in `.json` and has exactly one `.`).

The "fresh" run moves the jars dir aside first, so it finds nothing to copy and starts signed out, as the run intends. The copy rule is therefore the same for every run. `default_jar` takes the id:

```rust
    /// The Default jar's cookie file in this profile: `<id>.json`, the id read from the session (`default_jar_id`).
    fn default_jar(&self) -> io::Result<PathBuf> {
        let id = default_jar_id(&self.exe, &self.profile.join("session.mnf"))?;
        Ok(self.jars_dir().join(format!("{id}.json")))
    }
```

with `default_jar_id` beside `import_session` in `gate/src/probes.rs`, running `--dump` with `Stdio::piped()` and the parse above. Its error names the session path when no Default is found. The callers at 346, 361, 372 and 502 add `?` (`perjar_line` returns `io::Result<String>`; its caller adds `?`). The carry step lives in `run` itself, after `import_session`.

- [ ] **Step 6: Settings loses its Jars section**

Delete `settings_jars_section` (settings_page.rs:500-590), its call at 114, `SettingsRequests::jar_confirm` (54-55) and its handler (623-630), `use crate::jar::JarKey` and `JarLabel` imports, and the tests that drive the section. The Default's Clear Cookies is its card's menu item (Task 2).

- [ ] **Step 7: The Default cannot be closed, moved into a group or torn down**

`node_ops.rs` `close_node` (188), first line:

```rust
        if self.is_default_jar(id) {
            self.queue_toast("The Default jar can't be closed".to_string());
            return;
        }
```

`destroy_node` (200) returns early for the Default without a toast (it is reached only through `close_node` and group closes). `groups.rs` `destroy_group`'s member loop (the code that closes a group's contents) skips the Default and moves it to the top level instead (`self.edit_group(&id, None)`), as dissolve would. `closed.rs` `members_under` (49) leaves the Default out of a group's members (it was not closed, so a reopen must not bring a copy); as a root (Duplicate on the Default itself) it is recorded like any jar.

`session_sync.rs:1054` `edit_group`: a local move of the Default into a group is refused:

```rust
        if group.is_some() && self.is_default_jar(id) {
            return;
        }
```

`closed.rs:103` `live_fields`: drop `field::IS_DEFAULT` from what an entry records, so a Duplicate or reopen of the Default yields an ordinary jar (the test `duplicating_the_default_makes_an_ordinary_jar`).

`session_sync.rs` `materialize_imported` (the one way imported nodes go live: Recently Closed, Duplicate, session replace, Import Group): an imported jar whose `IS_DEFAULT` is true is not placed, as the running profile already has its Default, and every other imported node's `JAR` naming its fresh id is rewritten to the running Default before placing:

```rust
        let file_defaults: Vec<NodeId> = imported
            .iter()
            .filter(|i| i.kind == Kind::Jar && i.fields.get(&field::IS_DEFAULT) == Some(&FieldValue::Bool(true)))
            .map(|i| i.id)
            .collect();
        imported.retain(|i| !file_defaults.contains(&i.id));
        let running = self.default_jar();
        for i in &mut imported {
            if matches!(i.fields.get(&field::JAR), Some(FieldValue::Id(Some(j))) if file_defaults.contains(j)) {
                i.fields.insert(field::JAR, FieldValue::Id(running));
            }
        }
```

`import` itself is unchanged, so its contract that `imported[i]` is `json.nodes[i]` (relied on by `reopen_closed`, `replace_running_session` and `cli.rs`) still holds when they zip before calling `materialize_imported`.

`teardown_running_session` (2049): keep the Default card. Replace the `nodes.clear()`, `node_order.clear()`, `layout.clear()` with `retain`s keyed on `default` (`let default = self.default_jar().map(|d| d.to_string());`), and skip it in the delete and `forget_id` loops.

`card.rs`: `CardView` gains `pub(crate) closable: bool` (doc: "False for the Default jar, which has no Close."), set in `card_view` (983) as `!self.is_default_jar(id)`; `card_controls` (589) pushes `CardControl::Close` only when `view.closable`. `NodeMenuView` gains `closable: bool` the same way, and `node_menu_config`'s `TYPE_JAR` arm passes `if view.closable { delete("Close") } else { Vec::new() }`. Every test constructor of `CardView` and `NodeMenuView` sets `closable: true`.

- [ ] **Step 8: Run the tests**

Run: `cargo test -j 8 -p manifold jar`
Expected: PASS, including the six new tests.

Run: `cargo test -j 8 -p manifold`
Expected: PASS. Tests that relied on a Default without making one call `app.ensure_default_jar()` first (`main/tests.rs` `on_default` and `default_jar` helpers become thin wrappers over it).

Add the test the coordinator asked for, that a session replace re-points tabs to the surviving Default (`session_sync/tests.rs`):

```rust
#[test]
fn a_replaced_sessions_tabs_use_the_running_default() {
    use crate::store::json::{import, JsonDoc, JsonNode, Trust};
    let mut app = App::default();
    app.ensure_default_jar();
    let running = app.default_jar().unwrap();
    add(&mut app, Kind::Tab, sleeping_tab("https://old/", "Old"));
    app.teardown_running_session();
    assert_eq!(app.jar_defs().keys().copied().collect::<Vec<_>>(), [running], "only the Default survives");
    let node = |id: &str, fields: serde_json::Value| JsonNode { id: id.into(), fields: fields.as_object().unwrap().clone() };
    let doc = JsonDoc {
        nodes: vec![
            node("d", serde_json::json!({ "type": crate::store::model::TYPE_JAR, "name": "Default", "is_default": true })),
            node("w", serde_json::json!({ "type": crate::store::model::TYPE_JAR, "name": "Work" })),
            node("t", serde_json::json!({ "type": crate::store::model::TYPE_TAB, "url": "https://a/", "jar": "d" })),
            node("u", serde_json::json!({ "type": crate::store::model::TYPE_TAB, "url": "https://b/", "jar": "w" })),
        ],
        local: None,
    };
    let next = || app.alloc_node_id().parse().unwrap();
    let imported = import(&doc, next, Trust::Own).unwrap();
    let (work, t, u) = (imported[1].id, imported[2].id.to_string(), imported[3].id.to_string());
    app.materialize_imported(imported, None, false);
    assert_eq!(app.mirror.get(&t, crate::store::model::field::JAR), FieldValue::Id(Some(running)), "the file's Default is the running one");
    assert_eq!(app.mirror.get(&u, crate::store::model::field::JAR), FieldValue::Id(Some(work)), "a named jar comes with the session");
    assert_eq!(app.jar_defs().len(), 2);
}
```

(The same `next` borrow note as `an_imported_default_jar_is_the_running_one` applies.)

Run: `git grep -nE "JarKey|JarLabel::built_in|settings_jars_section|\"default\.json\"\)|join\(\"default\.json\"\)" -- crates`
Expected: no hits. The literal `default.json` file names in `jar/tests.rs` temp and aside tests and `store/atomic.rs` are arbitrary names; they do not match these patterns.

- [ ] **Step 9: fmt, clippy, commit**

Run: `cargo fmt --all` then the clippy command from Global Constraints.
Expected: clean.

```bash
git add -A crates/manifold
git commit -m "The Default jar is a card; JarKey is gone

IS_DEFAULT (field 22) marks one jar card, made at startup when none
exists and repaired to the lowest id when two replicas made one. The
Default can't be closed, moved into a group or torn down by a session
replace; an import skips it and a copy of it is an ordinary jar. Jar
files are <node id>.json. Settings' Jars section is gone.

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 4: One `jar` edge replaces `JAR_SET`, `DEFAULT_JAR` and the cascade

**Files:**
- Modify: `crates/manifold/src/caps/edges.rs` (`Scope`, `EdgeRow::scope`, `JAR_EDGE` row, tests), `crates/manifold/src/caps.rs` (re-exports)
- Modify: `crates/manifold/src/store/model.rs` (`JAR_SET`, `DEFAULT_JAR` removed, `RESERVED = [16, 17]`, `TYPES` tab and group rows, `FIELDS`, tests)
- Modify: `crates/manifold/src/links.rs:45-58` (`ConnectError::OutOfScope`), 102-165 (`check_connect`, `connect`, `disconnect`), 196-221 (`edges_of`)
- Modify: `crates/manifold/src/jar.rs:14` (`JarChoice` deleted)
- Modify: `crates/manifold/src/jar_defs.rs` (`resolve_jar`, `effective_jar`, `set_jar_edge`, `recheck_edge_scope`; deletions listed below), `JarChip`
- Modify: `crates/manifold/src/mirror.rs:270-295` (`default_jar`, `jar_choice` deleted; `jar` added), `Carried` (315-336), `seed` (338-360)
- Modify: `crates/manifold/src/session_sync.rs:274-299` (`spawn_snapshot`, `tab_fields`), 1010-1031 (`edit_jar`, `edit_default_jar` deleted)
- Modify: `crates/manifold/src/tabs.rs:183, 245, 339-356, 445, 486-508, 752-790, 885`, `crates/manifold/src/tabs/spawn_result.rs:280, 338-356`
- Modify: `crates/manifold/src/node_menu.rs` (`NodeAction` jar variants, `JarRow`, `JarMenu`, `jar_items`, `jar_row`, `jar_menu` 931-977, `apply_node_action` 1084-1102, `edge_menu` "what" labels 913-917)
- Modify: `crates/manifold/src/graph_nav.rs:147, 192, 204, 245`, `crates/manifold/src/groups.rs:34` (`refollow` callers)
- Modify: `crates/manifold/src/level.rs:38, 125` (`EdgeKind::Jar`), `crates/manifold/src/graph_ui.rs:1595` (`draw_graph_edges`), `crates/manifold/src/debug_control.rs:1577` (edge export)
- Modify: `crates/manifold/src/store/json.rs:200` (Foreign filter `JAR|DEFAULT_JAR|FROM|TO` loses `DEFAULT_JAR`) and its tests that write `DEFAULT_JAR`
- Modify: `crates/gate/src/jar.rs:513` (a seed's `n["jar_set"] = true` is deleted: import ignores the unknown name, so nothing fails, but it names a retired field)
- Test (every file that names `JarChoice`, `jar_choice`, `JAR_SET` or `DEFAULT_JAR`, from `git grep -lE "JarChoice|jar_choice|JAR_SET|DEFAULT_JAR" -- crates`): `crates/manifold/src/jar_defs/tests.rs`, `crates/manifold/src/main/tests.rs`, `crates/manifold/src/node_menu/tests.rs`, `crates/manifold/src/session_sync/tests.rs`, `crates/manifold/src/store/json.rs` tests, `crates/manifold/src/store/model.rs` tests, `crates/manifold/src/mirror.rs` tests (444), `crates/manifold/src/caps/edges.rs` tests, `crates/manifold/src/level.rs` tests

**Interfaces:**
- Consumes: Task 3 (`default_jar`, `is_default_jar`, `jar_exists`, `NodeId` jar keys), Task 1 (`create_jar(name, at)`).
- Produces:
  - `caps::edges::Scope { SameLevel, SameLevelOrAncestor }`; `EdgeRow::reach(self, scope: Scope) -> EdgeRow` (builder); `EdgeRow::scope(&self) -> Scope`.
  - `caps::JAR_EDGE: &str = "jar"`, re-exported from `caps` beside `LINK`.
  - `ConnectError::OutOfScope`.
  - `App::levels_from<'a>(&'a self, level: Option<&'a str>) -> impl Iterator<Item = Option<&'a str>> + 'a` (no allocation).
  - `App::reach_origin<'a>(&'a self, id: &'a str) -> Option<&'a str>`: the level an edge from `id` reaches out from (a group's own contents; anything else's group).
  - `App::in_scope(&self, scope: Scope, from: &str, to: &str) -> bool`.
  - `Mirror::jar(&self, id: &str) -> Option<NodeId>` (field `JAR`).
  - `App::resolve_jar(&self, own: Option<NodeId>, origin: Option<&str>) -> Option<(NodeId, bool)>`.
  - `App::effective_jar(&self, id: &str) -> Option<(NodeId, bool)>`.
  - `App::set_jar_edge(&mut self, id: &str, jar: Option<NodeId>)`.
  - `App::recheck_edge_scope(&mut self, moved: &str)`.
  - `Carried::jar: Option<NodeId>` (no `default_jar`); `TabSpawn::jar: Option<NodeId>` (the tab's own edge; `None` inherits).
  - `level::EdgeKind::Jar`; EXPORTSTATE edge kind `"jar"`.
  - Deleted: `JarChoice`, `Mirror::jar_choice`, `Mirror::default_jar`, `App::{resolved_jar, inherited_jar, new_tab_jar, tab_jar, set_tab_jar, follow_group_jar, resnapshot, refollow, linked_group, is_inside, governed_by, cascade_targets, cascade_count, set_group_default_jar, clear_group_default_jar, edit_jar, edit_default_jar}`, `NodeAction::{FollowGroupJar, SetGroupDefaultJar, ClearGroupDefaultJar}`, `JarRow::count`, `JarMenu::{can_follow, has_default}`, fields `JAR_SET`, `DEFAULT_JAR`.

- [ ] **Step 1: Write the failing tests**

In `crates/manifold/src/caps/edges.rs` tests:

```rust
    #[test]
    fn the_jar_edge_joins_a_tab_or_group_to_a_jar_in_reach() {
        use crate::store::model::{TYPE_GROUP, TYPE_JAR, TYPE_TAB};
        let t = edges();
        assert!(t.can_connect(JAR_EDGE, TYPE_TAB, TYPE_JAR) && t.can_connect(JAR_EDGE, TYPE_GROUP, TYPE_JAR));
        assert!(!t.can_connect(JAR_EDGE, "text/markdown", TYPE_JAR) && !t.can_connect(JAR_EDGE, TYPE_TAB, TYPE_TAB));
        let row = t.row(JAR_EDGE).unwrap();
        assert_eq!(row.storage(), Storage::Field(field::JAR));
        assert_eq!(row.scope(), Scope::SameLevelOrAncestor);
        assert_eq!(row.verb(), Some("Use jar\u{2026}"));
        assert!(t.rows.iter().filter(|r| r.id() != JAR_EDGE).all(|r| r.scope() == Scope::SameLevel));
    }
```

and in `the_columns_say_what_each_edge_does` the `user_creatable` order becomes `[OPENED_FROM, LINK, JAR_EDGE]`. `every_source_of_a_field_stored_edge_carries_the_field` stays as is: it now checks that tabs and groups carry `JAR`.

In `crates/manifold/src/jar_defs/tests.rs`, replacing every test of the cascade, follow and group default (`a_new_tab_snapshots_the_nearest_group_default`, `a_cascade_reaches_unset_tabs_in_nested_groups_without_their_own_default` and the others at 61, 81, 235 that call deleted functions):

```rust
fn with_default() -> (App, NodeId) {
    let mut app = App::default();
    app.ensure_default_jar();
    let default = app.default_jar().unwrap();
    (app, default)
}
fn jar_at(app: &mut App, name: &str, group: Option<&str>) -> NodeId {
    let at = crate::fetched::Placement { group: group.map(str::to_owned), pos: egui::Vec2::ZERO };
    app.create_jar(name, at)
}

#[test]
fn a_tab_resolves_its_own_edge_then_each_enclosing_groups_then_the_default() {
    let (mut app, default) = with_default();
    let outer = group_in(&mut app, None);
    let inner = group_in(&mut app, Some(&outer));
    let tab = tab_in(&mut app, Some(&inner));
    assert_eq!(app.effective_jar(&tab), Some((default, false)), "no edge anywhere");
    let work = jar_at(&mut app, "Work", None);
    app.set_jar_edge(&outer, Some(work));
    assert_eq!(app.effective_jar(&tab), Some((work, false)), "the grandparent's edge");
    let home = jar_at(&mut app, "Home", Some(&outer));
    app.set_jar_edge(&inner, Some(home));
    assert_eq!(app.effective_jar(&tab), Some((home, false)), "the nearer group's edge");
    let mine = jar_at(&mut app, "Mine", Some(&inner));
    app.set_jar_edge(&tab, Some(mine));
    assert_eq!(app.effective_jar(&tab), Some((mine, false)), "its own edge");
    app.set_jar_edge(&tab, None);
    assert_eq!(app.effective_jar(&tab), Some((home, false)), "cleared: inherits again");
}

#[test]
fn an_edge_to_a_closed_jar_dangles_to_the_default_and_does_not_climb() {
    let (mut app, default) = with_default();
    let group = group_in(&mut app, None);
    let tab = tab_in(&mut app, Some(&group));
    let work = jar_at(&mut app, "Work", None);
    let mine = jar_at(&mut app, "Mine", Some(&group));
    app.set_jar_edge(&group, Some(work));
    app.set_jar_edge(&tab, Some(mine));
    app.close_node(&mine.to_string());
    assert_eq!(app.effective_jar(&tab), Some((default, true)), "dangling, not the group's Work");
}

#[test]
fn an_out_of_scope_edge_from_another_device_is_kept_and_dangles() {
    let (mut app, default) = with_default();
    let (a, b) = (group_in(&mut app, None), group_in(&mut app, None));
    let tab = tab_in(&mut app, Some(&a));
    let far = jar_at(&mut app, "Far", Some(&b));
    // What a remote write looks like: the field set directly, with no scope check.
    app.mirror.set(&tab, crate::store::model::field::JAR, FieldValue::Id(Some(far)));
    assert_eq!(app.effective_jar(&tab), Some((default, true)));
    assert_eq!(app.mirror.jar(&tab), Some(far), "never rewritten");
}

#[test]
fn connect_reaches_a_jar_at_the_level_or_above_but_not_in_a_sibling_group() {
    let (mut app, _) = with_default();
    let (a, b) = (group_in(&mut app, None), group_in(&mut app, None));
    let tab = tab_in(&mut app, Some(&a));
    let here = jar_at(&mut app, "Here", Some(&a)).to_string();
    let above = jar_at(&mut app, "Above", None).to_string();
    let sibling = jar_at(&mut app, "Sibling", Some(&b)).to_string();
    let edge = crate::caps::JAR_EDGE;
    assert_eq!(app.connect(edge, &tab, &here), Ok(()));
    assert_eq!(app.connect(edge, &tab, &here), Err(crate::links::ConnectError::Exists));
    assert_eq!(app.connect(edge, &tab, &above), Ok(()), "replaces the edge: a field holds one");
    assert_eq!(app.mirror.jar(&tab), above.parse().ok());
    assert_eq!(app.connect(edge, &tab, &sibling), Err(crate::links::ConnectError::OutOfScope));
    app.disconnect(edge, &tab, &above);
    assert_eq!(app.mirror.jar(&tab), None);
}

#[test]
fn a_groups_edge_reaches_a_jar_inside_it_and_its_tabs_use_it() {
    let (mut app, _) = with_default();
    let (a, b) = (group_in(&mut app, None), group_in(&mut app, None));
    let inner = group_in(&mut app, Some(&a));
    let tab = tab_in(&mut app, Some(&inner));
    let inside = jar_at(&mut app, "Inside", Some(&a)).to_string();
    let deeper = jar_at(&mut app, "Deeper", Some(&inner)).to_string();
    let elsewhere = jar_at(&mut app, "Elsewhere", Some(&b)).to_string();
    let edge = crate::caps::JAR_EDGE;
    // Decision 2026-10-14 (pending the user's confirmation): a group's own contents count as its level.
    assert_eq!(app.connect(edge, &a, &inside), Ok(()), "a jar inside the group");
    assert_eq!(app.effective_jar(&tab), Some((inside.parse().unwrap(), false)), "inherited through `inner`");
    assert_eq!(app.connect(edge, &a, &deeper), Err(crate::links::ConnectError::OutOfScope), "not a nested group's");
    assert_eq!(app.connect(edge, &a, &elsewhere), Err(crate::links::ConnectError::OutOfScope), "not a sibling's");
    // A tab's edge still reaches its own level and above only.
    let top_tab = tab_in(&mut app, Some(&a));
    assert_eq!(app.connect(edge, &top_tab, &deeper), Err(crate::links::ConnectError::OutOfScope));
}

#[test]
fn a_move_clears_an_edge_it_takes_out_of_reach_and_keeps_one_in_reach() {
    let (mut app, default) = with_default();
    let (a, b) = (group_in(&mut app, None), group_in(&mut app, None));
    let kept = tab_in(&mut app, Some(&a));
    let lost = tab_in(&mut app, Some(&a));
    let plain = tab_in(&mut app, Some(&a));
    let top = jar_at(&mut app, "Top", None);
    let local = jar_at(&mut app, "Local", Some(&a));
    // Inside `b`: a group's edge reaches its own contents (decision 2026-10-14).
    let b_jar = jar_at(&mut app, "B", Some(&b));
    app.set_jar_edge(&kept, Some(top));
    app.set_jar_edge(&lost, Some(local));
    app.set_jar_edge(&b, Some(b_jar));
    for tab in [&kept, &lost, &plain] {
        app.edit_group(tab, Some(b.clone()));
        app.recheck_edge_scope(tab);
    }
    assert_eq!(app.mirror.jar(&kept), Some(top), "an ancestor jar is still in reach");
    assert_eq!(app.mirror.jar(&lost), None, "cleared");
    assert_eq!(app.effective_jar(&plain), Some((b_jar, false)), "no edge: takes its new group's jar");
    // Moving a jar out of reach clears the edges to it.
    app.edit_group(&local.to_string(), Some(b.clone()));
    app.set_jar_edge(&kept, Some(local));
    app.edit_group(&local.to_string(), Some(a.clone()));
    app.recheck_edge_scope(&local.to_string());
    assert_eq!(app.mirror.jar(&kept), None);
    assert!(app.toasts.save_toast_queue.iter().any(|t| t.contains("Local")), "a toast names it");
    let _ = default;
}
```

`tab_in`, `group_in` are the helpers at the top of this test file. Match the toast queue's real field (grep `fn queue_toast`).

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -j 8 -p manifold the_jar_edge_joins`
Expected: compile error: cannot find `JAR_EDGE`, `Scope`.

- [ ] **Step 3: The edge table's scope column and the jar row**

`caps/edges.rs`:

```rust
/// The edge from a tab or group to the cookie jar it uses (spec "Store").
pub(crate) const JAR_EDGE: &str = "jar";

/// Where an edge's target may sit, relative to its source's level (spec "Scope").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    /// In the source's group (both at the top level counts).
    SameLevel,
    /// In the level the source reaches out from or any group enclosing it. A group source reaches out from its own
    /// contents (so inside it, beside it, or above); any other source from its group (`App::reach_origin`).
    SameLevelOrAncestor,
}
```

`EdgeRow` gains `scope: Scope` (`Scope::SameLevel` in `new`), the builder

```rust
    /// The edge reaches targets under `scope` rather than at its source's level only.
    pub(crate) fn reach(mut self, scope: Scope) -> EdgeRow {
        self.scope = scope;
        self
    }
```

and the getter `pub(crate) fn scope(&self) -> Scope { self.scope }`. In `builtin_edges`, after `LINK`:

```rust
        EdgeRow::new(JAR_EDGE, &[TYPE_TAB, TYPE_GROUP], &[TYPE_JAR])
            .stored_in(Storage::Field(field::JAR))
            .reach(Scope::SameLevelOrAncestor)
            .user_creatable("Use jar\u{2026}"),
```

The module doc's "Cardinality and removal variants arrive with the code that reads them" stays: a `Field` edge holds one target by construction, so the spec's `at_most_one` column would be a column nothing reads, and is not added (flagged to the user).

`caps.rs`: re-export `JAR_EDGE` and `Scope` beside `Storage, CONTAINS, LINK, OPENED_FROM`.

- [ ] **Step 4: Schema**

`store/model.rs`: delete `JAR_SET` (16) and `DEFAULT_JAR` (17) from `mod field` and `FIELDS` (`[FieldDef; 17]`), set `RESERVED: [u32; 2] = [16, 17]` with the doc "Retired: a tab's `jar_set` and a group's `default_jar`, replaced by the `jar` edge (jars-as-cards spec). Never reused; old documents keep them raw." The tab row loses `JAR_SET`; the group row becomes `&[field::POS, field::COLOR, field::NAME, field::JAR]`. Update the model tests.

`store/json.rs:200`: the Foreign filter becomes `JAR | FROM | TO`.

- [ ] **Step 5: Generic field edges in `check_connect`, `connect`, `disconnect`**

`links.rs`: add to `ConnectError`:

```rust
    /// The target is outside the edge's reach (`Scope`): for `jar`, not at the source's level or above it.
    OutOfScope,
```

Replace the level check (117-119) with:

```rust
        if !self.in_scope(row.scope(), from, to) {
            return Err(match row.scope() {
                Scope::SameLevel => ConnectError::DifferentLevel,
                Scope::SameLevelOrAncestor => ConnectError::OutOfScope,
            });
        }
```

and the `exists` match (123-130) with:

```rust
        let exists = match row.storage() {
            // `parent` lives in the mirror's links, not its field map.
            Storage::Field(field::PARENT) => self.mirror.parent(from) == Some(to),
            Storage::Field(no) => to.parse::<NodeId>().is_ok_and(|to| self.mirror.get(from, no) == FieldValue::Id(Some(to))),
            Storage::Node(_) => match linked {
                Some(linked) => to.parse::<NodeId>().is_ok_and(|to| linked.contains(&to)),
                None => self.link_between(from, to).is_some(),
            },
            Storage::Tree => return Err(ConnectError::NotAllowed),
        };
```

`connect` (138):

```rust
        match crate::caps::edges().row(edge).map(|r| r.storage()) {
            Some(Storage::Field(field::PARENT)) => self.edit_parent(from, Some(to.to_string())),
            Some(Storage::Field(no)) => self.edit_field(from, no, FieldValue::Id(to.parse().ok())),
            Some(Storage::Node(_)) => {
                self.create_link(from, to);
            }
            _ => return Err(ConnectError::NotAllowed),
        }
```

`disconnect` (151):

```rust
            Some(Storage::Field(no)) => {
                if to.parse::<NodeId>().is_ok_and(|to| self.mirror.get(from, no) == FieldValue::Id(Some(to))) {
                    self.edit_field(from, no, FieldValue::Id(None));
                }
            }
```

(after the `PARENT` arm). Add to `links.rs`:

```rust
    /// `level` and every group enclosing it, out to the top level (`None`), nearest first. Borrowed and
    /// allocation-free: `effective_jar` runs it per card per frame. Bounded so a cycle in bad data ends.
    pub(crate) fn levels_from<'a>(&'a self, level: Option<&'a str>) -> impl Iterator<Item = Option<&'a str>> + 'a {
        std::iter::successors(Some(level), move |at| at.map(|g| self.mirror.group(g))).take(self.nodes.len() + 2)
    }

    /// The level an edge from `id` reaches out from (spec "Scope", decision 2026-10-14): a group's own contents,
    /// so its edge may name a card inside it; any other card's group.
    pub(crate) fn reach_origin<'a>(&'a self, id: &'a str) -> Option<&'a str> {
        if self.is_container(id) {
            Some(id)
        } else {
            self.mirror.group(id)
        }
    }

    /// Whether `to` is in reach of `from` under `scope`. `SameLevel`: both in one group. `SameLevelOrAncestor`:
    /// `to` sits at `from`'s `reach_origin` or in a group enclosing it.
    pub(crate) fn in_scope(&self, scope: Scope, from: &str, to: &str) -> bool {
        let at = self.mirror.group(to);
        match scope {
            Scope::SameLevel => self.mirror.group(from) == at,
            Scope::SameLevelOrAncestor => self.levels_from(self.reach_origin(from)).any(|level| level == at),
        }
    }

    /// Whether `node` is `root` or sits anywhere inside it.
    pub(crate) fn within(&self, node: &str, root: &str) -> bool {
        node == root || self.levels_from(self.mirror.group(node)).any(|level| level == Some(root))
    }
```

`is_container` (card_kind.rs:54) reads the mirror's type, so it holds for a group mid-anything and is false for an unknown id.

`FieldValue` must derive `PartialEq` (it does if `mirror.rs` compares field values; otherwise compare with `matches!(self.mirror.get(from, no), FieldValue::Id(Some(t)) if t == to)`).

`edges_of` (203): after the opened-from rows and before the links, list every `Field` edge other than `PARENT` both ways, so the node menu's Links section shows a tab's jar and a jar's users:

```rust
        let field_rows = crate::caps::edges()
            .user_creatable()
            .filter_map(|r| match r.storage() {
                Storage::Field(no) if no != field::PARENT => Some((r.id(), no)),
                _ => None,
            })
            .collect::<Vec<_>>();
        for (edge, no) in field_rows {
            if let FieldValue::Id(Some(to)) = self.mirror.get(node, no) {
                let other = to.to_string();
                if self.shown_with(node, &other) {
                    edges.push(EdgeRef { edge, other, link: None, outgoing: true });
                }
            }
            let me = node.parse::<NodeId>().ok();
            for other in self.nodes.keys().filter(|o| self.shown_with(node, o)) {
                if me.is_some() && self.mirror.get(other, no) == FieldValue::Id(me) {
                    edges.push(EdgeRef { edge, other: other.clone(), link: None, outgoing: false });
                }
            }
        }
```

`EdgeTable::user_creatable` iterates the private `rows`; if `rows()` is needed outside the module, add `pub(crate) fn rows(&self) -> &[EdgeRow]`. `node_menu.rs` `edge_menu` (913-917) gains labels for `JAR_EDGE`: outgoing "Uses jar", incoming "Used by".

- [ ] **Step 6: Resolution, `set_jar_edge`, `recheck_edge_scope`**

`mirror.rs`: delete `default_jar` (270-277) and `jar_choice` (286-293); add

```rust
    /// A tab's or group's `jar` edge (`field::JAR`): the jar it names, `None` to inherit. It may name a jar that no
    /// longer exists or sits out of reach (`App::resolve_jar`).
    pub(crate) fn jar(&self, id: &str) -> Option<NodeId> {
        match self.get(id, field::JAR) {
            FieldValue::Id(jar) => jar,
            _ => None,
        }
    }
```

`Carried::jar: Option<NodeId>`, `Carried::default_jar` deleted; `carried` sets `jar: self.jar(id)`; `seed` writes `jar.map(|j| (field::JAR, FieldValue::Id(Some(j))))` and drops the `jar_set` and `default_jar` entries.

`jar.rs:14`: delete `JarChoice`.

`jar_defs.rs`: import `use crate::caps::{Scope, Storage, JAR_EDGE};` (and `links.rs` imports `Scope` beside `Storage`), delete every function in the Interfaces "Deleted" list, then add:

```rust
    /// The jar a source resolves to (spec "Resolution") and whether it dangles. `own` is the source's own edge and
    /// `origin` the level that edge reaches out from (`reach_origin`). The walk stops at the first edge it meets,
    /// from `own` outward through each enclosing group's edge (a group's edge reaching out from its own contents).
    /// A live jar in reach is the jar. A jar gone, or out of reach (an edge that arrived so from another device), is
    /// the Default with `true`: an explicit edge to nothing warns rather than climbing on. No edge anywhere: the
    /// Default. `None` only when there is no Default.
    ///
    /// Per card per frame (`jar_chip`, `will_switch`, `sweep_jar_switches`): one mirror lookup per enclosing group,
    /// and at the edge it meets one `to_string` and a reach walk of the same depth; `default_jar` is an index read.
    /// No other allocation.
    pub(crate) fn resolve_jar(&self, own: Option<NodeId>, origin: Option<&str>) -> Option<(NodeId, bool)> {
        let (mut edge, mut origin, mut next) = (own, origin, origin);
        for _ in 0..=self.nodes.len() {
            if let Some(jar) = edge {
                let at = self.mirror.group(&jar.to_string());
                let in_reach = self.levels_from(origin).any(|level| level == at);
                return match self.jar_exists(jar) && in_reach {
                    true => Some((jar, false)),
                    false => self.default_jar().map(|d| (d, true)),
                };
            }
            let Some(g) = next.filter(|g| self.nodes.contains_key(*g)) else { break };
            edge = self.mirror.jar(g);
            origin = Some(g);
            next = self.mirror.group(g);
        }
        self.default_jar().map(|d| (d, false))
    }

    /// `resolve_jar` for a tab or group the mirror holds (live, asleep or mid-respawn). For a group with no edge of
    /// its own, `next` starts at the group itself, whose (empty) edge is read once more before the walk climbs.
    pub(crate) fn effective_jar(&self, id: &str) -> Option<(NodeId, bool)> {
        self.resolve_jar(self.mirror.jar(id), self.reach_origin(id))
    }

    /// The menu's and SETJAR's pick for tab or group `id`: draw its `jar` edge to `jar`, or clear it (`None`,
    /// "Inherit"). A refused edge (out of reach, not a jar) changes nothing. Live tabs whose jar changed switch.
    pub(crate) fn set_jar_edge(&mut self, id: &str, jar: Option<NodeId>) {
        match jar {
            Some(jar) => {
                let _ = self.connect(JAR_EDGE, id, &jar.to_string());
            }
            None => {
                if let Some(old) = self.mirror.jar(id) {
                    self.disconnect(JAR_EDGE, id, &old.to_string());
                }
            }
        }
        self.sweep_jar_switches();
    }

    /// After a local move of `moved` (a card, or a group with its contents): every edge whose row reaches beyond
    /// one level (`Scope::SameLevelOrAncestor`) with an end in `moved`'s subtree is checked again, and one whose live
    /// target is now out of reach is cleared, with a toast naming it (spec "Scope"). A dangling edge (target gone)
    /// is left: it warns, and is never rewritten. Then live tabs whose jar changed switch.
    pub(crate) fn recheck_edge_scope(&mut self, moved: &str) {
        let rows: Vec<(&'static str, u32)> = crate::caps::edges()
            .user_creatable()
            .filter(|r| r.scope() == Scope::SameLevelOrAncestor)
            .filter_map(|r| match r.storage() {
                Storage::Field(no) => Some((r.id(), no)),
                _ => None,
            })
            .collect();
        let mut cleared = Vec::new();
        for (edge, no) in rows {
            for source in self.linked_ids().map(str::to_owned).collect::<Vec<_>>() {
                let FieldValue::Id(Some(target)) = self.mirror.get(&source, no) else { continue };
                let to = target.to_string();
                let touched = self.within(&source, moved) || self.within(&to, moved);
                let scope = crate::caps::edges().row(edge).expect("a listed row").scope();
                if touched && self.nodes.contains_key(&to) && !self.in_scope(scope, &source, &to) {
                    self.edit_field(&source, no, FieldValue::Id(None));
                    cleared.push(to);
                }
            }
        }
        for to in cleared {
            let name = self.jar_label(to.parse().expect("a store id"));
            self.queue_toast(format!("{name} is out of reach here; the edge to it was removed"));
        }
        self.sweep_jar_switches();
    }
```

`recheck_edge_scope` names its toast with `jar_label` because the jar row is the only row with this scope; if a later row gets it, the toast takes `card_label`-style naming. `linked_ids` (live, asleep and mid-respawn) is the existing iterator `refollow` used; it includes groups (check: if it lists tabs only, chain `self.nodes.keys()` filtered to containers).

Every `refollow(id)` call becomes `recheck_edge_scope(id)`: `graph_nav.rs:147` (eject), 192 (Join), 204 (GroupWith), 245 (`merge_groups`), and `groups.rs:34` (`dissolve_group`). A move of a jar card goes through the same call sites (they are card-generic), so moving a jar clears edges to it that fall out of reach. Remote moves are not rechecked (spec: kept, dangling).

Spec deviation, flagged: the spec says this runs "for every row" and cites an existing path that drops cross-level `link` edges on a move. No such path exists today (the read rule hides cross-level links and opened-from edges, and they reappear when the cards share a level again). Running it for `SameLevel` rows would newly delete links and opened-from edges on every move, so the recheck covers rows whose scope reaches beyond one level, which today is the jar row alone.

- [ ] **Step 7: Tabs, spawns and switches read `effective_jar`**

- `tabs.rs:183` `TabSpawn::jar: Option<NodeId>` (doc: "The new tab's own `jar` edge; `None` inherits from its group."), default `None`.
- `spawn_tab_async` (339-356): `let own = if has { self.mirror.jar(&spawn.id) } else { spawn.jar };` then `spawn_snapshot(.., own)`, then `let Some((jar, _)) = self.resolve_jar(own, spawn.group.as_deref()) else { .. }` before `ensure_jar_loaded(jar)`; with `None`, skip the load and seed nothing.
- `session_sync.rs:274-299` `spawn_snapshot`/`tab_fields` take `jar: Option<NodeId>` and write `JAR` only (no `JAR_SET`).
- `tabs.rs:245` (`respawn`): `jar: carried.jar`.
- `tabs.rs:445` (`duplicate_tab`): the copy takes the original's own edge, `jar: self.mirror.jar(id)`.
- `tabs.rs:486-508` `spawn_new_tab_with_kind(.., jar: Option<NodeId>)`: passes `jar` through (`None` inherits); delete the `new_tab_jar` line. `tabs.rs:885` (`spawn_tab_at`): `jar: None`.
- `will_switch` (752) and `sweep_jar_switches` (762): `self.effective_jar(id).map(|(j, _)| j)` in place of `self.resolved_jar(&self.mirror.jar_choice(id)).0`; a tab with `None` never switches.
- `tabs/spawn_result.rs:338-356`: delete the deleted-jar fix-up in `seed_landing` (a landing on a gone jar now dangles by the read rule); line 280 `Carried { jar: .. }` takes the spawn's `Option<NodeId>`.
- `jar_chip`: `let (jar, missing) = self.effective_jar(id)?;`, `following = self.mirror.jar(id).is_none()`, hidden (`None`) when `jar` is the Default, not `missing` and not `will_switch`. Its tooltip line for `missing` reads "This tab's jar is gone or out of reach; it uses the Default jar". `JarChip::following`'s doc: "No edge of its own: it inherits from its group, or the Default."

- [ ] **Step 8: The Jar page**

`node_menu.rs`:
- `JarRow { id: Option<NodeId>, name: JarLabel, color: egui::Color32, ticked: bool }` (`count` gone; `id: None` is the "Inherit" row).
- `JarMenu { rows: Vec<JarRow>, will_switch: bool }`.
- `NodeMenuItem::Jars { heading, rows }` (`group` gone).
- `NodeAction`: delete `FollowGroupJar`, `SetGroupDefaultJar(NodeId)`, `ClearGroupDefaultJar`. `SetJar(Option<NodeId>)` stays and means "draw the edge, or Inherit". `StartNewJar`, `NewJar(String)`, `CancelNewJar`, `SwitchJarNow` stay.
- `jar_items` (566):

```rust
    let jar_items = |jars: &JarMenu, live: bool| -> Vec<NodeMenuItem> {
        let mut out = vec![NodeMenuItem::Jars { heading: "Jar", rows: jars.rows.clone() }, Button("New Jar...", A::StartNewJar)];
        if live && jars.will_switch {
            out.push(Button("Switch Jar Now", A::SwitchJarNow));
        }
        out
    };
    let jar_row = |items: &mut Vec<NodeMenuItem>, live: bool| -> Vec<NodeMenuItem> {
        let Some(jars) = &view.jars else { return Vec::new() };
        items.push(Page("Jar...", MenuPage::Jar));
        jar_items(jars, live)
    };
```

  The tab arms call `jar_row(&mut items, true)` / `jar_row(&mut items, false)`, the group arm `jar_row(&mut items, false)`; the group's page row now reads "Jar..." too (a group's edge is inherited by its tabs).
- `node_menu_view` builds `jars` for a tab or group:

```rust
    fn jar_menu(&self, id: &str) -> Option<JarMenu> {
        let own = self.mirror.jar(id);
        let inherit = JarRow { id: None, name: JarLabel { name: "Inherit".into(), marker: None }, color: egui::Color32::GRAY, ticked: own.is_none() };
        let targets = self.valid_targets(JAR_EDGE, id).into_iter().filter_map(|t| t.parse::<NodeId>().ok());
        let rows = std::iter::once(inherit)
            .chain(targets.chain(own).collect::<std::collections::BTreeSet<_>>().into_iter().map(|jar| JarRow {
                id: Some(jar),
                name: self.jar_label(jar),
                color: self.jar_color(jar),
                ticked: own == Some(jar),
            }))
            .collect();
        Some(JarMenu { rows, will_switch: self.will_switch(id) })
    }
```

  `valid_targets` refuses the jar the edge already names (`Exists`), so `own` is chained back in to show its tick. Replace the body of `jar_menu` (931-977) with this, keeping its name and call site.
- `apply_node_action` (1084-1102): `SetJar(jar) => self.set_jar_edge(id, jar)`; `NewJar(name)` creates the jar at the source's level and draws the edge:

```rust
            NodeAction::NewJar(name) => {
                let level = self.mirror.group(id).map(str::to_owned);
                let pos = self.layout_pos(id) + egui::vec2(0.0, -120.0);
                let jar = self.create_jar(&name, crate::fetched::Placement { group: level, pos });
                self.set_jar_edge(id, Some(jar));
                self.naming_jar = None;
            }
```

  (keep whatever the existing arm does with `naming_jar` and the empty-name case).
- `set_focused_tab_jar_named` (SETJAR): the focused tab's edge to the named jar, made at the tab's level when no jar has the name; `"default"` draws the edge to `self.default_jar()`; then `set_jar_edge`.
- Delete the node menu tests of the cascade, follow and group default (`the_group_menu_counts_the_cascade_and_a_set_tab_can_follow` at 802 and those around it that call deleted actions). Update the group menu label tests at 205-216: "Default jar..." becomes "Jar...". Add:

```rust
#[test]
fn the_jar_page_offers_inherit_and_the_jars_in_reach() {
    let mut app = App::default();
    app.ensure_default_jar();
    let group = crate::session_sync::tests::add_group(&mut app, "G");
    let tab = crate::session_sync::tests::add(&mut app, Kind::Tab, crate::session_sync::tests::sleeping_tab("https://a/", "A"));
    app.edit_group(&tab, Some(group.clone()));
    let inside = app.create_jar("Inside", crate::fetched::Placement { group: Some(group), pos: egui::Vec2::ZERO });
    let menu = app.jar_menu(&tab).unwrap();
    let names: Vec<String> = menu.rows.iter().map(|r| r.name.name.clone()).collect();
    assert_eq!(names, ["Inherit", "Default", "Inside"]);
    assert!(menu.rows[0].ticked);
    app.apply_node_action(&egui::Context::default(), &tab, NodeAction::SetJar(Some(inside)));
    assert!(app.jar_menu(&tab).unwrap().rows.iter().any(|r| r.id == Some(inside) && r.ticked));
}
```

  (the Default sits at the top level, an ancestor of the group, so it is in reach; rows after Inherit come in id order, and the Default was made first).

- [ ] **Step 9: The jar edge on the canvas**

`level.rs:38`, the enum gains a third variant:

```rust
/// Which edge source a `LevelEdge` came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EdgeKind {
    /// A `parent` link (or a JSON level's row-to-card link): drawn solid.
    Opened,
    /// A Link node: drawn dashed, and never a layout spring.
    Link,
    /// A tab's or group's `jar` edge (`field::JAR`) to a jar card on the same level: drawn dashed and faint, never
    /// clicked (`link` is `None`) and never a spring.
    Jar,
}
```

`level_edges` (125-188) builds `opened` and `links` as iterators over `LevelEdge { from: Pos2, to: Pos2, from_id, to_id, curve, highlight, kind, link }` and returns `opened.chain(links).collect()`. Add a third iterator in the same shape, after `links`:

```rust
        // A jar edge shows while its source and its jar are both live cards at this level. One reaching a jar in an
        // enclosing group, or a group's edge to a jar inside it, has no line here: its far end is on another level.
        let jars = self.nodes.keys().filter(|id| self.in_level(id)).filter_map(|id| {
            let jar = self.mirror.jar(id)?.to_string();
            self.nodes.get(&jar).filter(|_| self.in_level(&jar))?;
            let (from, to) = crate::link_gestures::facing_ends(rect_of(id), rect_of(&jar));
            Some(LevelEdge {
                from,
                to,
                from_id: id.clone(),
                to_id: jar,
                curve: EdgeCurve::Bow,
                highlight: false,
                kind: EdgeKind::Jar,
                link: None,
            })
        });
        opened.chain(links).chain(jars).collect()
```

The other `EdgeKind` match sites (`git grep -n "EdgeKind::" -- crates` lists them):
- `graph_ui.rs:1614-1627`, the stroke match, gains `EdgeKind::Jar => egui::Stroke::new(1.5 * inv_zoom, ui.visuals().weak_text_color()),`.
- `graph_ui.rs:1630`: `let dash = matches!(edge.kind, EdgeKind::Link | EdgeKind::Jar).then_some(inv_zoom);`.
- `debug_control.rs:1578-1579` gains `crate::level::EdgeKind::Jar => "jar",`.
- `link_gestures/tests.rs:31` builds kinds from `link` and needs nothing.

Line hits and retargets act on `edge.link` (a Link node id), which is `None` here, so a jar line is never clicked. Check `link_gestures.rs` for a `kind` test that would treat `Jar` as `Opened`, and keep `Jar` out of whatever `Opened` alone gets. `graph_nav.rs:537` spring edges: no change, because they read `affects_layout`, which the jar row lacks.

Add to `level.rs` tests:

```rust
    #[test]
    fn a_jar_edge_draws_only_when_both_ends_are_on_this_level() {
        let mut app = App::default();
        let tab = crate::session_sync::tests::add(&mut app, Kind::Tab, crate::session_sync::tests::sleeping_tab("https://a/", "A"));
        let here = crate::session_sync::tests::add_jar(&mut app, "Here");
        app.set_jar_edge(&tab, Some(here));
        let jar_lines = |app: &App| app.level_edges().into_iter().filter(|e| e.kind == EdgeKind::Jar).count();
        assert_eq!(jar_lines(&app), 1);
        app.edit_group(&here.to_string(), Some("elsewhere".into()));
        assert_eq!(jar_lines(&app), 0, "the jar is on another level");
    }
```

(`edit_group` to a group id that is not a node puts the jar on a level not shown, as the neighbouring `level_edges_join_parent_and_child_facing_sides_on_this_level_only` does with `"other"`.)

- [ ] **Step 10: Run the tests**

Run: `cargo test -j 8 -p manifold jar`
Expected: PASS, including the six new resolution, connect and move tests and `the_jar_page_offers_inherit_and_the_jars_in_reach`.

Run: `cargo test -j 8 -p manifold`
Expected: PASS. Old tests that asserted the cascade, `JAR_SET`, "Follow Group's Jar" or "Default jar for new tabs" are deleted (list them in the commit body). Task 3's `the_default_jar_cannot_be_closed_and_an_unset_tab_resolves_to_it` and the `jar_of` helper move to the new API: `assert_eq!(app.effective_jar(&tab), Some((default, false)))`, and `jar_of` is deleted.

Add to `closed.rs` tests, for the remap through `import` (`subtree_entry` records nodes under their real ids as labels, and `import` maps any `Id` field naming a label in the file to that node's fresh id, so a `JAR` edge to a jar closed with it follows the jar):

```rust
#[test]
fn reopening_a_group_points_its_tab_at_the_reopened_jar_and_keeps_an_outside_one() {
    let mut app = App::default();
    app.ensure_default_jar();
    let group = crate::session_sync::tests::add_group(&mut app, "G");
    let inside = app.create_jar("In", crate::fetched::Placement { group: Some(group.clone()), pos: egui::vec2(40.0, 40.0) });
    let outside = crate::session_sync::tests::add_jar(&mut app, "Out");
    let tab = crate::session_sync::tests::add(&mut app, Kind::Tab, crate::session_sync::tests::sleeping_tab("https://a/", "A"));
    let other = crate::session_sync::tests::add(&mut app, Kind::Tab, crate::session_sync::tests::sleeping_tab("https://b/", "B"));
    for t in [&tab, &other] {
        app.edit_group(t, Some(group.clone()));
    }
    app.set_jar_edge(&tab, Some(inside));
    app.set_jar_edge(&other, Some(outside));
    app.close_node(&group);
    let entry = app.closed.pop().unwrap();
    app.reopen_closed(entry);
    let by_url = |app: &App, url: &str| app.nodes.keys().find(|id| app.mirror.url(id) == url).cloned().unwrap();
    let new_inside = *app.jar_defs().iter().find(|(_, d)| d.name == "In").unwrap().0;
    assert_ne!(new_inside, inside);
    assert_eq!(app.mirror.jar(&by_url(&app, "https://a/")), Some(new_inside), "follows the reopened jar");
    assert_eq!(app.mirror.jar(&by_url(&app, "https://b/")), Some(outside), "an outside jar keeps its id");
}
```

Run: `git grep -nE "JarChoice|jar_choice|JAR_SET|DEFAULT_JAR|jar_set|inherited_jar|resolved_jar|new_tab_jar|tab_jar\b|set_tab_jar|follow_group_jar|FollowGroupJar|refollow|resnapshot|cascade_targets|cascade_count|GroupDefaultJar|edit_default_jar" -- crates`
Expected: no hits.

- [ ] **Step 11: fmt, clippy, commit**

Run: `cargo fmt --all` then the clippy command from Global Constraints.
Expected: clean.

```bash
git add -A crates/manifold
git commit -m "One jar edge: tabs and groups name their jar, resolved by walking up

The jar edge row (Field(JAR), scope SameLevelOrAncestor, \"Use jar...\")
replaces JAR_SET and DEFAULT_JAR (now RESERVED). check_connect, connect
and disconnect handle any Field edge; a target out of reach is
OutOfScope. effective_jar replaces resolved_jar, inherited_jar,
new_tab_jar, the cascade, follow and refollow; a local move clears an
edge it takes out of reach, with a toast. Jar edges draw on the canvas.

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Backing files: one keep set, one prune and release, one reopen rule

**Files:**
- Modify: `crates/manifold/src/fetched.rs:6-19` (`prune_unnamed`), 164-205 (`content_keep`, `may_prune_content`, `prune_content_dir`)
- Modify: `crates/manifold/src/session.rs:36-66` (`content_names`)
- Modify: `crates/manifold/src/closed.rs:207-235` (`touch_owned_content`, `record_closed`), 290-330 (`reopen_closed`)
- Modify: `crates/manifold/src/session_sync.rs:1988-2010` (`replace_running_session`), 764 (`remove_remotely_deleted`: the release)
- Modify: `crates/manifold/src/content.rs:200-217` (`prune` takes a cutoff) and its tests (331-339 and every other `prune(` call there)
- Modify: `crates/manifold/src/jar.rs:284-315` (`named_jar_of`, `prune_orphans` deleted), `crates/manifold/src/jar_defs.rs:495-510` (`prune_jar_files` deleted)
- Modify: `crates/manifold/src/event_loop.rs:184-185`
- Test: `crates/manifold/src/fetched/tests.rs:285, 323, 341, 410, 431`, `crates/manifold/src/jar/tests.rs` (the `prune_orphans`/`named_jar_of` tests deleted), `crates/manifold/src/session_sync/tests.rs:2224-2345` (jar prune tests), `crates/manifold/src/closed.rs` tests

**Interfaces:**
- Consumes: Task 3 (`Jar::path(dir, id)`, `App::jars_dir`), Task 1 (jars are nodes, so `self.nodes` and closed entries hold them).
- Produces:
  - `App::backing_keep(&self, except: Option<&str>) -> HashSet<String>` (was `content_keep`): live nodes' ids and `path` file names, and Recently Closed entries' ids and `path` file names.
  - `session::backing_names(dir: &Path) -> Option<HashSet<String>>` (was `content_names`): the same for each named session, closed entries' ids included.
  - `App::prune_backing_dirs(&self)` (replaces `prune_content_dir` and `prune_jar_files`).
  - `App::release_backing(&self, id: &str)`: a removed node's file family, deleted unless held, on a thread.
  - `content::prune(dir: &Path, cutoff: SystemTime, keep: impl Fn(&str) -> bool) -> usize` (takes the cutoff, not the snapshot; the caller subtracts `PRUNE_GRACE` or not).
  - `fetched::prune_unnamed(dir, cutoff, keep, sessions, only: impl Fn(&str) -> bool) -> Option<usize>` (`only` picks the candidate files).
  - `closed::touch_backing(entry: &ClosedEntry, content_dir: Option<&Path>, jars_dir: Option<&Path>)` (was `touch_owned_content`).
  - `App::rebind_jar_files(&self, doc: &store::json::JsonDoc, imported: &[store::json::Imported])`.

The release point (spec "One release point"; decision 2026-10-14, pending the user's confirmation, see "Decisions this plan applies"). The startup prune and the release are one mechanism with two parameters:

- the startup prune considers every file in the dir, with the grace;
- the release considers only the removed node's file family (stem = its id), with no grace.

Both use the same keep set (`backing_keep` plus `session::backing_names`) and the same `may_prune` guard. The release runs from `remove_remotely_deleted`, the one removal path that records no Recently Closed entry. A local close records its entry first (`close_node` -> `record_closed`; `worth_reopening` is true for jars and content), so its files are always held at that moment, and the release is not called there. Leaving Recently Closed (eviction past `MAX_CLOSED`) is not a release: those files wait for the startup prune and its grace. Explicit wipes (Clear Cookies, Make Ephemeral) stay as they are. Accepted risk, named in the decision: a sibling instance on this machine whose own Recently Closed list holds a remotely deleted jar loses its cookies.

The name rule needs no change: `content::kept` already keeps `<name>` and every `<stem>.<anything>` for a stem in the set, never a `.tmp`. A jar's family (`<id>.json`, `<id>.json.unreadable-*`, `<id>.json.<pid>-<n>.tmp`) has stem `<id>`, so a set holding the jar's id keeps its file and asides; a crashed save's `.tmp` older than the grace goes.

- [ ] **Step 1: Write the failing tests**

In `crates/manifold/src/fetched/tests.rs`:

```rust
#[test]
fn one_prune_keeps_held_jar_files_and_drops_unheld_old_ones() {
    use crate::content::tests::{write_aged, OLD};
    let jars = crate::store::testutil::temp_dir("backing-jars");
    let mut app = App { jars_dir_override: Some(jars.clone()), ..App::default() };
    let live = crate::session_sync::tests::add_jar(&mut app, "Live");
    let closed = crate::session_sync::tests::add_jar(&mut app, "Closed");
    app.close_node(&closed.to_string());
    for name in [format!("{live}.json"), format!("{closed}.json"), "9-9.json".into(), "default.json".into()] {
        write_aged(&jars.join(name), OLD);
    }
    write_aged(&jars.join("9-8.json"), std::time::Duration::ZERO); // recent: inside the grace
    let cutoff = std::time::SystemTime::now() - crate::content::PRUNE_GRACE;
    let pruned = prune_unnamed(&jars, cutoff, app.backing_keep(None), None, |_| true);
    assert_eq!(pruned, Some(2), "9-9.json and the old default.json");
    for kept in [format!("{live}.json"), format!("{closed}.json"), "9-8.json".into()] {
        assert!(jars.join(&kept).exists(), "{kept}");
    }
}
```

and, for the release:

```rust
#[test]
fn releasing_a_removed_jar_deletes_only_its_unheld_family_at_once() {
    let jars = crate::store::testutil::temp_dir("release-jars");
    let keep: HashSet<String> = ["1-1".to_string()].into();
    let files = ["1-1.json", "1-2.json", "1-2.json.unreadable-5", "1-3.json"];
    for name in files {
        std::fs::write(jars.join(name), b"x").unwrap(); // brand new: no grace applies to a release
    }
    // A release's cutoff: now, plus a second for a coarse filesystem clock (`release_backing`).
    let cutoff = std::time::SystemTime::now() + std::time::Duration::from_secs(1);
    let only = |file: &str| crate::content::stem(file) == "1-2";
    assert_eq!(prune_unnamed(&jars, cutoff, keep.clone(), None, only), Some(2));
    assert!(jars.join("1-1.json").exists() && jars.join("1-3.json").exists(), "other jars untouched");
    let only = |file: &str| crate::content::stem(file) == "1-1";
    assert_eq!(prune_unnamed(&jars, cutoff, keep, None, only), Some(0), "held");
}
```

`write_aged` and `OLD` are the content tests' helpers (content.rs:321); make them `pub(crate)` if they are not. `content::stem` is the rule `kept` already applies inline, factored out (and `kept` uses it):

```rust
/// A backing file's stem: its name before the first `.` (`<id>.png`, `<id>.json.unreadable-5` -> `<id>`).
pub(crate) fn stem(file: &str) -> &str {
    file.split_once('.').map_or(file, |(stem, _)| stem)
}

pub(crate) fn kept(names: &HashSet<String>, file: &str) -> bool {
    !file.ends_with(".tmp") && (names.contains(file) || (stem(file) != file && names.contains(stem(file))))
}
```

(`kept` matched a stem only for a name with a `.`; `stem(file) != file` keeps that.) A file written just now could carry an mtime equal to a `now` cutoff on a coarse filesystem clock and fail `t < cutoff`, so a release's cutoff is `now + 1 s`. If `close_node` touches `<closed>.json` (Step 4), write the aged file before the close or call `write_aged` again after it, so the test exercises the keep set rather than the grace.

In `crates/manifold/src/closed.rs` tests:

```rust
#[test]
fn reopening_a_closed_jar_carries_its_cookie_file_and_a_duplicate_starts_empty() {
    let jars = crate::store::testutil::temp_dir("rebind-jars");
    let mut app = App { jars_dir_override: Some(jars.clone()), ..App::default() };
    let jar = crate::session_sync::tests::add_jar(&mut app, "Work");
    std::fs::write(jars.join(format!("{jar}.json")), b"cookies").unwrap();

    app.duplicate_nodes(&[jar.to_string()]);
    let copy = *app.jar_defs().keys().find(|&&id| id != jar).unwrap();
    assert!(!jars.join(format!("{copy}.json")).exists(), "a duplicate starts signed out");

    app.close_node(&jar.to_string());
    let entry = app.closed.pop().unwrap();
    app.reopen_closed(entry);
    let back = *app.jar_defs().keys().find(|&&id| id != copy).unwrap();
    assert_ne!(back, jar, "a fresh id");
    assert_eq!(std::fs::read(jars.join(format!("{back}.json"))).unwrap(), b"cookies");
}

#[test]
fn closing_a_jar_touches_its_cookie_file() {
    use crate::content::tests::{write_aged, OLD};
    let jars = crate::store::testutil::temp_dir("touch-jars");
    let mut app = App { jars_dir_override: Some(jars.clone()), ..App::default() };
    let jar = crate::session_sync::tests::add_jar(&mut app, "Work");
    let file = jars.join(format!("{jar}.json"));
    write_aged(&file, OLD);
    app.close_node(&jar.to_string());
    let age = file.metadata().unwrap().modified().unwrap().elapsed().unwrap();
    assert!(age < crate::content::PRUNE_GRACE, "touched");
}
```

`duplicate_nodes` and `close_node` for a jar in a windowless `App::default()` need no journal; if `reopen_closed`'s `save_session` complains without one, it returns `false` and the test goes on.

In `crates/manifold/src/session.rs` tests (or beside the existing `content_names` test), extend the named-session test to assert that a closed entry's node id is in `backing_names`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -j 8 -p manifold one_prune_keeps_held_jar_files`
Expected: compile error: no method `backing_keep`.

- [ ] **Step 3: One keep set**

`fetched.rs:164-180`, renamed and widened to ids in closed entries:

```rust
    /// The backing-file names this session still uses, for `content::kept` (spec "Backing files"): live nodes' ids
    /// (a replica's own content copy is `<id>.<ext>`, a jar's cookies `<id>.json`) and `path` file names, and the
    /// same for every Recently Closed entry, which holds its nodes' files until it leaves the list. `except` leaves
    /// out one live node.
    pub(crate) fn backing_keep(&self, except: Option<&str>) -> HashSet<String> {
        let mut keep: HashSet<String> = HashSet::new();
        for (id, node) in self.nodes.iter().filter(|(id, _)| Some(id.as_str()) != except) {
            keep.extend(node.runtime.file_path().and_then(content::file_name));
            keep.insert(id.clone());
        }
        for node in self.closed.iter().flat_map(|entry| &entry.doc.nodes) {
            keep.extend(node.fields.get("path").and_then(|v| v.as_str()).and_then(content::file_name));
            keep.insert(node.id.clone());
        }
        keep
    }
```

Rename every `content_keep` call (`replaced_file_unused` callers in `fetched.rs`, and the tests at fetched/tests.rs 285-431).

`session.rs:36-66`, renamed `backing_names`, doc updated ("What the named sessions in `dir` still use of the backing-file dirs ... each one's live and Recently Closed nodes' ids and `path` file names"), and the id loop covers closed nodes too:

```rust
        let nodes = live.iter().chain(closed.iter().flat_map(|c| &c.nodes));
        for node in nodes.clone() {
            names.insert(node.id.clone());
        }
        names.extend(nodes.filter_map(|n| n.fields.get("path")).filter_map(|p| p.as_str()).filter_map(crate::content::file_name));
```

`prune_unnamed` (fetched.rs:9) calls `session::backing_names`, takes the cutoff, and gains `only`:

```rust
/// Prunes `dir` of the files `only` picks that neither `keep` nor any named session in `sessions` names and that
/// were last modified before `cutoff`. The startup prune picks every file with `cutoff` a grace before its snapshot;
/// a release picks one node's files with no grace. `None`, deleting nothing, when a session can't be read whole
/// (`session::backing_names`).
pub(crate) fn prune_unnamed(
    dir: &std::path::Path,
    cutoff: std::time::SystemTime,
    mut keep: HashSet<String>,
    sessions: Option<&std::path::Path>,
    only: impl Fn(&str) -> bool,
) -> Option<usize> {
    if let Some(sessions) = sessions {
        keep.extend(session::backing_names(sessions)?);
    }
    Some(content::prune(dir, cutoff, |file| !only(file) || content::kept(&keep, file)))
}
```

`content::prune` (content.rs:207) takes `cutoff` in place of `snapshot` and drops its own `checked_sub(PRUNE_GRACE)`. Its doc moves the snapshot paragraph to `prune_backing_dirs`, which computes `snapshot.checked_sub(PRUNE_GRACE).unwrap_or(UNIX_EPOCH)`. Its callers that passed a `snapshot` of `now` pass `SystemTime::now() - PRUNE_GRACE` instead, and `prune_unnamed` callers add `|_| true`: `content.rs:337, 339, 356`, `fetched/tests.rs:286, 323, 341`.

- [ ] **Step 4: One prune and one touch**

`may_prune_content` (185) becomes `may_prune` and also refuses a read-only journal (a newer build's document may hold nodes this build does not load, whose files would look unheld):

```rust
    /// Whether this launch knows every node that may hold a backing file: not with no journal, a rebuilt one
    /// (`store_recovered`), or one a newer build wrote (read-only), any of which may not have loaded everything.
    pub(crate) fn may_prune(&self) -> bool {
        self.journal.as_ref().is_some_and(|j| !j.read_only()) && !self.store_recovered
    }
```

`prune_content_dir` (192) becomes:

```rust
    /// Deletes the backing files nothing holds (spec "Backing files"), in the content and jars dirs, on a thread.
    /// Called once at launch, after restore. The thread also reads the named sessions, which hold their files too;
    /// files touched within `content::PRUNE_GRACE` stay (a sibling instance's Recently Closed may hold them).
    pub(crate) fn prune_backing_dirs(&self) {
        if !self.may_prune() {
            return;
        }
        // A file another instance wrote after this keep set was built is never a candidate (`snapshot`); one
        // touched within the grace stays too (a sibling instance's Recently Closed may hold it).
        let cutoff = std::time::SystemTime::now().checked_sub(content::PRUNE_GRACE).unwrap_or(std::time::UNIX_EPOCH);
        self.prune_backing_files(cutoff, None, "pruned");
    }

    /// A removed node's backing files (stem `id`, in the content and jars dirs), deleted at once unless something
    /// still holds them (decision 2026-10-14, spec "One release point"): the startup prune's keep rule with no
    /// grace. Called where a removal records no Recently Closed entry (`remove_remotely_deleted`), after the node
    /// has left `nodes`.
    pub(crate) fn release_backing(&self, id: &str) {
        if !self.may_prune() || self.backing_keep(None).contains(id) {
            return;
        }
        let cutoff = std::time::SystemTime::now() + std::time::Duration::from_secs(1);
        self.prune_backing_files(cutoff, Some(id.to_owned()), "released");
    }

    /// The shared body: on a thread, prunes each backing dir of the files `only` picks (every file with `None`)
    /// that nothing holds and that were modified before `cutoff`.
    fn prune_backing_files(&self, cutoff: std::time::SystemTime, only: Option<String>, verb: &'static str) {
        let dirs: Vec<std::path::PathBuf> = [profile::content_dir(), self.jars_dir()].into_iter().flatten().collect();
        let keep = self.backing_keep(None);
        let sessions = session::named_sessions_dir();
        std::thread::spawn(move || {
            let pick = |file: &str| only.as_deref().is_none_or(|id| content::stem(file) == id);
            for dir in dirs {
                match prune_unnamed(&dir, cutoff, keep.clone(), sessions.as_deref(), pick) {
                    Some(0) => {}
                    Some(n) => eprintln!("backing files: {verb} {n} file(s) in {}", dir.display()),
                    None => eprintln!("backing files: a named session couldn't be read whole; not deleting"),
                }
            }
        });
    }
```

(`eprintln!` naming the directory, never a file's contents). Match how the current `prune_content_dir` (fetched.rs:192-205) gets the named-sessions dir and the content dir, and keep those calls. The startup prune and the release share this one body and differ only in `cutoff` and `only`.

`session_sync.rs:764` `remove_remotely_deleted`: at its end, after the node is dropped (`drop_node`, `drop_tab` or `dissolve_group_quietly`), `self.release_backing(id);`. A group has no backing file of its own; the call finds nothing with its stem and costs one `backing_keep` and a short thread, as a remote delete is rare.

Add to `session_sync/tests.rs`:

```rust
#[test]
fn a_remote_jar_delete_releases_its_file_unless_recently_closed_holds_it() {
    let dir = temp_dir("release-remote");
    let jars = dir.join("jars");
    std::fs::create_dir_all(&jars).unwrap();
    let mut app = App { jars_dir_override: Some(jars.clone()), ..app_on(&dir.join("session.mnf"), 1) };
    let (gone, held) = (add_jar(&mut app, "Gone"), add_jar(&mut app, "Held"));
    assert!(app.save_session());
    for jar in [gone, held] {
        std::fs::write(jars.join(format!("{jar}.json")), b"x").unwrap();
    }
    // `held` was closed here earlier, so this instance's Recently Closed names it.
    app.record_closed(&held.to_string());
    for jar in [gone, held] {
        app.remove_remotely_deleted(&jar.to_string());
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while jars.join(format!("{gone}.json")).exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10)); // a poll interval, not a wait for a guessed duration
    }
    assert!(!jars.join(format!("{gone}.json")).exists(), "released");
    assert!(jars.join(format!("{held}.json")).exists(), "held by Recently Closed");
}
```

(the wait is on the observable file with a deadline, not a fixed sleep; match `app_on`'s real construction if `jars_dir_override` must be set another way). `event_loop.rs:184-185`: the two calls become `self.prune_backing_dirs();`. Delete `prune_jar_files` (jar_defs.rs:495-510), `jar::named_jar_of` and `jar::prune_orphans` (287-315) with their tests (jar/tests.rs, session_sync/tests.rs 2224-2345: port the cases they cover, a jar deleted while this device was away and a recovered store, to `one_prune_keeps_held_jar_files_and_drops_unheld_old_ones` and a `may_prune` assertion).

`closed.rs:207-218`:

```rust
/// Marks each backing file `entry`'s nodes hold as modified now (an owned content file in `content_dir`, a jar's
/// cookie file in `jars_dir`), so a recent close keeps its file through `content::PRUNE_GRACE` even if another
/// instance's Recently Closed list overwrites this one's.
pub(crate) fn touch_backing(entry: &ClosedEntry, content_dir: Option<&std::path::Path>, jars_dir: Option<&std::path::Path>) {
    let touch = |path: &std::path::Path| {
        let file = std::fs::OpenOptions::new().write(true).open(path);
        let _ = file.and_then(|f| f.set_modified(std::time::SystemTime::now()));
    };
    for node in &entry.doc.nodes {
        let path = node.fields.get("path").and_then(|p| p.as_str()).map(std::path::Path::new);
        if let (Some(dir), Some(path)) = (content_dir, path) {
            if content::owned_content_file(dir, &node.id, path) {
                touch(path);
            }
        }
        if let (Some(dir), Some(Kind::Jar)) = (jars_dir, node.kind()) {
            touch(&dir.join(format!("{}.json", node.id)));
        }
    }
}
```

`record_closed` (223) calls `touch_backing(&entry, profile::content_dir().as_deref(), self.jars_dir().as_deref())`.

- [ ] **Step 5: One reopen rule**

In `closed.rs`, beside `reopen_closed`:

```rust
impl App {
    /// After an import with fresh ids: each jar whose old id is no longer a live node gets a copy of its cookie file
    /// under its new id, so a reopened jar comes back signed in (spec "One reopen rule"). One whose old id is still
    /// live (a Duplicate) gets nothing: the copy starts signed out. A copy, not a move: a named session or another
    /// instance's Recently Closed list may still name the old id, and the startup prune removes the old file once
    /// nothing holds it. `imported[i]` is `doc.nodes[i]` (`store::json::import`).
    pub(crate) fn rebind_jar_files(&self, doc: &store::json::JsonDoc, imported: &[store::json::Imported]) {
        let Some(dir) = self.jars_dir() else { return };
        for (old, new) in doc.nodes.iter().zip(imported) {
            if new.kind != Kind::Jar || self.nodes.contains_key(&old.id) {
                continue;
            }
            let (from, to) = (dir.join(format!("{}.json", old.id)), crate::jar::Jar::path(&dir, new.id));
            if from.exists() && !to.exists() {
                if let Err(e) = std::fs::copy(&from, &to) {
                    eprintln!("jar: could not carry jar {} over to {}: {e}", old.id, new.id);
                }
            }
        }
    }
}
```

Call it in `reopen_closed` right after `import` succeeds and before `materialize_imported` (`self.rebind_jar_files(&entry.doc, &imported);`), and in `replace_running_session` (session_sync.rs:1988) after its `import`, before its `materialize_imported` (there the running session's nodes are already torn down, so a named session's jars always carry their files; the Default is skipped by `materialize_imported`, Task 3). A content file needs nothing: its `PATH` is copied verbatim.

- [ ] **Step 6: Run the tests**

Run: `cargo test -j 8 -p manifold backing`
Run: `cargo test -j 8 -p manifold jar`
Run: `cargo test -j 8 -p manifold release`
Expected: PASS.

Run: `cargo test -j 8 -p manifold`
Expected: PASS.

Run: `git grep -nE "content_keep|may_prune_content|prune_content_dir|content_names|touch_owned_content|prune_jar_files|prune_orphans|named_jar_of" -- crates`
Expected: no hits.

- [ ] **Step 7: fmt, clippy, commit**

Run: `cargo fmt --all` then the clippy command from Global Constraints.
Expected: clean.

```bash
git add -A crates/manifold
git commit -m "Backing files: jar cookies and fetched content share one keep set and prune

backing_keep and session::backing_names hold live and Recently Closed
nodes' ids and paths; one startup prune runs over the content and jars
dirs with the same grace, and a remote delete releases the removed
node's files at once through the same body unless something holds them.
Closing a jar touches its file; reopening it copies the file to the new
id, a duplicate starts signed out. Deleted: prune_jar_files,
jar::prune_orphans, jar::named_jar_of.

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Debug commands and gate smoke

**Files:**
- Modify: `crates/manifold/src/debug_control.rs:398-410` (enum docs), 577, 598-600 (help table), 852-863 (parser), 1366-1440 (handlers), 1476-1600 (`export_state` cards), the `SAMPLES` test near 1842-1853, `jar_commands_parse` at 2163
- Modify: `crates/gate/src/state.rs:6-12` (`JAR`), `crates/gate/src/smoke.rs` (new jar section after the note rename/duplicate block at 826-860), `crates/gate/src/jar.rs:44` (its local `JAR` const replaced by `state::JAR`), `crates/gate/src/layout.rs:83, 85` (scenarios `tab-menu-jar`, `group-menu-jar`)
- Test: `crates/manifold/src/debug_control.rs` tests, `crates/gate/src/state.rs` tests

**Interfaces:**
- Consumes: `create_jar(name, at)`, `set_jar_edge`, `set_jar_ephemeral`, `effective_jar`, `default_jar`, `jar_named`, `is_default_jar`.
- Produces:
  - Debug commands `NEWJAR <name>`, `USEJAR <id> <jar-id|none>`, `JAREPHEMERAL <id> on|off`; `SETJAR` and `CLEARJAR` keep their names.
  - EXPORTSTATE card keys `"jar"` (own edge id or null), `"effective_jar"` (id or null), `"is_default"` (bool, jar cards).
  - `gate::state::JAR = "application/vnd.manifold.jar"`.

- [ ] **Step 1: Write the failing parser test**

In `debug_control.rs` tests, extend `jar_commands_parse` (2163):

```rust
    assert_eq!(parse("NEWJAR Work Stuff"), Some(DebugCommand::NewJar("Work Stuff".into())));
    assert_eq!(parse("USEJAR 1-2 1-5"), Some(DebugCommand::UseJar { from: "1-2".into(), jar: Some("1-5".into()) }));
    assert_eq!(parse("USEJAR 1-2 none"), Some(DebugCommand::UseJar { from: "1-2".into(), jar: None }));
    assert_eq!(parse("JAREPHEMERAL 1-5 on"), Some(DebugCommand::JarEphemeral { id: "1-5".into(), on: true }));
    assert_eq!(parse("JAREPHEMERAL 1-5 maybe"), None);
```

(`parse` is whatever the test module calls the parser), and add the three to `SAMPLES`.

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -j 8 -p manifold jar_commands_parse`
Expected: compile error: no variant `NewJar`.

- [ ] **Step 3: Commands**

Enum (beside `SetJar`, 398-410):

```rust
    /// `NEWJAR <name>` -- a new jar card at the current level, as the Jar page's "New Jar..." makes, without an
    /// edge. See `App::create_jar`.
    NewJar(String),
    /// `USEJAR <id> <jar-id|none>` -- tab or group `id` uses that jar card (or inherits), as a Jar page row does.
    /// See `App::set_jar_edge`.
    UseJar { from: String, jar: Option<String> },
    /// `JAREPHEMERAL <id> on|off` -- jar card `id`'s Make/Stop Ephemeral, confirmed. See `App::set_jar_ephemeral`.
    JarEphemeral { id: String, on: bool },
```

Parser (852-863):

```rust
        "NEWJAR" => non_empty(DebugCommand::NewJar),
        "USEJAR" => {
            let from = parts.next()?.to_string();
            let jar = parts.next()?;
            Some(DebugCommand::UseJar { from, jar: (!jar.eq_ignore_ascii_case("none")).then(|| jar.to_string()) })
        }
        "JAREPHEMERAL" => {
            let id = parts.next()?.to_string();
            let on = match parts.next()? {
                "on" => true,
                "off" => false,
                _ => return None,
            };
            Some(DebugCommand::JarEphemeral { id, on })
        }
```

Handlers:

```rust
            DebugCommand::NewJar(name) => {
                // `placement` jitters by the string it is given and leaves a JSON level first, as a new card does.
                let at = self.placement(&name, None);
                let id = self.create_jar(&name, at);
                eprintln!("NEWJAR: {id}");
                self.save_session();
            }
            DebugCommand::UseJar { from, jar } => {
                let jar = jar.and_then(|j| j.parse().ok());
                self.set_jar_edge(&from, jar);
                self.save_session();
            }
            DebugCommand::JarEphemeral { id, on } => match id.parse() {
                Ok(jar) if self.jar_exists(jar) => {
                    self.set_jar_ephemeral(jar, on);
                    self.save_session();
                }
                _ => eprintln!("JAREPHEMERAL: {id} isn't a jar card; ignored"),
            },
```

`SETJAR` (1419) calls the Task 4 `set_focused_tab_jar_named`; `CLEARJAR` (1429) as Task 3 left it.

Help table (598-600), after `SETJAR`:

```rust
    ("NEWJAR name", "A new jar card at the current level, as \"New Jar...\" makes (no edge)"),
    ("USEJAR id jar|none", "Tab or group `id` uses jar card `jar`, or inherits with `none`, as a Jar page row"),
    ("JAREPHEMERAL id on|off", "Jar card `id`'s Make/Stop Ephemeral, confirmed"),
```

and `SETJAR`'s help text: "The focused tab's jar edge to that jar, by label or name (a new jar card at its level if none has it)".

`export_state` (1476): each card's JSON gains

```rust
            "jar": self.mirror.jar(id).map(|j| j.to_string()),
            "effective_jar": self.effective_jar(id).map(|(j, _)| j.to_string()),
            "is_default": self.is_default_jar(id),
```

(`effective_jar` for a non-tab, non-group card resolves through its group like any node; the gate reads it only for tabs and groups).

- [ ] **Step 4: Gate**

`gate/src/state.rs`: `pub const JAR: &str = "application/vnd.manifold.jar";` beside the others, and a case in `card_is_checks_type_loaded_and_json_cards` for a jar card.

`gate/src/smoke.rs`, after the note duplicate block (860), a jar section (the tab is any tab on the current level; the smoke has one by then):

```rust
    // Jars are cards: make one, link a tab to it, rename and duplicate it, then close and reopen a group holding
    // both, and the tab's edge follows the reopened jar.
    let before = sorted_ids(&duplicated);
    sm.send("NEWJAR Personal")?;
    let made = sm.settle("jar_made")?;
    let jar_id = new_card(&made, JAR, None, &before);
    check_found(&made, "jar_made", "NEWJAR adds a jar card", &jar_id);
    let tab_id = made
        .cards()
        .find(|(id, c)| State::card_is(c, TAB, None) && made.card_group(id) == made.current_group())
        .map(|(id, _)| id.to_string())
        .expect("a tab on this level");
    sm.send(&format!("USEJAR {tab_id} {jar_id}"))?;
    let used = sm.settle("jar_used")?;
    check_query(
        &used,
        "jar_used",
        "USEJAR draws the tab's jar edge",
        &format!("cards['{tab_id}']['jar'] == '{jar_id}' and cards['{tab_id}']['effective_jar'] == '{jar_id}'"),
        |s| s.card(&tab_id).is_some_and(|c| c["jar"] == jar_id.as_str() && c["effective_jar"] == jar_id.as_str()),
    );
    sm.send(&format!("RENAMEGROUP {jar_id} Work"))?;
    let renamed_jar = sm.settle("jar_renamed")?;
    check_query(
        &renamed_jar,
        "jar_renamed",
        "a jar card renames like any card",
        &format!("cards['{jar_id}']['name'] == 'Work'"),
        |s| s.card(&jar_id).is_some_and(|c| c["name"] == "Work"),
    );
    let before = sorted_ids(&renamed_jar);
    sm.send(&format!("DUPLICATENODE {jar_id}"))?;
    let dup_jar = sm.settle("jar_duplicated")?;
    let copy_jar = new_card(&dup_jar, JAR, None, &before);
    check_query(
        &dup_jar,
        "jar_duplicated",
        "the jar copy keeps the name; the tab keeps the original",
        &format!("cards['{copy_jar}']['name'] == 'Work' and cards['{tab_id}']['jar'] == '{jar_id}'"),
        |s| {
            s.card(&copy_jar).is_some_and(|c| c["name"] == "Work")
                && s.card(&tab_id).is_some_and(|c| c["jar"] == jar_id.as_str())
        },
    );
    let before = sorted_ids(&dup_jar);
    sm.send("NEWGROUP")?;
    let grouped = sm.settle("jar_group_made")?;
    let group_id = new_card(&grouped, GROUP, None, &before);
    for id in [&tab_id, &jar_id] {
        sm.send(&format!("MOVETOGROUP {id} {group_id}"))?;
    }
    let moved = sm.settle("jar_moved")?;
    check_query(
        &moved,
        "jar_moved",
        "a tab and its jar move together and the edge holds",
        &format!("cards['{tab_id}']['jar'] == '{jar_id}'"),
        |s| s.card(&tab_id).is_some_and(|c| c["jar"] == jar_id.as_str()),
    );
    sm.send(&format!("CLOSEGROUP {group_id}"))?;
    let closed = sm.settle("jar_group_closed")?;
    check_query(
        &closed,
        "jar_group_closed",
        "closing the group closes its jar",
        &format!("'{jar_id}' not in cards"),
        |s| s.card(&jar_id).is_none(),
    );
    let before = sorted_ids(&closed);
    sm.send("REOPENTAB")?;
    let reopened = sm.settle("jar_group_reopened")?;
    let new_jar = new_card(&reopened, JAR, None, &before);
    let new_tab = new_card(&reopened, TAB, None, &before);
    check_query(
        &reopened,
        "jar_group_reopened",
        "the reopened tab's edge names the reopened jar",
        &format!("cards['{new_tab}']['jar'] == '{new_jar}'"),
        |s| s.card(&new_tab).is_some_and(|c| c["jar"] == new_jar.as_str()),
    );
```

Match the real `State` accessors (`cards()`, `card_group`, `current_group`, `card`): use what smoke.rs's other sections use (`key(..)`, `s.card(id)`, `in_group`); if `NEWGROUP` groups the selection rather than making an empty group, select nothing first or use the smoke's existing way to make an empty group (grep `NEWGROUP` in smoke.rs). If `MOVETOGROUP` needs the group to be a sibling, both cards and the new group sit at the current level, which holds.

`gate/src/jar.rs:44` declares its own `const JAR`; delete it and import `state::JAR` (one constant). Its `default.json` and `jar_set` uses were already fixed in Tasks 3 and 4. `gate/src/layout.rs` scenarios `tab-menu-jar` (83) and `group-menu-jar` (85): the group menu row reads "Jar..." now; update expected labels.

- [ ] **Step 5: Run the tests**

Run: `cargo test -j 8 -p manifold jar_commands_parse`
Run: `cargo test -j 8 -p gate`
Expected: PASS.

Run: `git grep -nE "const JAR: &str" -- crates/gate`
Expected: one hit, in `state.rs`.

- [ ] **Step 6: fmt, clippy, commit**

Run: `cargo fmt --all` then the clippy command from Global Constraints.
Expected: clean.

```bash
git add -A crates/manifold crates/gate
git commit -m "Debug and gate: NEWJAR, USEJAR, JAREPHEMERAL; smoke covers jar cards

EXPORTSTATE cards carry jar, effective_jar and is_default. The smoke
makes a jar, links a tab, renames and duplicates the jar, and closes and
reopens a group holding both.

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Full gate run and docs

**Files:**
- Modify: `README.md` (debug command list)
- Modify: `docs/superpowers/specs/2026-10-02-jars-design.md:1`, `docs/superpowers/specs/2026-10-03-per-tab-jars-design.md:1` (superseded headers)
- Modify: `docs/superpowers/specs/2026-10-04-homogeneity-decisions.md:9, 37` (Links reach jars, open item 3)
- Modify: `docs/superpowers/specs/2026-10-13-jars-as-cards-design.md:3` (status), and its Scope and "One release point" wording once the user confirms decisions 2026-10-14

**Interfaces:**
- Consumes: everything above.
- Produces: a green `gate all LOG full` and docs that say jars are cards.

- [ ] **Step 1: Build**

Run (detached at BelowNormal, LONG_RUNS.md): `cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate`
Expected: `Finished` with no warnings.

- [ ] **Step 2: Gate**

Check first: no orphan `manifold.exe` or `gate.exe` (`Get-Process manifold, gate -EA 0`), CPU under ~20% (`(Get-Counter '\Processor(_Total)\% Processor Time' -SampleInterval 2 -MaxSamples 2).CounterSamples[-1].CookedValue`), Defender state (`(Get-MpComputerStatus).RealTimeProtectionEnabled`).

Run, detached at BelowNormal: `cmd //c start "" //b //belownormal target/debug/gate.exe all target/gate-jars-as-cards.log full`
Expected: the log ends with every stage passing. Report CPU load and Defender state with the result.

Triage rule for failures caused by the Default card now on every canvas: a check that counts all cards at the top level (for example smoke.rs:690 and 717, which count groups and images and are unaffected, versus any `len(cards)` check) filters out `JAR` cards; a layout scenario whose screenshot now includes the Default card is re-baselined only after looking at the new frame and confirming the only difference is the Default card. Any other failure is a bug to fix in the task that caused it, with its own commit.

- [ ] **Step 3: Docs**

`README.md`, the debug command list: add `NEWJAR name`, `USEJAR id jar|none`, `JAREPHEMERAL id on|off` with the help table's wording, and update `SETJAR`'s line.

`2026-10-02-jars-design.md` and `2026-10-03-per-tab-jars-design.md`, a first line:

```markdown
> Superseded by `2026-10-13-jars-as-cards-design.md` (jars are cards; one `jar` edge; the Default is a card). Kept for history.
```

`2026-10-04-homogeneity-decisions.md`: line 9 ("Links reach jars") gains "Done (2026-10-14): jars are cards and `link` reaches them, see `../plans/2026-10-14-jars-as-cards.md`." Open item 3 becomes "**Jars as cards.** Done (2026-10-14, `../plans/2026-10-14-jars-as-cards.md`): jar cards, the `jar` edge with its scope column, the Default card, one backing-file prune." and moves under "Done".

`2026-10-13-jars-as-cards-design.md:3`: "Status: built (plan `../plans/2026-10-14-jars-as-cards.md`)." and, under "Decided and open", the deviations this plan took: no `at_most_one` column (a `Field` edge holds one target), no `on_duplicate` column (content is shared by `PATH`, a jar's file is id-named so a copy starts empty by construction), the release runs where a removal records no Recently Closed entry (a remote delete) and shares the startup prune's body, the move recheck covers rows that reach beyond one level only, a reopened jar's file is copied rather than moved, and a stored pos at the exact origin reads as unplaced. Apply the Scope and release wording the user confirmed for decisions 2026-10-14 (the hand-back lists the exact edits); if the user has not confirmed them by then, stop and ask before writing this paragraph.

- [ ] **Step 4: Commit**

```bash
git add README.md docs/superpowers/specs
git commit -m "Docs: jars are cards; old jar specs superseded; homogeneity item 3 done

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

## Self-review log

Five rounds:

1. While writing: the `CardHeader` and `paint_card_header` API and the `NodeMenuItem::Button` constructor were wrong; skipping an imported Default inside `import` broke the `imported[i]` contract, so the skip moved to `materialize_imported`; a scope test had a group's jar edge pointing into the group itself (illegal under the spec's scope then; decision 2026-10-14 later made it legal).
2. Full pass: `ensure_default_jar` in `apply_remote` now runs only when a Default existed before (`had_default`); the `NEWJAR` handler borrowed `self` twice; the Task 3 test using `jar_of`/`resolved_jar` must move to `effective_jar` in Task 4; `rename_jar` is removed; between Tasks 1 and 5 the interim `prune_jar_files` keeps closed jars' files; the `Scope`, `Storage` and `JAR_EDGE` imports were missing.
3. Full pass: every caller of the renamed prune functions (`fetched.rs`, `fetched/tests.rs`, `closed.rs`, `session.rs`, `jar_defs.rs`, `event_loop.rs`) is covered by Task 5, and no placeholders remain. Nothing material found.
4. Independent review, each finding checked against the code: the remote ephemeral path compared against a mirror `mirror_remote` had already updated (split into `after_jar_ephemeral_changed`); an absent `POS` decodes as the origin, so `pos_of` treats it as unplaced; `apply_node_action` takes `ctx`; `paint_card_body` matches on `TYPE_*` and `own_text` has a `_` arm, so neither fails to compile when missed; `EdgeKind`/`LevelEdge` were written against the wrong shape; `default_jar` scanned every node per call (now a mirror index); the gate's `default.json` needed Task 3, and every gate re-import makes a new Default id, so the login file is carried to `<id>.json` via `--dump`; the card_kind draw-layer test, test-file lists and per-task leftover greps were missing; no caps count assertion exists (finding false). Decisions 2026-10-14 applied: release on remote delete, a group's edge reaches its own contents.
5. Full pass on the revision: `set_jar_ephemeral` still passed the removed `local` argument; a test read a nonexistent `fields_of_live` (now `app.snapshot`); a `yield_now` spin became deadline polling; a stale draft name sat in Task 4's leftover grep. Nothing further found.

