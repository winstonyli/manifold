---
status: done
---

# Undo for drops and moves — Design

## Question

Dissolve Group, Ungroup All and Merge are undone by reopening the closed group (`fold_group`, `regroup_held`). Join,
GroupWith, Eject, Reparent and a plain move have no undo (`PARKED.md` "Undo for moves and joins"). The aim is one
undo that covers all of them, session-local, without a document-layer undo stack (decided against: remote edits,
tombstones).

## Design

**One session-local undo stack on `App`**, newest last, capped at 10 (`UNDO_DEPTH`). It is in memory, not synced and
not saved. An entry is one of:

- `Revert(Snapshot)`: a drop or move, replayed through the one writers `edit_group`, `edit_pos` and `edit_parent`, so
  a revert syncs as an ordinary edit.
- `Reopen(Vec<String>)`: what `toasts.undo` holds today for Dissolve, Ungroup All and Merge (`undo_ungroup`). The
  group's `HELD` field stays the durable form (Recently Closed, restart, other devices); the stack entry only makes
  the same undo reachable from the key and the toast.

`toasts.undo: Option<(String, Vec<String>)>` becomes "the stack entry the toast offers" (message plus the entry's serial number,
a counter that never reuses, so a toast cannot undo a different entry after others are popped), so the toast, the key and Recently Closed all end in one `undo_entry`.

**Snapshot** (`Revert`): per moved card, taken before the effect applies:

| field | old | written |
| --- | --- | --- |
| group | `mirror.group(id)` | what the effect set |
| pos | `mirror.pos(id)` at drag start | what `release_drag` wrote |
| parent (Reparent only) | `mirror.parent(id)` | the new parent |

plus `created: Option<String>`, the group GroupWith made.

**Compare-and-swap per card.** A revert restores a card only if every field it holds still equals the *written*
value in the mirror. A card changed since (a later move, a remote edit, closed or deleted) is skipped and the toast
says "Restored 2 of 3". This is the rule `regroup_held` already applies ("members still at `dest`"), applied to
drops. No global "any edit invalidates the snapshot".

**GroupWith** also closes `created` (via `close_node`, so Recently Closed can bring it back), but only if every
member still in it is one the revert restored; otherwise it stays with the remaining members.

**Side effects a revert repeats.** `recheck_edge_scope` on each restored card, as `apply_drop` does. A revert
never touches links: `ungrouping_never_clears_an_edge` is the precedent, and gets a twin test for a revert. Merge
keeps its link retargeting and stays a `Reopen` entry (its links stay on the target, as documented).

**Triggers.**

- A drop with an effect (Join, GroupWith, Eject, Reparent) shows a toast "Moved 2 into Research" with Undo, the
  same toast Dissolve and Merge use.
- Ctrl+Z in the Graph view undoes the newest entry (and Ctrl+Z again the one before). Not in Tab view (a web page
  owns its keys) and not while egui wants keyboard input (`egui_wants_keyboard_input`, as `graph_ui.rs:1867`), so a
  note's text undo is untouched.
- A plain move (no effect) pushes nothing (Decided 1).
- Ctrl+Shift+Z and Ctrl+Y redo the newest redo entry, gated like Ctrl+Z.
- After an undo the toast reads "Undone: Moved 2 into Research" with a Redo button.

## Redo

A second list, `redo`, bounded by the same cap. Undoing an entry (toast or key) moves it to `redo`; redoing moves it
back to the undo stack. Only a fresh drop, Dissolve, Ungroup All or Merge pushes a new entry, and that empties `redo`
(Decided 4). An undo made outside the stack (Recently Closed's Reopen) pushes nothing, so the key does nothing after it.

- `Revert`: the same snapshot run the other way: restore a card to *written* only if it still equals *old*.
- GroupWith: also reopen `created` if it is still closed (`reopen_node`; if it was reopened meanwhile, skip that step),
  then move the members in. The same group id comes back, so Recently Closed references stay valid.
- `Reopen`: the entry also keeps `(group, dest)`. Redo calls `fold_group(group, dest)` if the group is shown and (for
  a merge) `dest` is shown. It folds the group *as it is now*, including a member dropped in after the undo (a
  deliberate ruling: redo means "dissolve it again", not "restore the old membership"). Merge's links stay on the
  target, so there is nothing to retarget.
- A redo of a revert reports "Redone 1 of 3" the same way.

## Stress test

Round 1 (against the code, `graph_nav.rs`, `card.rs`, `session_sync.rs`, `groups.rs`, `chrome_ui.rs`):

1. *The pre-drag position is gone by `apply_drop`.* `release_drag` (`card.rs:1371`, `1404`) writes the new pos
   before `GraphAction::Drop` reaches `apply_drop` (`card.rs:1407`), so the old pos must be captured where the drag
   starts (`card.rs:1356`, `1386`), for every card of a selection unit. Unit = `take_drag_unit`, taken in
   `apply_drop`, but the selection is known at drag start: capture the selection's positions there. Test: join a
   selected pair, revert, both are back where they were.
2. *`edit_pos` rounds nothing but `layout_pos` can differ from the mirror's pos by physics.* The CAS compares the
   mirror's pos to the value `release_drag` wrote (read back from the mirror right after the write), never to
   `layout_pos`, which physics moves.
3. *A revert is itself an edit.* It must not push a new entry (it would clear `redo`). Undo and redo only move an
   entry between the two lists.
4. *Eject from a nested group:* `remove_from_group` sends it to the group's own parent. The snapshot's old group is
   the group, written is the parent; revert puts it back only if it still sits in that parent.
5. *Reparent is an edge, not a move:* snapshot the old parent; `would_create_cycle` is re-checked on revert, since
   another replica may have re-parented meanwhile.
6. *The Default jar stays at the top level* (`edit_group` refuses a group for it): a revert that would put it in a
   group is refused by the same writer, no new rule.
7. *A card in the stack that is later closed:* skipped by CAS (not in `nodes`); if all are skipped the entry drops
   and the toast says nothing was restored.
8. *Stack entries outliving the toast:* the toast Undo reverts its own entry even when newer ones exist; Ctrl+Z takes
   the newest. Both remove the entry they used.
9. *Selection unit moved with `target` filtered out* (`take_drag_unit`): the snapshot lists only cards the effect
   actually moved.
10. *Dissolve/Merge on the stack:* a `Reopen` entry whose group was reopened another way (Recently Closed) is a
    no-op; `reopen_node` of a live node must be one (check).

Round 2 (against the design):

11. *Plain moves out of the stack* leaves a hole: Ctrl+Z after a plain move undoes an older Join, surprising. Hence
    Decision 1.
12. *Two stacks in the user's head:* the toast, the key and Recently Closed all act on the same entries, so there is
    no second set of semantics.
13. *Ctrl+Z conflicts:* no `Key::Z` binding exists in `crates/manifold/src` today; the node menu and address bar use
    their own text fields, which egui handles first.

Round 3 (redo):

14. *Redo of GroupWith when the user reopened the group from Recently Closed meanwhile:* the group is live, so skip the
    reopen and still move the members in if they equal their old values; idempotent.
15. *Redo of a fold folds the group as it is now:* a member dropped in after the undo is folded too. Ruling above;
    the alternative (CAS on the old membership) would need the members stored on the entry and gives a partial
    redo for little gain.
16. *Plain moves between undo and redo* neither clear `redo` nor are covered by it; the per-card check keeps a redo
    safe if the moved card was one of its cards (it is skipped).
17. *Toast serials:* the Redo button targets the serial of the entry now on `redo`, so a later undo cannot make it
    redo a different entry.
18. *Keys:* `Key::Y` and `Key::Z` are unbound in `crates/*/src` (grep 2026-10-19).

## Decided (2026-10-19)

1. Plain moves are not on the stack. 2. Every effect drop shows the toast with Undo. 3. Depth 10. 4. Redo exists,
cleared only by a fresh push; Ctrl+Shift+Z and Ctrl+Y; a Redo button on the "Undone" toast; redo covers Dissolve,
Ungroup All and Merge by folding again.

## Tests (what leaves a check)

A rig test per effect in `closed.rs`'s style: drop, revert, the mirror equals the pre-drop state, `edit_group` /
`edit_pos` queued writes equal what a manual drag back would write; a CAS test per skipped case (moved again, closed,
remotely re-grouped); GroupWith closes its group only when empty; the edge twin test; Ctrl+Z gating (Tab view, text
focus); the stack caps at `UNDO_DEPTH`; per effect, undo then redo returns to the post-drop state; a fresh push empties `redo`; GroupWith redo brings back the same group id; a fold redo after a Recently Closed reopen; Ctrl+Shift+Z and Ctrl+Y gating.

## Not done

Persistence across restart, sync of the stack, and a document-layer stack (`PARKED.md`, "decided against").
