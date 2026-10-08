# Wave 1: correctness and data integrity (design)

Status: Implemented 2026-10-08 on `wave-1-integrity` (commits `9f16f93`..`714343b`, gate tooling `8575d45`, `f8f5d8b`); see the plan's Outcome for deviations. The plan is `docs/superpowers/plans/2026-10-07-wave-1-integrity.md`. The scope and
the evidence come from the read-only fact check `.superpowers/survey/verified-wave-1.md` (at `c8b7485`). This spec re-checked
every line and symbol it relies on against the files at `c8b7485`. Where this spec departs from that file, it says so ("Deviation").

Paths are under `crates/manifold/src/`. Line numbers are as of `c8b7485`. Find code by the quoted text, not by the number.

## Decisions

1. **Ten items, one commit each, in the verified file's order:**
   - L4 doc fix
   - L1 centroid
   - P1 group close
   - C1 `forget_id`
   - H1 `end_stale_selection`
   - M5 Json view tolerance
   - H4 settings
   - M3 cookie dump log
   - M4 argument check
   - G2 guard test

   Each commit is one reviewable change with its own test, except L4, which is a doc fix. Two more commits frame the
   ten: the branch's first adds this spec and its plan (so the branch carries its own design), and its last, after the
   gate, sets this spec's Status and the plan's Outcome.
2. **One branch, one full gate.** The wave is built on a branch `wave-1-integrity` from `main` (CLAUDE.md "Branches": new
   branches start from `main` and merge back), with no worktree.
   - Each task runs its own tests, `cargo fmt --all` and the clippy line from CLAUDE.md.
   - One `scripts/gate.sh LOG full` covers the whole wave before the merge, because most items need `full`.
   - Each task's own gate level is also recorded, so a task landed alone later knows what it needs. The labels follow
     CLAUDE.md "Gate": `light` only for UI code (drawing, gestures, menus), scripts or docs. So L4 (a comment), L1 (a graph
     menu action) and H1 (the graph's selection state) are `light`. P1, C1, M5, H4 (settings load), M3 (jar store and
     engine cookie pull) and M4 (startup and the session tools) are `full`. G2 is `full` by the letter, since it edits
     `mirror.rs`, though it adds only a test.
   - The gate's probes launch the exe with `--headless` alone, so M4's other launch forms are run by hand in its task.
   - The merge into `main` waits for the user's go-ahead.
3. **Smallest change that removes each hazard.** The larger refactors the surveys proposed are parked (see "Not in scope /
   parked"), each with its reason.
4. **No silent data loss in either direction.**
   - What this build can't read is kept and written back (M5), or reset only where it stands (H4).
   - Cookie pulls stay all or nothing (M3). A cookie missing from a pull counts as a logout and deletes that cookie from the
     jar. So a bad cookie is never skipped: the failure is logged, and nothing more.
5. **Tests first.** Every code change gets a test that fails before the change and passes after. The exceptions:
   - G2 is a guard that passes today.
   - Some tests fail first only because the new function does not exist yet (C1's table test, M3's type change). That is
     said where it applies.

   No test touches the real `%APPDATA%\manifold`: tests use `App::default()` (whose `profile::dir()` is `None` under
   `cfg(test)`) and `store::testutil::temp_dir`.

## L4. `end_stale_drag`'s doc comment (`graph_nav.rs:503-506`)

Commit `cc0441f` meant to replace the doc's second sentence, but replaced its first line instead. The comment now starts
mid-sentence ("false), with no drop…") and makes two contradictory claims. The true one is that "the layout keeps the dragged
positions": `update_live_node` snaps only on a changed POS that isn't being dragged, and `end_stale_drag` only clears `dragging`
and `drag_moved`.

**Change.** Replace the four lines with the verified file's text, which starts "Ends a drag that is no longer live
(`live_drag`)…". This is a doc-only change, so it has no test.

## L1. Group Selected's centroid (`graph_ui.rs:315-340`)

**Bug.** `count = selected_tabs.len()`, but the sum skips ids missing from `nodes`.
- An id can be missing while its tab is mid-crash-respawn or mid-engine-swap. Both paths leave the selection alone.
- So the new group lands off-centre.

**Change.**
- Take the live ids once (`live`) and clear the selection.
- Return if none are live.
- Average the positions over `live`, and move exactly `live` into the new group.

**Test.**
- Two tabs plus a dead id `"999"` are selected.
- The new group's `mirror.pos` must be the mean of the two tabs' `layout_pos`.
- Before the fix, the sum is divided by 3.

## P1. Closing a group commits once (`main.rs:2650-2680`)

**Bug.** Closing a group of T tabs (at any depth) runs T+1 `save_session`s.
- `destroy_group_members` calls `destroy_node`, which calls `destroy_tab` for each tab. `destroy_tab` is `remove_tab` plus a
  save.
- Each save commits a frame, so another replica can read the group half deleted.
- A crash mid-close leaves the members deleted so far gone for good, with no Recently Closed entry for them.

**Change.**
- In `destroy_group_members`, a tab member goes through `remove_tab`, the non-saving close that already exists
  (`tabs.rs:1197`). Every other member still goes through `destroy_node`.
- `destroy_group`'s final `save_session` then commits the whole subtree in one frame.
- Rewrite `destroy_group`'s stale comment. It says the save exists because non-tab members don't save, and that there is
  "no save on exit". Both are wrong now: the save covers every member, and `close_store` saves on a clean exit.

**Test.** `closing_a_group_commits_one_frame` (`session_sync/tests.rs`).
- A group of three tabs in a temp journal is closed.
- The test counts `store::journal::inspect(..).frames.len()` before and after: the change must be exactly 1. Before the fix
  it is 4.
- The file stays under `COMPACT_MIN` (64 KiB), so no compaction runs.

## C1. One cleanup for a removed id: `forget_id` (`main.rs`, `tabs.rs`, `session_sync.rs`)

**Bug.** Five hand-written cleanup lists have drifted apart:
- `drop_node`
- `drop_tab`
- `remove_remotely_deleted`
- `teardown_running_session`
- `forget_removed_group`

`teardown_running_session` (Open or Import Session) calls `nodes.clear()` without freeing anything:
- every tab's thumbnail and texture, and every image's and PDF's texture, stay registered on the GPU for the life of the
  process;
- about a dozen per-tab tables keep entries for tabs that are gone.

**Change.**
- Add `App::forget_id(&mut self, id: &str)` in `main.rs`, next to `drop_node`. It holds every side effect of a permanent
  removal that doesn't depend on order:
  - `skip_convert`, `pending_sleeps` and `clear_cookie_state`, which covers `jar_tabs`, `jar_reseed`, `jar_pulls`,
    `jar_silent` and `cookie_holds`
  - `saved_scroll`, `nav_history`, `navigated_from`, `popup_gate.forget`, `tab_crashes` and `selected_tabs`
  - `naming_jar`, `renaming_group`, `node_menu`, `pending_respawn_focus`, `peek_suppressed`, `dragging` and `hovered`, each
    when it names `id`
  - `forget_connect_source`
  - with a GPU, `free_tab_thumbnail`, `free_tab_texture` and `free_image`. Each is a no-op for an id it lacks.
- Its doc comment names what it leaves out, and why:
  - `wakes_in_flight`: a tab closed while waking needs its entry, so the wake's landing commits the delete
    (`tabs.rs:379-391`).
  - `spawning`, `remote_in_flight` and `host_loads`: these are spawn lifecycle.
  - The `pending*` save queues: the save's `editable` filter drops them.
  - `layout` and `held_text`: pruned at every save.
  - A crash respawn or an engine swap never calls `forget_id`. Those removals are temporary, and must keep `tab_crashes`,
    `nav_history` and `saved_scroll`.
  - `idle_clock` (`idle_sleep::IdleClock`): it prunes ids that are gone at each sweep. It isn't `String`-typed, so the scan
    doesn't see it.
  - `pending_new_node` (`Option<egui::Vec2>`) and `card_hint` (`Option<(CardKind, egui::Rect)>`): they hold no node id, so
    neither the scan nor `forget_id` has anything to do with them.
- `drop_tab` and `drop_node` call `forget_id` in place of their hand lists. In `drop_node` this also replaces its
  image/PDF `free_image` block. Kind-specific steps stay inline:
  - reparenting the children, first, as now
  - `pdf_renderer.cancel`
  - the `current_group` switch
  - `node_order`
  - `drop_tab`'s split, focus, view-mode and `find.close()` logic
- `forget_removed_group` is deleted, and its two callers call `forget_id` instead: `dissolve_members` (`main.rs:2447`) and
  `merge_groups` (`graph_nav.rs:167`). **Deviation:** the verified file kept it as a wrapper. Deleting it leaves one list
  instead of a wrapper around it.
- `remove_remotely_deleted` drops its now-redundant `self.selected_tabs.remove(id)`. All three of its removal branches reach
  `forget_id`. Its `wakes_in_flight.remove` stays (verified file, gap 3).
- `teardown_running_session` calls `forget_id` for every id in `nodes` and in `spawning`, just before `nodes.clear()`. It keeps
  its session-wide clears.
  - **Deviation:** the verified file named `nodes` only. A tab whose spawn is in flight has no `nodes` entry, but it can
    already hold cookie and popup state.
- **Deviation:** `navigated_from` is in `forget_id`. It is a per-tab table that the verified file's list missed, and teardown
  already clears it.
- No separate `forget_all`: it would be a sixth list to drift.

**Tests.**
- `forget_id_clears_every_id_keyed_table` (`main.rs` tests). Fill every table above for `"t"`, plus a `"u"` control, then
  call `forget_id("t")`.
  - Every `"t"` entry must be gone, and `"u"`'s must stay.
  - It fails first only because `forget_id` doesn't exist.
- `every_id_keyed_field_is_forgotten_or_excused` (`main.rs` tests). This is a source scan in the style of `profile::tests`,
  and the guard that matters: it catches a new table nobody added.
  - It reads `include_str!("main.rs")` and takes the fields of `struct App {`. **Deviation:** the verified file wrote
    `pub(crate) struct App {`, but the real line is `struct App {`.
  - It lists every field whose type mentions `HashMap<String`, `HashSet<String`, `Option<String`, `Option<(String` or
    `Vec<String`.
  - Each must appear exactly once across two places: `self.<name>` in `forget_id`'s body, or the `NOT_FORGOTTEN` allowlist
    of (field, reason). Appearing in both fails too.
  - Every allowlisted name must also still be a field.
  - **Deviation:** the scan includes `Vec<String`, which the verified file left out.
- `teardown_forgets_the_old_tabs_per_id_state` (`session_sync/tests.rs`). After `teardown_running_session`, `saved_scroll`,
  `tab_crashes`, `skip_convert` and `jar_pulls` must be empty. It fails before the change.
- The GPU frees can't be unit-tested, because `gpu` is `None` under test, and there is no texture-count export. They are
  proven by review: each `free_*` is a no-op for an id it lacks (`gpu.rs:760`, `778`, `884`).

## H1. The rectangle selection drops tabs no longer on the level (`graph_nav.rs`, `main.rs`)

**Bug.** `selected_tabs` is meant to hold only this level's tabs. A remote move breaks that:
- `follow_remote_group` and `mirror.set_group` don't touch the selection.
- So after another replica moves a selected tab into another group, "Close Selected (2)" closes a tab the user can't see.

**Change.**
- Add `App::end_stale_selection` beside `end_stale_drag`. It keeps only the ids that are still in `nodes` and still
  `in_level`.
- `build_ui_inner` calls it every frame, right after `end_stale_connect` (`main.rs:2861`).

**Test.** `a_tab_another_replica_moved_away_leaves_the_selection` (`graph_nav.rs` tests). It runs through the real frame:
- Settings view, two selected tabs.
- `follow_remote_group(b, Some(h))`.
- One `build_ui_inner` frame, then `CloseSelected`.
- `a` must be closed and `b` must still be in `nodes`.
- It fails before the change on behavior, not on a missing name.

The full `GraphInteraction` extraction is parked.

## M5. A Json view keeps what this build can't read (`json_view.rs`, `json_level.rs`, `mirror.rs`)

**Bug.** `JsonView::from_field` turns any text it can't read into an empty view. One bad entry (a newer build's
`pages: {offset, pages}`, or a hand edit) fails the whole map. The next edit then writes the empty view's text over the
field, which syncs to every replica.

**Change.** `JsonView` keeps `.0` (so no call site changes) and gains `.1: Unread`, which holds three things:
- `entries`: whole entries that don't read as a `CardOverride` (wrong shape, or not an object), kept as `Value`s.
- `extra`: the unknown keys of entries that do read.
  - **Deviation:** the verified file dropped these. Today a newer build's added key, such as `"width"`, is silently lost on
    the next edit here. Keeping it costs one map.
- `opaque: Option<String>`: the raw text, when the text isn't an object at all.
  - **Deviation:** the verified file used a `bool`. Keeping the text makes `to_field` return it unchanged, so even a write
    that does get through (the Refresh prune compares views, not text) writes back the same bytes.

The methods:
- `from_field` reads each entry on its own: a bad entry no longer costs the good ones. For opaque text it logs, and stores the
  raw text.
- `to_field`:
  - returns opaque text unchanged;
  - otherwise writes the unreadable entries whole, and each override with its unknown keys after its own;
  - serialises `CardOverride` directly (through a `#[serde(flatten)]` wrapper), so `f32` positions keep their short form and
    today's text for a view with nothing unread is byte-for-byte the same.
- `entry(pointer)` removes an unreadable entry at `pointer`: the user's edit replaces that card's unreadable state.
- `retain_resolvable` also prunes `entries` and `extra` by pointer. It never touches opaque text.
- `is_empty()` checks all parts, and `Mirror::set` uses it, so a view holding only unread state is still cached.
- `is_opaque()`: `edit_json_view` refuses an edit to an opaque view. It relays the level out from the unchanged view (a drag
  moves its card live, so a refused drop would otherwise leave the card where it was dropped), queues the toast "This JSON
  view was saved by a newer manifold; changes to it aren't saved", and returns before writing.
  - **Deviation:** the verified file asked for a one-time toast with slightly different words. This one toasts on every
    refused edit and adds no state: each refused edit is a separate action that would otherwise silently do nothing, and a
    "told once" flag would need clearing whenever the view changes. The words name the node kind as well.
- Relayout clears `pos` only in the overrides this build reads. A position inside an unreadable entry is kept, like the rest
  of that entry: this build never edits what it can't read, apart from replacing a whole entry the user edits (`entry`).
- `from_field` logs for opaque text only, not per unreadable entry. Its one caller outside tests is `Mirror::set`, which
  sees VIEW at load and then only when its value changes (`edit_field` and the remote apply at `session_sync.rs:1138`
  compare first), so one bad view logs once per load and once per change, not per sync. Rejected from the cross-check:
  moving the log to the refusal site, which the toast already covers.
- The older spec `docs/superpowers/specs/2026-09-27-json-node-rework-design.md` ("Saved view state") gets one sentence:
  unreadable entries, unknown keys and unreadable text are now kept and written back.

**Tests.**
- `an_entry_this_build_cannot_read_survives_an_edit`: the verified file's test.
- `unknown_keys_and_unreadable_text_are_written_back`:
  - an extra key survives an edit;
  - a non-object entry is kept;
  - `f32` text stays short;
  - opaque text round-trips.
- `card_override_keys_are_the_fields_it_reads`: a guard that the known-key list matches `CardOverride`'s serialized fields.
- `an_unreadable_view_is_never_overwritten` (`json_level.rs`): VIEW `"[1]"` is still exactly `"[1]"` after a Toggle and a Move,
  and the dragged card is back at its laid-out rect.
- The existing assertion `from_field("not json") == default` becomes `from_field("not json").is_opaque()`.

## H4. One unreadable setting resets only itself; zoom is clamped (`settings.rs`, `shortcuts.rs`, `settings_page.rs`)

**Bug.** `Settings` already has `#[serde(default)]`, but the enums are strict. An unknown search engine, an unknown starting
engine, or a wrongly typed `zoom_factor` fails the whole file. `load_or_move_aside` then moves the file aside and every setting
resets: home page, CEF directory, window geometry.
- A hand-edited zoom of `0` or `-1` reaches the engines as is.

**Change.**
- Three per-field lenient deserializers (`search_engine`, `starting_engine`, `zoom`) sit on one helper, `lenient`. A value the
  helper can't read loads as that field's default, with a log line.
- `zoom` also clamps into the new `pub(crate) const ZOOM_RANGE: RangeInclusive<f64> = 0.5..=2.0`.
- The shared `engine_kind_serde` is not changed; only its `serialize` is reused.
- `ZOOM_RANGE` replaces the literal bounds in `zoom_in_shortcut` / `zoom_out_shortcut` (`shortcuts.rs:299`, `310`) and in the
  settings page's −/+ buttons (`settings_page.rs:156`, `160`).
- Change from the verified file: the default is passed as a closure, so `Settings::default()` runs only on a bad value.
- README "Settings" says what now happens.

**Test.**
- `an_unknown_value_resets_only_its_own_setting`: the verified file's test.
- `zoom_is_clamped_and_a_wrong_type_is_its_default`:
  - `9` loads as 2.0;
  - `"1.2"` loads as 1.0;
  - the defaults round-trip.

A wrong type in any *other* field still sets the whole file aside. That stays out of scope (see "Parked").

## M3. A cookie dump that doesn't parse is logged, without values (`jar.rs`, `tabs.rs`)

**Bug.** `jar::from_json` is `serde_json::from_str(json).ok()`, and `apply_pulled_cookies` returns `false` on a `None`.
- Nothing logs it, so a tab whose engine's dump shape drifts stops saving logins, with no trace anywhere, every 30 s.

**Change.**
- `from_json` returns `Result<Vec<Cookie>, String>`. The error gives only the category and position
  (`"{:?} error at line {} column {}"`). serde's message can quote the input, and a cookie value is a credential.
- `apply_pulled_cookies` logs `jar: tab {id}'s cookie dump didn't parse ({err}); its jar is not updated` and returns `false`.
  This covers both of its callers (`event_loop.rs:1216`, `tabs.rs:332`).
- The parse stays all or nothing. A comment at the match says why.

**Test.** `a_bad_dump_reports_where_without_echoing_a_value`.
- The bad cookie's value and its `same_site` are both `"SECRET"`. With serde's own message, "unknown variant `SECRET`" would
  echo it; the error must not contain `SECRET`.
- A dump with one good and one bad cookie must be an error as a whole.
- The existing `from_json("nope").is_none()` becomes `.is_err()`.

## M4. An unknown argument, or a flag missing its value, refuses to start (`cli.rs`, `profile.rs`, README)

**Bug.** Five independent argument scans, and nothing rejects anything.
- `--hedless`, `--profile-directory X` or a bare `--profile-dir` each open the GUI on the real profile.
- `--profile-dir --headless` takes `--headless` as the directory.

**Change.**
- `cli.rs` gains `FLAGS` (the eight flags this build reads, each with whether it takes a value) and `validate`.
- `run` calls `validate` first and returns `Some(2)` with `manifold: <why>` on stderr. `cli::run` is already `main`'s first
  call, before `profile::dir()` is read.
- A value must be present, non-empty and not start with `--`. `--profile-dir=<dir>` needs a non-empty `<dir>`.
  - Change from the verified file: an empty value (`--profile-dir ""`) is refused too. An unset `$DIR` quoted in a script
    gives exactly that.
- The scans themselves stay; consolidating them is parked.
- `profile.rs`'s two fallback assertions (`--profile-directory`, a bare `--profile-dir`) stay, with a comment that `validate`
  now refuses both before `dir_from` runs.
- README's `--script` / `--profile-dir` paragraph says what is refused.

**Test.** `an_unknown_flag_or_a_missing_value_refuses_to_start`: the verified file's cases, plus `--profile-dir=`,
`--profile-dir ""`, a bare positional argument and no arguments at all. The valid `--import-json … --out …` form stays
covered by the existing `import_json_builds_a_clean_file_that_dumps_back`.

## G2. Guard: `Carried` holds every tab field (`mirror.rs`)

Refuted as a bug:
- A respawn never re-seeds: the mirror entry outlives it.
- `Carried` already covers every tab field.

The remaining risk is drift: a new tab field added to `TYPES` but not to `Carried` would not be seeded for a brand-new spawn.

**Change.** Test only: `carried_holds_every_tab_field`.
- Set a non-absent value for every `Kind::Tab.fields()` entry, plus parent and group.
- Then `carried` → `seed` onto a fresh mirror, and compare. It passes today.

## Not in scope / parked

| Item | Why it waits |
|---|---|
| The full `GraphInteraction` extraction (H1) | A mechanical M/L diff over about 10 files that fixes no further user-visible bug once C1 and H1 land. It conflicts with C1's and P1's edits to the same functions. Do it after this wave, when `forget_id` has removed most of the per-id lines it would otherwise move. |
| One `Args` parsed once and passed down (M4) | M4's `validate` removes the hazard: every argument is now checked once, up front. Consolidating the five scans (`cli::run`, `main`'s two, `profile::dir_from`, `debug_control`) is a refactor with no behavior change. |
| `Carried` as a field map (G2) | No bug, and it costs type safety. It would lose the typed partial construction at `place_node`, `seed_landing` and `TabSpawn::new`, and would still have to keep "seed nothing once the entry exists". |
| Every close path saves once (P1 follow-up) | Make `destroy_node`'s Tab arm `remove_tab` and add one save after each loop (`close_all_except`, `close_all_at_current_level`, `close_focused_tab_shortcut`). Before that, all 11 non-test `close_node` callers need an audit: a missed save means a closed tab comes back after a crash. It is a separate S task once the audit is done. |
| A rate-limited cookie-failure toast (M3) | One log line per tab every 30 s is the minimum. A toast needs per-tab "already told" state, which is one more id-keyed table (C1's drift). Revisit if the log proves unseen. |
| Keeping unknown settings fields (H4) | `#[serde(flatten)] extra` would stop an older build dropping a newer build's fields on its next save. It is a downgrade concern, not this wave's per-value one. A wrong type in another field still sets the file aside; a hand-written `Deserialize` over a `Value` map would fix that, but isn't worth it now. |
| A GPU texture-count export (C1) | It would make the teardown free provable in a probe. `EXPORTSTATE` has no such field today, so C1's GPU frees are proven by review. |

## Risks

- **C1 ordering.** `drop_tab` must still reparent the children before `nodes.remove`, and keep its focus, split and view-mode
  logic after.
- **C1 scan reach.** The scan sees only fields whose type names a `String` key or value. A table that holds a node id
  inside another type (`pending_jar_confirm`, `context_menu`, `pending_focus`, `json_level`) is outside it. Those reset
  themselves or are reset by their own feature today; widening the scan to them is not part of this wave.
- **C1 lifecycle tables.** If a lifecycle table ever went into `forget_id`, a tab closed while waking would never commit its
  delete. The scan test's allowlist says why each one stays out.
- **P1 waking tab.** A `remove_tab` of a waking tab now has its delete committed by `destroy_group`'s save rather than at the
  wake's landing, which then saves a no-op. That is the same end state.
- **M4 launchers.** Any launcher passing a flag not in `FLAGS` now exits 2. A grep of `scripts/` (including the
  comma-separated `-ExeArgs` of `json-pan-profile.sh` and `foreground-watch.ps1`'s `.Arguments`) found only listed flags.
  The gate does not check this: `two-instance-test.sh` and `headless-smoke-test.sh` launch with `--headless` alone. So M4's
  task runs each other launch form by hand, headless and in a temp profile: `drive.sh` with a seed (`--import-json … --out
  …`, then `--headless`), json-pan-profile's `--headless --profile-dir … --script …`, `--dump`, `--check`, and one refused
  flag. `--debug-commands` opens a real window, so the unit test covers it instead.
- **M5 unreadable pointers.** An unreadable entry keeps its pointer even if the user never sees that card. `retain_resolvable`
  drops it once its pointer no longer resolves after a Refresh.
- **H1 respawn.** A tab mid-respawn drops out of the selection. That is acceptable: it isn't drawn.
- **H1 and headless scripts.** The per-frame prune also runs in headless runs. No debug command puts an id into
  `selected_tabs`: only a click or a marquee on drawn cards does (`card.rs`, `graph_ui.rs`), and the smoke test's marquee
  selects live tabs. So a script can't select an id that the prune then drops.

## Testing

- Per task: the named tests, run with `cargo test -j 8 -p manifold <name>` and seen to fail first where that is possible,
  then the module's tests, `cargo fmt --all` and clippy. M4 also launches the exe once in each launcher's form, by hand
  (see Risks).
- Once for the wave, before the merge:
  - the full `cargo test -j 8 -p manifold`;
  - `cargo build -j 8 -p manifold`;
  - `scripts/gate.sh LOG full`, detached at BelowNormal, reported with CPU load and Defender state.
