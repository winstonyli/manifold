# Unified Node Context Menu and Close Path Design

## Goal

Apply the toolbar rewrite's "shared frame + per-kind content, dispatched on
`ItemNodeKind`" pattern to two more places that are hand-copied once per
node kind:

1. **Context menus** — seven per-kind `*_context_menu`/`*_context_menu_cache`
   field pairs and seven builder calls become one state and one dispatcher.
2. **Close/delete** — five per-kind `*_close_requested` locals, four
   per-kind menu Delete arms, and four `reveal_*` functions become one
   `close_node`, one `reveal_node`, one `node_copy_path`.

Behavior stays identical except for one deliberate fix (below).

## Behavior change (decided)

Today, for Note/Image/Pdf/Json, a card's **menu Delete** adds the node to
Recently Closed but its **hover ✕** does not, so a hover-closed note can't
be reopened. After this change **both record**, matching menu Delete,
closing a tab, and `close_group_member` (closing a whole group).

## Plan-time corrections (to the spec)

Found while writing the implementation plan; these supersede the sections below where they conflict.

1. **One shared fade Id** (`"node_menu_fade"`), not one per kind. Per-kind fade Ids keyed off the *current* kind break `fade_popup_state`'s "poll every frame" rule (a kind's animation would go unpolled while another kind's menu is open, so the next open jumps straight to visible). Cost: right-clicking a card of a *different* kind while a menu is open now moves the menu instantly instead of cross-fading — exactly what right-clicking a second card of the *same* kind already does today. The popup **area** Ids stay per kind (`"download_context_menu"`, …, `"tab_context_menu"`), carried in `NodeMenuConfig::area_id` for the five simple kinds.
2. **`node_file_path`**, not `node_copy_path` — it feeds both Reveal and Copy Path, so it's named for what it returns.
3. **Escape closes any node menu.** Today Graph view's Escape clears only `group_context_menu`; with one state it clears `node_menu`, so Escape now also dismisses a Download/Note/Image/Pdf/Json/Tab menu. A second, deliberate, tiny behavior change (flag it in the final report).
4. **Tab hover ✕ keeps its own `close_requested` local** (applied as `close_tab`, before `clicked_focus`) — `close_node`'s Tab arm is exactly `close_tab`, and folding it in would move its apply point past `clicked_focus` for no gain.
5. **The Downloads popup's per-row Remove** also goes through `close_node` (it is a single node closing; spec: "used everywhere a single node closes").

## Non-goals

- The Tab-view link menu (`App::context_menu`, from `ContextMenuLink`) —
  a different menu; untouched.
- Group and Tab menu *bodies* (rename field, color swatches, pin,
  duplicate, …) — kept as-is; only their state moves to the shared field.
- "Close all at this level" / "close all except" still close only tabs
  (pre-existing scope gap).
- The graph card rendering loops (a separate, larger generalization).
- Any visual change to menus.

## Current state (for reference)

- State: `group_`, `download_`, `note_`, `image_`, `pdf_`, `json_`,
  `tab_context_menu` (+ `_cache` each). Every right-click in
  `build_graph_ui` resets all seven by hand; entering/leaving a group
  clears all seven. Builders are called in order group → download → note
  → image → pdf → json → tab.
- `build_node_context_menu(ui, id_str, pos, fade, NodeMenuConfig) ->
  (Option<NodeMenuAction>, clicked_outside)` already draws the menu for
  Download/Note/Image/Pdf/Json; `NodeMenuConfig { copy_text,
  reveal_and_copy_path, show_expand, has_group, delete_label, width }`;
  `NodeMenuAction { CopyText, Reveal, CopyPath, ExpandAsGraph,
  RemoveFromGroup, Delete }`. Group and Tab menus hand-roll their own
  `Area`/`Frame`.
- Per-kind configs today: Download (`reveal_and_copy_path`, `has_group:
  false` hardcoded, "Remove from List", width 170); Note (`copy_text`,
  real `has_group`, "Delete", width 150); Image/Pdf (`reveal_and_copy_path`,
  real `has_group`, "Delete", 170); Json (`reveal_and_copy_path`,
  `show_expand`, real `has_group`, "Delete", 170).
- `close_group_member` already performs the complete per-kind close
  (record to Recently Closed skipping synthetic JSON leaves, free GPU for
  image/pdf, clear `json_viewer_node`, remove; Tab → `close_tab`, Group →
  `close_group`) without saving — its callers save once.
- `reveal_download` translates WSL paths (`windows_path_for_download`);
  `reveal_image`/`reveal_pdf`/`reveal_json` use the stored path. Download
  Copy Path also translates; the others copy the stored path.

## 1. One context-menu state

- `App::node_menu: Option<(String, egui::Pos2)>` and
  `App::node_menu_cache: Option<(String, egui::Pos2)>` replace the seven
  pairs. At most one menu is open by construction.
- Right-click on a card sets `node_menu = Some((id, pos))`; right-click on
  empty canvas sets it `None`. The group enter/leave clearing becomes one
  assignment.
- `build_node_menu(ui)` replaces the seven builder calls, at the position
  of the first of them (the group call) so z-order relative to other
  popups is unchanged. It runs `fade_popup_state` on the shared
  state/cache, resolves the node's `ItemNodeKind` (node gone → clear and
  return), then:
  - Download/Note/Image/Pdf/Json → `node_menu_config(kind, has_group)`
    returns exactly today's `NodeMenuConfig` for that kind (Download keeps
    `has_group: false`), passed to `build_node_context_menu`; the returned
    action is applied by one match: CopyText → copy note text; Reveal →
    `reveal_node`; CopyPath → `node_copy_path` + copy; ExpandAsGraph →
    `expand_json_as_graph` + save; RemoveFromGroup → clear `group`;
    Delete → `close_node` + save (Download: no save, as today).
  - Group → today's `build_group_context_menu` body; Tab → today's
    `build_tab_context_menu` body — each adapted to read/clear
    `node_menu`, otherwise unchanged.
  - Any action or click-outside clears `node_menu`.
- The `egui::Id` strings used for each kind's popup area and fade stay
  distinct per kind (derived from the kind), so egui memory/animation
  state doesn't bleed between kinds.
- `DebugCommand::ExportGroup` reads `node_menu` filtered to a Group node.

## 2. One close path

- `close_node(&mut self, id: &str)` is `close_group_member`, renamed and
  used everywhere a single node closes. It does not save; callers save
  once (so closing a group still saves once).
- Callers: hover ✕ (one `node_close_requested: Option<String>` replaces
  `note_`/`image_`/`pdf_`/`json_`/`download_close_requested`), menu
  Delete, and group close. Tab hover ✕ keeps `close_tab` via `close_node`'s
  Tab arm. **Group hover ✕ keeps calling `dissolve_group`** (that is its
  behavior today — not a close). Download removal still doesn't save the
  session (downloads aren't persisted).
- `reveal_node(&self, id)` and `node_copy_path(&self, id) -> Option<String>`
  replace the four `reveal_*` functions and the per-kind Copy Path code;
  Download goes through `windows_path_for_download`, the rest use the
  stored path.

## Testing

- Unit: `node_menu_config` — one assertion per kind against today's
  values (table test).
- Unit: the Recently Closed recording rule extracted as a pure function
  (e.g. which `ClosedNode`, if any, a payload produces — `None` for
  Download, Tab, Group, and synthetic JSON leaves).
- Live headless (APPDATA isolated), baselines captured before any code
  change: right-click each kind → menu screenshot identical to baseline;
  menu Delete and hover ✕ on a note/image/pdf/json each put it in Recently
  Closed and reopening restores it; group hover ✕ still dissolves; tab
  menu Close still closes; download "Remove from List" still removes.

## Follow-ups

- Menu "Remove from Group" sends a node to the top level, while the card's
  hover eject sends it one level up — inconsistent for nested groups
  (pre-existing).
- **Done** (graph card frame, `docs/superpowers/specs/2026-09-23-graph-card-frame-design.md`): a download joins a group by dragging onto it like any card, and its menu offers "Remove from Group" when grouped.
- **Done:** one card frame for Download/Note/Image/Pdf/Json (same spec).
- **Fixed after merge review — Note hover was a real bug:** a Note card was
  never hovered at all (no hover border, no hover ✕), with a real mouse too,
  ever since Notes were added (`87d0e95`). Its hover-only interact was
  registered before its own `click_and_drag` interact, and egui 0.36
  (`interaction.rs`) counts a hover-only widget as hovered only when it sits
  on top of the topmost interactive widget under the pointer. It's now
  registered after the card's content (the hover border moved with it). Live
  checked: hover ✕ → Recently Closed (1) → reopen restores the note; drag
  and click behave the same as before the change. The rest of the "unreliable
  headless" symptoms were driver misuse, now written up on `DebugCommand`:
  a `MOUSEDOWN`/`MOUSEUP` pair spaced by `sleep 1` is a long press, not a
  click; a drag needs several moves; and a menu can be caught mid-fade-in if
  no frame renders between the click and the `SCREENSHOT`.
- Card hover is sticky for every kind: `is_hovered` includes
  `self.hovered == id`, and `finish_draggable_node` stores `is_hovered` back
  into `self.hovered`, so once hovered, a card keeps its ✕ until another card
  takes the hover (pre-existing). That's probably what keeps the ✕ reachable,
  since half of it hangs outside the card, so fixing it means giving the ✕'s
  own hover a say.
- **Fixed after merge review:** "Close Group" never saved, and there's no
  save on exit, so a closed group's Note/Image/Pdf/top-level-Json members
  reappeared after a restart unless another action saved first (Tab members
  were fine — `close_tab` saves). `close_group` now saves once at the end.
  (The review's first framing — "the group comes back" — was wrong: groups
  and "Expand as Graph" leaves aren't in `session.json` at all, and
  Dissolve / group hover ✕ change nothing persisted, so they need no save.)
- **Done:** closing a blank note (empty or whitespace-only) no longer adds it
  to Recently Closed. Before this, either close path recorded
  `ClosedNode::Note("")`, which restored nothing and could push a real entry
  out of the 10-slot cap. `closed_node_for` skips blank notes (unit-tested).
- Note edit mode never calls `request_focus()` on its `TextEdit`: after
  clicking a note to enter edit mode, you have to click again to actually
  type, and if you never click into it, it never loses focus either, so the
  note stays in editing mode indefinitely. Predates this branch — the old
  Note loop had no `request_focus()` call either (pre-existing).
- A card menu can close on the frame it opens. `clicked_outside_popup`
  runs on that first frame too, which is also egui's sizing pass for a
  never-shown `Area`. The sizing pass assumes a default ~400 px tall area,
  so a menu opened in the lower part of the screen is placed above the
  pointer. If the right button is released in that same frame, the release
  counts as a click outside the menu and closes it. The headless
  `CLICK ... right` always does this (found while checking Download menus
  with `ADDDOWNLOAD`). A real mouse normally releases a frame or more
  later, but a touchpad two-finger tap might press and release within one
  frame. Unverified on real hardware. A possible fix: skip the click-outside
  check on the frame the menu opens (pre-existing).
- A "Try grouping" Group card's own mini thumbnail (of its member tabs)
  sometimes renders blank in headless screenshots, seen across otherwise-
  identical runs during the graph-card-frame before/after comparison
  (`task-3-report.md`, "Group-thumbnail flake, investigated"). Traced to a
  pre-existing timing race in the thumbnail texture path specific to
  `ItemNodeKind::Group`'s own render branch, not to the shared card frame
  (`paint_card_body`/`card_style`/`draw_node_close_button` are never touched
  by that branch and were unaffected either way).
- **Fixed (`5699c3b`):** every parent->child edge was painted twice (double
  alpha on the translucent stroke). The download-only edge loop from
  `191fd30` became a copy of the all-nodes loop when `182e7e9` moved
  downloads into `self.nodes`; the copy is gone. The first-launch demo has
  no edges, so it can't show edge changes. To check edges headlessly, build
  `manifold-engine-webview2` too, then pre-seed the isolated APPDATA with a
  `settings.json` containing `"has_shown_first_launch_demo": true` and a
  `session.json` whose `tabs` are one root plus children (`"parent":
  "tab-10"`, `"engine": "webview2"`, `"url": "manifold://welcome"`, spread
  `pos_x`/`pos_y`). Run the in-tree `target/debug/manifold.exe`; copies of
  it elsewhere didn't start their engine hosts. `WAITTABS` (added later)
  replaces the old ~25 s wait before `SCREENSHOT`. The focused card's live thumbnail varies from run to run, so
  mask it out before diffing.
- **Fixed:** `IMPORTSESSION` + `CONFIRMIMPORT` while the first-launch demo's
  tabs were still spawning let those demo tabs land in the imported scene. When
  an imported id matched an in-flight one (`tab-0..2`), the late result replaced
  the imported node and that tab looped on "engine-host crashed ... restarting".
  The real Import Session dialog and Load Session hit the same race.
  `replace_running_session` now bumps `App::session_generation`, and
  `handle_tab_spawn_result` drops a spawn result from an older generation (the
  dropped `EngineTab` shuts its host down). Repro: a three-tab session file with
  ids `tab-0..2`, then `RESIZE`, `IMPORTSESSION`, `CONFIRMIMPORT` right away,
  `WAITTABS`, `sleep 8`, `EXPORTSESSION`. Before the fix, 1 of 2 runs
  crash-looped (5 restarts). After it, 4 of 4 runs came out clean: exactly the
  imported tabs, no crash lines, and no engine processes left afterwards.
- **Fixed:** "a restored/imported tab with `"url": "about:blank"` never
  appears" was never about `about:blank`. The seeded `session.json` left out
  `next_tab_id`, which had no `#[serde(default)]`, so it failed to parse.
  `Session::load_from` dropped the error (`.ok()`), and startup fell back to
  the first-launch demo. The demo's tabs are `manifold://welcome`, so a
  failed welcome seed looked like it had worked. The same seed with
  `next_tab_id` restores four `about:blank` tabs; so does a seed without it
  now. Also, the recipe's settings seed (`{"has_shown_first_launch_demo":
  true}` alone) doesn't parse either (`Settings` has required fields). It
  only worked because a restored session never reads the flag. Changes:
  - `next_tab_id` defaults like the other counters, and `apply_session`
    raises every id counter past the ids it restores (`bump_past_id`). A
    file with `next_note_id` etc. missing or too low used to hand the next
    new node an id already in use.
  - `Session::load_from`/`load_named` return the error, and every caller
    (startup, Open Session, the Import dialog, `IMPORTSESSION`) toasts it.
  - At startup an unreadable `session.json` is renamed to
    `session.unreadable-<secs>.json` before the fresh start. Before, the
    first save overwrote it: the fingerprint guard only covers a file this
    run loaded.
  - The toast is drawn in every view from `build_ui`. It was only drawn in
    Tab and Image view, so toasts queued in Graph view (crash/respawn
    notices included) were never seen.
  Checked headless: the recipe seed without `next_tab_id` restores
  `tab-10..13`, and a new tab gets `tab-14`. A truncated `session.json`
  toasts, is moved aside intact, and the demo loads. Follow-up: settings
  and bookmarks had the same silent fallback and overwrite. `json_file.rs`
  now holds `load` and `load_or_move_aside` for all of them, group import
  included. `Settings` is `#[serde(default)]`, so a partial file keeps what
  it has, and the recipe's one-field settings seed now loads. A toast's 4 s
  now starts when it's first drawn, not when it's queued. Checked headless:
  corrupt settings, bookmarks and session files are all moved aside, and
  their three toasts show in turn. Then: load-error toasts name only the
  file (`json_file` logs the full paths to stderr), and an image or JSON
  node whose file is missing or unreadable is toasted by
  `insert_image_node_with_id`/`insert_json_node_with_id` themselves.
  Session restore, add-file and reopen all used to drop the node silently.
  On restore the card now stays as a placeholder (`ImagePayload::error`/
  `JsonPayload::error`): the failed-PDF box with a `FILE_X` glyph and the
  error as its tooltip. It stays in `session.json`, and Delete removes it.
  A click tries the file again (`retry_missing_file`): loaded, the real card
  takes its place (same id, position, group, color and draw order); still
  failing, it toasts and keeps the new error. Checked headless: both
  placeholders render with their tooltips and survive the next save; a
  click while the file is missing re-toasts; after the files appear, a
  click on each turns it into the real image/JSON card in place. A failed
  PDF card (always kept) now toasts too, with a one-line reason, and a
  click renders it again; checked the same way.
- **Done:** a node menu's fade-out used to redraw from the node's *current* state, so it changed while it faded: after "Remove from Group" the row vanished and the menu shrank, and Pin turned into Unpin. Now `build_node_menu` reads a `NodeMenuView` each open frame, `node_menu_cache` freezes it for the fade-out, and a menu whose action removed its node (Delete, Close, Dissolve) fades too instead of vanishing. A fading menu is also click-through (`Area::interactable(false)`), so the rest of a double-click can't run an item twice. The four toolbar list popups (`build_list_popup_frame`) got the same click-through. Checked headless with the fade temporarily slowed to 3 s: the fading menu kept its "Remove from Group" row after the download had left the group, and a second click on it went through to the tab card behind. egui builds its hit test from the previous frame's layers, so the first fading frame can still take a click.
- **Done:** every other fading popup with clickable content is click-through while fading too: the quick switcher, the Tab-view link menu, tab actions, and the three engine pickers (`build_engine_picker_popup`). Peek preview only paints. Still open: the toolbar list popups redraw live content during the fade (after a Recently Closed reopen, the list loses that row mid-fade). The fade lasts ~83 ms, so this is cosmetic.
