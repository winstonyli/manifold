---
status: done
---

# Label name collisions — Design

## Question

Label names are unique only through the local paths (`create_label`, `rename_label`; ruling L3). Two replicas that
each create "urgent" offline, or a rename racing a create, leave two live `Label` nodes with the same `name_key`.
The label menu then shows two identical rows, `label_named` returns the lower id, and a card's chip, the filter and the
search index treat them as different labels. This makes same-key labels one label, on every replica, without a write
that could race. It supersedes PARKED.md "Label name collisions from import and sync".

Sources of a collision (verified 2026-10-26): sync of two replicas (the real one); a rename or create racing across
replicas. Not a source: a session file load (it replaces the document, `replace_running_session`) and Import Group
(its reuse hook matches by key, `import_group_json`).

## Decisions (conversation 2026-10-26)

1. **Read-time merge (1a).** Same-key live labels are one label; nothing is rewritten to merge them. The duplicate
   nodes stay in the store.
2. **Scope: sync collisions and Import Group together (2, as recommended).** One mechanism covers both.

## Design

- **Canonical label.** Among live labels with the same non-empty `name_key`, the canonical one has the lowest
  `NodeId`. Every replica sees the same set once synced, so every replica picks the same one. A label whose key is
  empty is its own group (never merged).
- **One choke point: `Mirror`.** The mirror keeps `label_group: HashMap<NodeId, NodeId>` (member to canonical),
  rebuilt at the mirror's three write points for a label: `add_stored` (a `Label`), `forget_stored`, and `set` of
  `field::NAME` on a stored `Label` (`set` is the one place a mirrored field is written; `init_field` and `seed` go through it).
  Labels are few, so a rebuild is a scan.
  canonical). `Mirror::label_members(canonical) -> &[NodeId]` lists a group (the canonical first).
- **Reads see only canonical labels.**
  - `Mirror::card_labels(card)` maps each membership to its canonical label and drops repeats, keeping the earliest
    `(added_at_ms, id)`. So a card carrying the same name under two ids shows one chip.
  - `App::label_defs()` returns canonical labels only, so the menu, `label_keys`, `label_named`, the debug export and
    Import Group's reuse see one row per name. `label_named` therefore returns the canonical id, and `create_label`
    selects it (L3 holds across replicas after sync).
  - `label_names`, `label_state`, the filter and the search index read `card_labels`, so they follow without change.
- **Writes address the group.**
  - `set_label(cards, label, on)`: whether a card "has" the label is read over the whole group (a card carrying only
    a duplicate already shows it, so `on` writes nothing for it and `off` must act). `on` writes the membership under
    the canonical id as today; off removes the card's membership under every member of the group (else the card would
    still show the label through a duplicate). The undo edits are the same `PropEdit`s.
  - `rename_label(id, name)`, `recolor_label`: write the field on every member, so a rename never splits a group
    (without this, renaming the canonical label would promote its duplicate and cards would keep the old name). The
    uniqueness check ignores the whole group, not just `id`.
  - `delete_label(id)`: forgets and deletes every member (L4: still not undoable), so no duplicate resurfaces as
    the new canonical.
- **Convergence.** Reads depend only on the synced set of label nodes, never on write order. The group writes above
  are idempotent field sets and deletes of the same value from any replica, so concurrent edits converge.
- **Self-healing, not required.** A later rename or recolor writes every member, so edited groups converge in storage
  too. An untouched duplicate stays until the label is edited or deleted; reads never depend on it.

## Edge cases

- **A group splits when a member is renamed on another replica** before this replica has seen the rename: the group
  recomputes on arrival; a card carrying the departed member shows that member's new name. Correct: it is a different
  label now.
- **Canonical deleted on one replica while a duplicate is live there:** `delete_label` deletes all members it knows;
  a member that arrives later becomes canonical (it is a label nobody deleted).
- **Three or more members:** the lowest id wins; the rest fold in.
- **Membership under a deleted or unknown id:** unchanged (shows nothing, `card_labels`).
- **Empty-key names:** never grouped.
- **Existing documents:** no migration; a document with no collisions has identity groups.
- **Format version:** unchanged (no stored shape changes).

## Testing

- `mirror` unit tests: groups by key and picks the lowest id; an empty key is its own group; renaming a member splits and
  re-forms the group; `card_labels` folds a card carrying two members into one entry, keeping the earlier stamp.
- `session_sync` tests with two replicas that each `create_label("urgent")` offline then sync: one menu row, one chip
  per card, `label_named` identical on both replicas; `set_label(.., false)` clears a card that carried the duplicate;
  rename and recolor reach both members; delete leaves no label; Import Group with duplicates in the target reuses the
  canonical.
- The existing label tests pass unchanged.
- Gate: `light` (sync and mirror). The two-instance stage already syncs; the plan adds the same-name case if it can be
  driven without a new harness.

## Out of scope

A visible "merged" affordance; collapsing the duplicate nodes in storage; label delete undo; the jar-style fragment
(`jar_fragment`), which this makes unnecessary for labels.

## Review log

- Round 1 (draft vs code): the cache invalidation points were vague; named `add_stored`, `forget_stored` and
  `Mirror::set` (verified the single write path).
- Round 2 (adversarial): `set_label` computed "has" on the one id only, so an off on a duplicate-only card was skipped;
  now group-wide. Search-index freshness after a group re-forms is the same as for any rename today (not widened).
