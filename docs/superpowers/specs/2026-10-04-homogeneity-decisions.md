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

Pinning for every node kind shipped 2026-10-12 (see the README entry "Pin on every card").

Plan 1 (tables and behavior-neutral consumers) shipped: see `../plans/2026-10-04-node-model-1-tables.md`.

## Status (2026-10-13, checked against the code)

Done:

- Plans 1, 2a and 2b (live state is the node, sync by ops), and B (links: `Kind::Link`, `App::connect`/`disconnect`, the node menu's Links section, drag to connect). `JsonLeaf`, `FROM_JSON`, `KEY` and `VALUE` are gone.
- Pin on every card (2026-10-12).
- The scan's fix-first list: Duplicate on an unloaded tab (not a failed one), the per-kind Del hint (now `NodeAction::key_hint`, and Close Contents is never hinted Del), the Pdf click (reveals the file), a download's opened-from (stored as `PARENT`), and Move to sibling group (`move_targets`). Not re-tested: the group Del/close/menu contradictions, and Move to sibling group against drop-merge.

Open:

1. **Plan 3, the `closed` field and transaction history.** A close is still a snapshot in `closed.rs`; there is no `closed` field. Generic undo waits on it.
2. **Generic verbs.** Duplicate is tab-only (`duplicate_tab`), the rectangle selection holds tabs only, and rename is a group and jar-name action. These are the rows and menu edits plan 3 lists for selection, rename and duplicate.
3. **Jars as cards.** Jars are already `Kind::Jar` store nodes (so no persistence migration), but they draw as chips on cards, and the edge table refuses a link to or from one. Missing: a jar card, and `jar` and `contains` edge rows.
4. **Link follow-ups.** No `edge-type` field (`link` is the only Node-stored edge), cardinality, or removal variants.
5. **The unknown-type card** (`Runtime::None` makes it possible; `card.rs` says there is none yet).
