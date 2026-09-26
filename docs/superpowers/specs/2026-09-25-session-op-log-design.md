# Sessions as a Replicated Operation Log

## Goal

Replace `session.json` and the group-export format with one data model,
stored in a custom binary operation log that can later be merged
between replicas. Three formats describe "a piece of the graph" today:
`session::Session` (one list per kind, six id counters), `group_export::
GroupExport` (tabs and nested groups only) and the in-memory
`ClosedEntry` (every kind). After this change they share one node model,
and the auto-restore session is a journal of operations that two
processes, and later two machines, can both write.

This is sub-project 1 of 4:

1. **Op-log core, single-player** (this spec): data model, operations
   and merge rules, byte format, journal, two local instances merging.
2. Sync between machines: version-vector exchange, a transport, op
   signing.
3. Presence: showing other participants' views.
4. A text CRDT for note text (last-writer-wins until then).

Everything here is designed so 2–4 add to the format without changing
it.

## Decisions

- **Clean break.** The new file is `session.mnf`. An existing
  `session.json` is ignored and left in place; nothing reads or deletes
  it. No legacy reader.
- **One node model** for the session, group export and Recently Closed.
- **Save as much as possible** (the list below), except state that is
  recomputed or cannot be carried over.
- **Custom operation log, designed for multiplayer now**, rather than a
  CRDT library (Loro, Automerge) or a snapshot format.
- **Hand-rolled TLV** record bodies, protobuf-like: numbered fields,
  skip unknown, default missing.
- **Bijective prefix varint** for integers: canonical and sortable by
  construction.
- **CRC-32C** per frame, covering length, type and body.
- **Ids are `(actor, counter)`** with a fresh random actor per launch.
- **Browsing history** moves out of sessions into its own
  `history.json` (it is app-wide, and today not saved at all).
- **Self-contained frames** (format revision, 2026-09-25). Each `Ops`
  and `Snapshot` frame carries its own actor table, author first, as
  Automerge's chunks do. Any frame decodes without the others, so damage
  costs only the damaged frame. An ops frame grows by about 15 B (median
  60–64 B against 46 B).
- **Personal state in a sidecar**, `session.mnf.local`, replaced whole
  (Obsidian, JetBrains and VS Code also keep UI state apart). The
  journal holds only shared state, so personal-state churn never grows
  it or triggers a compaction.
- **Recover, don't truncate.** Damage mid-file is skipped, not cut:
  reading resumes at the next valid frame. The original file is saved
  before the file is rewritten without the damage. `session.mnf.prev`,
  the file before the last compaction, rebuilds a lost snapshot. Precedents:
  LevelDB resyncs at block boundaries; Firefox keeps `previous.jsonlz4`.
- **Compaction prunes** tombstones, placeholders and dead actors that no
  later op can need, while the document has never been shared.
- **No compaction at exit when nothing changed.** Firefox's frequent
  full-session writes wore SSDs (bug 1305193, fixed partly by skipping
  idle writes, bug 1305950); Chrome doesn't rebuild its session file
  after a clean close.

## Data model

### Document id

Each document gets 128 random bits from the OS when it is created,
stored as 16 raw bytes in every `Snapshot`. Sync (sub-project 2) uses it
so peers know they hold the same document; invites carry it. Its text
form is 26 uppercase Crockford base32 digits (the 128 bits, big-endian,
top digit 0–3) plus two check digits, 28 characters: case-insensitive,
so it is safe in Windows file names; free of look-alike characters, so
it can be read out; URL-safe without percent-encoding; selectable with
one double-click; sortable; and all in QR alphanumeric mode. The check
digits are chosen so the whole 28-digit number is 0 mod 1021 (a prime
below 32²): every single-character substitution and every adjacent
transposition, check digits included, is rejected, and a random string
passes about 1 time in 1000. Decoding folds case, maps I/L to 1 and O
to 0, and rejects a failed check. Not Crockford's own mod-37 check
character: its extra symbols `* ~ $ =` break file names (`*`), QR
alphanumeric mode (`~ $ =`) and double-click selection. Not nanoid or
base64url: those are case-sensitive, and two ids differing only in case
would collide as file names. Save As
creates a new document id. Import Session and Open replace the live
document's nodes and keep its id (Plan B). Only sync shares one.

### Actors and ids

- Each launch draws a random 64-bit **actor id** from the OS
  (`getrandom`, already a transitive dependency). It is never persisted
  or reused, so a crash cannot make a counter repeat and a copied
  profile cannot produce a twin actor.
- A **node id** is `(actor, counter)`; the counter starts at 0 each
  launch and is shared by all kinds. Printed as `1f3a9c…-17`.
- The per-kind `NodeIds` counters and the `next_*_id` fields go away.
- An **op id** is also `(actor, counter)` from the same counter. A
  `Create`'s op id differs from the node id it creates: the node id is
  drawn when the node is made, op ids when ops are stamped.
- Every op's stamp `(hlc, actor)` is unique (`Clock::tick`); merge
  order depends on it.

### Hybrid logical clock

`Hlc { ms: u64, logical: u32 }`. A local event takes
`max(now_ms, last.ms)`; if that equals `last.ms`, `logical + 1`, else 0.
Receiving an op advances the clock past its timestamp. On open, the
clock starts past the highest timestamp in the file, so a clock that
went backwards (a reboot, a manual change) still orders new ops after
old ones. Ops are totally ordered by `(hlc, actor)`.

### The shared document

A set of nodes. Each node has:

- **Existence**: created by `Create`, removed by `Delete`. Ids are never
  reused. A deleted node stays a tombstone until compaction may prune it
  (see Journal); a node id that names no node reads as deleted.
- **Kind**, fixed at creation: `Tab`, `Group`, `Note`, `Image`, `Pdf`,
  `Json`, `JsonLeaf`, `Download`.
- **Fields**, each a last-writer-wins register holding the value and the
  `(hlc, actor)` that wrote it. A field is either **diffed** (only the
  user changes it, so comparing live state with the document finds
  every change) or **explicit** (the engine or the layout also changes
  it, so only named user actions emit a `Set`; see "Local edits become
  ops"):

| Field | Kinds | Emitted | Notes |
|---|---|---|---|
| `pos` | all | explicit | drag end, placement of a new node, drop, import (see Positions) |
| `color` | all | diffed | `None` = untagged; black is `Some(0)` and is encoded |
| `group` | all | diffed, as `Move` | the tree edge |
| `parent` | Tab | diffed | "opened from" link, a plain register |
| `url` | Tab | explicit | the target of the last user-started navigation (typed, clicked, back/forward). Redirects and script navigation should never set it, so replicas whose logins redirect differently don't bounce each other's tabs. **Plan B sets it on every url change the engine reports, redirects included**, because no engine reports a navigation's cause, except a change that follows a load the store started (another replica's `url`, a restore): that stays local while the tab loads and for 5 s after (`App::shares_url_change`). Instances on one profile share logins, but a site that redirects to a fresh url on every load (a consumed nonce) would otherwise have two instances navigate each other forever (two-instance test, scenario 3). Sub-project 2 must narrow it to user-started navigations before sync. |
| `engine`, `pinned` | Tab | diffed | |
| `name`, `from_json` | Group | diffed | |
| `text` | Note | diffed | whole string, LWW (sub-project 4 replaces it) |
| `path` | Image, Pdf, Json, JsonLeaf, Download | diffed | machine-local; see the sub-project 2 notes |
| `key`, `value` | JsonLeaf | diffed | `value` is its JSON text |

  Tab `title` and `zoom` are personal, not shared: each replica's own
  engine produces the title, and zoom is a viewing preference like mute.

- **Unknown fields** (a field number this build does not know, written
  by a newer build) are kept on the node as raw bytes with their
  timestamp and written back at compaction.
- **Unknown kinds** (a kind a newer build added) are kept whole: the
  node exists in the document, is not shown, and is written back at
  compaction.

A failed tab is saved as a `Tab`; it is retried on load. A download is
created in the document only once it completes; the diff skips
in-progress downloads, which cannot resume.

### Personal state

Kept per profile, never merged, in `session.mnf.local` beside the
journal (see "Personal-state file"):

- view: `focused`, view mode, `current_group`, `viewed_image`,
  `split_tab`, `groups_collapsed`, and the camera rect with the level it
  was saved at (the app has one camera; per-level cameras are later);
- per node: a note's `editing`; a tab's `muted`, `zoom` and `title`.
  The current url equals the shared `url` while Plan B sets `url` on
  every change, so it isn't stored; sub-project 2 adds it back;
- the last settled position of every node, so the graph reopens exactly
  as it was left rather than re-settling from the shared `pos`;
- Recently Closed: up to 20 entries, oldest first, each a JSON snapshot
  (root first, then its subtree; `local` carries tab zoom).

Encoded as JSON (`local_state::LocalState`) inside the personal-state
file's state bytes.

Two instances on one profile each keep their own personal state while
running, and the last write wins. So on the next launch one instance's
Recently Closed list replaces the other's; accepted.

When it is written (Plan B):
- only when it changed, and at most every 30 s;
- 2.5 s after a Recently Closed change (Chrome's save delay), since that
  alone can't be recomputed;
- at exit.

Settled positions count as changed only past 1 px, so physics jitter
never causes a write.

Sizing: about 95 B per tab (id, settled position, title, and the current
url when it differs from the shared one), 3.4 KB of Recently Closed and
200 B of view state. That is about 32 KB at 300 tabs and 320 KB at 3000,
half of today's `session.json`, which is rewritten on every save. Even
always changing, that is at most 3.8 MB/h or 38 MB/h. A crash loses at
most 30 s of recomputable state and 2.5 s of Recently Closed.

### Not saved

Loading, audio and back/forward flags; error text; image and PDF sizes
and render results (re-read from the file); hover, drag, menus; the find
bar; filter texts; a tab's back/forward history (the engine owns it, and
an engine swap already drops it).

### Semantics

- **Delete wins** over a concurrent `Set` or `Move` on the same node.
- **Deleting a group** deletes the nodes the deleting replica saw inside
  it (one `Delete` each, in one op batch). A node another replica moved
  in concurrently, which therefore has no `Delete`, is shown at the
  deleted group's nearest live ancestor level. This re-homing is
  computed when the document is materialized, not emitted as an op, so
  replicas never race to emit it. The node's `group` register still
  names the tombstone; the diff compares against the effective group, so
  re-homing does not read as a user `Move`.
- **Reopening** a Recently Closed entry or **importing** a group export
  creates new nodes with fresh ids (tombstones are final), remapping
  `group` and `parent` inside the entry. This is today's `reopen_closed`
  path, generalized.

## Operations and merge

```
Op { id: (actor, counter), hlc, body }
body = Create { node, kind, fields }
     | Set    { node, field_no, value }
     | Move   { node, new_group }
     | Delete { node }
```

- **Create** always applies (ids are unique).
- **Set**: per `(node, field_no)` the highest `(hlc, actor)` wins,
  whatever the arrival order. The value is encoded exactly as that field
  is inside a node message, so one field table drives encoding, merging
  and materializing. A `Set` for an unknown field number is stored raw.
- **Delete**: tombstone; its registers are cleared and later `Set`s on
  it are ignored. `Move`s still apply (below).
- **Move**: the replicated tree-move algorithm (Kleppmann et al., "A
  highly-available move operation for replicated trees"). The Move log
  is kept sorted by `(hlc, actor)`. Applying a move that would put a node
  inside itself (or its descendant) is a no-op. Tombstoned nodes keep
  their `group` edges in the tree the cycle check walks, so the check
  gives the same answer on every replica whatever it has deleted; only
  display skips them. Moves apply to tombstoned nodes like any other, so
  whether a `Delete` arrived first never changes the tree (ignoring them
  broke convergence: undo and redo of later moves differed between
  replicas). A node in a tombstoned group is displayed at its nearest
  live ancestor. A move arriving earlier
  than already-applied ones undoes those, applies, and redoes them, so
  every replica ends with the same acyclic tree. Single-player and
  in-order, the undo path is never taken.

### Local edits become ops

At each existing `save_session()` call, `session_sync` diffs the live
`App::nodes` against the materialized document and emits `Create`,
`Move`, `Delete`, and `Set` for **diffed** fields. The ~30 call sites do
not change. **Explicit** fields (`pos`, `url`) are never diffed:
comparing them would turn every physics step and every redirect into a
shared edit, and two replicas whose engines disagree would overwrite
each other forever. Instead the sites of the named user actions (drag
end, placement, drop, import, a typed or clicked navigation, back or
forward; in Plan B every reported url change, see the `url` row) mark the
node, and the next save emits a `Set` for marked
fields only. Personal fields (settled positions, title,
zoom, mute, note editing) are written to the personal-state file.

### Positions

Physics settling and layout motion stay local. Only drag ends and
explicit placements (new node, drop, import) emit `Set pos`. A remote
`pos` should arrive as a target the local layout eases toward; Plan B
sets it directly (a jump), and easing is left for later.

## Byte format

### Varint: bijective prefix, big-endian

The count of leading 1-bits in the first byte gives the number of extra
bytes (0–8). Payload bits are big-endian. Each tier adds the total size
of the tiers below it as an offset:

| First byte | Total bytes | Values |
|---|---|---|
| `0xxxxxxx` | 1 | 0 – 127 |
| `10xxxxxx` +1 | 2 | 128 – 16 511 |
| `110xxxxx` +2 | 3 | 16 512 – 2 113 663 |
| … | … | … |
| `11111111` +8 | 9 | up to `u64::MAX` |

Tier k (k extra bytes, k < 8) carries 7 + 7k payload bits; the 9-byte
tier carries 64. Every `u64` has exactly one encoding and byte order
equals numeric order. Every byte string of a tier's length decodes to
one value, with one exception: in the 9-byte tier, a payload larger
than `u64::MAX` minus that tier's offset would overflow, and the decoder
rejects it. That is the only validation the decoder does. Signed values
are zigzag-encoded first.

### TLV messages

A message is a sequence of fields. Each field is a key varint
`(field_no << 3) | wire_type` and a value:

| Wire type | Value |
|---|---|
| 0 | varint (bools, enums, counters, zigzag signed) |
| 1 | fixed 8 bytes little-endian (actor ids, `f64`) |
| 2 | varint length + bytes (strings, bytes, nested messages) |
| 5 | fixed 4 bytes little-endian (`f32`) |

Unknown fields are skipped by wire type; missing fields take their
default. Writers emit fields in ascending field number, each at most
once, and omit fields holding their default, so one value has one
encoding (what hashing and signing ops in sub-project 2 rely on).
Readers accept any order and keep the last occurrence. `f32`/`f64`
values are written with `-0.0` normalized to `0.0`; NaN is never
stored (a NaN position is not saved). Field numbers are never reused; retired ones go in a `RESERVED`
list checked by a test. Encodings of shared values:

- node id in a frame's own structure (op targets, `Move` groups,
  snapshot node keys and group edges, stamps): nested `{1: actor index,
  2: counter}`, the index into the frame's own actor table.
- node id inside a field value (`parent`): nested `{1: actor (fixed64),
  2: counter}`, the full actor. A field this build does not know is kept
  as raw bytes and could not be renumbered, so field values never hold
  file-local indexes. JSON (and sync later) also uses the full actor.
- `pos`: two `f32`; `color`: varint `0xRRGGBB`, absent when untagged
  (black is `Some(0)`, present). A field is omitted only when it holds
  exactly what absence decodes to;
- `engine`, kind: enum varints.

### File and frames

```
file   = header | frame*
header = magic "MNF\0" | format_version (varint) | generation (u64 LE)
         | prev_generation (u64 LE) | crc (u32 LE)
frame  = len: u32 LE | crc: u32 LE | frame_type (varint) | message
         (len counts frame_type + message)
```

- `generation` is random, new at each compaction: a reader holding an
  offset into the old file sees it change and re-reads from the start.
  `prev_generation` is the generation of the file this one was compacted
  from, which `session.mnf.prev` then holds; 0 for none. The header's
  `crc` is CRC-32C over the header bytes before it. Later format versions
  add nothing to the header, so its layout is known to every reader.
- A frame's `crc` is CRC-32C over `len ‖ frame_type ‖ message`, so an
  all-zero region never parses as a frame. `len = 0` and `frame_type =
  0` are invalid.
- Frame types:
  - `Snapshot` (2): `{1: document id, 2: node*, 3: version*, 4:
    reserved for a Move-log tail, 5: clock, 6: actor*}`. The actor table
    is sorted, and all indexes in the frame refer to it.
  - `Ops` (3): `{1: actor*, 2: op*}`, a batch from one actor. The table
    lists the author first (index 0), then the other actors its ops name.
    Counters and HLCs are delta-encoded within the batch.
  - Types 1 (a file-wide actor table) and 4 (personal state) are
    retired and never reused.
- **Reading.** A scan stops at a frame that overruns the file, fails its
  CRC, or has `len = 0` or type 0.
  - No valid frame after the stop: a **torn tail**. It is remembered and
    cut before the next append.
  - A valid frame after the stop, or a stopped frame that was whole but
    failed its CRC: **damage**. The next valid frame is sought first
    where the stopped frame's length points, then at each later byte.
    Reading resumes there.
  - Also damage: a whole frame that doesn't decode, and a `Snapshot`
    after another frame. Both are skipped.
- **Repair**, on open and whenever catching up finds damage:
  1. Save the file as it was to `session.mnf.damaged-<16 hex digits>`
     and fsync it. The digits are the generation, or the whole file's
     CRC-32C when the header is damaged.
  2. Rewrite the file as a snapshot of what survived. Leave
     `session.mnf.prev` and the header's link to it as they were.

  If no snapshot survived, the base is `.prev` when its generation equals
  the header's `prev_generation` (any `.prev` when the header is
  damaged); otherwise an empty document under a new id. The surviving ops
  apply on top. A build that must not compact the file (below) only
  saves the copy. The app tells the user what was repaired.

  `.prev` is still the file from before the damaged generation, so a
  later rebuild from it can miss changes made in that generation; this is
  reported as `from_prev`.
- A frame with a valid CRC and a **type this build does not know**
  (written by a newer build) is skipped and kept, never treated as a
  torn tail, so an older build cannot truncate a newer build's data. An
  `Ops` frame holding an op kind this build does not know is kept the
  same way.
- A build that opened a file containing unknown frame types, op kinds,
  or fields in the file's own structure (a frame that does not re-encode
  to the same bytes), or a `format_version` newer than its own, **never
  compacts** that file (compaction would drop what it cannot represent).
  It still appends its own ops. With a newer `format_version` it opens
  the file read-only and says so.

### Personal-state file

```
file  = magic "MNL\0" | format_version (varint) | frame
frame = as above, type 1 (`State`): {1: document id, 2: state}
```

It is replaced whole (a temp file, fsync, rename) under the journal's
lock. Reading it:
- A document id other than the journal's means the journal was replaced
  or recovered: the file is ignored.
- A damaged file returns a Corrupt error, which the app treats as no state.
- A newer format version is not read, and the next write replaces it.
  Personal state is recomputable, and newer builds read older versions.

CRC-32C is hand-rolled (table-driven), with an optional SSE4.2 path via
`std::arch` checked against the table path by a test.

## Journal

- **Open**: check magic and version, and read frames, skipping damage.
  Start from the `Snapshot` and apply later `Ops`. Repair any damage
  (see File and frames).
- **Append**: at a save point, diff → one `Ops` frame. `fsync` is
  debounced to about 1 s and forced on exit. On Windows the handle must
  be opened for writing, or `FlushFileBuffers` fails. A crash loses at
  most that second and never leaves an unreadable file. A write that
  fails partway leaves the replica out of step with the file, so it
  re-reads the file whole before its next write. Personal state goes to
  its own file.
- **Compact** when the ops since the last snapshot exceed twice its size
  and at least 64 KB, and on a clean exit if anything changed since this
  replica's last read. "Changed" means its own appends, another replica's
  frames, a torn tail, or damage (unless a rule above forbids
  compaction). The steps:
  1. Write a header with a new generation, then the `Snapshot`, to a
     temp file, and fsync it.
  2. Keep the old file as `session.mnf.prev`: a hard link, or a copy
     where links fail. Skipped when repairing a damaged file.
  3. Rename the temp file over the journal (atomic on Windows).

  Unknown node fields (kept as raw registers) carry over, and the
  Move-log tail field stays empty single-player.

  **Pruning**, while the document has never been shared, drops:
  - deleted nodes and placeholders (nodes whose `Create` never arrived)
    that no group edge names, repeated until nothing more drops;
  - version entries of actors that no node id, group edge or register
    names.

  What is shown doesn't change. A later op naming a dropped node makes a
  placeholder, which isn't shown. Id-valued registers (`parent`) are not
  counted: they may name a pruned node, which reads as deleted.
- **Two instances on one profile**: each append takes an exclusive lock
  on a separate `session.mnf.lock` file (`std::fs::File::lock`; a lock on
  the journal itself would be lost when compaction renames a new file
  over it). Under the lock it opens `session.mnf` by path; if its
  header's generation differs from the one it read last, or the file is
  shorter than its offset (another instance compacted), it re-reads the
  file from scratch, otherwise it reads only the frames
  appended since its last offset. It merges them, appends its own frame,
  then unlocks. Compaction also runs under the lock. The rename must
  succeed while the other instance holds the old file open; the
  headless two-instance test covers a compaction during the other's
  run. Merged remote ops update the live graph through
  `session_sync`: a remote `Create` of a tab spawns it, a remote `Delete`
  closes a node without adding it to Recently Closed, a remote `Set`
  updates the card. This replaces `session::fingerprint`,
  `conflict_path` and the conflict toast.
- **Named sessions**: each is its own `.mnf` in `sessions/`. Save As
  writes a compacted copy (no `.prev`) under a new document id, with the
  copy's personal-state file. Open replaces the running session's
  contents: it deletes every live node and recreates the file's nodes
  under fresh ids, remapping its personal state. `session.mnf` stays the
  one live file.

## JSON snapshots

The same model through serde, with no history:

- **Export Session / Import Session** (file dialogs) and
  **Export Group / Import Group**. A group export lists the group first,
  then its subtree. Import creates fresh nodes (see Semantics).
- **`EXPORTSESSION`** keeps writing this JSON, so probes keep reading it.
- **CLI**: `manifold --dump <file.mnf>` prints frames and the
  materialized document as JSON; `manifold --check <file.mnf>` reports
  CRC failures and the torn-tail offset; `manifold --import-json <in>
  --out <file.mnf>` builds a journal from a JSON snapshot (used by
  `drive.sh`'s `SEED`).

## Code layout

- `store/` (pure: no egui, no `App`):
  - `varint.rs`: bijective prefix varint, zigzag;
  - `crc32c.rs`;
  - `tlv.rs`: `Writer`, `Reader`, keys, skipping;
  - `model.rs`: `ActorId`, `NodeId`, `Hlc`, `Doc`, `Node`, `Kind`, the
    field table, raw unknown fields;
  - `ops.rs`: `Op`, merge rules, tree move with undo/redo;
  - `journal.rs`: frames, open/append/compact, lock, torn tail;
  - `json.rs`: serde snapshots.
- `session_sync.rs` (`impl App`): live nodes ↔ document; diff to ops;
  apply remote ops.
- Removed: `session.rs`'s `Session*` structs and fingerprinting,
  `group_export.rs`, `NodeIds`, `ClosedEntry`'s own node type (it holds
  `store` snapshots).
- `history.json`: `App::history` saved like `bookmarks.json`.

## Testing

Unit (in `store/`, no new dependencies; randomized tests use a small
seeded xorshift):

- varint: exhaustive round-trip over the 1- and 2-byte tiers, random
  values above, each tier's first and last value, `u64::MAX`; byte order
  equals numeric order; every byte string of a tier's length decodes
  (canonical), except 9-byte payloads past the overflow bound, which are
  rejected.
- CRC-32C: standard check vectors; SSE4.2 path equals table path.
- TLV: a message with extra fields decodes in an older reader; an
  unknown `Set` field survives compaction; `RESERVED` numbers unused.
- Torn tail: truncate a journal at every byte offset; each open yields
  exactly the whole frames before the cut. A zero-filled tail parses as
  no frames.
- Convergence: random ops from three actors, including concurrent moves
  that would form cycles and deletes racing moves, applied in many
  random orders, give byte-identical snapshots.
- Recovery: each ops frame, bit-flipped or zeroed, costs only its own
  ops, and the original file is saved first. A damaged snapshot is
  rebuilt from `.prev`. Damage that another replica's catch-up finds is
  repaired, not cut. Seeded random file damage (bit flips, zeroed and
  random runs, copied runs, truncation) never panics, and after a
  compacting close the file is clean.
- Pruning never changes what is shown.
- An idle open and close leaves the file byte-identical.

Headless:

- The smoke test passes, with its fixtures rewritten as JSON snapshots.
- Two instances on one `APPDATA`: A creates a group, B moves a tab into
  it; both `EXPORTSESSION` outputs match.
- Two instances, one compacting while the other stays open and keeps
  editing: both converge and nothing is lost.
- Kill: kill the process after edits; reopening has them. (A killed
  process loses nothing the OS already accepted; the fsync debounce
  matters only for power loss or an OS crash, which a headless test
  cannot produce. The torn-tail unit test covers partial frames.)

## Notes for sub-project 2

- Clock skew: a replica whose clock runs far ahead would win every LWW
  register. Receivers should clamp or reject remote timestamps more than
  a set drift (say, a minute) ahead of their own clock.
- `path` fields are machine-local: an Image, PDF, JSON or Download card
  from another machine points at a file this machine does not have.
  Sharing file contents (blobs) needs its own design; until then those
  cards show their missing-file state.
- A shared tab whose `engine` is not installed here falls back to a
  local engine without changing the shared field.
- Compaction prunes tombstones and dead actors only while a document
  has never been shared. Sub-project 1 never shares, so it always
  prunes. Sub-project 2 must record when a document is first shared (a
  `shared` field in the snapshot) and stop pruning from then on. Pruning
  a shared document needs causal stability (every peer has seen the
  tombstones), which only sync can establish.

- Invite links carry the document key after `#` (`manifold://join/<doc-id>#<key>`
  or the https equivalent): the fragment is never sent to a server, so a
  relay that logs URLs never sees the key.
- Parked: human-oriented encodings for reading invites aloud (proquints,
  BIP39 words, the PGP word list). The URL survey behind the Crockford
  choice compared hex, RFC 4648 base32 and base32hex, z-base-32,
  base36, base58, base62, base64url/nanoid, base85/Z85 and base45.

## Out of scope

Sync between machines and its transport, op signing (Ed25519) and
content addressing (BLAKE3), presence UI, a text CRDT for notes,
columnar op encoding for long histories, migrating `session.json`.
