# Homogeneous nodes and edges: owner decisions

Vision: anything (tabs, groups, cookie jars, files, JSON values, notes) can be manipulated and connected homogeneously.
Scan reports (git-ignored): `.superpowers/sdd/2026-10-04-review-scan-fixes/generalization-model-report.md` and `generalization-ui-report.md`.

## Decisions (2026-10-04)

- **Undo:** delete-only undo. Destructive actions (close, dissolve, delete) are undoable from an in-memory snapshot restored with fresh ids. No full op-log undo for now. This reverses the "no undo" line in `2026-10-03-per-tab-jars-design.md`.
- **Links reach jars:** jars become graph nodes now. This supersedes the "jars stay off the graph" choice in `2026-10-02-jars-design.md` and `2026-10-03-per-tab-jars-design.md`; it needs a jars design rewrite and a persistence migration. JSON values are not decided here (they remain a view level until `json-node-rework` is revisited).
- **Selection and pinning:** extend to every node kind. This lifts the "multi-select for non-tab kinds" non-goal.
- **Bookmarks and other lists (history, sessions):** not nodes yet. The downloads popup is the model for a view over nodes if revisited.

## Direction (from the scans, not yet specced)

Keep the store as is (kind plus registers keyed by field number; it already holds any kind). Add data tables, not a trait per kind (two specs rejected that): a capability table per kind, and a table of which fields are edges (drives drawing, delete clean-up, load order). Typed any-to-any links via a new `Link` node kind. Fix first: group Del/close/menu contradictions, "Move to sibling group" vs drop-merge, Pdf click action, Unloaded-tab Duplicate, the per-kind "Del close" hint, and persisting a download's "opened from" link.

## Update (2026-10-04)

Sub-project A is designed in `2026-10-04-node-model-design.md`. Decisions made there: no migration or compatibility; vendor MIME types; a closed tag enum; an edge-type table; one transaction-shaped history (delete-only for now).

Plan 1 (tables and behavior-neutral consumers) shipped: see `../plans/2026-10-04-node-model-1-tables.md`.
