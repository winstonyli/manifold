---
status: draft
---

# Groups as scopes, the nested layout and the hard anchor

Source: the PARKED entries "Groups as scopes inside one document", "One hard anchor register replaces `PINNED`", "Nested
bottom-up settle", "Soft pins", "Inherited context helper" and "Keep similarity edges in every settle of an arranged
level". Decisions (user, 2026-10-31 and 2026-11-01): groups are scopes inside one document (e1; separate documents
decided against); the container stays `Node.group` with levels (h1); derived regions first with `POS` kept as the seed
(j1); a dragged card is held by a hard anchor and soft pins are decided against (c1); no `rank` register for now (l1);
similarity edges stay a local Arrange by topic action (n1, revisit after this spec). The prototype numbers are in the
PARKED entry "Nested bottom-up settle" (675 cards, 60 groups, run 2026-10-31, output not kept): 33 ms for a full nested
pass against 690-730 ms flat, and about 15-20 passes to converge. Nothing else the prototype showed is relied on here.

## Problem

1. A group is a card you open into a level, so a group's members are never visible beside its siblings and the layout
   has no notion of a region.
2. Layout is one flat simulation per level (`settle_current_level`, `graph_nav.rs:554-600`); positions are
   group-local, so a nested layout is only reachable by opening each group.
3. A drag only seeds `POS` (`release_drag`, `graph_nav.rs:206-222`). The next settle (a card added, closed or resized,
   or a level change) restarts every unpinned card from `POS` and lets it drift: in the prototype a dropped card
   drifted 358-422 px over 20 added cards.
4. `PINNED` does three jobs: the layout hold (`graph_nav.rs:573-577`), the keep-awake exemption (`tabs/sleep.rs:136`
   into `idle_sleep.rs:29`) and the pin mark and menus. It also rides respawn (`TabCarry.pinned`).
5. The view state of a level (camera, thumbnail-grid hiding) is one global value (`local_state.rs:34`, `:380`), so
   switching levels forgets where you were.

## Design

Three slices, in this order. Slice 1 and Slice 2 are each checkable alone; Slice 3 needs Slice 2's per-level state.

### Slice 1: the anchor register replaces `PINNED`

One composite register, field 27, `ANCHOR`: `{1: x, 2: y, 3: group, 4: explicit}`. One idea: an anchor holds a card at
a point. The explicit bit says only who may release it: Tidy and Arrange release implicit anchors, only Unpin releases
an explicit one.

- **Encoding.** A new `FieldType::Anchor` and `FieldValue::Anchor(Option<Anchor>)` with
  `struct Anchor { pos: [f32; 2], group: Option<NodeId>, explicit: bool }`, modelled on `Pos` and `Held`
  (`store/model.rs:232-250`, `:423-448`). `Mirror::anchor(id) -> Option<Anchor>`. A set anchor is always written, even
  at the origin of the top level with no explicit bit, so it never reads as absent (the `Pos(Some)` rule); a clear is
  the empty encoding (as `HELD`, `groups.rs:74`). `ANCHOR` replaces `PINNED` in every kind field list and in `GENERIC` (`model.rs:183-198`, including Jar and Group; the
  asserts at `:946` and `:955` follow). Field 7 goes into `RESERVED` (`model.rs:330`, a `cfg(test)` array that
  grows from `[u32; 2]` to `[u32; 3]`; the test at `:817-821` already asserts no `FIELDS` entry is reserved); 25 and 26
  are taken (`model.rs:299-300`), so `FIELDS` stays at 19 entries.
- **No `FORMAT_VERSION` bump** (still 4, `journal.rs:26`). An unlisted field is stored raw and survives an old build's
  compaction (`store/ops.rs:86-87`, `:103`; test `a_newer_builds_frames_and_fields_survive`,
  `store/journal/tests.rs:236`). Fields 22 and 23 were added the same way; re-typing `HELD` needed a bump
  (`9db8124e`), which a new field avoids. One untested path: a `Create` carrying the new number through a snapshot and
  an old build's compaction, so a test with field 27 is part of this slice. `PROTOCOL_VERSION` is untouched: a peer on
  an old build keeps writing field 7, which a new build ignores, so mixed-version pins diverge (accepted; the next
  release notes say all peers should upgrade together).
- **Meaning.** The point `(x, y)` is in the group's local coordinates, as `POS` is. One predicate decides a hold:
  `Mirror::held(id) = anchor set && (explicit || anchor.group == mirror.group(id))`. It depends on the document only,
  so replicas agree.
    - An implicit anchor holds only in the group where it was written. A local Move, Join or fold clears it; an inert
      one arises only from a remote move that raced the write. Every reader ignores an inert anchor.
    - An explicit pin ignores the group: after a Move it holds at the same `(x, y)` read in the new group, exactly
      what its `POS` seed does today, and returns to its old meaning if the card is moved back.
    - `Mirror::pinned(id)` (the name callers already use) is the explicit bit, any group. The keep-awake exemption, the
      pin mark, the menu rows and Pin All read it and behave as now.
  The settle's held set is `held`, and a held card is seeded from the anchor point, not from `POS` or the layout
  position (today the settle seeds every card from `mirror.pos(id)`, `graph_nav.rs:~568`), then stays there and still
  repels. `POS` and the anchor can differ after a remote write; the anchor wins for a held card.
- **Writers.** Every producer of a set or cleared anchor, with its undo record (`Moved`, see Undo):
    - Pin (`set_pinned`, `shortcuts.rs:266`): writes `POS` and an explicit anchor at the drawn position in one step
      (today: a `POS` write plus a flag write). It always writes `(layout_pos, mirror.group(id))`, except that pinning
      a card whose anchor is `held` and already at the drawn position only sets the bit. An inert anchor (written in
      another group) is replaced, never promoted.
      Unpin clears an explicit anchor only (so Unpin All, `set_all_pinned`, `node_ops.rs:253`, keeps drag anchors) and leaves `POS`, so the card stays where it is until the next settle seeds from it.
    - Drag release (`release_drag`, `graph_nav.rs:206-222`): writes `POS` and an anchor at the dropped position in the
      current group, with `explicit = pinned(id)` (so dragging a pin in any group keeps it a pin). A plain release is
      not on the undo history, as now, but a release that ends in a Join or GroupWith is: `drag_origin` holds
      `(pos, anchor)` before the drag (its element type grows from `Option<Vec2>` to a pos and anchor pair), and
      `membership_snapshot` and `moved_since` (`graph_nav.rs:295-318`; the snapshot tuple gains the anchor) capture
      anchors, so the `Moved` "before" is the pre-drag anchor, not the one just written.
    - Move, Join, fold and put-back (`edit_group`, `session_sync/edits.rs:188`, which `fold_group` and `regroup_held`
      also use, `groups.rs`): clear an implicit anchor, keep an explicit one. The pair is in the `Moved` record.
    - Tidy: a new action beside Arrange by topic (toolbar chip `graph_ui/toolbar.rs:238-243`, omnibar row
      `omnibar/rows.rs:58` plus its own `Command` next to `static ARRANGE`, headless row `debug_control.rs:739`). On the
      current level it clears every implicit anchor of the level's cards, inert ones included (so a stale anchor cannot
      revive when a card is moved back); explicit pins stay. One `Moved` record with the anchor pairs and
      no position change.
    - Arrange by topic (`search/arrange.rs:57-98`): holds only explicit pins fixed (today's rule: `held` restricted to
      `pinned`). For each card it moves it writes `POS` and clears an implicit anchor, in its existing `Moved` record;
      otherwise a later settle would pull the card back to its drop point.
    - `debug_move_node` (`debug_control.rs:1207-1217`) writes the anchor as a drag release does, so the gate's
      `MOVENODE` tests the same path.
  Tidy, Arrange and Unpin are three ways to release holds; they share one `clear_anchors(ids, explicit)` writer, and
  Tidy is Arrange without moving (see Architecture check).
- **Readers.** The settle (`graph_nav.rs:573-577`) and Arrange read `held`. `SettleKey` (`level.rs:100-121`, today
  `{count, sizes}`) gains a hash of the held set with anchor points, so a remote Pin, Unpin or anchor write re-settles
  the level; a local drag release therefore re-settles too, which is intended (neighbours move around the card). the idle-sleep exemption
  (`tabs/sleep.rs:136`) reads `pinned()` only, so a drag-written anchor never keeps a tab awake. A remote `ANCHOR`
  change snaps the layout the way a remote `POS` does (`session_sync/remote.rs:111`). A concurrent drag (implicit) and
  Pin (explicit) are two writes to one register; last writer wins and the loser's bit is lost. Accepted, as for `POS`
  today; the user sees the result and can pin again.
- **Export.** `store/json.rs` drops anchors (the `Held` arm at `:62` and the import match at `:298-312`); `POS` stays,
  and Pin writes it, so an exported pinned card keeps its position but not its hold. Import ignores unknown field names
  (`json.rs:271`), so an old export's `"pinned"` is dropped silently.
- **Undo.** `Moved` (`undo.rs:14-19`) gains an anchor (before, after) pair beside the position pair; Move, Join, fold,
  put-back, Tidy and Arrange use it (Pin, Unpin and a plain release stay off the history). `replay_revert`
  (`undo.rs:229-250`) treats an anchor-only difference as a change, in its `holds` check and its `done` count;
  otherwise the Undo of a Tidy (same group, parent and position) does nothing. Replay writes through a new `edit_anchor` next to `edit_pos` and checks the held anchor in
  `replay_revert` (`undo.rs:241-243`), so undoing a Join, fold, Tidy or Arrange restores anchors with the positions.
- **Respawn.** `TabCarry` and `Carried::tab` carry the anchor, not a bool (`mirror.rs:487-506`, `node_ops.rs:114-124`,
  `session_sync.rs:270`, `tabs.rs:209`, `:237`, `:256`, `tabs/spawn_result.rs:18`, `:178`).

Deleted: `PINNED` (field 7) and its use in the kind field lists (`model.rs:183-198`, `:310`).

### Slice 2: per-level view state

`LocalState` (`local_state.rs:21-52`) replaces the single `camera` and `groups_collapsed` with
`levels: BTreeMap<Option<NodeId>, LevelView { camera, collapsed }>`, the key being the level's group (`None` is the top
level, a JSON level uses its own group key the same way). `collapsed` keeps its meaning (hide the thumbnail grid of group
cards at that level, `card/group_body.rs:43`). The file's JSON state has no version field (the `.local` wrapper's
`store/local.rs` `FORMAT_VERSION = 1` is untouched) and every field is `#[serde(default)]` with lenient ids, so an old
file loses its camera and collapsed bit once and reads without error (no migration shim, per the repo rule).

- `switch_group_level` (`graph_ui/scene.rs:111`) saves the outgoing level's camera rect before `current_group` changes
  (the Scene writes `camera.rect` every frame, `scene.rs:285`) and restores the incoming one, falling back to
  `fit_all_nodes_scene_rect()` when none is saved or the saved rect no longer intersects the level's bounds.
  `LocalState.camera` already carries its level (`local_state.rs:37-38`, checked at `:453`); this replaces that one slot
  with the map. `teardown_running_session` (`session_files.rs:490`) sets `current_group = None` without the switch and
  must not save. A switch the user did not make (`remote.rs:476`, `node_store.rs:127`) evicts a deleted level's entry
  after the switch.
- `remap` gets an arm for `levels` like the one for `nodes` (`local_state.rs:166-186`); `forget_id`
  (`node_store.rs:183`) evicts it; ids gone from the document are pruned at save (the `hidden_local` pattern,
  `local_state.rs:365-371`).
- The stale doc comment on `App::groups_collapsed` ("Session-only, resets on restart", `main.rs:212`) is corrected.
  README `:233` says Collapse Groups is top level only; the toolbar shows it at every level with groups
  (`graph_ui/toolbar.rs:244`), so that line is the stale one.
- Export Group is unchanged: it carries no local state (`session_files.rs:250-261` returns `subtree_to_json`); the
  hand-made local map is `duplicate.rs:119-126` (copy and duplicate, `{zoom, title}`). The PARKED scope entry's claim
  that Export Group is rebuilt from `LocalState` is corrected, not implemented.

### Slice 3: the nested settle and read-only inline groups

A group can be expanded in place. An expanded group is drawn as a box containing its members; the box is derived, never
stored. `LocalState` gains `expanded: BTreeSet<NodeId>` here (with the same remap, `forget_id` and prune arms as
`levels`), session-local across restarts as the camera is.

What it is for, stated plainly: an overview. You see a group's contents beside its siblings and pan and zoom over them;
you act on a member by opening the group (Enter), as now. It does not add a way to act inside the box (Slice 4).

- **Settle.** `settle_current_level` becomes `settle_group(g, ctx) -> Box { size, centre }`, run bottom-up over the
  expanded groups under the current level and then the level itself. `ctx` carries the document view, the expanded set,
  the per-group cache and the size function. Each call runs `step_sized` (90 iterations, dt 1/60) over `g`'s direct
  items. A group item's `layout` position is the origin of its member-local frame; an expanded child is one sized item
  whose rect is its box plus padding (`PAD`, 40 px), placed at `pos + box.centre`, so member-local positions never move
  when the box moves. A held expanded group is pinned at origin + centre, recomputed each pass, so the rendered box does not shift when
  its children re-settle. A box is the padded bounding rect of its items'
  rects; an empty or collapsed group is the fixed card it is today (`GROUP_NODE_SIZE`, defined at `card.rs:235`);
  a one-member group's box is that member's rect plus padding, with the origin at the member's position. The current
  level's own expanded flag is irrelevant: only its descendants expand.
  Membership stays level-direct (`in_level`, `session_sync/edits.rs:201`, and the settle's node filter, `graph_nav.rs`
  `:566`). Links between items in different child groups are lifted to the child items by a helper
  `lift(id, group) -> Option<NodeId>` over `walk::ancestors`; the lifted edge weight is the maximum of its parts.
  Seeds: the held anchor point, else `POS`, else a linked neighbour plus `spawn_jitter`; velocities start at zero.
- **Cost.** 33 ms per full pass over 675 cards in 60 groups (prototype; flat 690-730 ms). The settle re-runs only for
  groups whose key changed. The per-group cache is `BTreeMap<NodeId, (GroupKey, Box)>` beside `SettleKey`; `GroupKey`
  covers the group's direct item ids and sizes, child boxes, the held set with anchor points, the lifted edges and the
  direct children's expanded flags. Any ancestor of a changed group has a changed key through its child box.
- **Settle key.** `SettleKey` (`level.rs:100-121`) gains the expanded set and each expanded descendant's membership and
  card sizes, and the held set with anchor points of every expanded descendant (so a remote Pin on a nested member re-settles),
  so a nested card resizing re-settles its group and then every ancestor; pure card movement still does
  not change it. The key and the settle sizes both call one `card_size(id)` (today `node_card_size(&Runtime)` is
  called directly at `graph_nav.rs:580-585` and in the key, and used at `card.rs:953`), which returns the box size for
  an expanded group.
- **Frames.** `layout` stays member-local. A climb helper `frame_offset(id, level) -> Vec2` sums the box-centre
  offsets of a card's enclosing expanded groups up to the current level, at read time; `card_rect`, `card_culled` and
  `level_items` use it, so `entry.rect` and `card_rect` agree (`card.rs:1146`, `:1205`, `level.rs:231`). It shares the
  ancestor walk with `lift`; folding the other eight walkers into it stays parked.
- **Drawing.** Per-group painter order: the box, then its members, then the next sibling (today `level_scan` buckets by
  draw layer with groups last, `level.rs:129`, which would paint a box over its own members). Tab order stays the level's cards
  (`card_tab_order`, `graph_ui/keys.rs:205-209`); nested members are not in it, since Tab selects. Edges between items of one
  expanded group are drawn; `level_edges_from` runs once per visible group.
- **Expanding.** A per-group chevron on the group card. A group's plain click stays Enter (`cards.rs:143`). The group
  item itself stays interactive as today (select, drag, pin, link); a drag moves its frame origin and writes its
  anchor.
- **Read-only members.** Nested members are drawn and hit-tested for hover only: they are not selectable, draggable,
  link sources or drop targets, and a drop on the box is a drop on the group card. This avoids the frame mix in
  `drop_target` (`graph_nav.rs:129-176`), `apply_drop` (`graph_nav.rs:227`) and the "selection holds this level's
  cards" invariant (`graph_nav.rs:660-673`, `marquee.rs:62-68`). A nested member settles from its stored `POS` and
  anchor like any other; its anchors come from before it was nested, since it cannot be dragged or pinned here.
- **JSON levels.** `settle_current_level` serves graph levels only (`level.rs:309`, `shortcuts.rs:293`). Tidy, the
  chevron and `expanded` do nothing on a JSON level; only its camera entry applies.

## Behaviour changes (all deliberate)

- A dropped card stays where it was dropped across later settles (0 px against 358-422 px of drift) until Tidy, Move or
  Unpin. It stops making room for neighbours that move, which now move around it.
- A card dragged and released is anchored without being "pinned": no pin mark, no menu change, no keep-awake.
- Arrange by topic clears the implicit anchors of the cards it moves.
- Unpin leaves the card where it is; the next settle may move it.
- An exported, copied or duplicated pinned card keeps its position but arrives unpinned (`paste_copy` goes through
  `subtree_json` and `json::import`, `duplicate.rs:2,66,178`); tested for Duplicate.
- Switching levels returns to that level's camera instead of fitting all nodes.
- **Breaking:** old sessions: `PINNED` values are ignored (field 7 reserved), so previously pinned cards are unpinned.
  Per the repo's no-migration rule this is accepted; it is a `**Breaking:**` line in the changelog at the next release.

## Tests

- Anchor (`session_sync/tests/edits.rs`, beside `a_drag_release_saves_the_dropped_position`, `:90`): release writes
  point and group; an implicit anchor is inert once the group differs; an explicit pin survives Move, fold and put-back
  and holds at the same `(x, y)` in the new group; a drag of a pinned card in another group keeps the bit; Pin writes
  `POS` and Unpin leaves it; Tidy and Move clear implicit anchors only; Arrange clears implicit anchors of the cards it
  moves; Pin on an inert anchor replaces it with the drawn point; Tidy clears an inert implicit anchor, so moving
  the card back does not revive it (both tests make the anchor inert by applying a remote group write through the
  mirror without the local clearing, as `remote.rs` does); Unpin All keeps an implicit anchor; Undo of a drag-then-Join restores the pre-drag position
  and anchor; `debug_move_node` writes the anchor; a drag of an unpinned card does not keep a tab awake (`tabs/sleep.rs`); a remote peer sees the anchor
  (`remote.rs`, extending `:424`); a `Create` with field 27 survives an old build's compaction
  (`store/journal/tests.rs`); an old `PINNED` value is ignored and a JSON import with `"pinned"` loads.
- Undo: Join then Undo, fold then Undo, Tidy then Undo and Arrange then Undo each restore anchors and positions.
- Retarget the `PINNED` round-trip tests (18 sites in `session_sync/tests`, plus `model.rs:850`, `:946`, `:955`,
  `json.rs:469`, `:613`, `journal/tests.rs:472`, `mirror.rs:585`, `:760-764`). `node_menu/tests.rs:149` and the other
  view tests take a `pinned` bool and are unchanged.
- `LocalState`: round-trip, remap and prune of `levels` and `expanded`; a JSON level gets a camera entry; an old file
  with `camera` reads without error; the camera restore falls back to fit-all when the saved rect misses the bounds
  (extends `session_sync/tests/local_state.rs`).
- Nested settle (`graph_layout.rs` `mod tests`, beside `the_layout_is_deterministic`, `:477`): member-local positions do
  not change when the child's box moves in the parent; two runs are bitwise equal; empty, one-member and nested-twice
  groups; a pinned child inside an expanded group; a held expanded group keeps its box position; hover-only members and
  a drop on the box landing on the group card.
- `level.rs`: the settle key includes an expanded descendant's size and membership; moving a card without an anchor
  write does not change it, and an anchor write (a drag release, a remote Pin) does (extends `:341`, `:369`).
- Settle: a held card whose `POS` differs from its anchor is placed at the anchor point.
- Gate: `gate two` s12 (`crates/gate/src/two/groups.rs:262-272`) also checks that the anchor reaches the peer. That
  needs a `Field::Anchor` variant (`crates/gate/src/session.rs:99-123`) and an app-side export key `mirror_anchor`
  beside `mirror_pos`; the check compares the two mirrors only, since the document JSON drops anchors: `Field::Anchor` has no `doc_key`
  (`session.rs:112-117`) and a mirror-only wait helper replaces `wait_field`'s A, B and document wait for it. `shots` gets a
  view with an expanded group (pixel baselines for the changed graph views are re-taken). Gate tier: run
  `gate tier` on the change; anchor writes reach the store, so `light` at least (`crates/gate/src/tier.rs` defaults
  unlisted paths to `light`).

## Out of scope

- Interactive members of an expanded group (selection, drag, drop, linking across the box): Slice 4, to PARKED.
- Removing `POS` itself, removing user placement, a whole-tree canvas (j2, j3): decided against for now.
- A `rank` register, member order shared across replicas (l1).
- Similarity edges in the shared layout (n1; decided 2026-11-01: no topic force in the settle, topic structure comes from Suggest groups, and the Arrange action is removed; PARKED "Shared topic input for similarity edges" and "Remove Arrange by topic").
- Folding the group-chain walkers; soft pins; separate documents (PARKED).

## Architecture check

Arrange by topic was removed on 2026-11-01 (user, as1/au1; `git show cabfa377:crates/manifold/src/search/arrange.rs`),
so the Arrange mentions above are history. Tidy and Unpin are the two ways to release holds; they share
`clear_anchors`. The held
predicate is one function (`held`), not a union of two; `pinned` stays only as the user-facing bit.

## Docs to update when each slice lands

Per slice, grep each file line by line (the "pin" wording also names engine pinning, so a bare grep over-matches):
README Pin and anchor wording (`:136`, `:156`, `:187`, `:189`, `:235`, `:238`, `:280`, `:288`, `:409`, `:516`, `:719`,
`:784`, `:791`, `:792`, `:805`, each checked), `:233` (Collapse and Expand Groups, plus the toolbar wording above),
`:364` (`session.mnf.local`), `:38` (`level.rs` seam), `:150` and `:146` (layout history and overview);
`docs/superpowers/specs/2026-10-04-node-model-design.md:43,121,187,262` (`pinned` as a common field, "sorts first");
`ROADMAP.md` (not yet checked); PARKED lines for the scope entry (Export Group wording, format-bump answer), the anchor
entry (no bump, one-predicate semantics) and the pin-dot entry; CHANGELOG (the `**Breaking:**` line).

## Open questions for the plan

- `PAD` is 40 px; whether a box shows a header strip inside the padding (the nested-twice test depends on it).
- The Tidy label, and whether a faint cue marks drag-anchored cards (no evidence yet).
- Decided 2026-11-01 (b2): Slice 3 does not ship alone. It waits for Slice 4 (interactive members) and ships with it; Slice 4 needs its own spec. Slices 1 and 2 plan and ship now.
