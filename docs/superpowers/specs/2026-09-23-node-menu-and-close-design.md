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
- A Download's menu never offers "Remove from Group" (pre-existing).
- The graph card rendering loops are still per kind (the next
  generalization of this pattern).
- Headless right-click/hover-close on a Note card was unreliable (worked in
  one test session, not another) — reproduced on the pre-change binary, so
  pre-existing; unknown whether it's a real hit-test bug or a headless-driver
  quirk. When investigating, also verify Note hover ✕ → Recently Closed →
  reopen live (so far code-inspected only).
- **Fixed after merge review:** "Close Group" never saved, and there's no
  save on exit, so a closed group's Note/Image/Pdf/top-level-Json members
  reappeared after a restart unless another action saved first (Tab members
  were fine — `close_tab` saves). `close_group` now saves once at the end.
  (The review's first framing — "the group comes back" — was wrong: groups
  and "Expand as Graph" leaves aren't in `session.json` at all, and
  Dissolve / group hover ✕ change nothing persisted, so they need no save.)
- Hover ✕ on a never-typed note now records `ClosedNode::Note("")`, which
  can push a real entry out of the 10-slot Recently Closed cap (menu Delete
  already did this). Consider skipping blank notes in `closed_node_for` — a
  behavior change, needs a decision.
