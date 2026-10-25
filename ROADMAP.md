# Roadmap

Ordered milestones, each a question the release answers. Tagging moves the line to `CHANGELOG.md`. Deferred work is in `PARKED.md`.

Shipped: **v0.9 - Can you find and categorize cards?** Page-text search, suggestions, labels, the property tree (`CHANGELOG.md` 0.9.0).

- **v0.10 - Can you find and categorize everything, and edit what you keep?** Part 2 of the v0.9 theme.
  - *Properties 4b:* the write path for inline Json cards (edit scalars, add and delete keys, insert and reorder array elements, change a type; one undo step each), the Export action, `Ref` chips. 4a (read side, New JSON, Detach to inline) is built and untagged. Spec: `docs/superpowers/specs/2026-10-24-properties-design.md` section 4. Closes the parked sync hazards that need order keys (`between`): concurrent detach, whole-body tombstone, view overrides keyed by element id, undo for Detach.
  - *Search leftovers (PARKED.md):* saved search index, PDF text in the page index, topic chips on cards, suggestions following the search engine, notes, groups and downloads as omnibar rows, card rows and the Graph filter across hosts, accepting one suggested group clearing the others.
  - *Label leftovers (PARKED.md):* name collisions from import and sync, label export with a subtree copy, label menu polish.
