# Node model (sub-project A): MIME identity, capability table, edge-type table

Status: design, awaiting owner review (2026-10-04). Sub-project A of the homogeneity vision in
`2026-10-04-homogeneity-decisions.md`: anything (tabs, groups, cookie jars, files, JSON values, notes) can be
manipulated and connected homogeneously.

## Goal

Replace the five app-layer kind enums (`Kind`, `NodePayload`, `ItemNodeKind`, `ContentKind`, `LevelItemKind`) and the
per-kind `match`es with three data tables keyed by MIME type: identity, capabilities, edges. Selection, pinning, the
menu, duplicate, close and undo become generic verbs over those tables.

## Non-goals and decisions

- No migration, no compatibility, no stability promise. There is no session that must survive the change; the rename is
  as intrusive as needed (owner, 2026-10-04).
- Out of scope, each its own spec: B links (connect gesture UI), C jars as nodes, D views registry (in-process view
  trait, IPC hosts as proxies), D2 external plugins, E broader undo. Order: A, then B and E, then C, then D, then D2.
- Jars stay in their current store until C. A adds only their type and edge row.
- Bookmarks are not nodes yet.

## 1. Identity

A node's identity is its MIME type, a common field replacing `kind`. Manifold's own kinds use vendor types:

| Node | Type |
|---|---|
| tab (one merged type; "loaded" is runtime state) | `application/vnd.manifold.tab` |
| group | `application/vnd.manifold.group` |
| cookie jar | `application/vnd.manifold.jar` |
| link | `application/vnd.manifold.link` |
| note | `text/markdown` |
| JSON value | `application/json` |
| PDF | `application/pdf` |
| images | real `image/*` |
| other files | sniffed MIME (existing `content.rs` table), else `application/octet-stream` |

- Common fields: `type`, `name`, `color`, `pinned`. Edge fields (parent, opened-from, jar) are not common fields; they
  belong to the edge table (section 3).
- One name precedence rule, the existing `names.rs` rules. A stored `color` layers over the row's default color.
- Unknown types resolve to the `*/*` row; their fields are kept raw, as unknown kinds are today.

## 2. Capability table

Rows are plain data registered through `register(row)` (compiled-in now, so plugins can register later).

- **Match**: a row is `pattern` plus optional `requires` tags. Patterns, ranked low to high: `*/*`, type wildcard
  (`image/*`), suffix wildcard (`*/*+json`), subtype prefix, exact. Each `requires` tag raises rank. One shared
  `matches(pattern, type)` matcher serves capability and edge rows.
- **Layering**: all matching rows apply in rank order; a higher row overrides a lower row per field. List fields (such
  as `views`) append.
- **Tags** (`Tag`, a closed `#[non_exhaustive]` enum in a bitset): backing `inline | file | web`; state
  `failed | loaded | engine-missing`; `file-missing`. Tags are runtime facts, not stored.
- **`caps(node)`** is read at use time, never cached on the node; a per-(type, tags) merged-row cache backs it.
- **Flags after dedup**: `renamable`, `pinnable`, `portable` (duplicate and export), `restorable` (undo; implies
  `portable`), `activate` (default open action), `presentation` (icon, card style, and a `runtime` field naming the
  runtime payload family). Derived, not flags: containment/groupable and movable (from the `contains` edge row), copy
  forms (from backing). There is no `selectable` flag until a kind needs to refuse selection.

## 3. Edge-type table

An edge type is a row: endpoints (patterns plus tags), cardinality (`at-most-one` is a field on the source; `many` is
a Link node), `acyclic`, `on-target-removal` (`Clear | Cascade | Adopt`, named variants a verb may select),
`drawing`, `user-creatable`. Contains and opened-from share a row template.

Built-in rows:

| Edge | Source | Target | Cardinality | Removal | Notes |
|---|---|---|---|---|---|
| `contains` | tab, group, note, file, ... | group | at-most-one, acyclic | Adopt (Cascade for "Close Group and Contents") | nests inside the card |
| `opened-from` | any | any | at-most-one, acyclic | Clear | drawn as a line; persists for all kinds |
| `jar` | tab, group | jar | at-most-one | Clear | effective jar = own edge, else nearest ancestor via `contains`; edge presence means explicit (replaces `JAR_SET`) |
| `link` | any | any | many | Clear | stored as a Link node |

- A Link node (`application/vnd.manifold.link`) has `from`, `to`, `edge-type`, and the common `name`. Any later
  many-to-many type (such as `references`) uses the same storage with its own `edge-type` id.
- The connect gesture offers the edge types whose endpoints accept the pair. Cycle checks read `acyclic`.
- Removal policy lives only in the row. Verbs pick a variant by name: Dissolve is `Adopt`, Close Contents is `Cascade`,
  both defined on `contains`.
- Importing a subtree remaps every Id field, edge fields included, through the existing generic path
  (`store/json.rs`).

## 4. Selection, pinning, verbs

- **Selection** is UI state, a set of node ids of any mix of kinds. It replaces the separate tab and group selection
  paths.
- **Pinning** is the common `pinned` boolean on any `pinnable` row. Effect, identical for all kinds: sorts first in its
  container and is skipped by "close others" and bulk close. If pinning today also does something else, that effect
  goes in the row's `pinnable` entry, not in the verb (check during planning).
- **Verbs**, one implementation each, gated by a flag and written only through store ops: rename (`renamable`), pin
  (`pinnable`), duplicate (`portable`), close, undo(entry), connect/disconnect (edge table; moving a node is
  `connect(contains)`, so endpoint patterns and the acyclic check apply to drags), activate (open with the row's
  default view; rows with no view keep an explicit action, such as group enter or PDF reveal until D), open with (empty
  until D).
- A menu entry appears only if every selected node allows the verb. Per-kind behavior comes from rows and edges, never
  from a `match` on kind.

## 5. History: delete-only undo, built to widen

One history replaces both undo and Recently Closed.

- An entry is `{label, inverse: Vec<Op>}`, a transaction. Closing a `restorable` node records one transaction holding
  the removal-variant ops and the inverse that re-creates the subtree. A kind with `restorable` false closes without an
  entry and the close confirmation says so.
- `create_ops(subtree, remap)` is the single serializer: duplicate and export call it with `remap = true`; a close
  transaction calls it with `remap = false`, so incoming edges (links, opened-from children, jar edges) still resolve
  after undo. If a Delete tombstone blocks re-creating the same Id, restore becomes an undelete op (verify in the first
  plan task).
- Undo applies the newest entry's inverse as new ops with fresh timestamps (consistent with LWW); the Recently Closed
  menu applies any chosen entry through the same path. If the old parent is gone, the node is moved to the root.
- Only close records entries now (owner decision). Widening later means recording more transactions (Set and Move
  inverses are the old value), coalescing (one drag, or typing, is one entry), a redo stack and a UI; the data model does
  not change.
- The history is persisted in the session in the generic entry form; `closed.rs`'s per-kind entry type goes away.

## Rename plan and order

Each step leaves the app working:

1. Add the row table, edge table and `caps()` with the old enums still in place.
2. Switch consumers to `caps()` kind by kind, starting with the menu (`node_menu.rs`; heaviest sites: `node.rs` 58
   `NodePayload` matches, `session_sync.rs` 33, `fetched.rs` 30).
3. Add generic selection and the verbs.
4. Move undo and Recently Closed to transactions.
5. Remove the five enums, rewrite fixtures (`seed.json`, smoke and drive scripts, README, first-launch demo) and add a
   grep-guard test, in the style of the `APPDATA` guard in `profile::tests`, that fails if any source still names the
   old enums.

## Error handling

- Unknown type: the `*/*` row (closeable, generic card), fields kept raw.
- Undo that fails to apply: the reason goes in the status line and the history entry stays.
- A connect the edge table rejects (endpoints, cycle): no op is written and the gesture reports why.

## Testing

Table-driven tests for matcher rank, tag raising and per-field override; edge cardinality, `acyclic` and each removal
variant; a close/undo round trip with original Ids for every `restorable` type, including incoming edges; the verb gate
over mixed selections; and the grep guard. Tests never touch the real `%APPDATA%\manifold` (project CLAUDE.md).

## Open checks for the plan

- Does a Delete tombstone block re-creating an Id (section 5)?
- What does pinning do today beyond sort-first and bulk-close exemption?
- Which kinds, if any, must refuse selection (the dropped `selectable` flag)?
