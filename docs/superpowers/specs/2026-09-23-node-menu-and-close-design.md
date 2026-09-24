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
