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

## Data model

### Document id

Each document gets 128 random bits from the OS when it is created,
stored as 16 raw bytes in every `Snapshot`. Sync (sub-project 2) uses it
so peers know they hold the same document; invites carry it. Its text
form is Crockford base32 (26 characters): case-insensitive, so it is
safe in Windows file names, and free of look-alike characters, so it
can be read out. Not nanoid or base64url: those are case-sensitive, and
two ids differing only in case would collide as file names. Save As
and Import Session create a new document id; only sync shares one.

### Actors and ids

- Each launch draws a random 64-bit **actor id** from the OS
  (`getrandom`, already a transitive dependency). It is never persisted
  or reused, so a crash cannot make a counter repeat and a copied
  profile cannot produce a twin actor.
- A **node id** is `(actor, counter)`; the counter starts at 0 each
  launch and is shared by all kinds. Printed as `1f3a9c…-17`.
- The per-kind `NodeIds` counters and the `next_*_id` fields go away.
- An **op id** is also `(actor, counter)` from the same counter.

### Hybrid logical clock

`Hlc { ms: u64, logical: u32 }`. A local event takes
`max(now_ms, last.ms)`; if that equals `last.ms`, `logical + 1`, else 0.
Receiving an op advances the clock past its timestamp. Ops are totally
ordered by `(hlc, actor)`.

### The shared document

A set of nodes. Each node has:

- **Existence**: created by `Create`, removed by `Delete`. A deleted id
  stays a tombstone forever; ids are never reused.
- **Kind**, fixed at creation: `Tab`, `Group`, `Note`, `Image`, `Pdf`,
  `Json`, `JsonLeaf`, `Download`.
- **Fields**, each a last-writer-wins register holding the value and the
  `(hlc, actor)` that wrote it:

| Field | Kinds | Notes |
|---|---|---|
| `pos` | all | `Set` only on drag end or explicit placement (see Positions) |
| `color` | all | `None` = untagged |
| `group` | all | the tree edge; changed only by `Move` |
| `parent` | Tab | "opened from" link, a plain register |
| `url`, `engine`, `title`, `pinned`, `zoom` | Tab | `url` is set only by user-started navigation (typed, clicked, back/forward), never by redirects or script navigation, so replicas whose logins redirect differently don't bounce each other's tabs. `title` is written by whichever replica's engine loaded the page |
| `name`, `from_json` | Group | |
| `text` | Note | whole string, LWW (sub-project 4 replaces it) |
| `path` | Image, Pdf, Json, JsonLeaf, Download | |
| `key`, `value` | JsonLeaf | `value` is its JSON text |

- **Unknown fields** (a field number this build does not know, written
  by a newer build) are kept on the node as raw bytes with their
  timestamp and written back at compaction.

A failed tab is saved as a `Tab`; it is retried on load. A download is
created in the document only once it completes; an in-progress download
cannot resume and is not saved.

### Personal state

Kept per profile, never merged, stored in `Local` frames:

- view: `focused`, view mode, `current_group`, `viewed_image`,
  `split_tab`, `groups_collapsed`, camera rect per level (key: group id,
  or none for the top level);
- per node: a note's `editing`, a tab's `muted`;
- Recently Closed: up to 20 entries, newest first, each a list of node
  snapshots (the node model with its fields, no timestamps).

### Not saved

Loading, audio and back/forward flags; error text; image and PDF sizes
and render results (re-read from the file); hover, drag, menus; the find
bar; filter texts; a tab's back/forward history (the engine owns it, and
an engine swap already drops it).

### Semantics

- **Delete wins** over a concurrent `Set` or `Move` on the same node.
- **Deleting a group** deletes the nodes the deleting replica saw inside
  it (one `Delete` each, in one op batch). A node another replica moved
  in concurrently, which therefore has no `Delete`, is re-homed to the
  deleted group's nearest live ancestor level.
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
- **Delete**: tombstone; later `Set`/`Move` on it are ignored.
- **Move**: the replicated tree-move algorithm (Kleppmann et al., "A
  highly-available move operation for replicated trees"). The Move log
  is kept sorted by `(hlc, actor)`. Applying a move that would put a node
  inside itself (or its descendant) is a no-op. A move arriving earlier
  than already-applied ones undoes those, applies, and redoes them, so
  every replica ends with the same acyclic tree. Single-player and
  in-order, the undo path is never taken.

### Local edits become ops

At each existing `save_session()` call, `session_sync` diffs the live
`App::nodes` against the materialized document and emits `Create`,
`Set`, `Move` and `Delete` ops. The ~30 call sites do not change.

### Positions

Physics settling and layout motion stay local. Only drag ends and
explicit placements (new node, drop, import) emit `Set pos`. A remote
`pos` arrives as a target the local layout eases toward, not a jump.

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
| 1 | fixed 8 bytes little-endian (actor ids) |
| 2 | varint length + bytes (strings, bytes, nested messages) |
| 5 | fixed 4 bytes little-endian (`f32`) |

Unknown fields are skipped by wire type; missing fields take their
default. Field numbers are never reused; retired ones go in a `RESERVED`
list checked by a test. Encodings of shared values:

- node id: nested `{1: actor index, 2: counter}` (actor index into the
  file's actor table);
- `pos`: two `f32`; `color`: varint `0xRRGGBB`, absent when untagged;
- `engine`, kind: enum varints.

### File and frames

```
file  = magic "MNF\0" | format_version (varint) | frame*
frame = len: u32 LE | crc: u32 LE | frame_type (varint) | message
        (len counts frame_type + message)
```

- `crc` is CRC-32C over `len ‖ frame_type ‖ message`, so an all-zero
  region never parses as a frame. `len = 0` and `frame_type = 0` are
  invalid.
- Frame types: `Actors` (actor u64 → index table, appended as actors
  appear), `Snapshot` (document id, materialized document, tombstone set, version
  vector, and a reserved field for a retained Move-log tail),
  `Ops` (a batch from one actor; counters and HLCs delta-encoded within
  the batch), `Local` (personal state).
- Reading stops at the first frame that overruns the file or fails its
  CRC or type check; that offset is remembered and the file is truncated
  to it before the next append.

CRC-32C is hand-rolled (table-driven), with an optional SSE4.2 path via
`std::arch` checked against the table path by a test.

## Journal

- **Open**: check magic and version; read frames; start from the latest
  `Snapshot`, apply later `Ops`, take the latest `Local`.
- **Append**: at a save point, diff → one `Ops` frame; personal-state
  changes → one `Local` frame. `fsync` is debounced to ~1 s and forced on
  exit; a crash loses at most that second and never leaves an
  unreadable file.
- **Compact** when the ops since the last snapshot exceed twice its size,
  and on a clean exit: write `magic, Actors, Snapshot, Local` to a temp
  file, fsync, rename over the journal (atomic on Windows). Unknown raw
  fields and tombstones carry over; the Move-log tail field stays empty
  single-player.
- **Two instances on one profile**: each append takes an exclusive file
  lock (`std::fs::File::lock`), reads and merges frames other processes
  appended since its last offset, appends its own, then unlocks. If the
  file was replaced by a compaction (its file id changed), it reopens
  from scratch. Merged remote ops update the live graph through
  `session_sync`: a remote `Create` of a tab spawns it, a remote `Delete`
  closes a node without adding it to Recently Closed, a remote `Set`
  updates the card. This replaces `session::fingerprint`,
  `conflict_path` and the conflict toast.
- **Named sessions**: each is its own `.mnf` in `sessions/`. Save As
  writes a compacted copy; Open switches the live file.

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

Headless:

- The smoke test passes, with its fixtures rewritten as JSON snapshots.
- Two instances on one `APPDATA`: A creates a group, B moves a tab into
  it; both `EXPORTSESSION` outputs match.
- Crash: kill the process after edits and past the fsync debounce;
  reopening loses nothing.

## Out of scope

Sync between machines and its transport, op signing (Ed25519) and
content addressing (BLAKE3), presence UI, a text CRDT for notes,
columnar op encoding for long histories, migrating `session.json`.
