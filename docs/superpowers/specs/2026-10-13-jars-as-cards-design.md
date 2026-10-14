# Jars as cards

Status: draft for review. Implements open item 3 of `2026-10-04-homogeneity-decisions.md`. Supersedes the jar
design in `2026-10-02-jars-design.md` and `2026-10-03-per-tab-jars-design.md`.

## Goal

A cookie jar is an ordinary card: it has a position, sits inside a group, and is selected, renamed, duplicated,
closed and reopened like any node. A tab or group is connected to its jar by one edge, drawn and removed like
every other edge. The machinery that exists only because jars were not cards is deleted.

## Decisions (confirmed with the user)

1. **Scoped jars.** A jar card has `POS` and a group; where it sits is its scope.
2. **One `jar` edge** replaces the `JAR`/`JAR_SET`/`DEFAULT_JAR` trio.
3. **Dynamic inheritance.** A tab's jar is its own edge, else the nearest enclosing group's edge, else the Default.
4. **Default is a built-in card**, not a `JarKey::Default` special case.
5. **Cookie files live as long as the jar card or a Recently Closed entry that holds it.**
6. Elegance over upfront cost. **No migration code and no shims**: there are no users yet, so existing sessions
   are not carried forward (see "Existing sessions").

## Model

### Store

- `Kind::Jar` fields: `POS, COLOR, NAME, EPHEMERAL, IS_DEFAULT` (new field 22, Bool) plus the group every card
  has. Jars are stored like notes.
- `JAR` (15) stays and now means the `jar` edge on **both** tabs and groups (`Id`, absent = inherit). Tabs lose
  `JAR_SET` (16); groups lose `DEFAULT_JAR` (17). 16 and 17 go in `RESERVED`; they are never reused.
- Edge row `jar`: sources `TYPE_TAB, TYPE_GROUP`, targets `TYPE_JAR`, `Storage::Field(field::JAR)`, new column
  `at_most_one`, new column `scope: SameLevelOrAncestor` (every existing row is `SameLevel`), user-creatable with
  verb "Use jar\u{2026}". A jar joins `contains` (as a source) and `link` (both ends) by way of `CARD_TYPES`; the
  `opened-from` row keeps its own target list without it, so a jar is never "opened from" (the existing
  `opened_from_excludes_a_jar` test still holds).
- `check_connect` today handles only `Field(PARENT)` and `Node(_)`; it is generalised to any `Field(no)` edge, and
  its same-level rule reads the row's `scope`. Its refusal enum gains `OutOfScope`.

### The Default jar

- Exactly one jar has `IS_DEFAULT`. A startup invariant creates it when none exists (a new profile, or a document
  from before this change), named "Default", top level. It cannot be closed or deleted; Rename, Ephemeral,
  Clear Cookies stay.
- Two replicas that both created one (concurrent first launches) converge by the lowest `NodeId` keeping the
  flag; the other's flag is cleared by a deterministic repair at load and renamed "Default (old)". Its tabs
  resolve to the winner, and that device's logins stay reachable by rebinding to "Default (old)".
- `JarKey::Default` is deleted; every `JarKey` is a node id. The old `jars\default.json` is not adopted: the new
  Default starts with no cookies, and the orphan file is removed by the startup prune.

### Resolution

`effective_jar(tab) -> (NodeId, dangling)`: walk the tab, its group, that group's group, ... and stop at the
first node whose `JAR` is set. Named jar exists: that is the jar. Named jar is gone or out of scope: the Default
with `dangling = true` (warning chip; it does not keep climbing, since an explicit edge to nothing is a warning,
not a request to inherit). No node sets one: the Default.

This one function replaces `inherited_jar`, `new_tab_jar`, `governed_by`, `cascade_targets`, `cascade_count`,
`set_group_default_jar`, `clear_group_default_jar`, `resnapshot`, `refollow`, `follow_group_jar` and
`resolved_jar`.

### Scope

A `jar` edge is legal when the jar sits at the source's level or in any ancestor group of it. After any move
(a tab, a group with its contents, or a jar), every edge touching the moved subtree is re-checked and an
out-of-scope one is cleared, with a toast naming it. This is the code path that already drops cross-level
`link` edges on a move, run for every row. An edge that arrives out of scope from another device is kept and
resolves as dangling (Default plus warning chip); it is never rewritten behind the user's back.

### Live tabs and cookies

- A change of any tab's `effective_jar` (edge added or removed, an ancestor's edge changed, a move) goes through
  the existing `sweep_jar_switches`, now fed by diffing `effective_jar` instead of per-field edits.
- Cookie files stay one per jar id (`jars\<actor>-<counter>.json`, DPAPI). An ephemeral jar has no file.
- Clear Cookies and Ephemeral move from Settings to the jar card's menu next to the generic items.

### The jar card

A small fixed-size card: cookie glyph, the jar's color and name (drawn through `names::display_name` like any
name), an ephemeral marker, no body. Hit-testing, selection, z-order, pin and the rectangle selection treat it
like a note. Its menu: Rename, Duplicate, Pin, Link, Use jar\u{2026} omitted, Clear Cookies, Ephemeral, Close
(Default: no Close). A tab keeps its small chip showing its effective jar.

## Backing files: one concept for jars and content files

Today two mechanisms keep a node's file alive: fetched content (`PATH` into `content_dir`: `content_keep`,
`prune_unnamed`, `touch_owned_content`) and, new here, jar cookies. They become one. A kind that has a file
declares it in the type table:

- **`backing_file(node) -> Option<Name>`**: the file's name in its directory. Content kinds (Image, Pdf, Json,
  Download) derive it from `PATH` (so it survives an id change); a Jar derives it from its id (`<id>.json`).
- **`on_duplicate`**: `Share` or `Fresh`. Content files are immutable per file, so a duplicate shares the
  file (its `PATH` names the same one; this is what Duplicate already does today, and `replaced_file_unused`
  keeps a refreshed original's old file while the copy names it). A jar's file is mutable per jar, so a duplicate
  is `Fresh`: it starts logged out.
- **One liveness rule:** a backing file exists exactly while some live node or some Recently Closed entry names
  it. `content_keep` is already that rule for content; it generalises to take each kind's `backing_file`, and
  `prune_unnamed` and `prune_jar_files` become one prune over each file-backed directory. Nothing else deletes a
  backing file except an explicit wipe (Clear Cookies).
- **One release point:** when a node is removed (local close or a remote delete arriving) or a Recently Closed
  entry leaves the list (eviction past `MAX_CLOSED`, or the list cleared), the files it named that nothing else
  names are deleted. This replaces `forget_jar`'s unconditional `Jar::delete_files`. If the closed list cannot be
  read at startup, prune is skipped (the guard `may_prune_content` already gives).
- **One reopen rule:** `reopen_closed` imports with fresh ids. A path-derived name needs no change. An id-derived
  name (a jar) is moved to the new id's file when the **old id is no longer a live node**; when it is still live
  (Duplicate, which reuses `subtree_entry` and `reopen_closed`) nothing is moved, so the original keeps its
  cookies and the copy starts empty.

What it cannot cover: a `PATH` pointing outside the owned directory (a user's own file) is a reference, never
deleted (`owned_content_file` still decides this for the refresh-replace deletion); a note's text lives in the
store, not a file.

Consequences, stated plainly:
- Closing a jar or a group holding jars does not wipe credentials; Clear Cookies does, at once.
- Dropping the 21st closed entry deletes the cookies of any jar only it held.
- Tabs outside a closed group that named a jar inside it keep naming the old id; after reopening the group, the
  new jar has a fresh id, so those edges stay dangling (Default plus warning chip) until rebound. Edges from tabs
  inside the group to a jar inside it are remapped by `import` (an `Id` field naming a node in the file), so they
  survive. Edges from inside to a jar outside keep naming it, still valid.

## Behaviour changes (user-visible)

1. Clearing a group's edge: its tabs fall to the next ancestor's jar or the Default, and live tabs reload on it.
   Before, they kept their jar.
2. Moving a tab between groups changes its jar if it has no edge of its own (before: unset tabs only; now the
   same, without a flag). A tab whose own edge goes out of scope loses that edge, with a toast.
3. Closing a group closes its jars; outside tabs that named them show the warning chip; Recently Closed brings the
   jars back with cookies, per the section above.
4. Jars appear on the canvas as cards.
5. Duplicate of a jar copies name, color and ephemeral, not cookies.

## Deletions (replace, do not layer)

- Fields `JAR_SET`, `DEFAULT_JAR`; `JarChoice`; `Carried.jar` and `jar_set`; `Mirror::jar_choice`.
- `jar_defs.rs`: the functions listed under Resolution, `tab_jar`, `set_tab_jar`, `linked_group`, `is_inside`,
  the `delete_jar` cascade over tabs and groups, and `forget_jar`'s unconditional file delete (see above).
- The separate jar prune (`prune_jar_files`, `jar::prune_orphans`) and the content-only keep/touch/prune triple
  (`content_keep`, `touch_owned_content`, `prune_unnamed`): one backing-file implementation replaces both.
- `Mirror::jar_defs`/`mirror.jars` and their sync special-casing: jars become ordinary `App::nodes` entries with a
  `Runtime::Jar` variant (about 10 exhaustive match sites, each gaining a no-view arm).
- The "jars are not cards" exemptions: `closed.rs` (`worth_reopening` refusing a jar), `store/json.rs`
  (`is_half_made_link` and link-end filter, `creates`' card set, the line 121 guard), `session_sync.rs` (the
  Jar-skipping arms at 701, 734, 1478-1481, 1929, 2049), `card_kind.rs:108`, `caps/edges.rs` tests that assert a
  jar is refused.
- `JarKey::Default` and its special cases in `jar.rs`, `settings_page.rs`, `popups.rs`, `sleep_state.rs`,
  `main.rs`, `node_menu.rs`, `debug_control.rs`.
- Settings: jar list, rename, delete. Menus: "Follow Group's Jar", the group "Default jar" submenu.
- Docs: homogeneity spec item 3 and the `Links reach jars` line marked done; "superseded by" header on the two
  older jar specs.

## Existing sessions: no migration

There are no users yet, so no migration code is written. A session saved by an earlier build opens as follows,
and the developer's own session is simply reset if it looks wrong:

- Tab `JAR` (15) values keep their meaning as edges. Fields 16 and 17 stay raw on old nodes and are never read
  (the store already keeps retired fields 9, 12 and 13 this way); `RESERVED` lists them. An old group default is
  therefore ignored.
- The startup invariant creates the Default card. Old `jars\default.json` is not adopted.
- An old jar has no `POS`. The plan must check how a card without one is placed and add a placement rule only if
  none exists. This is the one open implementation question.
- No format-version bump: the field layout is a superset, and an older build would ignore the new fields.

## Testing

- `effective_jar`: own, group, grandparent, none, dangling, out-of-scope remote edge.
- `check_connect`: jar in the same group, in an ancestor, in a sibling group (`OutOfScope`).
- Move: no-edge tab changes jar; in-scope edge kept; out-of-scope edge cleared; moving a jar clears edges that
  fall out of scope.
- Close a group holding a jar, reopen: card back, cookie file moved to the new id, inside edges remapped, outside
  edges dangling. Duplicate: cookie file not moved.
- Eviction: the entry past `MAX_CLOSED` releases its jars' files; startup prune keeps held ones.
- Backing files, both kinds through the same tests: Duplicate of an Image shares the file and refreshing the
  original keeps it; Duplicate of a jar leaves the original's cookie file in place and starts the copy empty;
  closing and reopening each restores the file under the new id's expectations.
- Default invariant: created when absent, not duplicated when present; two Defaults converge.
- Gate smoke: `NEWJAR`, link a tab to it, rename, duplicate, close and reopen the group.

## Open questions for the user

1. Is a group's `jar` edge (inherited by every tab inside) the intended replacement for "Set default jar"?
2. Is a duplicated jar logged out the right default?
