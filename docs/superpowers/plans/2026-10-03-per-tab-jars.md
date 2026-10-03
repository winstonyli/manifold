# Per-tab Jars Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Each tab uses exactly one cookie jar, chosen by the user. A group supplies the jar its new tabs start with. Jars can be ephemeral. Which jar a tab uses syncs like any other tab field; cookie contents stay on the device.

**Architecture:**
- **Store.** `Kind::Jar` nodes and four new diffed fields (`jar`, `jar_set`, `default_jar`, `ephemeral`) are added to `store/model.rs`.
- **Bridging.** Jar nodes never enter `App.nodes`. They live in `App.jar_defs` and are bridged to the document in three places: `shared_nodes` (out), `materialize` (in) and `remove_remotely_deleted` (delete).
- **Cookies.** `App.jar` becomes `App.jars: BTreeMap<JarKey, Jar>`, with one file per key under `<profile>\jars`. Each running host's `jar_tabs` entry records the `JarKey` it was spawned with. Pulls merge into that jar and save only that jar's file.
- **Inheritance.** A new tab snapshots the nearest group default. Changing a group default cascades plain field writes to the unset tabs it governs.
- **Switching.** A live tab whose resolved jar differs from its spawn jar switches by sleeping (`SleepWhy::JarSwitch`), under the idle-sleep rules. "Switch now" respawns it in place, the way an engine swap does.
- **Hosts and IPC.** No host or IPC change.

**Tech Stack:** Rust, egui, the manifold store (`store/model.rs`, `session_sync.rs`, `store/json.rs`), `jar.rs`, bash + Python probe scripts.

**Spec:** `docs/superpowers/specs/2026-10-03-per-tab-jars-design.md` (the authority). Background: `docs/superpowers/specs/2026-10-02-jars-design.md`, `docs/superpowers/plans/2026-10-02-cookie-jar-v1.md`.

## Deviations from the spec

Each item is a decision the spec leaves open, or a small departure, with the reason. Implementers follow the plan. Reviewers check that each deviation is still justified.

1. **`jar_defs` is keyed by `NodeId`, not `String`.** The spec writes `BTreeMap<String, JarDef>`. `JarKey::Named` and the `jar`/`default_jar` fields already hold `NodeId`, so a `NodeId` key avoids parsing on every lookup. The graph keys by `String` only because it holds hand-built non-store ids in tests.
2. **The group menu's list omits "Default".** The default jar has no node, so `default_jar = None` already means "inherit from the parent group, else Default". The group menu therefore lists only named jars, plus "Clear Default Jar". A group cannot name the default jar as an explicit default that overrides an outer group's named default. Pick a named jar instead, or clear the outer default.
3. **Tabs a tab opens copy its jar.** Duplicate, split, page-opened tabs (`OpenTabRequested`, the page context menu's "Open in new tab") copy the source tab's `JarChoice`, both `jar` and `jar_set`. The spec only covers "a new tab in a group". Without this, a link opened from an account-B tab would land signed in as account A.
4. **Open Session and Import Session dedupe jars by name.**
   - Imported nodes get fresh ids, so an exported jar "Work" would otherwise come back as a second, empty "Work".
   - `materialize_imported` maps an imported jar to an existing `jar_defs` entry with the same name, and rewrites the imported tabs' `jar` and groups' `default_jar` to it.
   - An unmatched imported jar becomes a new jar.
   - `teardown_running_session` keeps `jar_defs`: jars outlive a session swap, as cookie files do.
5. **A pull with no `jar_tabs` entry is dropped.** v1 started an empty entry and merged it into the one jar. With several jars there is no safe jar to merge into. An entry is missing only for a host that was closed or replaced since the pull was sent.
6. **`restore_session` materializes jar nodes before anything else, and its emptiness check ignores them.** Otherwise a document holding only jars would start fresh, and the first save would delete the jars as "missing from live".
7. **Menu labels are Title Case,** matching the existing menus: "New Jar...", "Follow Group's Jar", "Switch Jar Now", "Clear Default Jar".
8. **"Switch now" is offered three ways, none of them a clickable tooltip:**
   - the tab menu item "Switch Jar Now";
   - a click on the address-bar jar dot in Tab view;
   - the headless `SWITCHJARNOW` command.

   The chip's hover text explains the "will switch" state. egui tooltips are not clickable.
9. **A local jar delete clears the `jar` field of the tabs and group defaults that named it.** The spec says those tabs "resolve to Default", which the code honours either way. Clearing the field means a local delete leaves no warning chip. A *remote* delete leaves the fields as they arrive (the deleting device's own writes clear them), and in the meantime the warning chip shows.
10. **Toggling `ephemeral` acts on the file.** Turning it on deletes the jar's file, keeping the in-memory cookies until quit. Turning it off saves the in-memory jar to its file. The spec states only the steady state, "has no file". (Final review I1: the jar is loaded before either, a locked jar's files are never deleted, and Settings asks before turning it on for a jar with a saved file; see the spec's "Ephemeral jars".)
11. **A tab with a dirty form is asked again each sweep.** A tab that refused a jar switch is re-asked every idle sweep (`SWEEP_EVERY`, 10 s), and also after every local jar operation and every `apply_remote` (each runs `sweep_jar_switches`), until it qualifies. The spec says "switches when it next qualifies". This is the cheapest way to notice when it does. Each attempt is one cookie pull plus one `QueryPageState`.
12. **Re-snapshot runs on any move.** An unset tab's snapshot is refreshed on every move between groups: drop, eject, Join, Merge, and Dissolve (members move to the parent). The spec says "moving".
13. **A tab menu shows the jar list only for tabs.** The Tab and Unloaded menus show it; other kinds (notes, images and so on) do not, since they have no host.
14. **Clearing a jar switches the live tabs using it.** "Clear cookies" on a jar used by live tabs sends those tabs through the switch flow, so a busy tab keeps the old cookies until it qualifies, as the spec says. Until its host is replaced, such a tab's pulls are dropped (`jar_reseed`), so the old cookies its engine still holds cannot refill the cleared jar. This stays within "puts the live tabs using it through the switch flow". The cost (Task 8): every pull from that jar's running tabs is dropped until they switch, including a login made in one of them after the clear. Accepted: cleared means cleared, and the chip's tooltip says anything signed in meanwhile is not kept.

## Global Constraints

Repo rules (CLAUDE.md):
- Work on `dev`, never `main`. New branches and worktrees start from `dev` and merge back into it.
- Run `cargo fmt --all` before each commit. `cargo fmt --all --check` and `cargo clippy -p manifold -p ipc-protocol --all-targets -- -D warnings` must pass. Never bypass the pre-commit hook with `--no-verify`.
- Don't silence a lint to pass: fix it, or `#[allow]` it with a comment saying why.
- No test may read or write the real `%APPDATA%\manifold`. Every path into it starts from `profile::dir()` (and `profile::jars_dir()`), which are `None` under `cfg(test)`. A test that needs files uses `store::testutil::temp_dir` and passes the path in. `profile::tests` fails if any other source file mentions `"APPDATA"` or `etcetera`.
- Never log cookie values (v1 rule).
- Commit trailer: `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`.

Binding rules copied from the spec (verbatim):
- "New fields get the next free numbers (15 onward; never reused). `jar`, `jar_set` and `default_jar` are ordinary diffed fields (`explicit: false`): the live payload holds the value and `session_sync::diff` writes a `Set` when it differs, as for `pinned` and `engine`."
- "The default jar is built in and has no node; its file stays `jars\default.json`. A node exists only for a named jar."
- "A jar is not a `NodePayload` and never enters `App.nodes` (the graph)"
- "`shared_nodes` adds a `Kind::Jar` entry per definition (otherwise `diff` would delete the document's jar nodes as "missing from live"), `materialize` routes a `Kind::Jar` change into `jar_defs`, and `remove_remotely_deleted` removes a definition another replica deleted."
- "One function, `resolved_jar(tab) -> JarKey` (`Default | Named(NodeId)`), is the only place a tab's `jar` field becomes a key. An unset field, a deleted jar node or an unknown node resolves to `Default` and flags the tab for a warning chip."
- "The writes are ordinary field edits (there is no user-level undo in manifold)."
- "Each running host's `jar_tabs` entry records the `JarKey` it was **spawned with**. Its pulls merge into that jar and save only that jar's file, so a half-switched tab cannot write one account's cookies into another's jar. If that jar is deleted while the tab runs, its pulls are dropped and the tab goes through the switch flow to `Default`."
- "A jar node with `ephemeral` true has no file. Its `Jar` lives in the in-memory map, is never loaded or saved, and is dropped when manifold quits (per session)."
- "`Jar::path` takes a key: `default.json` for `Default`, `<hex node id>.json` for a named jar. The encrypted format, the locked state on an unreadable file, the unique aside names and the temp-file naming are unchanged. A save failure is logged without cookie values and leaves the other jars alone."
- "No host or IPC change"
- "A remote edit uses the same table and never reloads the tab being looked at."
- "Migration: None. Existing sessions have no jar fields, so every tab resolves to `Default`; `default.json` is untouched."

## Exhaustive matches and creation sites

Every place this plan must touch because the compiler or a convention demands it.

**Exhaustive `Kind` matches** (`Kind::Jar` added):
- `session_sync.rs` `kind_fields` (around line 38). This gives the new arm `Kind::Jar => &[COLOR, NAME, EPHEMERAL]`.
- `session_sync.rs` `insert_materialized` (around line 637). `Kind::Unknown(_) => return` becomes `Kind::Jar | Kind::Unknown(_) => return`. Jars are routed away earlier in `materialize`, so the arm is never reached.
- `closed.rs` `worth_reopening` (around line 48). Add `Kind::Jar => false`, because Recently Closed holds graph nodes only.
- `store/model.rs` `KINDS` table, which grows to 9 entries.

**`NodePayload`:** no new variant, so no new arms. The spec puts jars outside `App.nodes`.

**New fields on payload structs.** Each site must add the new field.
- `GroupPayload { name, from_json }` gains `default_jar`. All of these switch to a new `GroupPayload::named(name)`:
  - `card.rs` 2186, 2286;
  - `closed.rs` 230, 235;
  - `level.rs` 147;
  - `graph_ui.rs` 325;
  - `node.rs` 678, 746;
  - `main.rs` 1884, 2156, 2810, 2841;
  - `json_level.rs` 461, 652, 703;
  - `session_sync.rs` 2594, 2611, 2612, 2812, 2879.

  `session_sync.rs` 563 (`insert_materialized`) builds the payload from fields instead.
- `UnloadedTabPayload` gains `jar: JarChoice`:
  - `card.rs` 2206, 2292, 2304;
  - `node.rs` 734;
  - `tabs.rs` 452 (failed card), 705 (`sleep_now`);
  - `session_sync.rs` 555, 1952, 2133, 2269, 2282.

  The pattern at `session_sync.rs` 310 uses `..` and stays as it is.
- `TabPayload` gains `jar: JarChoice`. There is one site, `tabs.rs` 329 (`handle_tab_spawn_result`).
- `TabCarry` gains `jar: JarChoice`:
  - `main.rs` `TabSpawn::new` (717) and `TabSpawn::respawn` (735);
  - `main.rs` `wake_tab` (2171–2190);
  - `tabs.rs` 240, the exhaustive destructure;
  - `tabs.rs` 423, the fallback respawn.
- `TabSpawnOutcome` gains `jar: JarKey`:
  - `tabs.rs` 96 (`spawn_tab_async`);
  - `tabs.rs` 239 (destructure);
  - `session_sync.rs` 2554 (`fail_spawns` test helper).

**New-tab creation sites, and the jar each one gets:**

| Site | Jar |
|---|---|
| `tabs.rs` 65 `spawn_root_tab` | Default (no group) |
| `main.rs` 1872, 1899 `spawn_first_launch_demo` | Default (the demo group has no default) |
| `tabs.rs` 500 `spawn_new_tab`, called from `graph_ui.rs` 295 and `shortcuts.rs` 311 | Inherits from `group` |
| `tabs.rs` 909 `spawn_tab_at`, called from `fetched.rs` 369 and 412 | Inherits from `group` |
| `tabs.rs` 491 `duplicate_tab` | Copies the source tab |
| `tabs.rs` 625 `start_split` | Copies the focused tab |
| `event_loop.rs` 1640 `OpenTabRequested` | Copies the opener, via the new `spawn_tab_from` |
| `node_menu.rs` 421, page link "Open in new tab" | Copies the focused tab, via `spawn_tab_from` |
| `TabSpawn::respawn`: crash (`event_loop.rs` 1603), swap (`view_modes.rs` 271), jar switch | Keeps the tab's own `jar` |
| `main.rs` `wake_tab` | Keeps the card's own `jar` |

**Moves between groups**, where an unset tab is re-snapshotted by `refollow`:
- `graph_nav.rs` `apply_drop` (Eject, Join, GroupWith, Merge), `eject` and `merge_groups`;
- `main.rs` `dissolve_group`.

---

### Task 1: Store model — `Kind::Jar` and four fields

**Files:**
- Modify: `crates/manifold/src/store/model.rs`
- Modify: `crates/manifold/src/session_sync.rs` (`kind_fields`, `insert_materialized` arm)
- Modify: `crates/manifold/src/closed.rs` (`worth_reopening`)
- Test: the inline tests in `store/model.rs` and `session_sync.rs`

**Interfaces:**
- Consumes: nothing new.
- Produces:
  - `Kind::Jar` (code 9, name `"jar"`);
  - `field::JAR: u32 = 15` (Id, `"jar"`), `field::JAR_SET: u32 = 16` (Bool, `"jar_set"`), `field::DEFAULT_JAR: u32 = 17` (Id, `"default_jar"`) and `field::EPHEMERAL: u32 = 18` (Bool, `"ephemeral"`), all `explicit: false`;
  - `FIELDS: [FieldDef; 18]` and `KINDS: [(Kind, u64, &str); 9]`.
  - `kind_fields` becomes:
    - `Tab`: `[POS, COLOR, PARENT, URL, ENGINE, PINNED, JAR, JAR_SET]`;
    - `Group`: `[POS, COLOR, NAME, FROM_JSON, DEFAULT_JAR]`;
    - `Jar`: `[COLOR, NAME, EPHEMERAL]`.

- [ ] **Step 1: Write the failing tests**

In `store/model.rs` `mod tests`, extend `field_numbers_are_unique_and_never_reserved` (after the `view` asserts):

```rust
        for (no, name, ty) in [
            (field::JAR, "jar", FieldType::Id),
            (field::JAR_SET, "jar_set", FieldType::Bool),
            (field::DEFAULT_JAR, "default_jar", FieldType::Id),
            (field::EPHEMERAL, "ephemeral", FieldType::Bool),
        ] {
            let def = field_by_name(name).unwrap();
            assert_eq!((def.no, def.ty, def.explicit), (no, ty, false), "{name}");
        }
        assert_eq!((field::JAR, field::JAR_SET, field::DEFAULT_JAR, field::EPHEMERAL), (15, 16, 17, 18));
```

Add to the `cases` array in `values_round_trip_and_defaults_encode_to_nothing`:

```rust
            (field::JAR, FieldValue::Id(Some(id))),
            (field::JAR_SET, FieldValue::Bool(true)),
            (field::DEFAULT_JAR, FieldValue::Id(Some(id))),
            (field::EPHEMERAL, FieldValue::Bool(true)),
```

Add after `kinds_have_codes_and_names`:

```rust
    #[test]
    fn jar_is_kind_nine_and_live() {
        assert_eq!((Kind::Jar.code(), Kind::Jar.name()), (9, Some("jar")));
        assert_eq!(Kind::from_name("jar"), Some(Kind::Jar));
        let node = Node { kind: Kind::Jar, fields: BTreeMap::new(), group: None, deleted: false };
        assert!(node.is_live(), "a build that knows jars shows them as live");
    }
```

In `session_sync.rs` `mod tests` add:

```rust
    #[test]
    fn jar_fields_belong_to_tabs_groups_and_jars() {
        use field::*;
        assert_eq!(kind_fields(Kind::Tab), &[POS, COLOR, PARENT, URL, ENGINE, PINNED, JAR, JAR_SET]);
        assert_eq!(kind_fields(Kind::Group), &[POS, COLOR, NAME, FROM_JSON, DEFAULT_JAR]);
        assert_eq!(kind_fields(Kind::Jar), &[COLOR, NAME, EPHEMERAL]);
    }
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p manifold --lib -- jar_is_kind_nine_and_live field_numbers_are_unique values_round_trip jar_fields_belong`
Expected: compile errors (`no variant named Jar`, `cannot find value JAR in module field`).

- [ ] **Step 3: Implement**

In `store/model.rs`:
- add `Jar,` to `enum Kind` after `Download`;
- change `KINDS` to `[(Kind, u64, &str); 9]` and append `(Kind::Jar, 9, "jar"),`;
- in `pub mod field`, after `VIEW`:

```rust
    /// A tab's cookie jar: a `Kind::Jar` node, absent for the default jar.
    pub const JAR: u32 = 15;
    /// The tab's jar was picked by hand, so group-default cascades skip it.
    pub const JAR_SET: u32 = 16;
    /// A group's jar for new tabs (and unset tabs, by cascade).
    pub const DEFAULT_JAR: u32 = 17;
    /// A jar with no file: dropped at quit.
    pub const EPHEMERAL: u32 = 18;
```

- change `FIELDS` to `[FieldDef; 18]` and append:

```rust
    FieldDef { no: field::JAR, name: "jar", ty: FieldType::Id, explicit: false },
    FieldDef { no: field::JAR_SET, name: "jar_set", ty: FieldType::Bool, explicit: false },
    FieldDef { no: field::DEFAULT_JAR, name: "default_jar", ty: FieldType::Id, explicit: false },
    FieldDef { no: field::EPHEMERAL, name: "ephemeral", ty: FieldType::Bool, explicit: false },
```

In `session_sync.rs` `kind_fields`:

```rust
        Kind::Tab => &[POS, COLOR, PARENT, URL, ENGINE, PINNED, JAR, JAR_SET],
        Kind::Group => &[POS, COLOR, NAME, FROM_JSON, DEFAULT_JAR],
        // ... unchanged arms ...
        Kind::Jar => &[COLOR, NAME, EPHEMERAL],
```

Two more changes in this step:
- In `insert_materialized`, change `Kind::Unknown(_) => return,` to `Kind::Jar | Kind::Unknown(_) => return,`. Task 3 routes jars away before this point.
- In `closed.rs` `worth_reopening`, add the arm `store::model::Kind::Jar => false,`.

`shared_nodes` has a `debug_assert_eq!` that the keys equal `kind_fields`. It fires until Task 2 adds the fields to the payloads. For this task only, add the fields as absent in `tab_fields` and in the Group arm:

```rust
    fields.insert(field::JAR, FieldValue::Id(None));
    fields.insert(field::JAR_SET, FieldValue::Bool(false));
```

and in the Group arm, `fields.insert(field::DEFAULT_JAR, FieldValue::Id(None));`. Task 2 replaces these with the payload values.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p manifold --lib`
Expected: all pass. The `shared(...)` test helper fills absent defaults from `kind_fields`, so the existing diff tests keep passing: absent values encode to nothing, so no `Set` is written.

- [ ] **Step 5: fmt, clippy, commit**

```
cargo fmt --all
cargo clippy -p manifold -p ipc-protocol --all-targets -- -D warnings
git add crates/manifold/src/store/model.rs crates/manifold/src/session_sync.rs crates/manifold/src/closed.rs
git commit -m "Store: Kind::Jar and the jar, jar_set, default_jar, ephemeral fields" -m "Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Jar fields on tabs and groups, round-tripped through the store

**Files:**
- Modify: `crates/manifold/src/jar.rs` (add `JarChoice`, `JarKey`)
- Modify: `crates/manifold/src/node.rs` (`TabPayload`, `UnloadedTabPayload`, `GroupPayload`, `GroupPayload::named`)
- Modify: `crates/manifold/src/main.rs` (`TabCarry`, `TabSpawn::new`, `TabSpawn::respawn`, `wake_tab`)
- Modify: `crates/manifold/src/tabs.rs` (`handle_tab_spawn_result`, `sleep_now`, failed card)
- Modify: `crates/manifold/src/session_sync.rs` (`tab_fields`, `shared_nodes`, `update_live_node`, `insert_materialized`, `spawn_shared`)
- Modify: every `GroupPayload` and `UnloadedTabPayload` construction site listed under "Exhaustive matches and creation sites"

**Interfaces:**
- Consumes: Task 1's fields.
- Produces:

```rust
// jar.rs
/// What a tab's `jar` / `jar_set` fields hold (spec "Data model").
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JarChoice {
    /// A `Kind::Jar` node; `None` is the default jar.
    pub jar: Option<NodeId>,
    /// Picked by hand: a group-default cascade leaves it alone.
    pub set: bool,
}

/// Which jar a running host merges into, and which file it saves (spec "Resolution").
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum JarKey {
    #[default]
    Default,
    Named(NodeId),
}
```

Further additions in this task:
- `node.rs`: `TabPayload.jar: JarChoice`, `UnloadedTabPayload.jar: JarChoice`, `GroupPayload.default_jar: Option<NodeId>`, and `GroupPayload::named(name: impl Into<String>) -> GroupPayload` (`from_json: false`, `default_jar: None`).
- `main.rs`: `TabCarry.jar: JarChoice`.
- `session_sync.rs`: `fn node_id_of(fields: &Fields, no: u32) -> Option<NodeId>` and `fn jar_choice_of(fields: &Fields) -> JarChoice`.

- [ ] **Step 1: Write the failing tests**

In `session_sync.rs` `mod tests`:

```rust
    #[test]
    fn a_tabs_jar_and_a_groups_default_project_to_fields_and_back() {
        let jar = NodeId { actor: ActorId(7), counter: 1 };
        let nodes = BTreeMap::from([
            (
                id(1),
                shared(
                    Kind::Tab,
                    None,
                    &[(field::URL, FieldValue::Str("https://a.com/".into())), (field::JAR, FieldValue::Id(Some(jar))),
                      (field::JAR_SET, FieldValue::Bool(true))],
                ),
            ),
            (id(2), shared(Kind::Group, None, &[(field::DEFAULT_JAR, FieldValue::Id(Some(jar)))])),
        ]);
        let app = app_with(&nodes);
        let tab = &app.nodes[&id(1).to_string()];
        assert_eq!(tab.payload.as_unloaded().unwrap().jar, crate::jar::JarChoice { jar: Some(jar), set: true });
        assert_eq!(app.nodes[&id(2).to_string()].as_group().unwrap().default_jar, Some(jar));
        assert_eq!(app.shared_nodes(), nodes, "the live payloads project back to the same fields");
    }

    #[test]
    fn a_remote_jar_edit_updates_a_sleeping_tab_and_a_group() {
        let jar = NodeId { actor: ActorId(2), counter: 50 };
        let tab = shared(Kind::Tab, None, &[(field::URL, FieldValue::Str("https://a.com/".into()))]);
        let group = shared(Kind::Group, None, &[]);
        let mut app = app_with(&BTreeMap::from([(id(1), tab.clone()), (id(2), group.clone())]));
        let mut tab_fields = tab.fields.clone();
        tab_fields.insert(field::JAR, FieldValue::Id(Some(jar)));
        let mut group_fields = group.fields.clone();
        group_fields.insert(field::DEFAULT_JAR, FieldValue::Id(Some(jar)));
        app.apply_remote(vec![(id(1), Some((Kind::Tab, tab_fields, None))), (id(2), Some((Kind::Group, group_fields, None)))]);
        assert_eq!(app.nodes[&id(1).to_string()].payload.as_unloaded().unwrap().jar.jar, Some(jar));
        assert_eq!(app.nodes[&id(2).to_string()].as_group().unwrap().default_jar, Some(jar));
    }
```

These use the existing `id`, `shared` and `app_with` helpers in `session_sync.rs` tests. `shared` fills absent defaults from `kind_fields`, so the projected map must equal `nodes` exactly. If `shared_nodes` adds a field these nodes lack, that is a bug in the bridge, not the test. If `apply_remote`'s argument type differs from `Vec<(NodeId, Option<(Kind, Fields, Option<NodeId>)>)>`, follow `app_with`, which builds the same thing.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p manifold --lib -- jar_and_a_groups_default remote_jar_edit`
Expected: compile errors (`no field jar on UnloadedTabPayload`, `no function named`).

- [ ] **Step 3: Implement the types**

Add `JarChoice` and `JarKey` to `jar.rs` (as under Interfaces), with `use crate::store::model::NodeId;`. Extend the module doc: "Pure: no egui, no App; it names jars by store `NodeId`."

Add the fields to `node.rs`:

```rust
    /// See `jar::JarChoice`. Synced (`field::JAR`, `field::JAR_SET`).
    pub(crate) jar: crate::jar::JarChoice,
```

on both `TabPayload` and `UnloadedTabPayload`, and on `GroupPayload`:

```rust
    /// The jar new tabs in this group start with (`field::DEFAULT_JAR`); `None` inherits from the parent group.
    pub(crate) default_jar: Option<crate::store::model::NodeId>,
}

impl GroupPayload {
    /// A plain group: not from JSON, no default jar.
    pub(crate) fn named(name: impl Into<String>) -> GroupPayload {
        GroupPayload { name: name.into(), from_json: false, default_jar: None }
    }
}
```

Replace every `GroupPayload { name: X, from_json: false }` at the listed sites with `GroupPayload::named(X)`. Then add `jar: Default::default()` to every listed `UnloadedTabPayload` literal in tests (`card.rs`, `node.rs`, `session_sync.rs` 1952/2133/2269/2282). Real-code sites:

- `tabs.rs` 705 `sleep_now`: copy the live tab's choice. Destructure `jar` from the `TabPayload` with the other fields and pass `jar` into the `UnloadedTabPayload`.
- `tabs.rs` 452 failed card: `jar: carry.jar` (from the destructured `TabCarry`).
- `tabs.rs` 329 `TabPayload` literal: `jar: carry_jar` (see below).

In `main.rs`:
- `TabCarry` gains `/// See `TabPayload::jar`.` and `jar: crate::jar::JarChoice,`.
- `TabSpawn::new` sets `jar: Default::default()`.
- `TabSpawn::respawn` sets `jar: tab.jar`.
- `wake_tab` reads `card.jar` alongside `pinned`/`muted`, and passes it into the `TabCarry`.

In `tabs.rs` `handle_tab_spawn_result`:
- destructure `TabCarry { pinned, color, zoom_factor, title, muted, jar }`;
- the wake branch that reads `(parent, group, pos, color, pinned, muted)` from the card also reads `jar` (the card may have changed while the host started);
- put `jar` into the `TabPayload` literal;
- the fallback `TabCarry` rebuild at 423 passes `jar`.

- [ ] **Step 4: Implement the bridge**

In `session_sync.rs`:

```rust
fn node_id_of(fields: &Fields, no: u32) -> Option<NodeId> {
    match fields.get(&no) {
        Some(FieldValue::Id(id)) => *id,
        _ => None,
    }
}

fn jar_choice_of(fields: &Fields) -> crate::jar::JarChoice {
    crate::jar::JarChoice { jar: node_id_of(fields, field::JAR), set: bool_of(fields, field::JAR_SET) }
}
```

`tab_fields` gains a `jar: crate::jar::JarChoice` parameter, which replaces Task 1's absent inserts:

```rust
    fields.insert(field::JAR, FieldValue::Id(jar.jar));
    fields.insert(field::JAR_SET, FieldValue::Bool(jar.set));
```

Pass the jar choice at every call site:
- `spawn_shared` passes `spawn.carry.jar`;
- the `shared_nodes` Tab arm destructures `jar` and passes `*jar`;
- the Unloaded arm does the same.

The Group arm inserts `field::DEFAULT_JAR, FieldValue::Id(g.default_jar)`.

`update_live_node`:
- Tab arm and Unloaded arm: `if changed(field::JAR) || changed(field::JAR_SET) { tab.jar = jar_choice_of(fields); }`.
- Group arm: `if changed(field::DEFAULT_JAR) { g.default_jar = node_id_of(fields, field::DEFAULT_JAR); }`.

`insert_materialized`:
- Tab arm: `let jar = jar_choice_of(fields);` and `UnloadedTabPayload { url, title, kind, pinned, muted, zoom_factor, state, jar }`.
- Group arm: `GroupPayload { name: str_of(fields, field::NAME), from_json: bool_of(fields, field::FROM_JSON), default_jar: node_id_of(fields, field::DEFAULT_JAR) }`.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p manifold --lib`
Expected: all pass, including the two new tests and `node_menu_config_matches_each_kinds_menu`, `saved_tab_keeps_a_failed_tab` and `every_card_body_draws_a_header`.

- [ ] **Step 6: fmt, clippy, commit**

```
cargo fmt --all
cargo clippy -p manifold -p ipc-protocol --all-targets -- -D warnings
git add -u crates/manifold/src
git commit -m "Jars: tabs carry a jar choice, groups a default jar, both synced" -m "Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 3: `jar_defs`, bridged to the document; create, rename, delete

**Files:**
- Modify: `crates/manifold/src/main.rs` (`JarDef`, `App.jar_defs`, `App::default`)
- Create: `crates/manifold/src/jar_defs.rs` (App methods: `create_jar`, `rename_jar`, `set_jar_color`, `forget_jar`, `delete_jar`, `materialize_jar`). Add `mod jar_defs;` beside the other `mod` lines.
- Modify: `crates/manifold/src/session_sync.rs` (`shared_nodes`, `materialize`, `remove_remotely_deleted`, `apply_remote` sort, `restore_session`, `materialize_imported`)
- Test: `jar_defs.rs` `mod tests`, `session_sync.rs` `mod tests`

**Interfaces:**
- Consumes: Task 1 (`Kind::Jar`, fields), Task 2 (`JarChoice`, `node_id_of`).
- Produces:

```rust
// main.rs
/// A named jar's synced definition (a `Kind::Jar` node). Not a graph node (spec "Data model").
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct JarDef {
    pub(crate) name: String,
    pub(crate) color: Option<egui::Color32>,
    pub(crate) ephemeral: bool,
}
// App field
jar_defs: BTreeMap<NodeId, JarDef>,

// jar_defs.rs, impl App
pub(crate) fn create_jar(&mut self, name: &str) -> NodeId;      // next palette colour, not ephemeral
pub(crate) fn rename_jar(&mut self, id: NodeId, name: &str);    // trimmed; empty is ignored
pub(crate) fn set_jar_ephemeral(&mut self, id: NodeId, on: bool); // Task 4 adds the file side
pub(crate) fn delete_jar(&mut self, id: NodeId);                // local: clears fields naming it, then forget_jar
pub(crate) fn forget_jar(&mut self, id: NodeId);                // removes the def (Task 4: and its cookies/file)
pub(crate) fn materialize_jar(&mut self, id: NodeId, fields: &Fields, seen: &Option<Shared>, now: &Shared);
pub(crate) fn jar_named(&self, name: &str) -> Option<NodeId>;   // exact, case-sensitive
```

- [ ] **Step 1: Write the failing tests**

In `session_sync.rs` `mod tests`:

```rust
    #[test]
    fn a_jar_def_is_saved_as_a_jar_node_and_survives_later_saves() {
        let dir = crate::store::testutil::temp_dir("jar-def-saved");
        let mut app = app_on(&dir.join("session.mnf"), 1);
        add(&mut app, &sleeping_tab("https://a.com/", "A"));
        let jar = app.create_jar("Work");
        app.save_session();
        let doc = app.journal.as_ref().unwrap().doc();
        let node = doc.nodes.get(&jar).filter(|n| n.is_live()).expect("jar node created");
        assert_eq!(node.kind, Kind::Jar);
        assert_eq!(fields_of(node).get(&field::NAME), Some(&FieldValue::Str("Work".into())));
        app.save_session();
        assert!(app.journal.as_ref().unwrap().doc().nodes[&jar].is_live(), "a second save must not delete it");
        assert!(!app.nodes.contains_key(&jar.to_string()), "a jar never enters the graph");
    }

    #[test]
    fn remote_jar_create_rename_and_delete_reach_jar_defs() {
        let jar = id(40);
        let named = |name: &str| shared(Kind::Jar, None, &[(field::NAME, FieldValue::Str(name.into()))]);
        let mut app = app_with(&BTreeMap::from([(jar, named("Home"))]));
        assert_eq!(app.jar_defs[&jar].name, "Home");
        assert!(!app.nodes.contains_key(&jar.to_string()), "never a graph node");
        app.apply_remote(vec![(jar, Some((Kind::Jar, named("House").fields, None)))]);
        assert_eq!(app.jar_defs[&jar].name, "House");
        assert_eq!(app.shared_nodes()[&jar], named("House"), "projects back unchanged");
        app.apply_remote(vec![(jar, None)]);
        assert!(!app.jar_defs.contains_key(&jar));
    }

    #[test]
    fn a_session_with_only_jars_still_restores_them() {
        let dir = crate::store::testutil::temp_dir("jar-only-restore");
        let path = dir.join("session.mnf");
        let mut app = app_on(&path, 1);
        let tab = add(&mut app, &sleeping_tab("https://a.com/", "A"));
        let jar = app.create_jar("Work");
        app.save_session();
        app.nodes.remove(&tab);
        app.save_session();
        drop(app);
        let mut back = app_on(&path, 3);
        assert!(!back.restore_session(), "no graph node: a fresh start");
        assert_eq!(back.jar_defs[&jar].name, "Work", "but the jar is kept, so the next save keeps it");
        back.save_session();
        assert!(back.journal.as_ref().unwrap().doc().nodes[&jar].is_live());
    }

    #[test]
    fn deleting_a_jar_clears_the_tabs_and_groups_that_named_it() {
        let mut app = App::default();
        let jar = app.create_jar("Work");
        let tab = add(&mut app, &sleeping_tab("https://a.com/", "A"));
        let grp = add(&mut app, &NodePayload::Group(GroupPayload { default_jar: Some(jar), ..GroupPayload::named("G") }));
        app.nodes.get_mut(&tab).unwrap().payload.as_unloaded_mut().unwrap().jar =
            crate::jar::JarChoice { jar: Some(jar), set: true };
        app.delete_jar(jar);
        assert!(app.jar_defs.is_empty());
        assert_eq!(app.nodes[&tab].payload.as_unloaded().unwrap().jar.jar, None);
        assert_eq!(app.nodes[&grp].as_group().unwrap().default_jar, None);
    }

    #[test]
    fn opening_a_session_reuses_a_jar_with_the_same_name() {
        let mut app = App::default();
        let work = app.create_jar("Work");
        // A file exported elsewhere: a jar "Work" (label j) and a tab using it.
        let json = serde_json::json!({ "nodes": [
            json_node("j", "jar", serde_json::json!({ "name": "Work" })),
            json_node("t", "tab", serde_json::json!({ "url": "https://a.com/", "jar": "j", "jar_set": true })),
        ]});
        let doc: JsonDoc = serde_json::from_value(json).unwrap();
        app.replace_running_session(&doc);
        assert_eq!(app.jar_defs.len(), 1, "no second Work");
        let tab = app.nodes.values().find_map(|n| n.payload.as_unloaded()).expect("tab imported");
        assert_eq!(tab.jar.jar, Some(work));
    }
```

`json_node` already exists in `session_sync.rs` tests. If its signature differs from `(label, kind, fields_json)`, follow the existing one. If `replace_running_session` needs a journal to commit, use `app_on(temp_dir(...))` instead of `App::default()`.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p manifold --lib -- jar_def deleting_a_jar only_jars reuses_a_jar`
Expected: compile errors (`no method named create_jar`, `no field jar_defs`).

- [ ] **Step 3: Implement `JarDef` and the App methods**

In `main.rs`, add `JarDef` (as under Interfaces), the field `jar_defs: BTreeMap<NodeId, JarDef>,` with a doc comment pointing at the spec section, and `jar_defs: BTreeMap::new(),` in `App::default`.

`crates/manifold/src/jar_defs.rs`:

```rust
//! Named cookie jars as synced definitions (`App::jar_defs`, spec "Data model"). A jar is a `Kind::Jar` store node
//! but never a graph node: `session_sync` bridges these to the document in `shared_nodes`, `materialize` and
//! `remove_remotely_deleted`.

use crate::session_sync::{Fields, Shared};
use crate::store::model::{field, FieldValue, NodeId};
use crate::theme::TAG_COLOR_PALETTE;
use crate::{App, JarDef};

impl App {
    /// A new named jar with the next palette colour. Saved by the caller's `save_session`.
    pub(crate) fn create_jar(&mut self, name: &str) -> NodeId {
        let id: NodeId = self.alloc_node_id().parse().expect("alloc_node_id returns a store id");
        let color = Some(TAG_COLOR_PALETTE[self.jar_defs.len() % TAG_COLOR_PALETTE.len()]);
        self.jar_defs.insert(id, JarDef { name: name.trim().to_string(), color, ephemeral: false });
        id
    }

    pub(crate) fn jar_named(&self, name: &str) -> Option<NodeId> {
        self.jar_defs.iter().find(|(_, d)| d.name == name).map(|(id, _)| *id)
    }

    pub(crate) fn rename_jar(&mut self, id: NodeId, name: &str) {
        let name = name.trim();
        if let (false, Some(def)) = (name.is_empty(), self.jar_defs.get_mut(&id)) {
            def.name = name.to_string();
        }
    }

    pub(crate) fn set_jar_ephemeral(&mut self, id: NodeId, on: bool) {
        if let Some(def) = self.jar_defs.get_mut(&id) {
            def.ephemeral = on;
        }
    }

    /// The user deleted jar `id`: tabs and groups that named it fall back to the default jar (deviation 9), then it is
    /// forgotten. Its document node is deleted by the next save (`diff`: live no longer has it).
    pub(crate) fn delete_jar(&mut self, id: NodeId) {
        for node in self.nodes.values_mut() {
            match &mut node.payload {
                crate::node::NodePayload::Tab(t) if t.jar.jar == Some(id) => t.jar = Default::default(),
                crate::node::NodePayload::Unloaded(t) if t.jar.jar == Some(id) => t.jar = Default::default(),
                crate::node::NodePayload::Group(g) if g.default_jar == Some(id) => g.default_jar = None,
                _ => {}
            }
        }
        self.forget_jar(id);
    }

    /// Drops jar `id`'s definition (a local or remote delete). Task 4 also drops its cookies and file.
    pub(crate) fn forget_jar(&mut self, id: NodeId) {
        self.jar_defs.remove(&id);
    }

    /// A `Kind::Jar` change from the document: applies the fields that differ from what this replica last saw.
    pub(crate) fn materialize_jar(&mut self, id: NodeId, fields: &Fields, seen: &Option<Shared>, now: &Shared) {
        let changed = |no: u32| seen.as_ref().is_none_or(|s| s.fields.get(&no) != now.fields.get(&no));
        let def = self.jar_defs.entry(id).or_insert_with(|| JarDef { name: String::new(), color: None, ephemeral: false });
        if changed(field::NAME) {
            def.name = match fields.get(&field::NAME) {
                Some(FieldValue::Str(s)) => s.clone(),
                _ => String::new(),
            };
        }
        if changed(field::COLOR) {
            def.color = fields.get(&field::COLOR).and_then(crate::session_sync::color_of);
        }
        if changed(field::EPHEMERAL) {
            let on = matches!(fields.get(&field::EPHEMERAL), Some(FieldValue::Bool(true)));
            self.set_jar_ephemeral(id, on);
        }
    }
}
```

Make `Fields`, `Shared` and `color_of` `pub(crate)` in `session_sync.rs` if they are not already.

- [ ] **Step 4: Implement the bridge in `session_sync.rs`**

`shared_nodes`, after the graph loop:

```rust
        // Jar definitions are not graph nodes but must be live, or `diff` deletes their document nodes.
        for (id, def) in &self.jar_defs {
            let fields = Fields::from([
                (field::COLOR, color_value(def.color)),
                (field::NAME, str(&def.name)),
                (field::EPHEMERAL, FieldValue::Bool(def.ephemeral)),
            ]);
            debug_assert_eq!(fields.keys().copied().collect::<Vec<_>>(), kind_fields(Kind::Jar));
            out.insert(*id, Shared { kind: Kind::Jar, group: None, fields });
        }
```

`materialize`, as the first statement:

```rust
        if kind == Kind::Jar {
            let Ok(jar) = id.parse::<NodeId>() else { return };
            let now = Shared { kind, group: None, fields: fields.clone() };
            let seen = self.last_shared.insert(id, now.clone());
            self.materialize_jar(jar, fields, &seen, &now);
            return;
        }
```

`remove_remotely_deleted`, as the first statement:

```rust
        if let Some(jar) = id.parse::<NodeId>().ok().filter(|j| self.jar_defs.contains_key(j)) {
            self.forget_jar(jar);
            return;
        }
```

`apply_remote`: change the sort key so jars come first, then groups. A tab naming a jar created in the same batch then resolves at once:

```rust
        changes.sort_by_key(|(_, c)| match c {
            Some((Kind::Jar, ..)) => 0,
            Some((Kind::Group, ..)) => 1,
            _ => 2,
        });
```

`restore_session` (deviation 6). Replace the emptiness check and the sort:

```rust
        let toasts = self.save_toast_queue.len();
        // Jars first: they are not graph nodes, so they survive a "fresh start" and the next save keeps them.
        for (id, kind, fields, _) in nodes.iter().filter(|(_, kind, ..)| *kind == Kind::Jar) {
            self.materialize(id.to_string(), *kind, fields, None, None);
        }
        nodes.retain(|(_, kind, ..)| *kind != Kind::Jar);
        if nodes.is_empty() {
            self.restore_closed(local.closed);
            self.local_written = Some(self.local_state());
            return false;
        }
        nodes.sort_by_key(|(_, kind, ..)| *kind != Kind::Group);
```

Delete the later `let toasts = ...` line that this replaces. `collapse_toasts_since(toasts)` stays where it is.

`teardown_running_session` must **not** clear `jar_defs`. Add a one-line comment saying so beside the `last_shared.clear()` line. `last_shared` entries for jars are rebuilt by the next `materialize`, and a missing entry only means "all fields changed".

`materialize_imported` (deviation 4). Before the main loop, build the alias map and drop matched jars:

```rust
        // A jar named like an existing one is that jar (deviation 4): its tabs and groups point at the existing id.
        let mut alias: BTreeMap<NodeId, NodeId> = BTreeMap::new();
        imported.retain(|i| {
            if i.kind != Kind::Jar {
                return true;
            }
            let name = match i.fields.get(&field::NAME) {
                Some(FieldValue::Str(s)) => s.as_str(),
                _ => "",
            };
            match self.jar_named(name) {
                Some(existing) => {
                    alias.insert(i.id, existing);
                    false
                }
                None => true,
            }
        });
        imported.sort_by_key(|i| match i.kind {
            Kind::Jar => 0,
            Kind::Group => 1,
            _ => 2,
        });
```

(This replaces the existing `imported.sort_by_key(|i| i.kind != Kind::Group);`. Make `imported` `mut` before the `retain`.) Inside the loop, after `imported_fields`:

```rust
            for no in [field::JAR, field::DEFAULT_JAR] {
                if let Some(FieldValue::Id(Some(j))) = fields.get_mut(&no) {
                    if let Some(existing) = alias.get(j) {
                        *j = *existing;
                    }
                }
            }
```

A jar has no `GROUP` field and no `POS`. `materialize`'s jar route ignores the computed `group`.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p manifold --lib`
Expected: all pass. The idempotency tests (a second save writes nothing) must still pass. A jar's `Shared` matches the document exactly once it has been saved.

- [ ] **Step 6: fmt, clippy, commit**

```
cargo fmt --all
cargo clippy -p manifold -p ipc-protocol --all-targets -- -D warnings
git add crates/manifold/src/jar_defs.rs crates/manifold/src/main.rs crates/manifold/src/session_sync.rs
git commit -m "Jars: named jar definitions bridged to the document" -m "Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 4: One jar per key — per-file I/O, lazy loads, per-spawn pulls, ephemeral jars

**Files:**
- Modify: `crates/manifold/src/jar.rs` (keyed file names, `Jar::delete_file`)
- Modify: `crates/manifold/src/main.rs` (`App.jars`, `App.jar_snapshots`, `App.jar_tabs` value type, `TabSpawnOutcome.jar`, `App::default`, existing jar tests)
- Modify: `crates/manifold/src/tabs.rs` (`spawn_tab_async`, `apply_pulled_cookies`, `handle_tab_spawn_result`)
- Modify: `crates/manifold/src/jar_defs.rs` (`resolved_jar`, `ensure_jar_loaded`, `save_jar`, `forget_jar`, `set_jar_ephemeral`, `clear_jar_cookies`)
- Modify: `crates/manifold/src/session_sync.rs` (`fail_spawns` helper)

**Interfaces:**
- Consumes: Tasks 2–3.
- Produces:

```rust
// jar.rs
impl JarKey {
    /// `default` or the node id (`{actor:016x}-{counter}`): safe as a file name on every OS.
    pub fn file_stem(&self) -> String;
}
impl Jar {
    pub fn path(dir: &Path, key: JarKey) -> PathBuf;              // <dir>\<stem>.json
    pub fn load(dir: &Path, key: JarKey) -> Jar;
    pub fn save(&self, dir: &Path, key: JarKey) -> std::io::Result<()>;
    pub fn delete_file(dir: &Path, key: JarKey) -> std::io::Result<()>; // NotFound is Ok
}
// main.rs
jars: BTreeMap<jar::JarKey, jar::Jar>,
jar_snapshots: Arc<std::sync::Mutex<BTreeMap<jar::JarKey, jar::Jar>>>,
jar_tabs: HashMap<String, (jar::JarKey, jar::TabCookies)>,
struct TabSpawnOutcome { spawn, generation, result, seeded, jar: jar::JarKey }
// jar_defs.rs, impl App
pub(crate) fn resolved_jar(&self, choice: &JarChoice) -> (JarKey, bool /* missing */);
pub(crate) fn ensure_jar_loaded(&mut self, key: JarKey);
fn save_jar(&self, key: JarKey);                                 // logs "jar: could not save ..." without values
pub(crate) fn clear_jar_cookies(&mut self, key: JarKey);        // Task 6 adds the switch of live tabs
```

- [ ] **Step 1: Write the failing tests**

In `jar.rs` `mod tests`. All existing file tests pass `JarKey::Default` where they called `path(&dir)`, `load(&dir)` or `save(&dir)`. `asides()` and the tmp tests keep their `default.json...` names, which proves the default jar's names are unchanged. New tests:

```rust
    #[test]
    fn a_named_jar_has_its_own_file_beside_the_default() {
        let dir = temp_dir("jar-named-file");
        let named = JarKey::Named(NodeId { actor: ActorId(0xab), counter: 3 });
        assert_eq!(Jar::path(&dir, JarKey::Default), dir.join("default.json"));
        assert_eq!(Jar::path(&dir, named), dir.join("00000000000000ab-3.json"));
        let a = Jar { locked: false, cookies: vec![c("sid", "alice")] };
        let b = Jar { locked: false, cookies: vec![c("sid", "bob")] };
        a.save(&dir, JarKey::Default).unwrap();
        b.save(&dir, named).unwrap();
        assert_eq!(Jar::load(&dir, JarKey::Default), a);
        assert_eq!(Jar::load(&dir, named), b);
        Jar::delete_file(&dir, named).unwrap();
        assert!(!Jar::path(&dir, named).exists());
        Jar::delete_file(&dir, named).unwrap(); // already gone: fine
        assert_eq!(Jar::load(&dir, JarKey::Default), a, "the default jar is untouched");
    }

    #[test]
    fn an_unreadable_named_jar_goes_aside_under_its_own_name() {
        let dir = temp_dir("jar-named-aside");
        let named = JarKey::Named(NodeId { actor: ActorId(1), counter: 2 });
        std::fs::write(Jar::path(&dir, named), "garbage").unwrap();
        assert!(Jar::load(&dir, named).cookies.is_empty());
        let names: Vec<String> =
            std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        assert!(names.iter().any(|n| n.starts_with("0000000000000001-2.json.unreadable-")), "{names:?}");
        assert!(asides(&dir).is_empty(), "no default.json aside");
    }
```

(Import `crate::store::model::{ActorId, NodeId}` in the test module.)

In `main.rs` `mod tests`, first update the existing jar tests. They now give each tab an entry, because pulls without one are dropped (deviation 5):

```rust
    fn on_default(app: &mut App, tab: &str) {
        app.jar_tabs.insert(tab.into(), (jar::JarKey::Default, jar::TabCookies::default()));
    }
    fn default_jar(app: &App) -> &jar::Jar {
        &app.jars[&jar::JarKey::Default]
    }
```

Then:
- `pulled_cookies_reach_the_jar_and_a_logout_removes_them` starts with `on_default(&mut app, "a")`;
- `a_tab_that_never_saw_a_cookie_cannot_delete_it` calls `on_default` for "a" and "b";
- `a_seeded_tab_deletes...` calls `on_default(&mut app, "a")` and inserts `("b", (JarKey::Default, TabCookies::seeded(&default_jar(&app).cookies)))`, and reads `app.jar_tabs["b"].1.reported`;
- every `app.jar.cookies` becomes `default_jar(&app).cookies`;
- `closing_a_tab_clears_its_cookie_bookkeeping` inserts `(JarKey::Default, TabCookies::default())`.

New tests:

```rust
    #[test]
    fn a_pull_merges_into_the_jar_the_tab_was_spawned_with() {
        let mut app = App::default();
        let work = app.create_jar("Work");
        app.ensure_jar_loaded(jar::JarKey::Named(work));
        app.jar_tabs.insert("a".into(), (jar::JarKey::Named(work), jar::TabCookies::default()));
        assert!(app.apply_pulled_cookies("a", SID));
        assert_eq!(app.jars[&jar::JarKey::Named(work)].cookies.len(), 1);
        assert!(app.jars.get(&jar::JarKey::Default).is_none_or(|j| j.cookies.is_empty()), "default untouched");
    }

    #[test]
    fn a_pull_with_no_entry_or_a_deleted_jar_is_dropped() {
        let mut app = App::default();
        assert!(!app.apply_pulled_cookies("nobody", SID), "no jar_tabs entry: dropped (deviation 5)");
        let work = app.create_jar("Work");
        app.jar_tabs.insert("a".into(), (jar::JarKey::Named(work), jar::TabCookies::default()));
        app.forget_jar(work);
        assert!(!app.apply_pulled_cookies("a", SID), "its jar was deleted while it ran: dropped");
        assert!(app.jars.values().all(|j| j.cookies.is_empty()));
    }

    #[test]
    fn resolved_jar_falls_back_to_default_and_flags_a_missing_jar() {
        let mut app = App::default();
        let work = app.create_jar("Work");
        let gone = NodeId { actor: store::model::ActorId(5), counter: 5 };
        let pick = |jar| jar::JarChoice { jar, set: true };
        assert_eq!(app.resolved_jar(&pick(None)), (jar::JarKey::Default, false));
        assert_eq!(app.resolved_jar(&pick(Some(work))), (jar::JarKey::Named(work), false));
        assert_eq!(app.resolved_jar(&pick(Some(gone))), (jar::JarKey::Default, true));
    }

    #[test]
    fn an_ephemeral_jar_never_writes_a_file() {
        let dir = store::testutil::temp_dir("jar-ephemeral");
        let mut app = App { jars_dir_override: Some(dir.clone()), ..App::default() };
        let tmp = app.create_jar("Throwaway");
        app.set_jar_ephemeral(tmp, true);
        app.ensure_jar_loaded(jar::JarKey::Named(tmp));
        app.jar_tabs.insert("a".into(), (jar::JarKey::Named(tmp), jar::TabCookies::default()));
        assert!(app.apply_pulled_cookies("a", SID));
        assert!(!jar::Jar::path(&dir, jar::JarKey::Named(tmp)).exists());
        app.set_jar_ephemeral(tmp, false);
        assert!(jar::Jar::path(&dir, jar::JarKey::Named(tmp)).exists(), "turned off: saved (deviation 10)");
        app.set_jar_ephemeral(tmp, true);
        assert!(!jar::Jar::path(&dir, jar::JarKey::Named(tmp)).exists(), "turned on: file deleted");
        assert_eq!(app.jars[&jar::JarKey::Named(tmp)].cookies.len(), 1, "cookies kept in memory until quit");
    }

    #[test]
    fn deleting_a_jar_deletes_its_file_and_clearing_empties_it() {
        let dir = store::testutil::temp_dir("jar-delete-clear");
        let mut app = App { jars_dir_override: Some(dir.clone()), ..App::default() };
        let work = app.create_jar("Work");
        let key = jar::JarKey::Named(work);
        app.ensure_jar_loaded(key);
        app.jar_tabs.insert("a".into(), (key, jar::TabCookies::default()));
        app.apply_pulled_cookies("a", SID);
        assert!(jar::Jar::path(&dir, key).exists());
        app.clear_jar_cookies(key);
        assert!(app.jars[&key].cookies.is_empty());
        assert!(jar::Jar::load(&dir, key).cookies.is_empty(), "cleared on disk too");
        app.delete_jar(work);
        assert!(!jar::Jar::path(&dir, key).exists());
        assert!(!app.jars.contains_key(&key));
    }
```

**Test seam:** `jars_dir_override: Option<PathBuf>` is a new `#[cfg(test)]` App field, `None` by default. It is read by one helper:

```rust
    /// `profile::jars_dir()`, or a test's temp dir. Never the real profile under test (`profile::dir()` is None).
    pub(crate) fn jars_dir(&self) -> Option<std::path::PathBuf> {
        #[cfg(test)]
        if let Some(dir) = &self.jars_dir_override {
            return Some(dir.clone());
        }
        crate::profile::jars_dir()
    }
```

Every jar file path in App code goes through `self.jars_dir()`. The name contains no "APPDATA", so `profile::tests` stays green.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p manifold --lib -- jar`
Expected: compile errors (`this function takes 2 arguments`, `no field jars`).

- [ ] **Step 3: Implement keyed files in `jar.rs`**

```rust
impl JarKey {
    pub fn file_stem(&self) -> String {
        match self {
            JarKey::Default => "default".to_string(),
            JarKey::Named(id) => id.to_string(),
        }
    }
    fn file_name(&self) -> String {
        format!("{}.json", self.file_stem())
    }
}
```

Thread `file: &str` (the `file_name()`) through the helpers:
- `remove_dead_tmps(dir, file)` uses `n.strip_prefix(file)?.strip_prefix('.')?.strip_suffix(".tmp")?`;
- `asides_in(dir, file)` uses `name.strip_prefix(file)?.strip_prefix(".unreadable-")?`;
- `prune_asides(dir, file)`;
- `unique_aside(dir, file)` uses `format!("{file}.unreadable-{secs}")` and `format!("{file}.unreadable-{secs}-{n}")`.

`Jar::path(dir, key) = dir.join(key.file_name())`. `load(dir, key)` and `save(&self, dir, key)` pass `&key.file_name()` down. The temp file is `dir.join(format!("{}.{}.tmp", key.file_name(), std::process::id()))`. Update the doc comments ("`<dir>\<key>.json`; `default.json` for the default jar"). Add:

```rust
    /// Removes key `key`'s file (a deleted or newly ephemeral jar). A file already gone is fine.
    pub fn delete_file(dir: &Path, key: JarKey) -> std::io::Result<()> {
        match std::fs::remove_file(Self::path(dir, key)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
```

A `"default.json".strip_prefix("0000...-3.json")` never matches, so each jar's helpers touch only that jar's own files. The `an_unreadable_named_jar_goes_aside...` test checks this.

- [ ] **Step 4: Implement the App side**

`main.rs`:
- Replace `jar: jar::Jar` with `jars: BTreeMap<jar::JarKey, jar::Jar>`, and `jar_snapshot` with `jar_snapshots: Arc<std::sync::Mutex<BTreeMap<jar::JarKey, jar::Jar>>>`.
- Change `jar_tabs` to `HashMap<String, (jar::JarKey, jar::TabCookies)>`. Its doc says: "the key is the jar the host was **spawned with** (spec "Live tabs when `jar` changes")".
- Add `jar: jar::JarKey` to `TabSpawnOutcome`, and `#[cfg(test)] jars_dir_override: Option<PathBuf>`.
- In `App::default`:

```rust
        let jars: BTreeMap<jar::JarKey, jar::Jar> = profile::jars_dir()
            .map(|dir| (jar::JarKey::Default, jar::Jar::load(&dir, jar::JarKey::Default)))
            .into_iter()
            .collect();
        let jar_snapshots = Arc::new(std::sync::Mutex::new(jars.clone()));
```

`jar_defs.rs`:

```rust
    /// The only place a `jar` field becomes a key (spec "Resolution"). `true` when it names a jar that no longer
    /// exists (deleted, or not synced yet): the tab uses the default jar and its chip warns.
    pub(crate) fn resolved_jar(&self, choice: &JarChoice) -> (JarKey, bool) {
        match choice.jar {
            None => (JarKey::Default, false),
            Some(id) if self.jar_defs.contains_key(&id) => (JarKey::Named(id), false),
            Some(_) => (JarKey::Default, true),
        }
    }

    /// Loads `key`'s jar the first time a tab needs it (spec "Manifold state": named jars load lazily). An ephemeral
    /// jar starts empty and is never read from disk.
    pub(crate) fn ensure_jar_loaded(&mut self, key: JarKey) {
        if self.jars.contains_key(&key) {
            return;
        }
        let ephemeral = matches!(key, JarKey::Named(id) if self.jar_defs.get(&id).is_some_and(|d| d.ephemeral));
        let jar = match self.jars_dir() {
            Some(dir) if !ephemeral => Jar::load(&dir, key),
            _ => Jar::default(),
        };
        self.publish_jar(key, jar);
    }

    /// Stores `jar` under `key` and copies it to the spawn threads' snapshot map.
    fn publish_jar(&mut self, key: JarKey, jar: Jar) {
        if let Ok(mut snapshots) = self.jar_snapshots.lock() {
            snapshots.insert(key, jar.clone());
        }
        self.jars.insert(key, jar);
    }

    fn is_ephemeral(&self, key: JarKey) -> bool {
        matches!(key, JarKey::Named(id) if self.jar_defs.get(&id).is_some_and(|d| d.ephemeral))
    }

    /// Writes `key`'s file unless it is ephemeral. A failure is logged (never cookie values) and leaves other jars alone.
    pub(crate) fn save_jar(&self, key: JarKey) {
        let (Some(dir), Some(jar)) = (self.jars_dir(), self.jars.get(&key)) else { return };
        if self.is_ephemeral(key) {
            return;
        }
        if let Err(e) = jar.save(&dir, key) {
            eprintln!("jar: could not save {}: {e}", key.file_stem());
        }
    }

    /// Empties `key`'s jar and saves it. Task 6 also switches the live tabs using it (their engines still hold the
    /// old cookies).
    pub(crate) fn clear_jar_cookies(&mut self, key: JarKey) {
        self.ensure_jar_loaded(key);
        let locked = self.jars.get(&key).is_some_and(|j| j.locked);
        self.publish_jar(key, Jar { locked, cookies: Vec::new() });
        self.save_jar(key);
    }
```

`set_jar_ephemeral` gains the file side (deviation 10):

```rust
    pub(crate) fn set_jar_ephemeral(&mut self, id: NodeId, on: bool) {
        let Some(def) = self.jar_defs.get_mut(&id) else { return };
        if def.ephemeral == on {
            return;
        }
        def.ephemeral = on;
        let key = JarKey::Named(id);
        if on {
            if let Some(dir) = self.jars_dir() {
                if let Err(e) = Jar::delete_file(&dir, key) {
                    eprintln!("jar: could not delete {}: {e}", key.file_stem());
                }
            }
        } else if self.jars.contains_key(&key) {
            self.save_jar(key);
        }
    }
```

`forget_jar` gains:

```rust
        let key = JarKey::Named(id);
        self.jars.remove(&key);
        if let Ok(mut snapshots) = self.jar_snapshots.lock() {
            snapshots.remove(&key);
        }
        if let Some(dir) = self.jars_dir() {
            if let Err(e) = Jar::delete_file(&dir, key) {
                eprintln!("jar: could not delete {}: {e}", key.file_stem());
            }
        }
```

`tabs.rs` `spawn_tab_async`:

```rust
        let (jar, _) = self.resolved_jar(&spawn.carry.jar);
        self.ensure_jar_loaded(jar);
        // ... existing lines ...
        let snapshots = self.jar_snapshots.clone();
        std::thread::spawn(move || {
            let seeded = std::cell::RefCell::new(crate::jar::TabCookies::default());
            let seed = || {
                let cookies =
                    snapshots.lock().ok().and_then(|s| s.get(&jar).map(|j| j.cookies.clone())).unwrap_or_default();
                *seeded.borrow_mut() = crate::jar::TabCookies::seeded(&cookies);
                serde_json::to_string(&cookies).unwrap_or_else(|_| "[]".to_owned())
            };
            // ... spawn_tab unchanged ...
            let _ = proxy.send_event(UserEvent::TabSpawned(Box::new(TabSpawnOutcome {
                spawn, generation, result, seeded, jar,
            })));
        });
```

The `#[cfg(test)]` early return pushes to `test_spawns` before the thread, so the resolve and load go above it. That lets tests see `ensure_jar_loaded` happen.

`handle_tab_spawn_result` destructures `jar`. The Ok branch does `self.jar_tabs.insert(id.clone(), (jar, seeded));`.

`apply_pulled_cookies`:

```rust
    /// Folds tab `tab_id`'s cookie dump into the jar its host was spawned with, and saves that jar's file when it
    /// changed (spec: "Its pulls merge into that jar and save only that jar's file"). `false` for a dump that does not
    /// parse, a tab with no `jar_tabs` entry (closed or replaced since it was asked; deviation 5), or a named jar
    /// deleted while it ran.
    pub(crate) fn apply_pulled_cookies(&mut self, tab_id: &str, json: &str) -> bool {
        let Some(pulled) = crate::jar::from_json(json) else { return false };
        let Some((key, _)) = self.jar_tabs.get(tab_id) else { return false };
        let key = *key;
        if matches!(key, JarKey::Named(id) if !self.jar_defs.contains_key(&id)) {
            return false;
        }
        self.ensure_jar_loaded(key);
        let (Some(jar), Some((_, tab))) = (self.jars.get_mut(&key), self.jar_tabs.get_mut(tab_id)) else {
            return false;
        };
        let changed = jar.merge(pulled, tab);
        if changed {
            let jar = jar.clone();
            if let Ok(mut snapshots) = self.jar_snapshots.lock() {
                snapshots.insert(key, jar);
            }
            self.save_jar(key);
        }
        changed
    }
```

`session_sync.rs` `fail_spawns` adds `jar: Default::default()` to its `TabSpawnOutcome`.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p manifold --lib`
Expected: all pass, including `profile::tests`.

- [ ] **Step 6: fmt, clippy, commit**

```
cargo fmt --all
cargo clippy -p manifold -p ipc-protocol --all-targets -- -D warnings
git add -u crates/manifold/src
git commit -m "Jars: one jar per key with its own file; pulls merge into the spawn jar" -m "Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Inheritance and the group-default cascade

**Files:**
- Modify: `crates/manifold/src/jar_defs.rs` (`inherited_jar`, `new_tab_jar`, `set_tab_jar`, `follow_group_jar`, `set_group_default_jar`, `clear_group_default_jar`, `cascade_count`, `refollow`)
- Modify: `crates/manifold/src/tabs.rs` (`spawn_new_tab_with_kind` gains `jar: Option<JarChoice>`, `duplicate_tab`, `start_split`, `spawn_tab_at`, new `spawn_tab_from`)
- Modify: `crates/manifold/src/event_loop.rs` (1640, `OpenTabRequested`)
- Modify: `crates/manifold/src/node_menu.rs` (421, page link "Open in new tab")
- Modify: `crates/manifold/src/graph_nav.rs` (`apply_drop`, `eject`, `merge_groups`), `crates/manifold/src/main.rs` (`dissolve_group`)

**Interfaces:**
- Consumes: Tasks 2–4.
- Produces (all `impl App`, `pub(crate)`):

```rust
fn inherited_jar(&self, group: Option<&str>) -> Option<NodeId>; // nearest group default walking up
fn new_tab_jar(&self, group: Option<&str>) -> JarChoice;          // { jar: inherited_jar(group), set: false }
fn tab_jar(&self, id: &str) -> Option<JarChoice>;                 // Tab or Unloaded payload's choice
fn set_tab_jar(&mut self, id: &str, jar: Option<NodeId>);        // set = true
fn follow_group_jar(&mut self, id: &str);                         // set = false, re-snapshot
fn refollow(&mut self, id: &str);                                 // unset tabs at id (a tab or a group's subtree)
fn set_group_default_jar(&mut self, group: &str, jar: NodeId) -> usize; // returns tabs changed
fn clear_group_default_jar(&mut self, group: &str);
fn cascade_count(&self, group: &str, jar: NodeId) -> usize;
fn spawn_tab_from(&mut self, opener: &str, url: &str) -> String;
fn spawn_new_tab_with_kind(&mut self, parent, group, url, kind, purpose, jar: Option<JarChoice>) -> String;
// None: inherit from `group`
```

The cascade helper (private):

```rust
/// Whether `group` governs tab-or-group `at`'s jar: `group` is `at`'s group or an ancestor, and no group between
/// them has a default of its own (spec: "nested groups with no default of their own included").
fn governed_by(&self, at: &str, group: &str) -> bool;
```

- [ ] **Step 1: Write the failing tests**

In `jar_defs.rs` `mod tests`. Use `crate::session_sync::tests::add` and `sleeping_tab`, both already `pub(crate)`:

```rust
    use crate::jar::JarChoice;
    use crate::node::{GroupPayload, NodePayload};
    use crate::session_sync::tests::{add, sleeping_tab};
    use crate::App;

    fn tab_in(app: &mut App, group: Option<&str>) -> String {
        let id = add(app, &sleeping_tab("https://a.com/", "A"));
        app.nodes.get_mut(&id).unwrap().group = group.map(str::to_string);
        id
    }
    fn group_in(app: &mut App, parent: Option<&str>) -> String {
        let id = add(app, &NodePayload::Group(GroupPayload::named("G")));
        app.nodes.get_mut(&id).unwrap().group = parent.map(str::to_string);
        id
    }
    fn jar_of(app: &App, tab: &str) -> JarChoice {
        app.tab_jar(tab).unwrap()
    }

    #[test]
    fn a_new_tab_snapshots_the_nearest_group_default() {
        let mut app = App::default();
        let work = app.create_jar("Work");
        let outer = group_in(&mut app, None);
        let inner = group_in(&mut app, Some(&outer));
        assert_eq!(app.new_tab_jar(Some(&inner)), JarChoice::default(), "no default anywhere: no jar");
        app.set_group_default_jar(&outer, work);
        assert_eq!(app.new_tab_jar(Some(&inner)), JarChoice { jar: Some(work), set: false });
        assert_eq!(app.new_tab_jar(None), JarChoice::default());
    }

    #[test]
    fn a_cascade_reaches_unset_tabs_in_nested_groups_without_their_own_default() {
        let mut app = App::default();
        let (work, home) = (app.create_jar("Work"), app.create_jar("Home"));
        let outer = group_in(&mut app, None);
        let plain = group_in(&mut app, Some(&outer)); // no default of its own
        let own = group_in(&mut app, Some(&outer));
        app.set_group_default_jar(&own, home);
        let a = tab_in(&mut app, Some(&outer));
        let b = tab_in(&mut app, Some(&plain));
        let c = tab_in(&mut app, Some(&own));
        let picked = tab_in(&mut app, Some(&outer));
        app.set_tab_jar(&picked, None);
        assert_eq!(app.cascade_count(&outer, work), 2);
        assert_eq!(app.set_group_default_jar(&outer, work), 2);
        assert_eq!(jar_of(&app, &a), JarChoice { jar: Some(work), set: false });
        assert_eq!(jar_of(&app, &b), JarChoice { jar: Some(work), set: false });
        assert_eq!(jar_of(&app, &c).jar, Some(home), "a nested group with its own default is skipped");
        assert_eq!(jar_of(&app, &picked), JarChoice { jar: None, set: true }, "a hand-picked tab is skipped");
        app.clear_group_default_jar(&outer);
        assert_eq!(jar_of(&app, &a).jar, Some(work), "clearing the default leaves tabs alone");
    }

    #[test]
    fn moving_an_unset_tab_re_snapshots_and_a_set_tab_keeps_its_jar() {
        let mut app = App::default();
        let work = app.create_jar("Work");
        let g = group_in(&mut app, None);
        app.set_group_default_jar(&g, work);
        let unset = tab_in(&mut app, None);
        let set = tab_in(&mut app, None);
        app.set_tab_jar(&set, None);
        for t in [&unset, &set] {
            app.nodes.get_mut(t).unwrap().group = Some(g.clone());
            app.refollow(t);
        }
        assert_eq!(jar_of(&app, &unset).jar, Some(work));
        assert_eq!(jar_of(&app, &set), JarChoice { jar: None, set: true });
        app.nodes.get_mut(&unset).unwrap().group = None;
        app.refollow(&unset);
        assert_eq!(jar_of(&app, &unset), JarChoice::default(), "moved out: no default, no jar");
    }

    #[test]
    fn follow_group_jar_clears_set_and_re_snapshots() {
        let mut app = App::default();
        let work = app.create_jar("Work");
        let g = group_in(&mut app, None);
        app.set_group_default_jar(&g, work);
        let t = tab_in(&mut app, Some(&g));
        app.set_tab_jar(&t, None);
        app.follow_group_jar(&t);
        assert_eq!(jar_of(&app, &t), JarChoice { jar: Some(work), set: false });
    }

    #[test]
    fn a_new_tab_inherits_and_an_opened_tab_copies_its_opener() {
        let mut app = App::default();
        let (work, home) = (app.create_jar("Work"), app.create_jar("Home"));
        let g = group_in(&mut app, None);
        app.set_group_default_jar(&g, work);
        app.spawn_new_tab(None, Some(g.clone()), "https://new/");
        let (spawn, _) = app.test_spawns.last().unwrap();
        assert_eq!(spawn.carry.jar, JarChoice { jar: Some(work), set: false });
        let opener = tab_in(&mut app, Some(&g));
        app.set_tab_jar(&opener, Some(home));
        app.spawn_tab_from(&opener, "https://opened/");
        let (spawn, _) = app.test_spawns.last().unwrap();
        assert_eq!(spawn.carry.jar, JarChoice { jar: Some(home), set: true }, "deviation 3");
    }
```

`test_spawns` holds `(TabSpawn, u64)` and `TabSpawn.carry` is private to `main.rs`. Make `carry` and `TabCarry.jar` `pub(crate)` so these tests can read them.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p manifold --lib -- jar_defs::tests`
Expected: compile errors (`no method named set_group_default_jar`).

- [ ] **Step 3: Implement**

`jar_defs.rs`:

```rust
    /// The nearest default jar walking up from `group` (spec "Inheritance").
    pub(crate) fn inherited_jar(&self, group: Option<&str>) -> Option<NodeId> {
        let mut at = group.map(str::to_string);
        for _ in 0..=self.nodes.len() {
            let node = self.nodes.get(at.as_deref()?)?;
            if let Some(jar) = node.as_group().and_then(|g| g.default_jar) {
                return Some(jar);
            }
            at = node.group.clone();
        }
        None // a group cycle (never built, but the loop must end)
    }

    pub(crate) fn new_tab_jar(&self, group: Option<&str>) -> JarChoice {
        JarChoice { jar: self.inherited_jar(group), set: false }
    }

    pub(crate) fn tab_jar(&self, id: &str) -> Option<JarChoice> {
        match &self.nodes.get(id)?.payload {
            NodePayload::Tab(t) => Some(t.jar),
            NodePayload::Unloaded(t) => Some(t.jar),
            _ => None,
        }
    }

    fn put_tab_jar(&mut self, id: &str, choice: JarChoice) -> bool {
        let slot = match self.nodes.get_mut(id).map(|n| &mut n.payload) {
            Some(NodePayload::Tab(t)) => &mut t.jar,
            Some(NodePayload::Unloaded(t)) => &mut t.jar,
            _ => return false,
        };
        let changed = *slot != choice;
        *slot = choice;
        changed
    }

    pub(crate) fn set_tab_jar(&mut self, id: &str, jar: Option<NodeId>) {
        self.put_tab_jar(id, JarChoice { jar, set: true });
    }

    pub(crate) fn follow_group_jar(&mut self, id: &str) {
        let group = self.nodes.get(id).and_then(|n| n.group.clone());
        self.put_tab_jar(id, self.new_tab_jar(group.as_deref()));
    }

    /// Re-snapshots every unset tab at `id` (a tab, or each tab in a group's subtree) from its new place. Called after
    /// a move between groups (deviation 12).
    pub(crate) fn refollow(&mut self, id: &str) {
        let tabs: Vec<String> = self
            .nodes
            .iter()
            .filter(|(t, n)| n.payload.saved_tab().is_some() && (t.as_str() == id || self.is_inside(t, id)))
            .map(|(t, _)| t.clone())
            .collect();
        for t in tabs {
            if self.tab_jar(&t).is_some_and(|c| !c.set) {
                self.follow_group_jar(&t);
            }
        }
    }

    /// Whether `at` sits somewhere inside group `group`.
    fn is_inside(&self, at: &str, group: &str) -> bool {
        let mut cur = self.nodes.get(at).and_then(|n| n.group.clone());
        for _ in 0..=self.nodes.len() {
            match cur {
                Some(g) if g == group => return true,
                Some(g) => cur = self.nodes.get(&g).and_then(|n| n.group.clone()),
                None => return false,
            }
        }
        false
    }

    fn governed_by(&self, at: &str, group: &str) -> bool {
        let mut cur = self.nodes.get(at).and_then(|n| n.group.clone());
        for _ in 0..=self.nodes.len() {
            let Some(g) = cur else { return false };
            if g == group {
                return true;
            }
            let node = self.nodes.get(&g);
            if node.and_then(|n| n.as_group()).is_some_and(|gp| gp.default_jar.is_some()) {
                return false; // a nearer default wins
            }
            cur = node.and_then(|n| n.group.clone());
        }
        false
    }

    fn cascade_targets(&self, group: &str, jar: NodeId) -> Vec<String> {
        self.nodes
            .keys()
            .filter(|t| self.tab_jar(t).is_some_and(|c| !c.set && c.jar != Some(jar)))
            .filter(|t| self.governed_by(t, group))
            .cloned()
            .collect()
    }

    pub(crate) fn cascade_count(&self, group: &str, jar: NodeId) -> usize {
        self.cascade_targets(group, jar).len()
    }

    /// Writes `group`'s default, then the new jar into every unset tab it governs (spec "Changing a group's
    /// `default_jar`"). Plain field edits; the next save writes them.
    pub(crate) fn set_group_default_jar(&mut self, group: &str, jar: NodeId) -> usize {
        let Some(g) = self.nodes.get_mut(group).and_then(|n| n.as_group_mut()) else { return 0 };
        g.default_jar = Some(jar);
        let targets = self.cascade_targets(group, jar);
        for t in &targets {
            self.put_tab_jar(t, JarChoice { jar: Some(jar), set: false });
        }
        targets.len()
    }

    pub(crate) fn clear_group_default_jar(&mut self, group: &str) {
        if let Some(g) = self.nodes.get_mut(group).and_then(|n| n.as_group_mut()) {
            g.default_jar = None;
        }
    }
```

`set_group_default_jar` computes the targets *after* writing the default, so `governed_by` sees the group's own new default. That is harmless: the check stops only at groups strictly between the tab and `group`.

`tabs.rs`:
- `spawn_new_tab_with_kind` gains a last parameter `jar: Option<crate::jar::JarChoice>`. Before `spawn_tab_async`:

  ```rust
          let jar = jar.unwrap_or_else(|| self.new_tab_jar(group.as_deref()));
          let mut spawn = TabSpawn { parent, group, ..spawn };
          spawn.carry.jar = jar;
          self.spawn_tab_async(spawn);
  ```

- `spawn_new_tab` passes `None`.
- `duplicate_tab` reads `self.tab_jar(id)` and passes it.
- `start_split` passes `self.tab_jar(&focused_id)`.
- `spawn_tab_at` sets `spawn.carry.jar = self.new_tab_jar(group.as_deref())`.
- Add:

```rust
    /// A tab opened by tab `opener` (a page link or `window.open`): a child of it, in its group, with its jar
    /// (deviation 3).
    pub(crate) fn spawn_tab_from(&mut self, opener: &str, url: &str) -> String {
        let parent = Some(opener.to_string());
        let group = self.group_of(&parent);
        let (kind, jar) = (self.settings.default_starting_engine, self.tab_jar(opener));
        self.spawn_new_tab_with_kind(parent, group, url, kind, SpawnPurpose::New, jar)
    }
```

Two call sites switch to `spawn_tab_from`:
- `event_loop.rs` `OpenTabRequested` becomes `self.spawn_tab_from(tab_id, &url);`;
- the `node_menu.rs` page link becomes `if let Some(opener) = self.focused.clone() { self.spawn_tab_from(&opener, &menu.href); } else { self.spawn_new_tab(None, None, &menu.href); }`.

Moves:
- `graph_nav.rs` `apply_drop` calls `self.refollow(&id)` before its `save_session` for the Eject, Join, GroupWith and Merge outcomes. Merge refollows the source group's former members, which are now in the target.
- `eject` calls `self.refollow(id)`.
- `merge_groups` refollows `target`.
- `main.rs` `dissolve_group` collects the member ids first and refollows each after the move.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p manifold --lib`
Expected: all pass.

- [ ] **Step 5: fmt, clippy, commit**

```
cargo fmt --all
cargo clippy -p manifold -p ipc-protocol --all-targets -- -D warnings
git add -u crates/manifold/src
git commit -m "Jars: new tabs inherit the group default; group defaults cascade to unset tabs" -m "Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Live tabs switch jars by sleeping

**Files:**
- Modify: `crates/manifold/src/sleep_state.rs` (`SleepWhy::JarSwitch`, `verdict`, `JarSwitch`, `jar_switch`)
- Modify: `crates/manifold/src/idle_sleep.rs` (`TabIdleView::busy`)
- Modify: `crates/manifold/src/tabs.rs` (`expire_pending_sleeps`, `tab_idle_views`, `sweep_idle_tabs`, `will_switch`, `sweep_jar_switches`, `switch_jar_now`)
- Modify: `crates/manifold/src/jar_defs.rs` (`clear_jar_cookies` and the jar-changing methods call `sweep_jar_switches`)
- Modify: `crates/manifold/src/session_sync.rs` (`apply_remote` ends with `sweep_jar_switches`)

**Interfaces:**
- Consumes: Tasks 4–5.
- Produces:

```rust
// sleep_state.rs
pub(crate) enum SleepWhy { Manual, Idle, JarSwitch }
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum JarSwitch { Nothing, SleepNow, Wait }
/// The spec's "Live tabs when `jar` changes" table, before the dirty-form check (`verdict`). `reseed`: the host must
/// be replaced although its key is unchanged (its jar was cleared).
pub(crate) fn jar_switch(spawned_with: Option<JarKey>, wanted: JarKey, reseed: bool, busy: bool, focused: bool) -> JarSwitch;
// idle_sleep.rs
impl TabIdleView<'_> { pub(crate) fn busy(&self) -> bool }
// tabs.rs, impl App
pub(crate) fn will_switch(&self, id: &str) -> bool;
pub(crate) fn sweep_jar_switches(&mut self);
pub(crate) fn switch_jar_now(&mut self, id: &str);
```

- [ ] **Step 1: Write the failing tests**

`sleep_state.rs` `mod tests`:

```rust
    #[test]
    fn the_jar_switch_table() {
        use crate::jar::JarKey;
        use crate::store::model::{ActorId, NodeId};
        let work = JarKey::Named(NodeId { actor: ActorId(1), counter: 1 });
        let d = JarKey::Default;
        assert_eq!(jar_switch(None, work, false, false, false), JarSwitch::Nothing, "asleep or not spawned yet");
        assert_eq!(jar_switch(None, work, true, false, false), JarSwitch::Nothing, "no host to reseed");
        assert_eq!(jar_switch(Some(work), work, false, false, false), JarSwitch::Nothing, "already on it");
        assert_eq!(jar_switch(Some(work), work, true, false, false), JarSwitch::SleepNow, "its jar was cleared");
        assert_eq!(jar_switch(Some(d), work, false, false, false), JarSwitch::SleepNow);
        assert_eq!(jar_switch(Some(d), work, false, true, false), JarSwitch::Wait, "busy");
        assert_eq!(jar_switch(Some(d), work, false, false, true), JarSwitch::Wait, "focused");
    }

    #[test]
    fn a_jar_switch_stays_awake_for_a_dirty_form() {
        assert_eq!(verdict(SleepWhy::JarSwitch, &state(true)), Verdict::Stay);
        assert_eq!(verdict(SleepWhy::JarSwitch, &state(false)), Verdict::Sleep);
    }
```

`idle_sleep.rs` `mod tests`:

```rust
    #[test]
    fn busy_is_any_reason_to_stay_awake() {
        let quiet = tab("a");
        assert!(!quiet.busy());
        assert!(TabIdleView { pinned: true, ..tab("a") }.busy());
        assert!(TabIdleView { downloading: true, ..tab("a") }.busy());
    }
```

`tabs.rs` tests (or `main.rs` `mod tests` beside the jar tests). Live tabs here are hand-built with `crate::session_sync::tests::add` on a live `NodePayload::Tab`. Use the existing helper that builds a live test tab: grep `fn live_tab\|fn test_tab` in `main.rs`/`tabs.rs` tests and use whichever exists. If none exists, test `will_switch` through its pure parts (`jar_switch` above). Also test the Unloaded case, which needs no host:

```rust
    #[test]
    fn a_sleeping_tab_never_needs_a_switch_and_a_deleted_jar_switches_to_default() {
        let mut app = App::default();
        let work = app.create_jar("Work");
        let t = crate::session_sync::tests::add(&mut app, &crate::session_sync::tests::sleeping_tab("https://a/", "A"));
        app.set_tab_jar(&t, Some(work));
        assert!(!app.will_switch(&t), "asleep: it spawns into the new jar");
        // A host spawned with Work whose jar is then deleted must head for Default.
        app.jar_tabs.insert(t.clone(), (jar::JarKey::Named(work), jar::TabCookies::default()));
        app.forget_jar(work);
        assert_eq!(app.resolved_jar(&app.tab_jar(&t).unwrap()).0, jar::JarKey::Default);
    }
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p manifold --lib -- jar_switch busy_is_any a_sleeping_tab_never`
Expected: compile errors.

- [ ] **Step 3: Implement**

`sleep_state.rs`:

```rust
pub(crate) enum SleepWhy {
    Manual,
    Idle,
    /// The tab's jar changed while it ran (spec "Live tabs when `jar` changes"); follows the idle rules.
    JarSwitch,
}

pub(crate) fn verdict(why: SleepWhy, state: &PageState) -> Verdict {
    if why != SleepWhy::Manual && state.dirty_form { Verdict::Stay } else { Verdict::Sleep }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum JarSwitch {
    Nothing,
    SleepNow,
    Wait,
}

/// `spawned_with` is the running host's jar (`None`: no host, so it will spawn into `wanted`).
pub(crate) fn jar_switch(
    spawned_with: Option<JarKey>,
    wanted: JarKey,
    reseed: bool,
    busy: bool,
    focused: bool,
) -> JarSwitch {
    match spawned_with {
        Some(had) if had != wanted || reseed => {
            if busy || focused {
                JarSwitch::Wait
            } else {
                JarSwitch::SleepNow
            }
        }
        _ => JarSwitch::Nothing,
    }
}
```

(Update the existing `only_idle_sleep_stays_awake_for_a_dirty_form` doc line to "only a manual sleep discards a dirty form".)

`idle_sleep.rs`: add `busy()` to `TabIdleView`, and make `IdleClock::sweep` call `tab.busy()`:

```rust
impl TabIdleView<'_> {
    /// On screen, playing audio, pinned, loading or downloading: never put to sleep by a sweep or a jar switch.
    pub(crate) fn busy(&self) -> bool {
        self.on_screen || self.audio_playing || self.pinned || self.loading || self.downloading
    }
}
```

`tabs.rs`:
- Extract the view-building in `sweep_idle_tabs` into `fn tab_idle_views(&self) -> Vec<crate::idle_sleep::TabIdleView<'_>>`, which collects owned ids into the views. `sweep_idle_tabs` calls it, then calls `self.sweep_jar_switches()` at its end.
- `expire_pending_sleeps` changes its match to `SleepWhy::Idle | SleepWhy::JarSwitch => self.idle_clock.restart(&id, now),`.
- Add:

```rust
    /// A live WebView2 or CEF tab running on a jar other than the one it now resolves to: it switches when it next
    /// qualifies (chip "will switch").
    pub(crate) fn will_switch(&self, id: &str) -> bool {
        let Some(tab) = self.nodes.get(id).and_then(|n| n.as_tab()) else { return false };
        let Some((had, _)) = self.jar_tabs.get(id) else { return false };
        matches!(tab.tab.kind, EngineKind::Webview2 | EngineKind::Cef)
            && (*had != self.resolved_jar(&tab.jar).0 || self.jar_reseed.contains(id))
    }

    /// Puts every live tab whose jar changed and that is not busy, focused or showing to sleep; it wakes into the
    /// new jar (spec table). Runs after a local jar change, a remote batch (`apply_remote`) and each idle sweep, so a
    /// waiting tab switches once it qualifies (deviation 11).
    pub(crate) fn sweep_jar_switches(&mut self) {
        let views = self.tab_idle_views();
        let todo: Vec<String> = views
            .iter()
            .filter(|v| self.will_switch(v.id))
            .filter(|v| {
                let tab = self.nodes[v.id].as_tab().expect("a view is a live tab");
                let had = self.jar_tabs.get(v.id).map(|(k, _)| *k);
                let focused = self.focused.as_deref() == Some(v.id);
                let reseed = self.jar_reseed.contains(v.id);
                sleep_state::jar_switch(had, self.resolved_jar(&tab.jar).0, reseed, v.busy(), focused)
                    == sleep_state::JarSwitch::SleepNow
            })
            .map(|v| v.id.to_string())
            .collect();
        for id in todo {
            self.request_sleep(&id, SleepWhy::JarSwitch);
        }
    }

    /// "Switch jar now": the old jar gets the tab's final cookies, then the host is replaced in place by one seeded
    /// from the new jar (the engine-swap path, same engine).
    pub(crate) fn switch_jar_now(&mut self, id: &str) {
        let Some((kind, url)) = self.nodes.get(id).and_then(|n| n.as_tab()).map(|t| (t.tab.kind, t.url.clone()))
        else {
            return;
        };
        if !self.will_switch(id) {
            return;
        }
        self.pull_cookies(id);
        if let Some(held) = self.hold_for_cookies(id, Held::Snapshot(kind, url)) {
            self.run_held(id, held);
        }
    }
```

`tab_idle_views` borrows `self`, so collect `todo` before calling `request_sleep`, as shown. If the borrow checker rejects `views` alive across `self.will_switch`, make `tab_idle_views` return owned `(String, bool /*busy*/)` pairs. Do not clone the nodes.

`finish_swap` already handles `target == old kind`. `TabSpawn::respawn` carries `tab.jar`, and `spawn_tab_async` resolves the new key (Task 4). `handle_tab_spawn_result` then records the new key in `jar_tabs`, and `clear_cookie_state` has dropped the old entry.

Call `self.sweep_jar_switches()` at:
- the end of `apply_remote`, after `collapse_toasts_since`;
- the end of `set_tab_jar`, `follow_group_jar`, `refollow`, `set_group_default_jar`, `delete_jar` and `forget_jar`.

For `forget_jar`: a tab whose spawn jar was deleted now resolves to Default, so `will_switch` is true.

`clear_jar_cookies` (deviation 14). After saving the jar:

```rust
        // Their engines still hold the old cookies: they must be replaced although their key is unchanged.
        let running: Vec<String> =
            self.jar_tabs.iter().filter(|(_, (k, _))| *k == key).map(|(id, _)| id.clone()).collect();
        self.jar_reseed.extend(running);
        self.sweep_jar_switches();
```

`jar_reseed: HashSet<String>` is a new App field: hosts that must be replaced even though their key has not changed. Its doc says "hosts still holding cookies from a jar the user cleared". Initialise it empty in `App::default`.
- `will_switch` returns true for a live WebView2 or CEF tab in `jar_reseed` (see its code above: `|| self.jar_reseed.contains(id)` beside the key comparison).
- `sweep_jar_switches` passes `self.jar_reseed.contains(v.id)` as `jar_switch`'s `reseed` argument.
- `clear_cookie_state(id)` removes `id` from `jar_reseed`. It runs when the respawned host lands (the Ok branch of `handle_tab_spawn_result`) and when the tab closes.

Add this test:

```rust
    #[test]
    fn clearing_a_jar_marks_its_running_tabs_for_a_reseed() {
        let mut app = App::default();
        app.jar_tabs.insert("a".into(), (jar::JarKey::Default, jar::TabCookies::default()));
        app.clear_jar_cookies(jar::JarKey::Default);
        assert!(app.jar_reseed.contains("a"));
        app.clear_cookie_state("a");
        assert!(!app.jar_reseed.contains("a"));
    }
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p manifold --lib`
Expected: all pass. The existing sleep tests (`finish_sleep_without_a_request_does_nothing`, `request_sleep_needs_a_live_tab`, `only_idle_sleep_stays_awake_for_a_dirty_form`) still pass.

- [ ] **Step 5: fmt, clippy, commit**

```
cargo fmt --all
cargo clippy -p manifold -p ipc-protocol --all-targets -- -D warnings
git add -u crates/manifold/src
git commit -m "Jars: live tabs switch jars by sleeping; switch-now respawns in place" -m "Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 7: UI — menus, chip, Tab-view dot, Settings "Jars", confirm popup

**Files:**
- Modify: `crates/manifold/src/node_menu.rs` (`NodeAction`, `NodeMenuItem::Jars`, `JarRow`, `JarMenu`, `NodeMenuView.jars`, `node_menu_config`, `build_node_context_menu`, `build_node_menu`, `node_menu_view`, `apply_node_action`, tests)
- Modify: `crates/manifold/src/main.rs` (`FocusTarget::JarName`, `App.naming_jar`, `App.pending_jar_confirm`, popup call beside `build_engine_confirm_popup`)
- Modify: `crates/manifold/src/jar_defs.rs` (`JarChip`, `jar_chip`, `jar_tab_count`, `JarConfirm`)
- Modify: `crates/manifold/src/card.rs` (chip on `paint_tab_body` and `paint_unloaded_tab_body`)
- Modify: `crates/manifold/src/tab_view.rs` (`ItemToolbarActions.switch_jar`, a dot in the trailing cluster)
- Modify: `crates/manifold/src/settings_page.rs` (`settings_jars_section`, `SettingsRequests` jar fields)
- Modify: `crates/manifold/src/popups.rs` (`build_jar_confirm_popup`)

**Interfaces:**
- Consumes: Tasks 3–6.
- Produces:

```rust
// node_menu.rs
pub(crate) enum NodeAction { /* existing */ SetJar(Option<NodeId>), StartNewJar, NewJar(String), CancelNewJar,
    FollowGroupJar, SwitchJarNow, SetGroupDefaultJar(NodeId), ClearGroupDefaultJar }
pub(crate) struct JarRow { pub(crate) id: Option<NodeId>, pub(crate) name: String,
    pub(crate) color: Option<egui::Color32>, pub(crate) ticked: bool, pub(crate) count: Option<usize> }
pub(crate) struct JarMenu { pub(crate) rows: Vec<JarRow>, pub(crate) can_follow: bool,
    pub(crate) will_switch: bool, pub(crate) has_default: bool }
pub(crate) enum NodeMenuItem { /* existing */ Jars { heading: &'static str, rows: Vec<JarRow>, group: bool } }
// NodeMenuView gains: jars: Option<JarMenu>
// jar_defs.rs
pub(crate) struct JarChip { pub(crate) name: String, pub(crate) color: egui::Color32, pub(crate) following: bool,
    pub(crate) missing: bool, pub(crate) will_switch: bool, pub(crate) engine_ignores: bool }
pub(crate) fn jar_chip(&self, id: &str) -> Option<JarChip>;     // None for a default-jar tab with nothing to warn
pub(crate) fn jar_tab_count(&self, key: JarKey) -> usize;       // tabs (live or asleep) resolving to key
pub(crate) enum JarConfirm { Clear(JarKey), Delete(NodeId) }
// main.rs
naming_jar: Option<(String, String)>,                           // (tab id, draft name)
pending_jar_confirm: Option<JarConfirm>,
FocusTarget::JarName
```

Menu layout:
- Tab: `<kind items> | <jars> | New Jar... | [Follow Group's Jar] | [Switch Jar Now] | <colors> ...`.
- Unloaded: the same, without "Switch Jar Now" (a sleeping tab never needs it).
- Group: `Rename | Sleep Tabs | <jars> | [Clear Default Jar] | <colors> ...`.

`<jars>` appears only when `view.jars` is `Some`, so the existing expectations (built with `jars: None`) are unchanged.

- [ ] **Step 1: Write the failing tests**

In `node_menu.rs` tests:
- add `jars: None,` to `view_of`;
- add the arm `NodeMenuItem::Jars { .. } => "<jars>",` to `menu_of`;
- append to `node_menu_config_matches_each_kinds_menu`:

```rust
        // Jars (per-tab jars plan, Task 7): tabs and groups list them ahead of the colours.
        let rows = vec![JarRow { id: None, name: "Default".into(), color: None, ticked: true, count: None }];
        let jars = |can_follow, will_switch, has_default| {
            Some(JarMenu { rows: rows.clone(), can_follow, will_switch, has_default })
        };
        let tab = |j| NodeMenuView { jars: j, ..view_of(ItemNodeKind::Tab) };
        assert_eq!(
            menu_of(&tab(jars(false, false, false)), false).2,
            "Pin | Duplicate | Sleep | Copy URL | Fit Subtree | <jars> | New Jar... | <colors> | --- | Close"
        );
        assert_eq!(
            menu_of(&tab(jars(true, true, false)), false).2,
            "Pin | Duplicate | Sleep | Copy URL | Fit Subtree | <jars> | New Jar... | Follow Group's Jar | Switch Jar Now | <colors> | --- | Close"
        );
        let unloaded = NodeMenuView { jars: jars(true, true, false), ..view_of(ItemNodeKind::Unloaded) };
        assert_eq!(
            menu_of(&unloaded, false).2,
            "Open | Pin | Copy URL | <jars> | New Jar... | Follow Group's Jar | <colors> | --- | Close"
        );
        let group = |has_default| NodeMenuView { jars: jars(false, false, has_default), ..view_of(ItemNodeKind::Group) };
        assert_eq!(
            menu_of(&group(false), false).2,
            "Rename | Sleep Tabs | <jars> | <colors> | --- | Dissolve | Close Group | Export Group..."
        );
        assert_eq!(
            menu_of(&group(true), false).2,
            "Rename | Sleep Tabs | <jars> | Clear Default Jar | <colors> | --- | Dissolve | Close Group | Export Group..."
        );
```

A test for the new-jar flow, in the style of `move_to_group_offers_sibling_groups_and_joins`:

```rust
    #[test]
    fn new_jar_names_a_jar_and_picks_it_for_the_tab() {
        let mut app = App::default();
        let ctx = egui::Context::default();
        let t = crate::session_sync::tests::add(&mut app, &crate::session_sync::tests::sleeping_tab("https://a/", "A"));
        app.apply_node_action(&ctx, &t, NodeAction::StartNewJar);
        assert_eq!(app.naming_jar, Some((t.clone(), String::new())));
        app.apply_node_action(&ctx, &t, NodeAction::NewJar("  Work ".into()));
        assert_eq!(app.naming_jar, None);
        let work = app.jar_named("Work").expect("created, trimmed");
        assert_eq!(app.tab_jar(&t).unwrap(), crate::jar::JarChoice { jar: Some(work), set: true });
        app.apply_node_action(&ctx, &t, NodeAction::StartNewJar);
        app.apply_node_action(&ctx, &t, NodeAction::NewJar("   ".into()));
        assert_eq!(app.jar_defs.len(), 1, "an empty name creates nothing");
    }

    #[test]
    fn the_tab_menu_lists_default_then_named_jars_with_a_tick() {
        let mut app = App::default();
        let work = app.create_jar("Work");
        let t = crate::session_sync::tests::add(&mut app, &crate::session_sync::tests::sleeping_tab("https://a/", "A"));
        app.set_tab_jar(&t, Some(work));
        let rows = app.node_menu_view(&t).unwrap().jars.unwrap().rows;
        let shown: Vec<(&str, bool)> = rows.iter().map(|r| (r.name.as_str(), r.ticked)).collect();
        assert_eq!(shown, [("Default", false), ("Work", true)]);
    }
```

In `jar_defs.rs` tests:

```rust
    #[test]
    fn the_chip_shows_only_non_default_or_warning_states() {
        let mut app = App::default();
        let work = app.create_jar("Work");
        let t = tab_in(&mut app, None);
        assert!(app.jar_chip(&t).is_none(), "default jar: no chip");
        app.set_tab_jar(&t, Some(work));
        let chip = app.jar_chip(&t).unwrap();
        assert_eq!((chip.name.as_str(), chip.following, chip.missing), ("Work", false, false));
        app.follow_group_jar(&t); // no group default: back to the default jar
        assert!(app.jar_chip(&t).is_none());
        app.set_tab_jar(&t, Some(work));
        app.jar_defs.remove(&work); // a remote delete arrives before the tab's own field edit
        let chip = app.jar_chip(&t).unwrap();
        assert!(chip.missing);
    }
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p manifold --lib -- node_menu jar_chip new_jar`
Expected: compile errors.

- [ ] **Step 3: Implement the menu**

`node_menu.rs`:
- Add the actions. `key_hint` returns `None` and `hover_text` returns `None` for them, except `SwitchJarNow`: "Reload this tab in its new jar now".
- Add `JarRow` (derive `Clone, Debug, PartialEq`) and `JarMenu` (derive `Clone, Debug, PartialEq`).
- Add `NodeMenuItem::Jars`.
- Add `jars: Option<JarMenu>` to `NodeMenuView`.

In `node_menu_config`, inside the Tab, Unloaded and Group arms, after their own items and before `finish(...)`:

```rust
    let jar_items = |jars: &JarMenu, tab: bool, live: bool| -> Vec<NodeMenuItem> {
        let mut out = vec![Jars {
            heading: if tab { "Jar" } else { "Default jar for new tabs" },
            rows: jars.rows.clone(),
            group: !tab,
        }];
        if tab {
            out.push(Button("New Jar...", A::StartNewJar));
            if jars.can_follow {
                out.push(Button("Follow Group's Jar", A::FollowGroupJar));
            }
            if live && jars.will_switch {
                out.push(Button("Switch Jar Now", A::SwitchJarNow));
            }
        } else if jars.has_default {
            out.push(Button("Clear Default Jar", A::ClearGroupDefaultJar));
        }
        out
    };
```

The Tab arm uses `items.extend(view.jars.iter().flat_map(|j| jar_items(j, true, true)))`. Unloaded uses `(j, true, false)`, Group uses `(j, false, false)`.

`build_node_context_menu` renders `Jars`:

```rust
                    NodeMenuItem::Jars { heading, rows, group } => {
                        ui.label(egui::RichText::new(*heading).small().weak());
                        for row in rows {
                            let tick = if row.ticked { "\u{2713} " } else { "   " };
                            let count = row.count.map_or(String::new(), |n| format!("  ({n})"));
                            let text = format!("{tick}{}{count}", row.name);
                            let response = ui.horizontal(|ui| {
                                let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                                match row.color {
                                    Some(c) => ui.painter().circle_filled(dot.center(), 4.0, c),
                                    None => ui.painter().circle_stroke(dot.center(), 4.0, ui.visuals().weak_text_color()),
                                }
                                menu_button(ui, &text, None, &mut first_item)
                            }).inner;
                            if response.clicked() {
                                action = Some(match (group, row.id) {
                                    (true, Some(id)) => NodeAction::SetGroupDefaultJar(id),
                                    (true, None) => NodeAction::ClearGroupDefaultJar,
                                    (false, id) => NodeAction::SetJar(id),
                                });
                            }
                        }
                    }
```

`menu_button`'s label parameter type decides whether `&text` or `text.as_str()` is passed. Check its signature and match it.

`build_node_menu`:
- `rename` also comes from `naming_jar` (`Some((tid, draft)) if live && *tid == id`), with `take_focus(&mut self.pending_focus, FocusTarget::JarName)`.
- After the call, map the generic field actions:

```rust
        let naming = self.naming_jar.as_ref().is_some_and(|(tid, _)| *tid == id);
        let action = action.map(|a| match (a, naming) {
            (NodeAction::CommitRename(name), true) => NodeAction::NewJar(name),
            (NodeAction::CancelRename, true) => NodeAction::CancelNewJar,
            (a, _) => a,
        });
        let close = match action {
            Some(NodeAction::Rename | NodeAction::StartNewJar) => false,
            Some(_) => true,
            None => clicked_outside && view.renaming.is_none(),
        };
```

`node_menu_view`:
- `renaming` also reads `naming_jar` for this id.
- `jars` for tab and unloaded payloads:

```rust
            jars: self.tab_jar(id).map(|choice| {
                let group_default = self.inherited_jar(node.group.as_deref());
                let mut rows = vec![JarRow { id: None, name: "Default".into(), color: None, ticked: choice.jar.is_none(), count: None }];
                rows.extend(self.jar_defs.iter().map(|(jid, d)| JarRow {
                    id: Some(*jid), name: d.name.clone(), color: d.color, ticked: choice.jar == Some(*jid), count: None,
                }));
                JarMenu { rows, can_follow: choice.set && group_default.is_some(), will_switch: self.will_switch(id), has_default: false }
            })
            .or_else(|| node.as_group().map(|g| JarMenu {
                rows: self.jar_defs.iter().map(|(jid, d)| JarRow {
                    id: Some(*jid), name: d.name.clone(), color: d.color, ticked: g.default_jar == Some(*jid),
                    count: Some(self.cascade_count(id, *jid)),
                }).collect(),
                can_follow: false, will_switch: false, has_default: g.default_jar.is_some(),
            })),
```

(Extract it into `fn jar_menu(&self, id: &str, node: &GraphNode) -> Option<JarMenu>` to keep `node_menu_view` readable.)

`apply_node_action` arms:

```rust
            NodeAction::SetJar(jar) => self.set_tab_jar(id, jar),
            NodeAction::StartNewJar => {
                self.naming_jar = Some((id.to_string(), String::new()));
                self.pending_focus = Some(FocusTarget::JarName);
            }
            NodeAction::NewJar(name) => {
                self.naming_jar = None;
                if !name.trim().is_empty() {
                    let jar = self.create_jar(&name);
                    self.set_tab_jar(id, Some(jar));
                }
            }
            NodeAction::CancelNewJar => self.naming_jar = None,
            NodeAction::FollowGroupJar => self.follow_group_jar(id),
            NodeAction::SwitchJarNow => self.switch_jar_now(id),
            NodeAction::SetGroupDefaultJar(jar) => {
                self.set_group_default_jar(id, jar);
            }
            NodeAction::ClearGroupDefaultJar => self.clear_group_default_jar(id),
```

`main.rs`:
- add `JarName` to `FocusTarget`, with a doc line;
- add the `naming_jar` and `pending_jar_confirm` fields, defaulting to `None`;
- `forget_removed_group`'s sibling for tabs: `remove_tab` and `destroy_tab` clear `naming_jar` when it names the removed tab. Add that line beside `clear_cookie_state` in `remove_tab`.

- [ ] **Step 4: Implement the chip, the Tab-view dot, Settings and the confirm**

`jar_defs.rs`:

```rust
    /// The card's jar chip (spec "UI"): `None` for a tab on the default jar with nothing to warn about.
    pub(crate) fn jar_chip(&self, id: &str) -> Option<JarChip> {
        let choice = self.tab_jar(id)?;
        let (key, missing) = self.resolved_jar(&choice);
        let will_switch = self.will_switch(id);
        let engine = match &self.nodes.get(id)?.payload {
            NodePayload::Tab(t) => t.tab.kind,
            NodePayload::Unloaded(t) => t.kind,
            _ => return None,
        };
        let engine_ignores = !matches!(engine, EngineKind::Webview2 | EngineKind::Cef) && key != JarKey::Default;
        let (name, color) = match key {
            JarKey::Named(j) => {
                let d = &self.jar_defs[&j];
                (d.name.clone(), d.color.unwrap_or(crate::theme::TAG_COLOR_PALETTE[0]))
            }
            JarKey::Default if missing || will_switch => ("Default".into(), egui::Color32::GRAY),
            JarKey::Default => return None,
        };
        Some(JarChip { name, color, following: !choice.set, missing, will_switch, engine_ignores })
    }

    /// Tabs (live or asleep) that resolve to `key`, for Settings' count.
    pub(crate) fn jar_tab_count(&self, key: JarKey) -> usize {
        self.nodes.keys().filter_map(|t| self.tab_jar(t)).filter(|c| self.resolved_jar(c).0 == key).count()
    }
```

A shared painter goes in `card.rs`:

```rust
/// A jar chip: a dot (outlined when following the group, warning-coloured for a missing jar) and the name, plus an
/// arrow and a second dot while the tab waits to switch. Returns its rect for hover text.
pub(crate) fn paint_jar_chip(painter: &egui::Painter, at: egui::Pos2, chip: &crate::jar_defs::JarChip, font: egui::FontId, text: egui::Color32) -> egui::Rect
```

Draw it at the thumbnail's bottom-left in `paint_tab_body` (inside `tab_thumb_rect`, inset by 4 px at `inv_zoom`), and below the title in `paint_unloaded_tab_body`. The missing colour is `ui.visuals().warn_fg_color`.

Hover text: the missing case reads "This tab's jar was deleted; it uses the default jar". `engine_ignores` reads "This engine does not use jars yet". `will_switch` reads "Switches to <name> when it is next out of sight; right-click for Switch Jar Now". Return the hover through `CardBody.tooltip` the way the existing tooltip is returned. If `CardBody.tooltip` already carries one, chain them with a newline.

`tab_view.rs`:
- Add `switch_jar: bool` to `ItemToolbarActions` and its exhaustive destructure at 455–473.
- In the trailing cluster before the bookmark button, when `self.jar_chip(focused)` is `Some`, draw an 8 px dot with the chip's colour and the name as hover text.
- A click sets `actions.switch_jar = chip.will_switch`. The handler calls `self.switch_jar_now(&focused)` (deviation 8).

`settings_page.rs`:
- `SettingsRequests` gains `rename_jar: Option<(NodeId, String)>`, `jar_ephemeral: Option<(NodeId, bool)>` and `jar_confirm: Option<JarConfirm>`.
- Add `fn settings_jars_section(&mut self, ui, req: &mut SettingsRequests)`, called between Sessions and Bookmarks with the existing section pattern (`ui.add_space(10.0); ui.separator(); ui.label(heading_text("Jars"));`).
- One row for "Default": a grey dot, "Default", `jar_tab_count(Default)` and "Clear Cookies".
- One row per `jar_defs` entry:
  - swatch;
  - a `TextEdit::singleline` name, committed on lost focus into `req.rename_jar`;
  - count;
  - an "Ephemeral" checkbox into `req.jar_ephemeral`;
  - "Clear Cookies" into `req.jar_confirm = Some(JarConfirm::Clear(key))`;
  - "Delete" into `req.jar_confirm = Some(JarConfirm::Delete(id))`.
- The name buffers live in egui temp data keyed by `("jar_name", id)`, so the field can be edited without writing every keystroke.
- `apply_settings_requests` applies them: `rename_jar`, `set_jar_ephemeral`, `pending_jar_confirm = Some(..)`, then `save_session()`.

`popups.rs`: `build_jar_confirm_popup`, in the pattern of `build_engine_confirm_popup`:
- `Clear(key)`: title "Clear cookies in <name>?", detail "Tabs using it are signed out.", button "Clear".
- `Delete(id)`: title "Delete jar <name>?", detail "<n> tabs use this; they'll use Default." (from `jar_tab_count`), button "Delete".

Yes calls `clear_jar_cookies(key)`, or `delete_jar(id)` then `save_session()`. Cancel and Escape clear `pending_jar_confirm`. Call it in `main.rs` beside `build_engine_confirm_popup`.

- [ ] **Step 5: Run the tests, then look at it**

Run: `cargo test -p manifold --lib`
Expected: all pass.

Then build (`cargo build -p manifold`) and drive it headless to check that it renders. Use whatever screenshot command `scripts/drive.sh` offers. Memory note: the probes run the existing exe, so build first. Seed a tab, use the menu (`RIGHTCLICK`/`MENUCLICK` or the equivalents in `COMMAND_HELP`), pick "New Jar...", type a name, and confirm the chip shows on the card. If the debug commands cannot drive a context menu, say so in the commit message and rely on Task 8's probe.

- [ ] **Step 6: fmt, clippy, commit**

```
cargo fmt --all
cargo clippy -p manifold -p ipc-protocol --all-targets -- -D warnings
git add -u crates/manifold/src
git commit -m "Jars: tab and group menus, card chip, Tab-view dot, Settings section" -m "Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 8: Debug commands, probe run, docs

**Files:**
- Modify: `crates/manifold/src/debug_control.rs` (`DebugCommand::SetJar(String)`, `DebugCommand::SwitchJarNow`, `COMMAND_HELP`, `parse_line`, tests)
- Modify: `crates/manifold/src/event_loop.rs` (handlers beside `SleepNode`, around 1220–1233)
- Modify: `scripts/cookie_server.py` (`/login?u=X`, a `user=` log column)
- Modify: `scripts/jar-probe.sh` (`perjar` and `jarswitch` runs)
- Modify: `README.md` (lines 321, 323, 387)
- Modify: `docs/superpowers/specs/2026-10-03-per-tab-jars-design.md` (Status line)

**Interfaces:**
- Consumes: everything above.
- Produces:
  - `SETJAR <jar name|default>`: the focused tab picks that jar (`set_tab_jar`), creating a named jar if none has that name.
  - `SWITCHJARNOW`: `switch_jar_now(focused)`.

- [ ] **Step 1: Write the failing tests**

`debug_control.rs` tests:

```rust
    #[test]
    fn jar_commands_parse() {
        assert!(matches!(parse_line("SETJAR Work"), Some(DebugCommand::SetJar(n)) if n == "Work"));
        assert!(matches!(parse_line("SETJAR default"), Some(DebugCommand::SetJar(n)) if n == "default"));
        assert!(parse_line("SETJAR").is_none(), "a jar name is required");
        assert!(matches!(parse_line("SWITCHJARNOW"), Some(DebugCommand::SwitchJarNow)));
    }
```

If `parse_line` returns `Result` or another shape, follow the existing tests' `matches!` form. There is also an existing test checking that `COMMAND_HELP` has one entry per `parse_line` arm, in order. If it exists it fails until Step 3 adds the help lines; if it does not exist, nothing extra is needed.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p manifold --lib -- jar_commands_parse`
Expected: compile error (`no variant SetJar`).

- [ ] **Step 3: Implement**

`debug_control.rs`:
- Add `SetJar(String),` and `SwitchJarNow,` to the `#[rustfmt::skip]` enum, after `IdleSweep`.
- Add after the `IDLESWEEP` help entry:

```rust
    ("SETJAR <name|default>", "The focused tab uses that jar (a new named jar if none has the name)"),
    ("SWITCHJARNOW", "The focused tab moves to its new jar now, if it is waiting to switch"),
```

- In `parse_line`, after `"IDLESWEEP"`:

```rust
        "SETJAR" => non_empty(DebugCommand::SetJar),
        "SWITCHJARNOW" => Some(DebugCommand::SwitchJarNow),
```

`event_loop.rs`:

```rust
            DebugCommand::SetJar(name) => {
                if let Some(tab) = self.focused.clone() {
                    let jar = if name.eq_ignore_ascii_case("default") {
                        None
                    } else {
                        Some(self.jar_named(&name).unwrap_or_else(|| self.create_jar(&name)))
                    };
                    self.set_tab_jar(&tab, jar);
                    self.save_session();
                }
            }
            DebugCommand::SwitchJarNow => {
                if let Some(tab) = self.focused.clone() {
                    self.switch_jar_now(&tab);
                }
            }
```

`cookie_server.py`:
- `/login?u=X` sets `sid=PROBE-SESSION-X`. Plain `/login` keeps `sid=PROBE-SESSION`, so the existing runs are unchanged.
- `authed` becomes "any `sid=PROBE-SESSION` prefix".
- The log line gains `user=<X or ->`, parsed from the cookie (never the full value; X is a probe username, not a secret).
- `/whoami` answers `AUTH <user>` or `ANON`.
- Update the module docstring to match.

```python
def user_of(cookie):
    for part in cookie.split(";"):
        name, _, value = part.strip().partition("=")
        if name == "sid" and value.startswith("PROBE-SESSION"):
            return value[len("PROBE-SESSION-"):] or "default"
    return None
```

In the log line, `authed = user is not None`, and `f"... auth={authed} user={user or '-'} names={names}\n"`. In `do_GET`, `/login` reads `u` with `urllib.parse.parse_qs(urllib.parse.urlparse(self.path).query).get("u", [""])[0]`.

`jar-probe.sh`:
- `jar_summary` takes the jar file as `$2`. The default stays `jars/default.json`, so the existing calls are unchanged.
- Two new runs:
  - **`perjar`** seeds, via the import JSON, jar nodes `Alice` and `Bob` (labels `ja`, `jb`), plus two WebView2 tabs on `/login?u=alice` (jar `ja`, `jar_set` true) and `/login?u=bob` (jar `jb`). The script is `WAITTABS; GRAPHRIGHT; GRAPHOPEN; NEXTTAB; SLEEP 38; EXPORTSESSION <tmp>/s.json; QUIT`.
    - 38 s covers one 30 s pull.
    - The `EXPORTSESSION` output maps the names to the fresh ids: `python -P - < /dev/null` reading `s.json`.
    - The run then checks that `jars/<alice id>.json` decrypts to `sid` for `alice` and `jars/<bob id>.json` to `bob`, and that `jars/default.json` holds no `sid`.
    - Print: `perjar: alice jar=alice bob jar=bob default=none`.
  - **`jarswitch`** reuses the `perjar` APPDATA. Script: `WAITTABS; GRAPHOPEN; ADDRESSBAR <base>/whoami?switch; SLEEP 3; SETJAR Bob; SWITCHJARNOW; SLEEP 8; ADDRESSBAR <base>/whoami?switch2; SLEEP 3; QUIT`.
    - The server log must show `/whoami?switch ... user=alice` and then `/whoami?switch2 ... user=bob`.
    - Pass/fail is printed like the existing runs.
- Adapt to the actual names of `run`, `seed_file` and the driver commands already in `jar-probe.sh`. If `ADDRESSBAR` is not a command, use whatever navigates the focused tab (check `COMMAND_HELP`).
- Add `perjar jarswitch` to the default run list. Keep `RUNS=` narrowing as the existing runs do.

`README.md`:
- **Line 321.** `jars\default.json` becomes "`jars\default.json` and one `jars\<jar id>.json` per named jar (ephemeral jars have no file); the aside and temp names use each file's own name (`<file>.unreadable-...`, `<file>.<pid>.tmp`)".
- **Line 323.** Replace "Not built yet: per-tab or per-group jars" with a paragraph covering:
  - each tab uses one jar (Default, or a named jar from the tab's menu);
  - a group's "Default jar for new tabs" is copied into new and moved tabs, and cascades to unset ones;
  - a live tab whose jar changes switches by sleeping, under the idle-sleep rules, or now via "Switch Jar Now";
  - the jar choice syncs, cookies do not;
  - Settings "Jars" renames, toggles ephemeral, clears and deletes.

  Cite `docs/superpowers/specs/2026-10-03-per-tab-jars-design.md`.
- **Line 387.** Add the `perjar` and `jarswitch` runs to the probe paragraph.

Spec status line: "Status: built (plan `docs/superpowers/plans/2026-10-03-per-tab-jars.md`)", after the probe passes.

- [ ] **Step 4: Run the tests and the probe**

```
cargo test -p manifold --lib
cargo build -p manifold
bash scripts/jar-probe.sh
```

Expected:
- tests: all pass;
- probe: the existing runs unchanged, plus `perjar: alice jar=alice bob jar=bob default=none` and `jarswitch: whoami alice -> bob PASS`.

Before the probe:
- check for other sessions' manifold processes (`Get-Process manifold*`);
- check CPU load (LONG_RUNS.md);
- record both with the result.

If `perjar` fails because the second tab never woke, add `SLEEP 5` after `NEXTTAB`. Do not change the product to fit the probe.

- [ ] **Step 5: fmt, clippy, commit**

```
cargo fmt --all
cargo clippy -p manifold -p ipc-protocol --all-targets -- -D warnings
git add crates/manifold/src/debug_control.rs crates/manifold/src/event_loop.rs scripts/cookie_server.py scripts/jar-probe.sh README.md docs/superpowers/specs/2026-10-03-per-tab-jars-design.md
git commit -m "Jars: SETJAR/SWITCHJARNOW, per-jar probe runs, docs" -m "Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

## Self-review

**Spec coverage.** Each spec section maps to a task:

| Spec section | Task |
|---|---|
| Data model | 1, 2, 3 |
| Resolution | 4 (`resolved_jar` is the only key source; spawn, pull, chip, switch and cascade all call it) |
| Inheritance | 5 |
| Live tabs when `jar` changes | 6 |
| Ephemeral jars | 4 (no file), 6 (clear flow) |
| Manifold state | 4 |
| Hosts | no change; the Servo/WebKit chip tooltip is in 7 |
| UI | 7 |
| Testing | unit tests in 1–7; the probe in 8 |
| Migration | absent fields resolve to `Default` (tested in 4); `default.json` names unchanged (the existing `jar.rs` tests keep their names) |
| Out of scope | nothing from that list is built |

**Names used across tasks:** `JarChoice`, `JarKey`, `JarDef`, `jar_defs`, `jars`, `jar_snapshots`, `jar_tabs: (JarKey, TabCookies)`, `jar_reseed`, `resolved_jar`, `ensure_jar_loaded`, `save_jar`, `create_jar`, `delete_jar`, `forget_jar`, `jar_named`, `set_tab_jar`, `follow_group_jar`, `refollow`, `set_group_default_jar`, `clear_group_default_jar`, `cascade_count`, `will_switch`, `sweep_jar_switches`, `switch_jar_now`, `jar_chip`, `jar_tab_count`, `naming_jar`, `pending_jar_confirm`, `jars_dir()`.

**Placeholders.** Several steps say "adapt to the existing helper's name" rather than guess at it:
- the remote-apply test helper;
- `Journal::commit`;
- the live-tab test helper;
- `menu_button`'s label type;
- the drive commands.

Each names the grep that settles it.

**Risks the reviewer should watch:**
- **`diff` and Id validation.** `valid()` drops an Id naming a non-live node on `Set`, and a Create validates only `PARENT`. So a tab created in the same save as its jar keeps the reference: both are new. But a tab created naming a jar a remote just deleted writes a dangling id. It resolves to Default with the warning chip, which is the spec's stated fallback.
- **`last_shared` for jars.** `materialize` records jar `Shared`s, and `teardown_running_session` clears `last_shared`. After an Open Session the first remote jar change is therefore applied in full ("all fields changed"), which is correct but noisy. No test depends on it.
- **The cascade is O(tabs × depth).** That is fine at graph sizes in the hundreds. `governed_by`, `is_inside` and `inherited_jar` each bound their walk by `nodes.len()` against cycles.
- **Two instances on one profile** still overwrite each other's jar files, as in v1 (unchanged, documented there).
- **A sleeping tab never switches.** It wakes into whatever it resolves to at that moment, so a remote change while asleep needs nothing.
