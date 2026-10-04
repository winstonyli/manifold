# Per-tab jars: design, 2026-10-03

Status: built (plan `docs/superpowers/plans/2026-10-03-per-tab-jars.md`). Builds on the shipped cookie jar v1 (`2026-10-02-jars-design.md`, `jar.rs`, the per-tab merge
state, the WebView2 and CEF hosts). Decisions from the user on 2026-10-03: a jar choice **per tab**, a **group default**
that tabs inherit, and **ephemeral** jars; the group default is a **snapshot that cascades** (it behaves as if live, but
every tab stores a plain value); live tabs **switch by sleeping**; jars are **nodes in the synced document**; the jar
reference is an **explicit field**, not an edge; cookie contents stay **local to each device**.

## Goal

A tab uses exactly one jar, chosen by the user, and a group can supply the jar new tabs start with. Two tabs on the same
site can be signed in as different accounts, a group can be "work", and a tab can be a private throwaway session. Which jar
a tab uses syncs between devices like any other tab field. Cookies do not.

## Data model

Nodes are last-writer-wins registers keyed by field number; a field a build does not know is kept raw and written back, and
a `Kind` it does not know is kept but never shown (`store/model.rs`). New things, all additive:

- `Kind::Jar`, code 9, name `"jar"`. Fields: `name` (existing 8), `color` (existing 2, from `TAG_COLOR_PALETTE`), and a new
  `ephemeral` Bool. Older builds keep jar nodes without showing them.
- Tab field `jar`: an Id naming a Jar node. Absent means the **default jar**.
- Tab field `jar_set`: a Bool, true when the user picked this tab's jar explicitly. (Not `jar_pinned`: `pinned` already
  means pinned in the graph.)
- Group field `default_jar`: an Id.

New fields get the next free numbers (15 onward; never reused). `jar`, `jar_set` and `default_jar` are ordinary diffed
fields (`explicit: false`): the live payload holds the value and `session_sync::diff` writes a `Set` when it differs, as for
`pinned` and `engine`. (`explicit` is for fields that change continuously, `pos` and `url`, where only a named action writes.)
They join `kind_fields` (`Tab`: `jar`, `jar_set`; `Group`: `default_jar`; `Jar`: `name`, `color`, `ephemeral`) and are carried
by both tab payloads (`TabPayload`, `UnloadedTabPayload`) and `GroupPayload`.

The default jar is built in and has no node; its file stays `jars\default.json`. A node exists only for a named jar.
A jar is not a `NodePayload` and never enters `App.nodes` (the graph): `NodePayload` is matched exhaustively in many places
and a jar has no position or card. Jar definitions live in `App.jar_defs: BTreeMap<String, JarDef>` and are bridged to the
document at the three places the app and the store meet: `shared_nodes` adds a `Kind::Jar` entry per definition (otherwise
`diff` would delete the document's jar nodes as "missing from live"), `materialize` routes a `Kind::Jar` change into
`jar_defs`, and `remove_remotely_deleted` removes a definition another replica deleted. `store::json` export and import
already handle any `Kind` by name, so a whole-document export includes jar nodes. An imported jar is always a new jar under its
fresh id, never matched to an existing jar by name (see "Names").

The reference is a field, not an edge. In this codebase an edge is already an Id field (`parent`, `group`), so the field is
the edge's storage; it enforces one jar per tab, reuses the sync machinery and is written atomically by the cascade. Drawing
a tab-to-jar line later is a rendering change with no store change.

## Resolution

One function, `resolved_jar(tab) -> JarKey` (`Default | Named(NodeId)`), is the only place a tab's `jar` field becomes a
key. An unset field, a deleted jar node or an unknown node resolves to `Default` and flags the tab for a warning chip.
Spawn, pull, swap, the chip and the cascade all use it.

## Inheritance: a snapshot that cascades

- A **new tab** in a group copies the nearest group default into `jar`, with `jar_set` false. A tab with no default anywhere
  has no `jar`.
- **Moving** an unset tab (`jar_set` false) into another group re-snapshots it from that group's nearest default.
- **Picking a jar** on a tab writes `jar` and sets `jar_set`. "Follow group's jar" clears `jar_set` and re-snapshots.
- **Changing a group's `default_jar`** writes it, then walks the group's subtree (nested groups with no default of their
  own included) and writes the new `jar` into every tab whose `jar_set` is false. The writes are ordinary field edits (there
  is no user-level undo in manifold). "Clear default" removes `default_jar` and leaves existing tabs alone.
- A remote device's cascade arrives as ordinary tab edits. Each device's writes follow its own group write, so concurrent
  changes converge on whichever group change wins. A tab created elsewhere during the change can keep the old default until
  a later cascade; accepted.

## Live tabs when `jar` changes (local or remote)

The new reason `SleepWhy::JarSwitch` follows the idle-sleep rules (`idle_sleep.rs`, `sleep_state::verdict`): a tab on screen,
playing audio, pinned, loading, downloading, with a dirty form, or the focused/split tab is not put to sleep.

| Tab state | Effect |
|---|---|
| Sleeping or not yet spawned | Nothing now; it spawns into the new jar. |
| Live, not busy, no dirty form | Goes to sleep with the new jar pending and wakes into it on the next visit. The old jar gets its final cookies from the pull `request_sleep` already does first. |
| Live and busy, focused, or a dirty form | Keeps running on the jar it was spawned with. A "will switch" chip offers "switch now"; it switches when it next qualifies. |

A remote edit uses the same table and never reloads the tab being looked at.

Each running host's `jar_tabs` entry records the `JarKey` it was **spawned with**. Its pulls merge into that jar and save
only that jar's file, so a half-switched tab cannot write one account's cookies into another's jar. If that jar is deleted
while the tab runs, its pulls are dropped and the tab goes through the switch flow to `Default`.

## Ephemeral jars

A jar node with `ephemeral` true has no file. Its `Jar` lives in the in-memory map, is never loaded or saved, and is dropped
when manifold quits (per session). Switching into one is the ordinary flow.

Toggling the flag, whether here or on another device (final review I1): the jar is first loaded from its file, while it
still counts as saved, so nothing on disk is dropped unread. Turning it on then deletes the file (with its unreadable
copies and crashed saves' temp files), and its cookies stay in memory until quit; a locked jar's files (an unreadable file
that could not be moved aside) are never touched. Turning it off saves the loaded jar (a locked one writes nothing).
Turning it on from Settings for a jar with a saved file asks first ("Make Work ephemeral?": the saved sign-ins are deleted
from disk now; tabs keep them until Manifold quits). Clearing the cookies of any jar empties it and
puts the live tabs using it through the switch flow (their engines still hold the old cookies); this signs the user out, so it
asks for confirmation. A host still spawning or waking when the clear lands counts too (final review I3): each jar's clears
are counted beside its seed copy, the spawn records the count when it reads its seed, and a host that lands seeded with
cookies from before a clear goes through the same switch flow.

## Manifold state

`App.jar` and `App.jar_snapshot` become maps from `JarKey` to `Jar`, the snapshot map shared with the spawn threads. The seed
for a spawn is read from the resolved jar's snapshot when the host connects, as today. Named jars load lazily the first time
a tab spawns with one.

`Jar::path` takes a key: `default.json` for `Default`, `<hex node id>.json` for a named jar. The encrypted format, the locked
state on an unreadable file, the unique aside names and the temp-file naming are unchanged. A save failure is logged without
cookie values and leaves the other jars alone.

A named jar's node can sync to a device where its file does not exist; that device starts the jar empty and signs in
separately.

The file goes with the delete (final review I2): deleting a jar, here or on another device, deletes its file, its
unreadable copies and its crashed saves' temp files (each a full copy of its cookies); a running instance's temp file is
left alone. A jar deleted on another device while this one was not running never reaches that path, so at startup, after
the restore, the files of named jars the document no longer has are deleted the same way. That sweep touches only names
of the form `<node id>.json[.unreadable-…|.<pid>.tmp]`, never `default.json` or its copies, and is skipped when the
document may not name every jar: no session store, a repaired one (including one rebuilt as a new document), one written
by a newer build, or one with no live node at all.

## Hosts

No host or IPC change: seeding and pulling are already per host. Servo and WebKit ignore jars; a tab on one of them with a
non-default jar shows the chip with a tooltip saying that engine does not use jars yet, and the picker stays available so a
jar can be chosen before an engine swap. (Wiring those engines is a separate piece of work.)

## UI

Built on the node context menu (a declarative list of `Button`, `Colors`, `MoveTo`, `Separator` items; the caller applies the
action) and the settings page's sections.

- **Tab menu:** a `Jars` item listing "Default", each named jar with its colour dot and a tick on the current one, then
  "New jar...". "New jar..." swaps the menu for a name field (as group rename does). Enter creates the jar with the first
  palette colour no jar uses (cycling once all are used) and picks it for the tab; Escape or a click away cancels (unlike
  group rename, where a click away commits). "Follow group's jar" appears only for a set tab whose group has a default.
  New actions: `SetJar(Option<NodeId>)`, `NewJar(String)`, `FollowGroupJar`, `SwitchJarNow`.
- **Group menu:** the same list under "Default jar for new tabs" with a tick on the current default, and "Clear default". The
  entry shows how many tabs the cascade will change (nothing when none); no confirm dialog (sleeping a background tab loses
  nothing, and setting the default back re-cascades to the tabs that are still unset).
- **Chip:** a coloured dot with the jar name on a tab's card, only for a non-default jar. An outlined dot means following the
  group (unset, and its nearest group default is that jar; after "Clear default" the dot is plain); a warning colour means a
  missing jar; a second dot with an arrow means "will switch", its tooltip offering "switch now". The same dot sits beside
  the address bar in Tab view.
- **Settings, "Jars" section:** one row per jar: swatch, editable name, count of tabs using it, ephemeral toggle, "Clear
  cookies" (confirm) and "Delete" (shows "N tabs use this; they'll use Default (built-in)." and deletes on confirm; the file goes with
  it). The default jar is listed first and cannot be renamed or deleted; "Clear cookies" is available on it.

**Names.** A jar is its node id; its name is only what it is called, and need not be unique. A rename to a name in use is
allowed, and import never matches jars by name. Every view shows a jar's *label*, derived each time and never stored or
synced (`jar_defs::JarLabel`, from `named_jar_label` and `App::jar_label`). A label is a name and an optional marker:
- A name no other jar has, compared by its clean form (name rules: hidden characters dropped, spaces collapsed,
  trimmed) and in any case, is shown with no marker, through the name rules' display rule.
- When two or more jars share a name, each one's marker is `#<frag>`. `<frag>` is the start of the SHA-256 hex of the node
  id's text, 4 characters, or as many more as it takes to differ from the other same-named jars'. It derives from the id
  alone, so it stays the same for the jar's life.
- The built-in Default jar's label is "Default" with the marker `built-in` (also what a tab naming a deleted jar shows, as it
  uses the built-in jar). A user jar may be called "Default"; it is shown as stored.

Rich-text sites draw the marker in its own style: after the name, in monospace at 80% of the name's size, in the theme's weak
text colour, centred on the line, and without the plain text's " · ". The built-in's name is also italic. A name typed to
look like a marker ("Work · #1a2b") is drawn as an ordinary name, so it never looks like a genuine marker, and the built-in
jar never looks like a user jar named "Default". The rich sites are the tab, unloaded and group menus, Settings' "Jars"
rows, the card chip and Tab view's dot hover. The chip ellipsizes only the name and keeps the marker whole; on a card too
narrow for a short name and the marker it shows just the dots. Its scrim is always dark, so its marker is the chip text
dimmed by the theme's weak text alpha. Plain-text sites (the confirm dialogs, the chip's warning and will-switch tooltip)
use the plain string: `"<name> · #<frag>"`, the name alone, or "Default (built-in)" for the built-in jar.

Settings' name field edits the stored name. A same-named jar's marker sits beside the field as separate read-only text, so
it can never be saved into a name; the built-in's row shows its styled label. Headless `SETJAR` takes a label's plain string
first, then a stored name. When several jars share that stored name, the lowest node id wins. `SETJAR default`, any case,
still names the built-in jar, so `SETJAR` cannot reach a user jar named "Default" (the menus can).

Deleting a jar is an ordinary node delete: tabs that named it resolve to `Default`.

## Testing

Unit tests, using `store::testutil::temp_dir` and never the real profile (`profile::dir()` is `None` under test):
`resolved_jar` and every fallback; the cascade over nested groups with set and unset tabs; a new tab and a move inheriting;
a pull merging into the spawn jar rather than the current one; ephemeral jars never writing a file; jar delete and clear
cookies; the switch table's decisions as a pure function over the busy-tab inputs. `scripts/jar-probe.sh` gains a run where two
tabs on the same site use two named jars and stay signed in as different accounts, then a switch of one tab between jars.

## Migration

None. Existing sessions have no jar fields, so every tab resolves to `Default`; `default.json` is untouched.

## Out of scope

Syncing cookie contents between devices (DPAPI keys do not travel; it needs its own encryption design), import and export of a
jar, dragging a tab onto a jar, drawing jars or jar edges in the graph, Servo and WebKit cookie wiring, jar-scoped
localStorage and the other storage the v1 spec leaves out.

## Open questions

- Whether "apply to group" needs a confirm once groups are large (there is no undo, so a mistaken default is corrected by setting it again).
- Ephemeral lifetime: per session is the rule here; "until the last tab using it closes" is the alternative.
- Whether a remote jar delete should offer to keep the local cookie file (today the file goes with the delete).
- Name rules for every user-editable name (jars, groups, the saved-session name, bookmark folders) are built (`2026-10-03-name-rules-design.md`): names are cleaned when committed, and a stored name's hidden characters are shown as placeholders.
- After "Clear cookies", every pull from that jar's running tabs is dropped until they switch, including a login made after the clear (plan deviation 14). Accepted as "cleared means cleared"; revisit if users sign in again in a tab still waiting to switch and expect it kept.
