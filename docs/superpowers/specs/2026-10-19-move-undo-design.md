---
status: draft
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
- A plain move (no effect) pushes nothing, see Decision 1.

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
3. *A revert is itself an edit.* It must not push onto the stack (else Ctrl+Z redoes). It pushes nothing; redo is
   out of scope.
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

## Decisions for you

1. **Plain moves on the stack?** (a) No, only effects; a plain move is undone by dragging back (recommended, keeps
   the stack to things that are hard to see). (b) Yes, so Ctrl+Z is "undo the last thing I did", at the cost of
   filling 10 slots with nudges.
2. **Toast on every effect drop?** (a) Yes, same toast as Dissolve (recommended: discoverable without a tutorial).
   (b) Key only, no toast, fewer toasts.
3. **Depth.** (a) 10 (recommended). (b) 1, the parked design. (c) Unbounded.
4. **Redo.** (a) None (recommended: a revert is a plain edit; move again). (b) Redo stack.

## Tests (what leaves a check)

A rig test per effect in `closed.rs`'s style: drop, revert, the mirror equals the pre-drop state, `edit_group` /
`edit_pos` queued writes equal what a manual drag back would write; a CAS test per skipped case (moved again, closed,
remotely re-grouped); GroupWith closes its group only when empty; the edge twin test; Ctrl+Z gating (Tab view, text
focus); the stack caps at `UNDO_DEPTH`.

## Not done

Persistence across restart, sync of the stack, and a document-layer stack (`PARKED.md`, "decided against").
