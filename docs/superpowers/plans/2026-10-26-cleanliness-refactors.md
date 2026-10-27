---
status: active
---

# Cleanliness refactors (survey of 2026-10-26)

A read-only scan of `crates/manifold/src` and `crates/gate/src` at `fe35568`, in four areas (UI chrome, graph UI,
store and sync, engines and event loop). Each finding was read in the code by the scanning agent; the starred ones
were re-checked by hand. The user approved doing all of it (2026-10-26); the design calls below are confirmed before
each batch is built. Line numbers are as of `fe35568` and drift as batches land.

Ids are stable: `UI` chrome, `GR` graph, `S` store and sync, `E` engines and event loop, `M` main and gate misc,
`GT` gate.

## Decisions (user, 2026-10-26)

- GR5: Enter on a card whose file failed retries it, like a click does (a small behaviour change).
- GR2: `Click::Edit` and `Click::Enter`, read by both the hint strip and the activation match.
- M1: four groups: `PageInput` (`cursor_pos`, `native_drag_tab`, `key_modifiers`, `click_counter`,
  `swallowed_keys`), `SettingsState` (`settings`, `settings_fingerprint`, `settings_save_due`,
  `settings_return_view`, `home_page_draft`, `settings_focus`), `GraphUi` (`card_nav_active`, `hints_revealed`,
  `card_hint`, `graph_detail`, `truncate_cache`, `pending_graph_zoom`, `graph_settled_key`), `Workers`
  (`favicon_fetcher`, `pdf_renderer`, `image_decoder`, `redirect_fetcher`, `favicon_resolved`); file loads move to
  a constructor.
- Shapes: GR1 uses `emath::TSTransform`; UI1's body returns whether a row closed the popup, with a
  `close_on_outside` flag (built instead as `settle_popup(open, which, applied, clicked_outside)`, since the
  builders draw under a shared borrow of `App` and apply the row afterwards); E1 a `Waiters` list with `release_when`; GT1 `Run::Work(fn(exe, work))`; S5 a
  `ProfileFile { name, what }` with `load` and `save`.

## Batches, in order

Each batch is its own commit or few, with `cargo test -p manifold` and the pre-commit hook. A batch is marked done
here with its commit.

### B1. Comments state the rule, not the history

- **S1\*** 191 `Ruling N` / `Task N` / `deviation N` references (53 files, tests included) point into plan files
  (`docs/superpowers/plans/2026-10-05-node-model-2b-5-*` and others). Replace each with the rule in a few words, or
  delete a bare review or task tag.
- **S2\*** `store/journal.rs:574-578`: the `Ops:` frame doc sits on `OP_CREATE`; move it to `ops_message`.
- **UI2** `popups/dropdowns.rs` 442-444, 551-552, 647-652: three comments narrating a removed `is_empty()` close
  condition; plus "Same Area/Frame pattern as" at 453-455, 662-663.
- **UI5** `theme.rs` `toolbar_panel` doc (639-663), `chip_icon_button_enabled` (764-767), `toolbar_chip`
  (798-810), `icon_button_eased` (713-730): stories of how they came to be.
- **UI6** `settings_page.rs` 269-277, `theme.rs` 997, `tab_view.rs` 229-237, `chrome_ui.rs` 10, 53.
- **UI7** `theme.rs` `lerp_color32` doc 622-627.
- **GR10** `graph_ui/toolbar.rs` 60-68, 216-240; `graph_nav.rs` `settle_current_level` doc (~563); `card.rs` 864,
  871-874; `graph_ui/cards.rs` 269-276; `graph_ui/scene.rs` 184-218; `view_info.rs` 257; `card/jar_chip.rs` 20.
- **S8** `tabs.rs` 128-133, 344-346; `tabs/spawn_result.rs` 5-12; `store/atomic.rs` 83-85; `store/journal.rs` 836;
  `store/model.rs` 270; `session_sync/remote.rs` 258.
- **E6 (comments)** `gpu.rs` `render_thumbnail` doc 674-721; `event_loop.rs` 135 ("Measured 2026-10-01").
- **camera** `graph_ui/camera.rs` 120-126 ("verified live by screenshotting") and 204 (points at a `world_pos` that
  `handle_rect_select` no longer has).

A design note worth keeping moves to the doc it belongs in; a measurement moves to `docs/runs.md` or the commit.

### B2. `Caps` holds the per-kind card facts

Done: 6fd6cf7

- **GR2** `Click` gains `Edit` (note) and `Enter` (JSON, group). `card_hint_pairs` (`graph_ui/hints.rs:26-40`) reads
  its verb from `Caps::click`; the activation match in `graph_ui/cards.rs:143-174` becomes one match on it. The JSON
  `enterable()` and note `was_editing` checks stay as guards.
- **GR6** `node_menu.rs:532-613`: every kind's `menu_area` comes from `Caps` (rows for download, note, loaded and
  sleeping tab, group, jar); the literal ids go. The `json_context_menu` id is also the JSON level's menu id
  (`json_level_ui.rs:886`): rename one.
- **GR5** One `Node` predicate for "its file or load failed" (`graph_ui/cards.rs:129-134`, `graph_nav.rs:540`,
  `graph_ui/camera.rs:353-358`, `node_menu.rs:803-806`).

### B3. One popup lifecycle

Done: 76efb88

- **UI1** A `popup_frame.rs` helper owns `interactable(open == Some(which))`, the clicked-outside test and
  `close_popup`; the body returns whether a row closed it. Covers `build_recently_closed_popup`,
  `build_bookmarks_popup` (outside clicks ignored while renaming), `build_history_popup`, `build_downloads_popup`,
  `build_tab_actions_popup`, `build_trail_branches_popup`, `build_anchored_view_picker`. Tab actions' three
  `bool = true; close = true` pairs become one enum.
- **GR9** `draw_pending_new_node_menu` (`graph_ui/cards.rs:217-254`): five flags become one `Option<GraphAction>`.

### B4. One waiter mechanism for the headless waits

- **E1** `debug_control.rs:1022-1075`, `:1175-1177`, `event_loop/debug_input.rs:51-88`, `tabs.rs:51`,
  `tabs/spawn_result.rs:26-30`: a `Waiters` list with one "ready: release, else ask for a frame" step, used by
  WAITTABS, WAITIDLE and WAITINDEX. `idle_frames` stays idle's own debounce.
  Done: 9667c78

### B5. Typed transforms and field reads

- **GR1** `GraphCamera::transform` returns `egui::emath::TSTransform` (egui's own scale-then-translate type, with
  `mul_pos`, `mul_rect`, `inverse`) instead of `(f32, Vec2)`. Call sites: `graph_ui/scene.rs` 226 and 322,
  `popups/dropdowns.rs` 853-854, `graph_ui/camera.rs` 193-208, `graph_ui/keys.rs` 223, 287, 292-293,
  `link_gestures.rs` 408, 481.
- **GR4** `ctx.layer_transform_to_global(ui.layer_id()).map_or(p, |t| t * p)` 8 times (`card.rs` 1154, 1265, 1340,
  1356; `identity.rs` 181, 194, 212; `link_gestures.rs` 455): one helper. And the `Origin::Virtual` then
  `virtual_source(name)` lookup 5 times (`card.rs` 1266-1271, 1358-1362, 1421; `graph_ui/cards.rs` 201;
  `graph_ui/keys.rs` 242-248).
- **S3** `FieldValue::as_*` accessors in `store/model.rs`; `str_of`, `id_of`, `view_field`, `pos_of`
  (`session_sync.rs:231-266`), `str_field`, `pos_in`, `pinned_in`, `jar_in`, `color_in`, `view_code`
  (`mirror.rs`) collapse onto them, with one home for "(0, 0) is no position".
  Done: 8e6a09e (GR1), 59210b0 (GR4), e925ca5 (S3)

### B6. Gate scaffolding

- **GT1** `crates/gate/src`: a `Run::Work(fn(&Path, &Path) -> ExitCode)` stage kind; the dispatcher runs `setup` and
  `finish`, and `usage` follows from the kind. Replaces about 20 copies of the `match setup(...)` prologue and the
  `manifold_exe()` / `work_dir()` spellings.
- **GT2** A per-view probe loop helper and `Instance::settle()` (WAITTABS then WAITIDLE) for click_count,
  iframe_typing, page_text, search, probes, sleep.
- **M2** Drop the blanket `#![allow(dead_code)]` (`gate/src/main.rs:10`) and act on what it hid; drop the doubled
  `#[cfg(windows)]` (36-37).

### B7. Error and file plumbing

- **E2** `impl From<io::Error> for InstallError` instead of the closure defined 7 times in `view_install.rs`; one
  NotFound-tolerant remove helper instead of `remove_dir`, `remove_if_present`, `remove_file`.
- **E3** `install_with` / `unpack_with` thread a test-only `on_entry` hook; move it out of the production
  signatures.
- **S5** `bookmarks.rs`, `history.rs`, `nav_history.rs`, `settings.rs`: one profile-file load/save helper; each
  keeps its own fixup. `HistoryUi` holds bookmark popup state; name or split it to match.

### B8. Long functions split by concern

- **GR3** `card.rs` `draw_card_inside_overlay_scope` (~330 lines).
- **E4** `event_loop.rs` `window_event` (~290 lines): `focused_tab_mut`, `pointer_in_content`, `resize_panes`, one
  method per event.
- **E5** `debug_export.rs` `export_state` (~270 lines): `rect_json` and per-section builders merged into a `Map`,
  which ends the `json!` recursion-limit workaround (303-317).
- **E6** `gpu.rs` `GpuState::render` (~275 lines): atlas sync, frame acquire, thumbnail flush, readback.
- **GR8** `graph_ui/keys.rs` `handle_rect_select` (110 lines) moves to `graph_ui/marquee.rs` with the right-press
  dispatch split out.
- **GR7** `graph_ui/cards.rs:50-65` Ctrl-reveal block to `keys.rs`; `graph_ui/scene.rs` repeated
  `minimap.map(|(rect, _, _)| rect)` and `connect_drag_end` calls.

### B9. `App` fields grouped

- **M1** `main.rs:181-533`: about 100 fields, a 120-line hand-written `Default` that also loads files. Group into
  sub-structs (like `pacing`, `menus`, `debug` already are), derive their `Default`, and move the loads to a
  constructor. `main/tests.rs` `NOT_FORGOTTEN` pins field names.

### B10. Theme helpers

- **UI3** `theme.rs` `icon_button_eased` (732-746) and `chip_icon_button_enabled` (769-790): one eased icon button
  with presets.
- **UI4** `add_borderless` (215-232), `rest_borderless` (240-246), `json_level_ui.rs:815-840`: `ui.scope` instead
  of saving and restoring visuals by hand.
- **UI8** `toolbar_panel` and `toolbar_field` both set `extreme_bg_color` and the inactive stroke (675, 702); give
  the field look one owner.

### B11. Small singles

- **S4** `session_sync.rs:281-310` `spawn_snapshot` / `tab_fields` against `mirror.rs:505-520` `Carried::tab`.
- **S6** `store/journal.rs` snapshot version entries (474-481, ~590-605) through `id_message` / `decode_id`;
  `decode_hlc` and `decode_stamp` share a field reader. Bytes must not change.
- **S7** `fetched.rs:370-385` and `session_sync/remote.rs:262-300`: one file-card placeholder constructor.
- **S9** `tabs.rs:761-798` `sweep_jar_switches` repeats what `will_switch` checked.
- **S10** `undo.rs:125-139` `replay_detach` returns a bool where the rest return `(done, total)`.
- **UI9** `popups/menus.rs:199-203` and `settings_page.rs:323-329` share the unavailable-engine colour.
- **UI10** `settings_page.rs:281-310` hand-rolls rows `setting_row` already makes.
- **M3** `process_manager.rs:879, 900` `prepare_*` take an unused `_custom`.

## Considered and rejected

- Merging `store/model.rs` and `store/journal.rs` `id_message` / `decode_id`: different wire formats (fixed64 actors
  against actor-table indexes).
- Table-driving `encode_field` / `decode_field` and the op arms: the arms differ in real encoding.
- `debug_control` `VERBS`: already the table it would become.
- Per-engine branches in `process_manager.rs`: `ViewKind::desc()` is already data.
- Merging `focus_ring` and `nav_row`; `card_at` / `row_at` against stored-card hit tests; the keys.rs undo gating
  trio: each differs on purpose.
- The `forget_id` tables: only as part of M1, if at all.

## Open

- `store/` `pub fn` dead-code sweep did not finish (grep loop timed out); the compiler's dead-code lint after M2 is
  the cheaper route.
