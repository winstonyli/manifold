# Session Store Format Revision — Implementation Plan (Plan A′)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Revise the unshipped `store/` format before Plan B builds on it. The changes: self-contained frames, personal state in a sidecar file, no compaction at an idle exit, pruning at compaction, and recovery from mid-file damage instead of truncating.

**Architecture:** All changes stay inside `crates/manifold/src/store/`.
- `journal.rs` changes in place: frames carry their own actor tables, the header gains a CRC and a link to `.prev`, and a resyncing scan plus a repair step replace "stop at the first bad frame".
- A new `local.rs` owns the `session.mnf.local` sidecar.
- `ops.rs` gains `Doc::prune`.

Nothing outside `store/` uses the module yet (`#![allow(dead_code)]`).

**Tech Stack:** Rust (nightly 1.100, edition 2021), std only. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-09-25-session-op-log-design.md`. Task 1 writes this plan's decisions into it first. The user skipped a separate spec edit, so the plan carries it.

**Prior plan:** `docs/superpowers/plans/2026-09-25-session-store-core.md` (Plan A). Its "Outcome" section describes the code as it stands. Task 9 updates that section's contracts for Plan B.

## Global Constraints

- Work on the branch `store-format-revision` (from `dev`; it merges back into `dev`). Never push. Commit at the end of each task with the message given, ending with the trailer line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Run `cargo fmt --all` before each commit (style: `rustfmt.toml`, width 120). Keep the surrounding comment style: sparse, full sentences. `OpBody` is `#[rustfmt::skip]`; keep its variants on one line each.
- Build and test with `-j 12`.
- Every task ends with all of these green:
  - `cargo test -q -p manifold -j 12 store::` (the task gives the expected count);
  - `cargo clippy -q -j 12 -p manifold -p ipc-protocol --all-targets -- -D warnings` passes (the pre-commit hook runs it and `cargo fmt --all --check`; never bypass it with `--no-verify`);
  - `cargo fmt --all --check`.
- Tests touch files only under `store::testutil::temp_dir`. Nothing touches `%APPDATA%`, and no task runs the app.
- `FORMAT_VERSION` stays `1` although the header and frames change. The format is unshipped, and nothing outside tests has written a `.mnf` file. (Assumption: verify with `git grep -n "Journal::open" -- crates/manifold/src ':!crates/manifold/src/store'`, which prints nothing.)
- **This plan's code was not compiled before it was written**, unlike Plan A's. The blocks are complete and were checked by reading against the source at `fd6b036`. The workspace was reformatted afterwards (`db79f32`), so code this plan quotes as "replace this" may now be wrapped differently: match it by content, not by line breaks. Compile errors are transcription or plan defects: fix them minimally and note them in the report. Behaviour that differs from the tests is a real finding. Report it; don't weaken a test.
- Frame types 1 and 4 are retired and never reused. Field numbers are never reused either.

## Why these changes (one line each; Task 1 puts the full text in the spec)

- **F, self-contained frames.** Today a lost `Actors` frame makes every later frame that uses its indexes undecodable. With a per-frame table, damage costs only the damaged frame. That is what makes resync (C) worth doing.
- **Sidecar.** Personal state churns: camera, settled positions, titles. In the journal it grows the file and drives compactions. Kept apart, it is written by its own throttle (30 s; 2.5 s for Recently Closed).
- **A′.** Compacting on every exit rewrote the whole file even after an idle session. That is the Firefox SSD-wear pattern (bug 1305193).
- **P2.** Tombstones and dead actors otherwise grow forever. They are safe to drop while the document has never been shared.
- **C + E + B.** One bad mid-file frame currently truncates every valid frame after it at the next append (Plan A's "plan-level risk"). The fix: resync past damage (C), keep `.prev` from the last compaction (E), and save the damaged original before any repair (B).

## Edge cases and how each is handled

| # | Case | Handling | Where |
|---|---|---|---|
| 1 | Power loss persists a later append but not an earlier one: a zero gap, then valid frames | Resync past the gap. The later frames count; ops on nodes whose `Create` was lost make placeholders, which are hidden and later pruned | T7 test (zeroed frame) |
| 2 | Power loss leaves a whole-length frame at the tail with a partial body | `BadCrc` counts as damage: the original is saved, then the frame is cut. A harmless extra copy after a crash | T7 test (last frame) |
| 3 | Bit flip in a frame body, length intact | Resync tries the stopped frame's own end first, so only that frame is lost | T7 test |
| 4 | Bit flip in a length field | Byte-by-byte search for the next valid frame | T8 fuzz |
| 5 | Several damaged regions | `scan_all` loops, recording each region | T8 fuzz |
| 6 | Snapshot damaged | Base comes from `.prev` when its generation is the header's `prev_generation`. Otherwise an empty document with a new id. Surviving ops apply on top | T7 test |
| 7 | Header damaged (CRC fails, magic intact) | Read as is, flagged, and rewritten by the repair. `.prev` is used without the generation check. The copy is named by the whole file's CRC | T7 test |
| 8 | Magic damaged | `NotAJournal`: left alone, since it can't be told from a foreign file | existing test |
| 9 | CRC-valid frame that doesn't decode | Skipped and counted as damage (was: refuse to open). The original is saved | T7 test |
| 10 | A snapshot after another frame (a misdirected or duplicated write) | Skipped as damage: writers only put a snapshot first | T7 (`take_all`), T8 fuzz |
| 11 | Duplicated ops frame | Applying ops is idempotent | T8 fuzz |
| 12 | User data (a URL) crafted to be a CRC-valid frame, sitting right after damage | Accepted risk. It needs damage at exactly that spot, and trying the claimed end first avoids the common case | — |
| 13 | Resync cost | Linear in practice: most offsets fail on a zero or oversized length. Quadratic only for adversarial files, which aren't attacker-written | — |
| 14 | Another replica's frames are damaged, found by `catch_up` under the lock | Full reload plus repair, never a cut. That replica loses the in-memory ops that sat in the damaged region; its next `begin` reports them in `remote` as missing | T7 test |
| 15 | A `BadCrc` tail in a file this build can't compact (`keep_unknown`/read-only) | Each `begin` reloads it until the next append cuts it. Rare, bounded | — |
| 16 | The damaged copy can't be saved (disk full) | No rewrite. An append that would cut unsaved damaged bytes (a torn tail, or a `BadCrc` tail on the file's own truncating append) is refused until a retried save of the damaged original succeeds | code comment |
| 17 | The repair rewrite fails | `compaction_failed` and `stale` are set; the next `begin` reloads and retries | code |
| 18 | No hard links (FAT/exFAT, some network drives) | Fall back to copying. If the copy fails too, there is no `.prev`; it is a best-effort safety net | code |
| 19 | Crash between creating `.prev` and the rename | `.prev` then has the current generation, not the linked one, so it is ignored | by construction |
| 20 | Save As over an existing file | No `.prev` and `prev_generation` 0, so another document never becomes a base | T6 test |
| 21 | A stale `.prev` next to a re-created file of the same name | The generation check rejects it; it is unchecked only when the header is damaged (accepted) | — |
| 22 | Two instances writing personal state | Under the journal's lock, one temp name, last rename wins | T3 |
| 23 | A sidecar from another document (replaced or recovered journal, copied file) | Ignored: it carries the document id | T3 test |
| 24 | A sidecar written by a newer format version | Not read; the next write replaces it. Personal state is recomputable, and newer builds read older versions | T3 test |
| 25 | A damaged sidecar | `Corrupt`; Plan B starts fresh and the next write replaces it | T3 test, T8 fuzz |
| 26 | A read-only journal | Writes no sidecar | T3 |
| 27 | Physics jitter keeps positions "changed" | Plan B compares settled positions with a 1 px tolerance | Task 9 contract |
| 28 | Idle launch | The file stays byte-identical. Another replica's frames, a torn tail or damage mark it dirty, and close compacts | T4 test |
| 29 | A pruned node named by a later op | A placeholder, hidden. `remote` reports pruned ids; missing means deleted | T5 test |
| 30 | A deleted group with a live child | Kept, since `effective_group` needs the chain. A cycle among tombstones (only a damaged snapshot has one) is never pruned | T5 test |
| 31 | `parent` naming a pruned tab | Reads as deleted, as before (the "opened from" link only matters when live) | spec rule |
| 32 | A newer build's unknown kinds or id-valued fields | Unknown kinds are kept unless deleted. Unknown id fields may dangle: missing means deleted | spec rule |
| 33 | A shared document | Sub-project 2 adds a `shared` flag and stops pruning | spec note |
| 34 | Duplicate actors in a frame's table (crafted) | Decodes but doesn't re-encode the same, so it blocks compaction | code |
| 35 | Empty actor table | No author: the frame doesn't decode and counts as damage | T2 test |
| 36 | Old dev files holding frame types 1 or 4 | Unknown type, never compacted. None exist | — |
| 37 | Header layout in later versions | Fixed through the CRC; newer versions add only to frames | spec |
| 38 | Version byte flipped (the CRC fails too) | Version taken as read, so it may open read-only wrongly; needs two faults | — |

---

### Task 1: Write this plan's decisions into the spec

**Files:**
- Modify: `docs/superpowers/specs/2026-09-25-session-op-log-design.md`

**Interfaces:** none (docs only).

- [ ] **Step 1: Add to the end of the Decisions list** (after the "Browsing history" bullet, line 43):

```markdown
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
```

- [ ] **Step 2: Existence of deleted nodes.** In "The shared document", replace

```markdown
- **Existence**: created by `Create`, removed by `Delete`. A deleted id
  stays a tombstone forever; ids are never reused.
```

with

```markdown
- **Existence**: created by `Create`, removed by `Delete`. Ids are never
  reused. A deleted node stays a tombstone until compaction may prune it
  (see Journal); a node id that names no node reads as deleted.
```

- [ ] **Step 3: Personal state.** Replace the first line of "Personal state":

```markdown
Kept per profile, never merged, stored in `Local` frames:
```

with

```markdown
Kept per profile, never merged, in `session.mnf.local` beside the
journal (see "Personal-state file"):
```

and replace the paragraph after the list:

```markdown
Two instances on one profile each keep their own personal state while
running; `Local` frames are never merged live. On open, the latest
`Local` frame wins, so one instance's Recently Closed list is replaced
by the other's; accepted.
```

with

```markdown
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
```

- [ ] **Step 4: Actor indexes.** In the encodings list, replace

```markdown
  snapshot node keys and group edges, stamps): nested `{1: actor index,
  2: counter}`, the index into the file's `Actors` table, renumbered at
  compaction.
```

with

```markdown
  snapshot node keys and group edges, stamps): nested `{1: actor index,
  2: counter}`, the index into the frame's own actor table.
```

- [ ] **Step 5: File and frames.** Replace the whole section, from `### File and frames` up to (not including) `CRC-32C is hand-rolled`, with:

````markdown
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
- A damaged file reads as no state.
- A newer format version is not read, and the next write replaces it.
  Personal state is recomputable, and newer builds read older versions.

````

- [ ] **Step 6: Journal.** In "## Journal", replace the **Open**, **Append** and **Compact** bullets with:

```markdown
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
  placeholder, which isn't shown.
```

And in the **Named sessions** bullet, replace the words `writes a compacted copy;` (after "Save As", across a line break) with `writes a compacted copy (no \`.prev\`) and the copy's personal-state file;`.

- [ ] **Step 7: Notes for sub-project 2.** Replace

```markdown
- Tombstones and the version vector grow with every delete and every
  launch; pruning them needs causal stability (every peer has seen
  them), which only sync can establish.
```

with

```markdown
- Compaction prunes tombstones and dead actors only while a document
  has never been shared. Sub-project 1 never shares, so it always
  prunes. Sub-project 2 must record when a document is first shared (a
  `shared` field in the snapshot) and stop pruning from then on. Pruning
  a shared document needs causal stability (every peer has seen the
  tombstones), which only sync can establish.
```

- [ ] **Step 8: Testing.** Add these bullets to the end of the "Unit" list in "## Testing":

```markdown
- Recovery: each ops frame, bit-flipped or zeroed, costs only its own
  ops, and the original file is saved first. A damaged snapshot is
  rebuilt from `.prev`. Damage that another replica's catch-up finds is
  repaired, not cut. Seeded random file damage (bit flips, zeroed and
  random runs, copied runs, truncation) never panics, and after a
  compacting close the file is clean.
- Pruning never changes what is shown.
- An idle open and close leaves the file byte-identical.
```

- [ ] **Step 9: Check and commit**

Run: `git diff --stat` (one file). Read the diff once for broken markdown.

```bash
git add docs/superpowers/specs/2026-09-25-session-op-log-design.md
git commit -m "Spec: format revision (self-contained frames, sidecar, recovery, pruning, idle exit)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Self-contained frames (per-frame actor tables)

**Files:**
- Modify: `crates/manifold/src/store/journal.rs`

**Interfaces:**
- Produces:
  - `fn write_table(w: &mut Writer, no: u32, actors: &Actors)` and `fn read_table(buf: &[u8], no: u32) -> Result<Actors, JournalError>`;
  - `fn snapshot_message(doc: &Doc) -> Vec<u8>`, `fn decode_snapshot(buf: &[u8]) -> Result<Doc, JournalError>`;
  - `fn ops_message(ops: &[Op]) -> Vec<u8>`, `fn decode_ops(buf: &[u8]) -> Result<Vec<Op>, JournalError>`;
  - `pub fn snapshot_bytes(doc: &Doc) -> Vec<u8>`, which now returns one `SNAPSHOT` frame;
  - `frame::RETIRED: [u64; 1] = [1]`;
  - `Journal::take` never mutates state when a frame fails to decode.
- Removes: `frame::ACTORS`, `actors_message`, `decode_actors`, the `Journal.actors` field.

- [ ] **Step 1: Write the failing test.** Add to `mod tests`:

```rust
    #[test]
    fn every_frame_reads_on_its_own() {
        // Each frame carries its own actor table, author first, so any
        // frame decodes without the frames before it.
        let dir = temp_dir("self-contained");
        let path = dir.join("session.mnf");
        let mut a = Journal::open(&path, ActorId(1)).unwrap();
        let mut b = Journal::open(&path, ActorId(2)).unwrap();
        let mut txn = a.begin().unwrap();
        let group = txn.next_id();
        txn.commit(vec![OpBody::Create { node: group, kind: Kind::Group, fields: Vec::new() }], None).unwrap();
        let mut txn = b.begin().unwrap();
        let (tab, create) = create_tab(&mut txn, "https://f.example/");
        txn.commit(vec![create, OpBody::Move { node: tab, group: Some(group) }], None).unwrap();

        let bytes = std::fs::read(&path).unwrap();
        let header = read_header(&bytes).unwrap();
        let frames = scan(&bytes[header.len..], header.len).frames;
        assert_eq!(frames.iter().map(|f| f.ty).collect::<Vec<_>>(), [frame::SNAPSHOT, frame::OPS, frame::OPS]);
        assert!(decode_snapshot(frames[0].message).is_ok());
        let theirs = frames[2].message;
        assert_eq!(read_table(theirs, 1).unwrap().list, [ActorId(2), ActorId(1)]);
        let ops = decode_ops(theirs).unwrap();
        assert!(ops.iter().all(|op| op.id.actor == ActorId(2)));
        assert!(ops.iter().any(|op| op.body == OpBody::Move { node: tab, group: Some(group) }));

        // A frame with no table has no author.
        assert!(decode_ops(&[]).is_err());
        assert!(![frame::SNAPSHOT, frame::OPS].iter().any(|ty| frame::RETIRED.contains(ty)));
    }
```

- [ ] **Step 2: Run it to see it fail**

Run: `cargo test -q -p manifold -j 12 store::journal::tests::every_frame_reads_on_its_own`
Expected: compile errors (`read_table` and `frame::RETIRED` don't exist; `decode_ops` takes two arguments).

- [ ] **Step 3: Frame types.** Replace the `frame` module:

```rust
/// Frame types. 0 is invalid, so a zeroed region never parses.
pub mod frame {
    pub const SNAPSHOT: u64 = 2;
    pub const OPS: u64 = 3;
    /// Never reused: 1 was a file-wide actor table (each frame now
    /// carries its own).
    pub const RETIRED: [u64; 1] = [1];
}
```

- [ ] **Step 4: Tables.** Change the doc comment on `pub struct Actors` to `/// A frame's actor table: the frame names actors by their index here.`. Delete `actors_message` and `decode_actors`. In their place, add:

```rust
/// Writes `actors` as repeated fixed64 field `no`, in table order.
fn write_table(w: &mut Writer, no: u32, actors: &Actors) {
    for actor in &actors.list {
        w.fixed64(no, actor.0);
    }
}

/// The table in repeated field `no`, in file order. A repeated actor keeps
/// its first index; the frame then doesn't re-encode to itself, which
/// blocks compaction.
fn read_table(buf: &[u8], no: u32) -> Result<Actors, JournalError> {
    let mut actors = Actors::default();
    for field in Reader::new(buf) {
        let field = field?;
        if field.no == no {
            let actor = ActorId(field.value.fixed64()?);
            let at = actors.list.len() as u64;
            actors.index.entry(actor).or_insert(at);
            actors.list.push(actor);
        }
    }
    Ok(actors)
}
```

- [ ] **Step 5: Snapshot.** Change the doc comment and signature of `snapshot_message`, and write the table last (fields go in ascending order):

```rust
/// Snapshot: `{1: doc id, 2: node*, 3: version*, 4: reserved for a
/// retained Move-log tail, 5: clock, 6: actor*}`. A node is `{1: id, 2:
/// kind, 3: deleted, 4: group, 5: register*}` and a register `{1: field,
/// 2: stamp, 3: value encoding}`. The actor table is sorted, and nodes and
/// registers are in id and field order, so equal documents encode to
/// equal bytes.
fn snapshot_message(doc: &Doc) -> Vec<u8> {
    let actors = doc_actors(doc);
    let actors = &actors;
```

Leave the body unchanged; it uses `actors` as a `&Actors`. Before `w.into_bytes()`, add:

```rust
    write_table(&mut w, 6, actors);
```

Change `decode_snapshot` to read the table first:

```rust
fn decode_snapshot(buf: &[u8]) -> Result<Doc, JournalError> {
    let actors = read_table(buf, 6)?;
    let actors = &actors;
```

Leave the rest unchanged; field 6 falls into the existing `_ => {}` arm. Replace `snapshot_bytes`:

```rust
/// The snapshot frame for `doc`, as a compacted file holds it. Equal
/// documents give equal bytes.
pub fn snapshot_bytes(doc: &Doc) -> Vec<u8> {
    let mut out = Vec::new();
    encode_frame(&mut out, frame::SNAPSHOT, &snapshot_message(doc));
    out
}
```

- [ ] **Step 6: Ops.** Replace the doc comment and head of `ops_message`, up to and including the `if index != 0 { … }` block:

```rust
/// Ops: `{1: actor*, 2: op*}`, the frame's table with the author first
/// (index 0) and then every other actor its ops name, in first-use order.
/// An op is `{1: counter delta, 2: ms delta (both zigzag, from the
/// previous op in the frame), 3: logical, then one body field: 4 Create
/// {1: node, 2: kind, 3: fields}, 5 Set {1: node, 2: field, 3: value
/// encoding}, 6 Move {1: node, 2: group}, 7 Delete {1: node}}`.
fn ops_message(ops: &[Op]) -> Vec<u8> {
    let author = ops[0].id.actor;
    let mut table = Actors::default();
    for actor in ops.iter().flat_map(op_actors) {
        table.push(actor); // `op_actors` starts with the op's author
    }
    let actors = &table;
    let mut w = Writer::new();
    write_table(&mut w, 1, actors);
```

The rest of the body is unchanged.

In `decode_ops`, change the signature and replace the first loop (the one reading `author`) plus `let author = actors.get(author)?;` with:

```rust
fn decode_ops(buf: &[u8]) -> Result<Vec<Op>, JournalError> {
    let actors = read_table(buf, 1)?;
    let actors = &actors;
    let author = actors.get(0)?;
```

The rest is unchanged.

- [ ] **Step 7: The journal.** Make these changes:
  - Delete the `actors: Actors,` field from `struct Journal`.
  - Delete `actors: Actors::default(),` in `blank`.
  - Delete `self.actors = Actors::default();` in `load`.
  - Replace `take` with:

```rust
    /// Applies one frame read from the file. A frame that doesn't decode
    /// changes nothing.
    fn take(&mut self, frame: &RawFrame) -> Result<Vec<NodeId>, JournalError> {
        let touched = match frame.ty {
            frame::SNAPSHOT => {
                let doc = decode_snapshot(frame.message)?;
                self.keep_unknown |= snapshot_message(&doc) != frame.message;
                self.clock.observe(doc.clock);
                let before = std::mem::replace(&mut self.doc, doc);
                (self.snapshot_len, self.since_snapshot) = (frame.size as u64, 0);
                return Ok(before.nodes.into_keys().chain(self.doc.nodes.keys().copied()).collect());
            }
            frame::OPS => {
                // Encodings are canonical, so a frame that doesn't re-encode
                // to itself held a field or op kind this build skipped.
                let ops = decode_ops(frame.message)?;
                self.keep_unknown |= ops.is_empty() || ops_message(&ops) != frame.message;
                let mut touched = Vec::new();
                for op in &ops {
                    self.clock.observe(op.hlc);
                    touched.extend(self.doc.apply(op));
                }
                touched
            }
            frame::LOCAL => {
                self.local = Some(frame.message.to_vec());
                Vec::new()
            }
            _ => {
                self.keep_unknown = true; // a newer build's frame: kept, not read
                Vec::new()
            }
        };
        self.since_snapshot += frame.size as u64;
        Ok(touched)
    }
```

`frame::LOCAL` still exists: its constant stays until Task 3. Keep `pub const LOCAL: u64 = 4;` in the `frame` module for now, above `RETIRED`.

In `write_ops`, delete the whole `new_actors` block, from `let mut new_actors` through the closing `}` of `if !new_actors.is_empty() { … }`. Then change the ops line to:

```rust
        if !ops.is_empty() {
            encode_frame(&mut out, frame::OPS, &ops_message(&ops));
        }
```

- [ ] **Step 8: Update the tests that used the file-wide table**
  - In `a_newer_builds_structural_fields_block_compaction`:
    - Delete the three lines that build `actors` (`let mut actors = Actors::default();` and `actors.push(ActorId(1));`).
    - Change `ops_message(&[op], &actors)` to `ops_message(&[op])`.
    - In the snapshot part, delete `let actors = doc_actors(&doc);` and the `encode_frame(&mut bytes, frame::ACTORS, …)` line.
    - Change `snapshot_message(&doc, &actors)` to `snapshot_message(&doc)`.
  - In `inspect_reports_frames_and_the_torn_tail`, change `[frame::ACTORS, frame::SNAPSHOT]` to `[frame::SNAPSHOT]`.
  - In `a_failed_commit_rereads_the_file_and_keeps_it_readable`, change the comment `// the actor table grew in memory only` to `// the file is read-only`.

- [ ] **Step 9: Run the tests**

Run: `cargo test -q -p manifold -j 12 store::`
Expected: 48 passed (47 + 1).

Run clippy as in Global Constraints. Expected: it passes.

- [ ] **Step 10: Commit**

```bash
git add crates/manifold/src/store/journal.rs
git commit -m "store: each frame carries its own actor table, author first

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Personal state moves to a sidecar file

**Files:**
- Create: `crates/manifold/src/store/local.rs`
- Modify: `crates/manifold/src/store/mod.rs`, `crates/manifold/src/store/journal.rs`

**Interfaces:**
- Consumes: `encode_frame`, `scan` (already `pub`), `JournalError`.
- Produces:
  - `local::{MAGIC, FORMAT_VERSION, path_for(session: &Path) -> PathBuf, read(session: &Path, doc: DocId) -> Result<Option<Vec<u8>>, JournalError>, write(session: &Path, doc: DocId, state: &[u8]) -> Result<(), JournalError>}`;
  - `journal::{with_suffix(path: &Path, suffix: &str) -> PathBuf, write_synced(path: &Path, bytes: &[u8]) -> io::Result<()>}`, both `pub(crate)`;
  - `Journal::read_local(&self) -> Result<Option<Vec<u8>>, JournalError>`;
  - `Journal::write_local(&self, state: &[u8]) -> Result<(), JournalError>`.
- Changes:
  - `Txn::commit(self, bodies: Vec<OpBody>) -> Result<(), JournalError>` (the `local` parameter goes);
  - `Journal::read(bytes) -> Result<Doc, JournalError>`;
  - `Journal::save_copy(&self, path: &Path, local: Option<&[u8]>)`;
  - private `write_compacted(path, doc)`;
  - `frame::RETIRED: [u64; 2] = [1, 4]`.
- Removes: `frame::LOCAL`, the `Journal.local` field, `Journal::local()`.

- [ ] **Step 1: Write the failing tests.** Add to `journal.rs`'s `mod tests`:

```rust
    #[test]
    fn personal_state_lives_beside_the_journal() {
        let dir = temp_dir("sidecar");
        let path = dir.join("session.mnf");
        let journal = Journal::open(&path, ActorId(1)).unwrap();
        assert_eq!(journal.read_local().unwrap(), None);
        journal.write_local(b"view").unwrap();
        journal.write_local(b"view two").unwrap();
        let reopened = Journal::open(&path, ActorId(2)).unwrap();
        assert_eq!(reopened.read_local().unwrap().as_deref(), Some(&b"view two"[..]));
        let bytes = std::fs::read(&path).unwrap();
        assert!(inspect(&bytes).unwrap().frames.iter().all(|f| f.1 == frame::SNAPSHOT || f.1 == frame::OPS));
        assert!(!with_suffix(&local::path_for(&path), ".tmp").exists());
    }

    #[test]
    fn another_documents_personal_state_is_ignored() {
        let dir = temp_dir("sidecar-doc");
        let path = dir.join("session.mnf");
        let copy = dir.join("copy.mnf");
        let journal = Journal::open(&path, ActorId(1)).unwrap();
        journal.write_local(b"mine").unwrap();
        journal.save_copy(&copy, Some(b"copied")).unwrap();
        let copied = Journal::open(&copy, ActorId(2)).unwrap();
        assert_eq!(copied.read_local().unwrap().as_deref(), Some(&b"copied"[..]));
        // A sidecar moved beside another document isn't read.
        std::fs::copy(local::path_for(&path), local::path_for(&copy)).unwrap();
        assert_eq!(copied.read_local().unwrap(), None);
        assert_eq!(journal.read_local().unwrap().as_deref(), Some(&b"mine"[..]));
    }

    #[test]
    fn a_damaged_or_newer_sidecar_is_not_read_and_is_replaced() {
        let dir = temp_dir("sidecar-bad");
        let path = dir.join("session.mnf");
        let side = local::path_for(&path);
        let journal = Journal::open(&path, ActorId(1)).unwrap();
        journal.write_local(b"state").unwrap();
        let clean = std::fs::read(&side).unwrap();
        let mut damaged = clean.clone();
        let last = damaged.len() - 1;
        damaged[last] ^= 1;
        std::fs::write(&side, &damaged).unwrap();
        assert!(matches!(journal.read_local(), Err(JournalError::Corrupt(_))));
        let mut newer = clean.clone();
        newer[4] = 2; // format_version 2
        std::fs::write(&side, &newer).unwrap();
        assert_eq!(journal.read_local().unwrap(), None);
        journal.write_local(b"again").unwrap();
        assert_eq!(journal.read_local().unwrap().as_deref(), Some(&b"again"[..]));
    }
```

- [ ] **Step 2: Run to see them fail**

Run: `cargo test -q -p manifold -j 12 store::journal::tests::personal_state`
Expected: compile errors (`read_local`, `local`, `with_suffix` missing).

- [ ] **Step 3: Create `local.rs`**

```rust
//! Personal state (spec "Personal state", "Personal-state file"):
//! `<session>.local`, beside the session file, replaced whole under the
//! session's lock. It holds the session's document id, so a file left
//! from another document is ignored.

use std::io;
use std::path::{Path, PathBuf};

use super::journal::{encode_frame, scan, with_suffix, write_synced, JournalError};
use super::model::DocId;
use super::tlv::{Reader, Writer};
use super::varint;

pub const MAGIC: [u8; 4] = *b"MNL\0";
pub const FORMAT_VERSION: u64 = 1;

/// The one frame type: `{1: document id, 2: state}`.
const STATE: u64 = 1;

pub fn path_for(session: &Path) -> PathBuf {
    with_suffix(session, ".local")
}

/// The state saved for document `doc`: `None` when there is none, when it
/// is another document's, or when a newer format version wrote it.
pub fn read(session: &Path, doc: DocId) -> Result<Option<Vec<u8>>, JournalError> {
    let bytes = match std::fs::read(path_for(session)) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    let damaged = || JournalError::Corrupt("personal state".into());
    if bytes.get(..4) != Some(&MAGIC[..]) {
        return Err(damaged());
    }
    let (version, n) = varint::read(&bytes[4..]).map_err(|_| damaged())?;
    if version > FORMAT_VERSION {
        return Ok(None);
    }
    let at = 4 + n;
    let scan = scan(&bytes[at..], at);
    if scan.stop.is_some() {
        return Err(damaged());
    }
    let mut state = None;
    for frame in scan.frames.iter().filter(|f| f.ty == STATE) {
        let (mut id, mut value) = (Vec::new(), Vec::new());
        for field in Reader::new(frame.message) {
            let field = field?;
            match field.no {
                1 => id = field.value.bytes()?.to_vec(),
                2 => value = field.value.bytes()?.to_vec(),
                _ => {}
            }
        }
        state = Some((id, value));
    }
    Ok(state.filter(|(id, _)| id[..] == doc.0[..]).map(|(_, value)| value))
}

/// Replaces the saved state. A newer format version's file is replaced
/// too: personal state is recomputable, and newer builds read older
/// versions. Call under the session's lock (`Journal::write_local` does).
pub fn write(session: &Path, doc: DocId, state: &[u8]) -> Result<(), JournalError> {
    let mut bytes = MAGIC.to_vec();
    varint::write(&mut bytes, FORMAT_VERSION);
    let mut m = Writer::new();
    m.bytes(1, &doc.0);
    if !state.is_empty() {
        m.bytes(2, state);
    }
    encode_frame(&mut bytes, STATE, &m.into_bytes());
    replace(&path_for(session), &bytes).map_err(JournalError::from)
}

/// Writes `bytes` to a temp file beside `path`, fsyncs it, and renames it
/// over `path`. A failure leaves `path` as it was.
fn replace(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = with_suffix(path, ".tmp");
    let written = write_synced(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}
```

In `mod.rs`, add `pub mod local;` between `pub mod json;` and `pub mod model;`.

- [ ] **Step 4: Shared file helpers in `journal.rs`.** Add below `lock_file`:

```rust
/// `path` with `suffix` appended to its file name (`session.mnf` →
/// `session.mnf.lock`).
pub(crate) fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Creates `path` holding `bytes`, and fsyncs it.
pub(crate) fn write_synced(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}
```

In `lock_file`, replace the two `lock_path` lines and the `open(lock_path)` call with `…open(with_suffix(path, ".lock"))?;`.

Replace `write_compacted`:

```rust
/// Writes a compacted file (header, snapshot) beside `path`, fsyncs it,
/// and renames it over `path`. Returns its bytes.
fn write_compacted(path: &Path, doc: &Doc) -> Result<Vec<u8>, JournalError> {
    let mut bytes = header_bytes(getrandom::u64().expect("the OS random source failed"));
    bytes.extend(snapshot_bytes(doc));
    let tmp = with_suffix(path, ".tmp");
    write_synced(&tmp, &bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(bytes)
}
```

- [ ] **Step 5: Retire `Local` frames.** In `journal.rs`:
  - In the `frame` module, delete `LOCAL`. Replace `RETIRED` and its comment with:

```rust
    /// Never reused: 1 was a file-wide actor table (each frame now
    /// carries its own), 4 was personal state (now `<session>.local`).
    pub const RETIRED: [u64; 2] = [1, 4];
```

  - Add `use super::local;` below `use super::crc32c::crc32c;`.
  - Delete the `local: Option<Vec<u8>>,` field, `local: None,` in `blank`, and `self.local = None;` in `load`.
  - Delete the `frame::LOCAL => { … }` arm in `take`.
  - Delete `pub fn local(&self)` and its doc comment.
  - Replace `Journal::read`:

```rust
    /// Reads a session file's document, as `open` would, without a lock or
    /// an actor. For `--dump` and tests.
    pub fn read(bytes: &[u8]) -> Result<Doc, JournalError> {
        let mut journal = Journal::blank(Path::new(""), ActorId(0));
        journal.load(bytes)?;
        Ok(journal.doc)
    }
```

  - In `open`, change `write_compacted(path, &Doc::new(DocId::random()), None)?;` to `write_compacted(path, &Doc::new(DocId::random()))?;`.
  - In `compact`, change the call to `write_compacted(&self.path, &self.doc)?;`.
  - Add, after `read_only`:

```rust
    /// This profile's personal state for this document (spec "Personal
    /// state"). `None` when there is none, or it is another document's.
    pub fn read_local(&self) -> Result<Option<Vec<u8>>, JournalError> {
        let _lock = self.lock()?;
        local::read(&self.path, self.doc.id)
    }

    /// Replaces this profile's personal state for this document. Refused
    /// read-only, and while this replica may not match its file.
    pub fn write_local(&self, state: &[u8]) -> Result<(), JournalError> {
        if self.read_only {
            return Err(JournalError::ReadOnly);
        }
        if self.stale {
            return Err(JournalError::Stale);
        }
        let _lock = self.lock()?;
        local::write(&self.path, self.doc.id, state)
    }
```

  - Replace `save_copy`:

```rust
    /// Writes this document, compacted, to `path` under a new document id
    /// (spec "Named sessions": Save As), with `local` as the copy's
    /// personal state, under `path`'s lock. Refused while this replica may
    /// not match its own file. Not for its own path.
    pub fn save_copy(&self, path: &Path, local: Option<&[u8]>) -> Result<(), JournalError> {
        if self.stale {
            return Err(JournalError::Stale);
        }
        let _lock = lock_file(path)?;
        let mut doc = self.doc.clone();
        doc.id = DocId::random();
        doc.forget_moves();
        write_compacted(path, &doc)?;
        if let Some(state) = local {
            local::write(path, doc.id, state)?;
        }
        Ok(())
    }
```

  - Replace `Txn::commit` and the head of `write_ops`:

```rust
    /// Stamps `bodies` as this replica's next ops, applies them, and
    /// appends them to the file. After an error the ops may or may not be
    /// in the file; the next `begin` re-reads it, so diff again against
    /// `doc()` rather than retrying the same bodies.
    pub fn commit(self, bodies: Vec<OpBody>) -> Result<(), JournalError> {
        let Txn { journal, _lock, .. } = self;
        journal.guard(|j| j.write_ops(bodies))
    }
```

```rust
    fn write_ops(&mut self, bodies: Vec<OpBody>) -> Result<(), JournalError> {
        let j = self;
        if j.read_only {
            return Err(JournalError::ReadOnly);
        }
        if bodies.is_empty() {
            return Ok(());
        }
```

In the rest of `write_ops`:
  - Replace `if !ops.is_empty() { encode_frame(…OPS…) }` with a plain `encode_frame(&mut out, frame::OPS, &ops_message(&ops));`. `ops` is non-empty here.
  - Delete the `if let Some(local) = &local { … }` block and the `if local.is_some() { j.local = local; }` block.

- [ ] **Step 6: Mechanical test updates** in `journal.rs` `mod tests`:
  - Every `.commit(…, None)` becomes `.commit(…)`. Remove the `, None` before the closing parenthesis; one sits in `every_frame_reads_on_its_own` from Task 2.
  - `txn.commit(vec![create], Some(vec![i]))` → `txn.commit(vec![create])` (in `a_cut_at_any_byte…`).
  - Rename `reopening_gives_back_the_document_and_local_state` to `reopening_gives_back_the_document`. In it, `txn.commit(vec![create], Some(b"view".to_vec()))` → `txn.commit(vec![create])`, and delete the line `assert_eq!(reopened.local(), Some(&b"view"[..]));`.
  - In `rich_file`, `txn.commit(bodies, Some(b"local one".to_vec()))` and `…Some(b"local two".to_vec()))` → `txn.commit(bodies)`.
  - Every `let (doc, _) = Journal::read(` → `let doc = Journal::read(`.
  - In `use_damaged`, `if let Ok((doc, _)) = Journal::read(bytes)` → `if let Ok(doc) = Journal::read(bytes)`, and `let (back, _) = Journal::read(&again)` → `let back = Journal::read(&again)`.
  - In `frames_round_trip_and_damage_stops_the_scan`, both `frame::LOCAL` → `frame::SNAPSHOT`.
  - In `a_failed_commit_rereads_the_file_and_keeps_it_readable`, `journal.save_copy(&dir.join("copy.mnf"))` → `journal.save_copy(&dir.join("copy.mnf"), None)`.
  - In `a_newer_format_version_opens_read_only`:
    - Change `txn.commit(vec![], Some(vec![1]))` to `txn.commit(vec![])`.
    - After that assertion, add `assert!(matches!(journal.write_local(b"x"), Err(JournalError::ReadOnly)));` and `assert!(!local::path_for(&path).exists());`.

Check nothing is left: `grep -n "None)\|Some(b\"local\|(doc, _)\|\.local()" crates/manifold/src/store/journal.rs` should show no `commit(…, None)`, no `(doc, _)` and no `.local()`. Other `None)` hits, such as `group: None)`, are fine.

- [ ] **Step 7: Run the tests**

Run: `cargo test -q -p manifold -j 12 store::`
Expected: 51 passed (48 + 3).

Run clippy as in Global Constraints. Expected: it passes.

- [ ] **Step 8: Commit**

```bash
git add crates/manifold/src/store/local.rs crates/manifold/src/store/mod.rs crates/manifold/src/store/journal.rs
git commit -m "store: personal state in a sidecar file; Local frames retired

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Don't compact at exit when nothing changed

**Files:**
- Modify: `crates/manifold/src/store/journal.rs`

**Interfaces:**
- Produces: the private field `Journal.dirty: bool`. Task 7 sets it from `load`, and Task 7's `load` replaces the line added here.

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn closing_without_changes_leaves_the_file_alone() {
        let dir = temp_dir("idle");
        let path = dir.join("session.mnf");
        let mut journal = Journal::open(&path, ActorId(1)).unwrap();
        let mut txn = journal.begin().unwrap();
        let (tab, create) = create_tab(&mut txn, "https://idle.example/");
        txn.commit(vec![create]).unwrap();
        journal.close().unwrap(); // changed: compacts
        let compacted = std::fs::read(&path).unwrap();
        for actor in 2..5 {
            let mut idle = Journal::open(&path, ActorId(actor)).unwrap();
            drop(idle.begin().unwrap());
            idle.close().unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), compacted, "launch {actor} rewrote an unchanged file");
        }
        let frames = |path: &Path| inspect(&std::fs::read(path).unwrap()).unwrap().frames.iter().map(|f| f.1).collect::<Vec<_>>();

        // A replica that appended and died without closing: the next close compacts.
        let mut crashed = Journal::open(&path, ActorId(5)).unwrap();
        let txn = crashed.begin().unwrap();
        txn.commit(vec![OpBody::Delete { node: tab }]).unwrap();
        drop(crashed);
        Journal::open(&path, ActorId(6)).unwrap().close().unwrap();
        assert_eq!(frames(&path), [frame::SNAPSHOT]);
        assert!(!Journal::read(&std::fs::read(&path).unwrap()).unwrap().nodes.get(&tab).is_some_and(Node::is_live));

        // Another live replica's frames count too.
        let mut idle = Journal::open(&path, ActorId(7)).unwrap();
        let mut other = Journal::open(&path, ActorId(8)).unwrap();
        let mut txn = other.begin().unwrap();
        let (_, create) = create_tab(&mut txn, "https://other.example/");
        txn.commit(vec![create]).unwrap();
        idle.close().unwrap();
        assert_eq!(frames(&path), [frame::SNAPSHOT]);

        // So does a torn tail.
        OpenOptions::new().append(true).open(&path).unwrap().write_all(&[7, 0, 0, 0, 1, 2]).unwrap();
        Journal::open(&path, ActorId(9)).unwrap().close().unwrap();
        assert_eq!(inspect(&std::fs::read(&path).unwrap()).unwrap().torn, None);
    }
```

- [ ] **Step 2: Run to see it fail**

Run: `cargo test -q -p manifold -j 12 store::journal::tests::closing_without_changes`
Expected: FAIL, "launch 2 rewrote an unchanged file" (every close compacts to a new generation).

- [ ] **Step 3: Implement**
  - Field, after `compaction_failed`:

```rust
    /// The file holds more than one clean snapshot: frames after it, a torn
    /// tail, or damage. `close` compacts only then.
    dirty: bool,
```

  - `blank`: `dirty: false,`.
  - `load`: after `let scan = …` and its loop, add:

```rust
        self.dirty = !(scan.stop.is_none() && scan.frames.len() == 1 && scan.frames[0].ty == frame::SNAPSHOT);
```

  - `catch_up`'s incremental branch: after `self.end = scan.end as u64;`, add `self.dirty |= !scan.frames.is_empty() || scan.stop.is_some();`.
  - `append`: after `self.since_snapshot += …`, add `self.dirty = true;`.
  - `close`: replace the closure's `if j.keep_unknown { … } else { j.compact() }` with:

```rust
            if j.keep_unknown {
                sync_data(&j.path)?;
                j.unsynced = false;
                Ok(())
            } else if j.dirty {
                j.compact()
            } else {
                Ok(())
            }
```

  - Update `close`'s doc comment to: `/// Clean exit: catch up, then compact if the file changed since it was last compacted, or just fsync when this build must not compact this file.`
  - Update the test import line: `use crate::store::model::{encode_field, field, FieldValue, Node, FIELDS};`. `Node::is_live` is used as a path.

- [ ] **Step 4: Run the tests**

Run: `cargo test -q -p manifold -j 12 store::`
Expected: 52 passed. Clippy: nothing.

- [ ] **Step 5: Commit**

```bash
git add crates/manifold/src/store/journal.rs
git commit -m "store: an idle close leaves the file as it is

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Prune at compaction

**Files:**
- Modify: `crates/manifold/src/store/ops.rs`, `crates/manifold/src/store/journal.rs`, `crates/manifold/src/store/mod.rs` (testutil)

**Interfaces:**
- Produces:
  - `pub fn Doc::prune(&mut self)`, called only after `forget_moves` (it asserts this in debug builds);
  - `store::testutil::shown(doc: &Doc) -> Vec<(NodeId, BTreeMap<u32, Reg>, Option<NodeId>)>`.

- [ ] **Step 1: `shown` in testutil.** In `mod.rs`'s `testutil`, add below `use std::path::PathBuf;`:

```rust
    use std::collections::BTreeMap;

    use super::model::{Doc, NodeId, Reg};
```

Then add at the end of the module:

```rust
    /// What a document shows: its live nodes, with their fields and the
    /// group each is shown in.
    pub fn shown(doc: &Doc) -> Vec<(NodeId, BTreeMap<u32, Reg>, Option<NodeId>)> {
        doc.live().map(|(id, node)| (id, node.fields.clone(), doc.effective_group(id))).collect()
    }
```

- [ ] **Step 2: Write the failing tests.** In `ops.rs` `mod tests`, change the testutil import to `use crate::store::testutil::{shown, XorShift};` and add:

```rust
    #[test]
    fn prune_drops_what_no_later_op_can_need() {
        let (outer, g, t, u) = (id(1, 0), id(1, 1), id(1, 2), id(1, 3));
        let (v, w) = (id(2, 0), id(4, 0));
        let mut doc = apply_all(&[
            op(1, 0, 1, create(outer, Kind::Group, None)),
            op(1, 1, 1, create(g, Kind::Group, Some(outer))),
            op(1, 2, 1, create(t, Kind::Tab, Some(g))),
            op(1, 3, 1, create(u, Kind::Tab, Some(g))),
            op(2, 0, 2, create(v, Kind::Tab, None)),
            op(3, 0, 3, name(t, "t")),
            op(4, 1, 3, name(w, "never created")),
            op(1, 4, 4, OpBody::Delete { node: u }),
            op(2, 1, 4, OpBody::Delete { node: v }),
            op(1, 5, 4, OpBody::Delete { node: g }),
        ]);
        doc.forget_moves();
        let before = shown(&doc);
        doc.prune();
        assert_eq!(shown(&doc), before);
        assert!(![u, v, w].iter().any(|n| doc.nodes.contains_key(n)));
        assert!(doc.nodes[&g].deleted); // kept: t is shown through it, at outer
        assert_eq!(doc.effective_group(t), Some(outer));
        assert_eq!(doc.version.keys().copied().collect::<Vec<_>>(), [ActorId(1), ActorId(3)]);

        // Once t goes, nothing names g.
        doc.apply(&op(1, 6, 5, OpBody::Delete { node: t }));
        doc.prune();
        assert!(!doc.nodes.contains_key(&t) && !doc.nodes.contains_key(&g));
        assert_eq!(doc.live().map(|(id, _)| id).collect::<Vec<_>>(), [outer]);
    }

    #[test]
    fn pruning_never_changes_what_is_shown() {
        let mut rng = XorShift(0x5EED_CAFE);
        for _ in 0..200 {
            let mut doc = apply_all(&random_ops(&mut rng));
            doc.forget_moves();
            let before = shown(&doc);
            doc.prune();
            assert_eq!(shown(&doc), before);
            assert!(doc.nodes.values().filter_map(|n| n.group).all(|g| doc.nodes.contains_key(&g)), "an edge lost its group");
            let once = (doc.nodes.clone(), doc.version.clone());
            doc.prune();
            assert_eq!((doc.nodes, doc.version), once);
        }
    }
```

In `journal.rs` `mod tests`, add:

```rust
    #[test]
    fn compaction_prunes_and_other_replicas_hear_of_it() {
        let dir = temp_dir("prune");
        let path = dir.join("session.mnf");
        let mut a = Journal::open(&path, ActorId(1)).unwrap();
        let mut b = Journal::open(&path, ActorId(2)).unwrap();
        let mut txn = a.begin().unwrap();
        let (gone, create_gone) = create_tab(&mut txn, "https://gone.example/");
        let (kept, create_kept) = create_tab(&mut txn, "https://kept.example/");
        txn.commit(vec![create_gone, create_kept]).unwrap();
        let txn = a.begin().unwrap();
        txn.commit(vec![OpBody::Delete { node: gone }]).unwrap();
        drop(b.begin().unwrap()); // b holds the tombstone
        a.close().unwrap();
        let doc = Journal::read(&std::fs::read(&path).unwrap()).unwrap();
        assert!(!doc.nodes.contains_key(&gone) && doc.nodes[&kept].is_live());
        let txn = b.begin().unwrap();
        assert!(txn.remote.contains(&gone)); // missing from the document: deleted
        assert!(!txn.doc().nodes.contains_key(&gone));
    }
```

- [ ] **Step 3: Run to see them fail**

Run: `cargo test -q -p manifold -j 12 store::`
Expected: compile error (`prune` missing).

- [ ] **Step 4: Implement `prune`.** In `ops.rs`, add `use std::collections::BTreeSet;` above the `use super::model…` line, and add to `impl Doc` after `forget_moves`:

```rust
    /// Drops what no later op can need while the document has never been
    /// shared (spec "Compact"). First, deleted nodes and placeholders that
    /// no group edge names; this repeats, since dropping a child can free
    /// its group. Then the version entries of actors nothing names any
    /// more. What is shown doesn't change; a later op naming a dropped
    /// node makes a placeholder, which isn't shown. Call after
    /// `forget_moves`: the move log names nodes too.
    pub fn prune(&mut self) {
        debug_assert!(self.moves.is_empty(), "prune after forget_moves");
        loop {
            let named: BTreeSet<NodeId> = self.nodes.values().filter_map(|n| n.group).collect();
            let before = self.nodes.len();
            self.nodes.retain(|id, n| named.contains(id) || !(n.deleted || matches!(n.kind, Kind::Unknown(0))));
            if self.nodes.len() == before {
                break;
            }
        }
        let mut used = BTreeSet::new();
        for (id, node) in &self.nodes {
            used.insert(id.actor);
            used.extend(node.group.map(|g| g.actor));
            used.extend(node.fields.values().map(|reg| reg.stamp.actor));
        }
        self.version.retain(|actor, _| used.contains(actor));
    }
```

- [ ] **Step 5: Call it.** In `journal.rs`:
  - In `compact`, after `self.doc.forget_moves();`, add `self.doc.prune();`.
  - In `save_copy`, after `doc.forget_moves();`, add `doc.prune();`.
  - Update `compact`'s doc comment to `/// Rewrites the file as one pruned snapshot (spec "Compact"). Holds the lock.`

- [ ] **Step 6: Run the tests**

Run: `cargo test -q -p manifold -j 12 store::`
Expected: 55 passed (52 + 3). Clippy: nothing.

- [ ] **Step 7: Commit**

```bash
git add crates/manifold/src/store/ops.rs crates/manifold/src/store/journal.rs crates/manifold/src/store/mod.rs
git commit -m "store: compaction prunes tombstones, placeholders and dead actors

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Header CRC and `.prev`

**Files:**
- Modify: `crates/manifold/src/store/journal.rs`

**Interfaces:**
- Produces:
  - `pub struct Header { version, generation, prev_generation: u64, damaged: bool, len }`;
  - `fn header_bytes(generation: u64, prev_generation: u64) -> Vec<u8>`;
  - `pub fn prev_path(path: &Path) -> PathBuf`;
  - private `write_compacted(path, doc, prev_generation: u64, rotate: bool)` and `keep_prev(path)`;
  - `Inspect` gains `prev_generation: u64` and `header_damaged: bool`.
- Header layout for version 1: bytes 0..4 magic, 4 version, 5..13 generation, 13..21 prev_generation, 21..25 CRC, so `len == 25`.

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn the_header_is_checked_and_names_the_file_kept_as_prev() {
        let dir = temp_dir("header");
        let path = dir.join("session.mnf");
        let mut journal = Journal::open(&path, ActorId(1)).unwrap();
        let first = read_header(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!((first.len, first.prev_generation, first.damaged), (25, 0, false));
        assert!(!prev_path(&path).exists());
        let mut txn = journal.begin().unwrap();
        let (_, create) = create_tab(&mut txn, "https://h.example/");
        txn.commit(vec![create]).unwrap();
        let before = std::fs::read(&path).unwrap();
        journal.close().unwrap();
        let after = std::fs::read(&path).unwrap();
        assert_eq!(read_header(&after).unwrap().prev_generation, first.generation);
        assert_eq!(std::fs::read(prev_path(&path)).unwrap(), before);

        // Save As keeps no `.prev` and links none.
        let copy = dir.join("copy.mnf");
        journal.save_copy(&copy, None).unwrap();
        assert!(!prev_path(&copy).exists());
        assert_eq!(read_header(&std::fs::read(&copy).unwrap()).unwrap().prev_generation, 0);

        // A flipped bit anywhere after the magic is caught.
        for at in 4..25 {
            let mut flipped = after.clone();
            flipped[at] ^= 0x10;
            assert!(!matches!(read_header(&flipped), Ok(h) if !h.damaged), "byte {at}");
        }
    }
```

- [ ] **Step 2: Run to see it fail**

Run: `cargo test -q -p manifold -j 12 store::journal::tests::the_header_is_checked`
Expected: compile errors (`prev_generation`, `damaged`, `prev_path`).

- [ ] **Step 3: Implement the header.** Replace `Header`, `read_header` and `header_bytes`:

```rust
pub struct Header {
    pub version: u64,
    /// Random per compacted file: a changed generation means another
    /// process replaced the file, so it must be read again from the start.
    pub generation: u64,
    /// The generation of the file this one was compacted from, which
    /// `<path>.prev` then holds; 0 for none.
    pub prev_generation: u64,
    /// The header's CRC failed: its fields may be wrong. The file is
    /// repaired (spec "File and frames").
    pub damaged: bool,
    pub len: usize,
}

/// Reads the header. Its layout through the CRC is the same in every
/// format version, so a newer build's header reads too.
pub fn read_header(buf: &[u8]) -> Result<Header, JournalError> {
    if buf.get(..4) != Some(&MAGIC[..]) {
        return Err(JournalError::NotAJournal);
    }
    let (version, n) = varint::read(&buf[4..]).map_err(|_| JournalError::NotAJournal)?;
    let at = 4 + n;
    let word = |i: usize| buf.get(i..i + 8).map(|b| u64::from_le_bytes(b.try_into().expect("8 bytes"))).ok_or(JournalError::NotAJournal);
    let (generation, prev_generation) = (word(at)?, word(at + 8)?);
    let crc = buf.get(at + 16..at + 20).ok_or(JournalError::NotAJournal)?;
    let damaged = crc32c(&buf[..at + 16]) != u32::from_le_bytes(crc.try_into().expect("4 bytes"));
    Ok(Header { version, generation, prev_generation, damaged, len: at + 20 })
}

fn header_bytes(generation: u64, prev_generation: u64) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    varint::write(&mut out, FORMAT_VERSION);
    out.extend_from_slice(&generation.to_le_bytes());
    out.extend_from_slice(&prev_generation.to_le_bytes());
    let crc = crc32c(&out);
    out.extend_from_slice(&crc.to_le_bytes());
    out
}
```

- [ ] **Step 4: `.prev`.** Replace `write_compacted` and add `keep_prev` and `prev_path` after it:

```rust
/// Writes a compacted file (header, snapshot) beside `path`, fsyncs it,
/// and renames it over `path`. With `rotate`, the file it replaces is kept
/// as `<path>.prev` first, and `prev_generation` is that file's
/// generation. Returns its bytes.
fn write_compacted(path: &Path, doc: &Doc, prev_generation: u64, rotate: bool) -> Result<Vec<u8>, JournalError> {
    let mut bytes = header_bytes(getrandom::u64().expect("the OS random source failed"), prev_generation);
    bytes.extend(snapshot_bytes(doc));
    let tmp = with_suffix(path, ".tmp");
    write_synced(&tmp, &bytes)?;
    if rotate {
        keep_prev(path);
    }
    std::fs::rename(&tmp, path)?;
    Ok(bytes)
}

/// Keeps `path` as `<path>.prev`: a hard link, so no bytes are copied,
/// or a copy where links fail (FAT, some network drives). Best effort: a
/// failure only means there is no `.prev` to recover from.
fn keep_prev(path: &Path) {
    let prev = prev_path(path);
    let _ = std::fs::remove_file(&prev);
    if std::fs::hard_link(path, &prev).is_err() {
        let _ = std::fs::copy(path, &prev);
    }
}

/// The file before the last compaction (spec "Compact").
pub fn prev_path(path: &Path) -> PathBuf {
    with_suffix(path, ".prev")
}
```

Callers:
- `open`: `write_compacted(path, &Doc::new(DocId::random()), 0, false)?;`
- `compact`: `write_compacted(&self.path, &self.doc, self.generation, true)?;`
- `save_copy`: `write_compacted(path, &doc, 0, false)?;`. A copy links to no `.prev`: one there could belong to another document.

- [ ] **Step 5: `inspect`.** Add to `Inspect`, after `generation`:

```rust
    pub prev_generation: u64,
    pub header_damaged: bool,
```

and in `inspect`, after `generation: header.generation,`, add `prev_generation: header.prev_generation, header_damaged: header.damaged,`.

- [ ] **Step 6: Update the tests that build headers**
  - `a_newer_builds_structural_fields_block_compaction`: `header_bytes(7)` → `header_bytes(7, 0)`.
  - `use_damaged`: `header_bytes(1)` → `header_bytes(1, 0)`.
  - `a_newer_format_version_opens_read_only`: after `bytes[4] = 2; // format_version 2`, add:

```rust
        let crc = crc32c(&bytes[..21]);
        bytes[21..25].copy_from_slice(&crc.to_le_bytes());
```

- [ ] **Step 7: Run the tests**

Run: `cargo test -q -p manifold -j 12 store::`
Expected: 56 passed. Clippy: nothing.

- [ ] **Step 8: Commit**

```bash
git add crates/manifold/src/store/journal.rs
git commit -m "store: header CRC; compaction keeps the old file as .prev

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Recover from damage instead of truncating

**Files:**
- Modify: `crates/manifold/src/store/journal.rs`

**Interfaces:**
- Consumes:
  - `Header.damaged` and `Header.prev_generation`, `prev_path`, `write_compacted(…, prev_generation, rotate)` (Task 6);
  - `Journal.dirty` (Task 4);
  - `Doc::prune` (Task 5);
  - `with_suffix`, `write_synced` (Task 3).
- Produces:
  - `pub struct FullScan<'a> { frames, damaged: Vec<(usize, usize)>, end, stop }` and `pub fn scan_all(buf: &[u8], base: usize) -> FullScan<'_>`;
  - `fn resync(buf: &[u8], at: usize) -> Option<usize>`;
  - `pub struct Recovery { damaged: Vec<(usize, usize)>, skipped_frames: usize, header_damaged: bool, from_prev: bool, new_document: bool, saved: Option<PathBuf> }` (Debug, Clone, Default, PartialEq);
  - `pub fn Journal::take_recovery(&mut self) -> Option<Recovery>`;
  - `pub fn damaged_path(path: &Path, bytes: &[u8]) -> PathBuf`;
  - `Inspect.damaged: Vec<(usize, usize)>`.
- Behaviour changes:
  - `load` never fails on a frame. It fails only on a header it can't read (`NotAJournal`).
  - `open` and a reloading `catch_up` repair the file on disk.

- [ ] **Step 1: Write the failing tests.** Add these helpers and tests to `mod tests`. Also change the testutil import to `use crate::store::testutil::{shown, temp_dir, XorShift};`.

```rust
    /// Three tabs compacted into a snapshot (its `.prev` holds the file
    /// before), then three ops frames of one tab each from another replica.
    fn layered_file(path: &Path) -> Vec<u8> {
        let mut journal = Journal::open(path, ActorId(1)).unwrap();
        for i in 0..3 {
            let mut txn = journal.begin().unwrap();
            let (_, create) = create_tab(&mut txn, &format!("https://s{i}.example/"));
            txn.commit(vec![create]).unwrap();
        }
        journal.close().unwrap();
        let mut journal = Journal::open(path, ActorId(2)).unwrap();
        for i in 0..3 {
            let mut txn = journal.begin().unwrap();
            let (_, create) = create_tab(&mut txn, &format!("https://o{i}.example/"));
            txn.commit(vec![create]).unwrap();
        }
        drop(journal);
        std::fs::read(path).unwrap()
    }

    /// `(offset, size)` of each whole frame.
    fn frame_spans(bytes: &[u8]) -> Vec<(usize, usize)> {
        let header = read_header(bytes).unwrap();
        scan(&bytes[header.len..], header.len).frames.iter().map(|f| (f.offset, f.size)).collect()
    }

    #[test]
    fn a_damaged_ops_frame_loses_only_its_own_ops() {
        let dir = temp_dir("resync");
        let path = dir.join("session.mnf");
        let bytes = layered_file(&path);
        let spans = frame_spans(&bytes);
        assert_eq!(spans.len(), 4);
        for &(offset, size) in &spans[1..] {
            let mut without = bytes[..offset].to_vec();
            without.extend_from_slice(&bytes[offset + size..]);
            let expected = shown(&Journal::read(&without).unwrap());
            let last = offset + size == bytes.len();
            // A flipped bit (the frame fails its CRC), or a zeroed frame
            // (writes persisted out of order).
            for zeroed in [false, true] {
                let mut damaged = bytes.clone();
                if zeroed {
                    damaged[offset..offset + size].fill(0);
                } else {
                    damaged[offset + size - 1] ^= 1;
                }
                std::fs::write(&path, &damaged).unwrap();
                let mut journal = Journal::open(&path, ActorId(3)).unwrap();
                assert_eq!(shown(journal.doc()), expected, "frame at {offset}, zeroed {zeroed}");
                if zeroed && last {
                    // Zeros at the end are a torn tail: an append that never landed.
                    assert!(journal.take_recovery().is_none());
                    continue;
                }
                assert_eq!(inspect(&damaged).unwrap().damaged, [(offset, offset + size)]);
                let recovery = journal.take_recovery().expect("damage is reported");
                assert_eq!(recovery.damaged, [(offset, offset + size)]);
                let saved = recovery.saved.expect("the file as it was is kept");
                assert_eq!(std::fs::read(&saved).unwrap(), damaged);
                std::fs::remove_file(saved).unwrap();
                assert_eq!(frame_spans(&std::fs::read(&path).unwrap()).len(), 1);
                assert!(Journal::open(&path, ActorId(4)).unwrap().take_recovery().is_none());
            }
        }
    }

    #[test]
    fn a_lost_snapshot_is_rebuilt_from_prev() {
        let dir = temp_dir("from-prev");
        let path = dir.join("session.mnf");
        let bytes = layered_file(&path);
        let original = Journal::read(&bytes).unwrap();
        let prev = std::fs::read(prev_path(&path)).unwrap();
        let (offset, size) = frame_spans(&bytes)[0];
        let mut damaged = bytes.clone();
        damaged[offset + size - 1] ^= 1;
        std::fs::write(&path, &damaged).unwrap();
        let mut journal = Journal::open(&path, ActorId(3)).unwrap();
        assert_eq!(shown(journal.doc()), shown(&original));
        assert_eq!(journal.doc().id, original.id);
        let recovery = journal.take_recovery().unwrap();
        assert!(recovery.from_prev && !recovery.new_document);
        // A repair leaves `.prev` as it was, and still linked.
        assert_eq!(std::fs::read(prev_path(&path)).unwrap(), prev);
        let header = read_header(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(header.prev_generation, read_header(&prev).unwrap().generation);

        // Without `.prev`: a new document holding what the surviving ops made.
        std::fs::remove_file(prev_path(&path)).unwrap();
        std::fs::write(&path, &damaged).unwrap();
        let mut journal = Journal::open(&path, ActorId(5)).unwrap();
        assert!(journal.take_recovery().unwrap().new_document);
        assert_ne!(journal.doc().id, original.id);
        let urls: Vec<FieldValue> = journal.doc().live().map(|(id, _)| url(journal.doc(), id)).collect();
        assert_eq!(urls, (0..3).map(|i| FieldValue::Str(format!("https://o{i}.example/"))).collect::<Vec<_>>());
    }

    #[test]
    fn damage_found_while_catching_up_is_repaired_not_cut() {
        let dir = temp_dir("live-damage");
        let path = dir.join("session.mnf");
        let mut a = Journal::open(&path, ActorId(1)).unwrap();
        let mut b = Journal::open(&path, ActorId(2)).unwrap();
        let mut txn = b.begin().unwrap();
        let (x, create_x) = create_tab(&mut txn, "https://x.example/");
        txn.commit(vec![create_x]).unwrap();
        let mut txn = b.begin().unwrap();
        let (y, create_y) = create_tab(&mut txn, "https://y.example/");
        txn.commit(vec![create_y]).unwrap();
        // X's frame is damaged on disk; Y's, after it, is whole.
        let mut bytes = std::fs::read(&path).unwrap();
        let (offset, size) = frame_spans(&bytes)[1];
        bytes[offset + size - 1] ^= 1;
        std::fs::write(&path, &bytes).unwrap();

        let mut txn = a.begin().unwrap();
        assert!(txn.remote.contains(&y));
        let (z, create_z) = create_tab(&mut txn, "https://z.example/");
        txn.commit(vec![create_z]).unwrap();
        let doc = Journal::read(&std::fs::read(&path).unwrap()).unwrap();
        assert!(doc.nodes[&y].is_live() && doc.nodes[&z].is_live());
        assert!(!doc.nodes.contains_key(&x));
        assert!(a.take_recovery().is_some_and(|r| r.saved.is_some()));
        // B held X only in memory: it learns X is gone (spec: accepted).
        let txn = b.begin().unwrap();
        assert!(txn.remote.contains(&x) && !txn.doc().nodes.contains_key(&x));
    }

    #[test]
    fn a_frame_that_does_not_decode_is_skipped_and_kept_aside() {
        let dir = temp_dir("undecodable");
        let path = dir.join("session.mnf");
        let mut journal = Journal::open(&path, ActorId(1)).unwrap();
        let mut txn = journal.begin().unwrap();
        let (tab, create) = create_tab(&mut txn, "https://ok.example/");
        txn.commit(vec![create]).unwrap();
        drop(journal);
        let mut bytes = std::fs::read(&path).unwrap();
        encode_frame(&mut bytes, frame::OPS, &[]); // CRC-valid, but no actor table
        std::fs::write(&path, &bytes).unwrap();
        let mut journal = Journal::open(&path, ActorId(2)).unwrap();
        assert!(journal.doc().nodes[&tab].is_live());
        let recovery = journal.take_recovery().unwrap();
        assert_eq!((recovery.skipped_frames, recovery.damaged.len()), (1, 0));
        assert_eq!(std::fs::read(recovery.saved.unwrap()).unwrap(), bytes);
    }

    #[test]
    fn a_damaged_header_is_rewritten() {
        let dir = temp_dir("bad-header");
        let path = dir.join("session.mnf");
        let bytes = layered_file(&path);
        let expected = shown(&Journal::read(&bytes).unwrap());
        let mut damaged = bytes.clone();
        damaged[8] ^= 1; // inside the generation
        std::fs::write(&path, &damaged).unwrap();
        let mut journal = Journal::open(&path, ActorId(3)).unwrap();
        assert_eq!(shown(journal.doc()), expected);
        let recovery = journal.take_recovery().unwrap();
        assert!(recovery.header_damaged && !recovery.from_prev);
        let saved = recovery.saved.unwrap();
        assert!(saved.to_string_lossy().ends_with(&format!(".damaged-{:016x}", crc32c(&damaged))));
        assert!(!read_header(&std::fs::read(&path).unwrap()).unwrap().damaged);
    }
```

Also add to `a_torn_tail_is_cut_before_the_next_append`, right after `let mut journal = Journal::open(&path, ActorId(2)).unwrap();`:

```rust
        assert!(journal.take_recovery().is_none()); // a torn tail is no damage
```

- [ ] **Step 2: Run to see them fail**

Run: `cargo test -q -p manifold -j 12 store::`
Expected: compile errors (`take_recovery`, `Inspect.damaged` missing).

- [ ] **Step 3: The resyncing scan.** Add after `scan`:

```rust
/// Whole frames of `buf` (the file from offset `base` on), read past
/// damage (spec "File and frames"). Where a scan stops, reading resumes at
/// the next valid frame. With none after it the stop is a torn tail,
/// except that a whole frame failing its CRC is damage either way.
pub struct FullScan<'a> {
    pub frames: Vec<RawFrame<'a>>,
    /// `(start, end)` of each damaged region skipped.
    pub damaged: Vec<(usize, usize)>,
    /// Where the whole frames end: the end of the file, or a torn tail.
    pub end: usize,
    pub stop: Option<Stop>,
}

pub fn scan_all(buf: &[u8], base: usize) -> FullScan<'_> {
    let mut frames = Vec::new();
    let mut damaged = Vec::new();
    let mut pos = 0;
    loop {
        let part = scan(&buf[pos..], base + pos);
        frames.extend(part.frames);
        let at = part.end - base;
        let Some(stop) = part.stop else {
            return FullScan { frames, damaged, end: part.end, stop: None };
        };
        match resync(buf, at) {
            Some(next) => {
                damaged.push((base + at, base + next));
                pos = next;
            }
            None => {
                if stop == Stop::BadCrc {
                    damaged.push((base + at, base + buf.len()));
                }
                return FullScan { frames, damaged, end: part.end, stop: Some(stop) };
            }
        }
    }
}

/// Where the next valid frame after a stop at `at` starts: where the
/// stopped frame's own length points (a damaged body), else the first
/// later offset holding one. `None` when none follows.
fn resync(buf: &[u8], at: usize) -> Option<usize> {
    let valid = |p: usize| p < buf.len() && frame_at(&buf[p..]).is_ok();
    let claimed = buf.get(at..at + 4).map(|b| at + 8 + u32::from_le_bytes(b.try_into().expect("4 bytes")) as usize);
    if let Some(p) = claimed.filter(|&p| p > at + 8 && valid(p)) {
        return Some(p);
    }
    (at + 1..buf.len()).find(|&p| valid(p))
}
```

- [ ] **Step 4: `Recovery` and the journal fields.** Add before `pub struct Journal`:

```rust
/// What reading a damaged file skipped or rebuilt (spec "File and
/// frames"). The app tells the user.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Recovery {
    /// `(start, end)` of each damaged region skipped.
    pub damaged: Vec<(usize, usize)>,
    /// Whole frames that didn't decode, or a snapshot after other frames.
    pub skipped_frames: usize,
    pub header_damaged: bool,
    /// The snapshot was lost; `.prev` gave the base.
    pub from_prev: bool,
    /// The snapshot was lost and no `.prev` fit: a new, empty document
    /// under a new id, with the surviving ops on top.
    pub new_document: bool,
    /// Where the file as it was is kept, once saved.
    pub saved: Option<PathBuf>,
}
```

Fields in `Journal`, after `dirty`:

```rust
    /// The file on disk still holds the damage `load` found; cleared once
    /// a rewrite succeeds.
    damaged_on_disk: bool,
    /// Repairs since `take_recovery` last took them.
    recovery: Option<Recovery>,
    /// The header's link to `.prev`, kept by a repair's rewrite.
    prev_generation: u64,
```

`blank`: `damaged_on_disk: false, recovery: None, prev_generation: 0,`.

- [ ] **Step 5: `load`, `take_all`, `prev_base`.** Replace `load`:

```rust
    /// Replaces this replica's state with `bytes`, a whole file, read past
    /// damage. Fails only on a header it can't read.
    fn load(&mut self, bytes: &[u8]) -> Result<(), JournalError> {
        let header = read_header(bytes)?;
        self.generation = header.generation;
        self.read_only = header.version > FORMAT_VERSION;
        self.keep_unknown = self.read_only;
        self.stale = false;
        let scan = scan_all(&bytes[header.len..], header.len);
        let mut found = Recovery { damaged: scan.damaged, header_damaged: header.damaged, ..Recovery::default() };
        self.prev_generation = if header.damaged { 0 } else { header.prev_generation };
        if !self.take_all(&scan.frames, Doc::new(DocId([0; 16])), &mut found.skipped_frames) {
            // Every file starts with its snapshot: this one was lost.
            if let Some((base, generation)) = self.prev_base(&header) {
                (found.from_prev, found.skipped_frames, self.prev_generation) = (true, 0, generation);
                self.take_all(&scan.frames, base, &mut found.skipped_frames);
            } else {
                self.doc.id = DocId::random();
                found.new_document = true;
            }
        }
        self.end = scan.end as u64;
        self.damaged_on_disk = found != Recovery::default();
        self.dirty = self.damaged_on_disk || !(scan.stop.is_none() && scan.frames.len() == 1 && scan.frames[0].ty == frame::SNAPSHOT);
        if self.damaged_on_disk {
            self.recovery = Some(found);
        }
        Ok(())
    }

    /// Applies `frames` on top of `base`, skipping and counting frames that
    /// don't decode, and snapshots after another frame (only damage puts
    /// one there). Returns whether the first frame taken was a snapshot.
    fn take_all(&mut self, frames: &[RawFrame], base: Doc, skipped: &mut usize) -> bool {
        self.clock.observe(base.clock);
        self.doc = base;
        (self.snapshot_len, self.since_snapshot) = (0, 0);
        let (mut taken, mut snapshot) = (0, false);
        for f in frames {
            if f.ty == frame::SNAPSHOT && taken > 0 {
                *skipped += 1;
                continue;
            }
            match self.take(f) {
                Ok(_) => {
                    snapshot |= taken == 0 && f.ty == frame::SNAPSHOT;
                    taken += 1;
                }
                Err(_) => *skipped += 1,
            }
        }
        snapshot
    }

    /// The document in `<path>.prev` and its generation, when that is the
    /// file this one was compacted from: its generation is the header's
    /// `prev_generation`, or the header is damaged and can't say. Read
    /// past damage itself, but never from its own `.prev`.
    fn prev_base(&self, header: &Header) -> Option<(Doc, u64)> {
        if self.path.as_os_str().is_empty() {
            return None;
        }
        let bytes = std::fs::read(prev_path(&self.path)).ok()?;
        let prev = read_header(&bytes).ok()?;
        if !header.damaged && (header.prev_generation == 0 || prev.generation != header.prev_generation) {
            return None;
        }
        let mut journal = Journal::blank(Path::new(""), ActorId(0));
        journal.load(&bytes).ok()?;
        if journal.recovery.as_ref().is_some_and(|r| r.new_document) {
            return None;
        }
        Some((journal.doc, prev.generation))
    }
```

- [ ] **Step 6: `catch_up`.** Replace it:

```rust
    /// Reads the frames other processes appended, or the whole file again
    /// if another process compacted it or damage turned up. Returns the
    /// nodes they touched.
    fn catch_up(&mut self) -> Result<Vec<NodeId>, JournalError> {
        let bytes = std::fs::read(&self.path)?;
        let header = read_header(&bytes)?;
        let mut touched = Vec::new();
        let mut reload = self.stale || header.generation != self.generation || (bytes.len() as u64) < self.end;
        if !reload {
            let scan = scan(&bytes[self.end as usize..], self.end as usize);
            for f in &scan.frames {
                // A snapshot after other frames, or a frame that doesn't
                // decode, is damage: `load` skips it.
                let taken = if f.ty == frame::SNAPSHOT { Err(JournalError::Corrupt("a snapshot after other frames".into())) } else { self.take(f) };
                match taken {
                    Ok(t) => touched.extend(t),
                    Err(_) => {
                        reload = true;
                        break;
                    }
                }
            }
            // A whole frame failing its CRC, or a stop with a valid frame
            // after it, is damage, not a torn tail.
            reload |= scan.stop == Some(Stop::BadCrc) || (scan.stop.is_some() && resync(&bytes, scan.end).is_some());
            if !reload {
                self.end = scan.end as u64;
                self.dirty |= !scan.frames.is_empty() || scan.stop.is_some();
            }
        }
        if reload {
            let before: Vec<NodeId> = self.doc.nodes.keys().copied().collect();
            self.load(&bytes)?;
            self.repair(&bytes);
            touched = before.into_iter().chain(self.doc.nodes.keys().copied()).collect();
        }
        self.note_seen();
        Ok(self.doc.with_descendants(touched.into_iter().collect()).into_iter().collect())
    }
```

- [ ] **Step 7: Repair and rewrite.** Replace `compact`:

```rust
    /// Rewrites the file as one pruned snapshot (spec "Compact"), keeping
    /// the old file as `.prev` unless it is damaged. Holds the lock.
    fn compact(&mut self) -> Result<(), JournalError> {
        self.rewrite(!self.damaged_on_disk)
    }

    fn rewrite(&mut self, rotate: bool) -> Result<(), JournalError> {
        self.doc.forget_moves();
        self.doc.prune();
        let prev_generation = if rotate { self.generation } else { self.prev_generation };
        let bytes = write_compacted(&self.path, &self.doc, prev_generation, rotate)?;
        let doc = std::mem::replace(&mut self.doc, Doc::new(DocId([0; 16])));
        self.load(&bytes)?;
        debug_assert_eq!(self.doc.nodes, doc.nodes);
        self.note_seen();
        self.unsynced = false;
        self.compaction_failed = false;
        Ok(())
    }

    /// After a load that found damage: saves the file as it was beside it,
    /// then rewrites it without the damage when this build may compact it.
    /// `.prev`, the good copy, is left as it is. If the save fails, nothing
    /// is rewritten; if the rewrite fails, the next `begin` tries again.
    fn repair(&mut self, bytes: &[u8]) {
        if !self.damaged_on_disk {
            return;
        }
        let saved = damaged_path(&self.path, bytes);
        if save_damaged(&saved, bytes).is_err() {
            return;
        }
        if let Some(recovery) = &mut self.recovery {
            recovery.saved = Some(saved);
        }
        if !self.keep_unknown && !self.read_only && self.rewrite(false).is_err() {
            (self.compaction_failed, self.stale) = (true, true);
        }
    }

    /// The repairs since the last call: the app tells the user once.
    pub fn take_recovery(&mut self) -> Option<Recovery> {
        self.recovery.take()
    }
```

Add below `prev_path`:

```rust
/// Where a damaged file is kept before its repair: `<path>.damaged-<16 hex
/// digits>`, its generation, or the whole file's CRC-32C when its header
/// is damaged.
pub fn damaged_path(path: &Path, bytes: &[u8]) -> PathBuf {
    let tag = match read_header(bytes) {
        Ok(header) if !header.damaged => header.generation,
        _ => u64::from(crc32c(bytes)),
    };
    with_suffix(path, &format!(".damaged-{tag:016x}"))
}

/// Writes `bytes` to `path` unless it exists, through a temp file, so a
/// file there is always whole.
fn save_damaged(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if path.exists() {
        return Ok(());
    }
    let tmp = with_suffix(path, ".tmp");
    write_synced(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}
```

In `open`, replace `journal.load(&std::fs::read(path)?)?;` with:

```rust
        let bytes = std::fs::read(path)?;
        journal.load(&bytes)?;
        journal.repair(&bytes);
```

Update `open`'s doc comment to: `/// Opens \`path\`, first creating an empty document there if there is none, and repairs damage (spec "File and frames"). \`NotAJournal\` leaves the file as it was.`

Update the module doc comment at the top of `journal.rs`:

```rust
//! The session file (spec "File and frames", "Journal"): a header, then
//! CRC-checked frames, each readable on its own. Reading resumes past
//! damage and the file is repaired, keeping the original beside it. A
//! torn tail is cut by the next append. Every read and write happens
//! under a lock on a separate `.lock` file, so two processes on one
//! profile take turns and each merges what the other appended.
```

Also update `JournalError::Corrupt`'s doc comment to `/// A message that doesn't decode (personal state; a frame is skipped instead).`

- [ ] **Step 8: `inspect`.** Add `pub damaged: Vec<(usize, usize)>,` to `Inspect` before `torn` (doc comment `/// \`(start, end)\` of each damaged region.`). Then in `inspect`, use `let scan = scan_all(&bytes[header.len..], header.len);`, add `damaged: scan.damaged,`, and keep `torn: scan.stop.map(|stop| (scan.end, stop))`. Put the `frames` line before `damaged` so `scan.frames` is read before the move.

- [ ] **Step 9: Keep the fuzz test's directory clean.** `use_damaged` opens files, and a repair or compaction leaves `.prev`/`.damaged-*` behind. A later iteration must not recover from an earlier one's `.prev`. Add the helper:

```rust
    /// Removes what opening `path` may leave beside it (`.prev`,
    /// `.damaged-*`, `.local`, temp files), but not the lock.
    fn clear_siblings(path: &Path) {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        for entry in std::fs::read_dir(path.parent().unwrap()).unwrap().flatten() {
            let other = entry.file_name().to_string_lossy().into_owned();
            if other.starts_with(&format!("{name}.")) && !other.ends_with(".lock") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
```

and call `clear_siblings(path);` as the last statement inside `use_damaged`'s `if open { … }` block.

- [ ] **Step 10: Run the tests**

Run: `cargo test -q -p manifold -j 12 store::`
Expected: 61 passed (56 + 5). Clippy: nothing.

If `a_cut_at_any_byte_keeps_exactly_the_whole_frames_before_it` fails: a cut inside the snapshot now reads as a lost snapshot (a new, empty document). Its `live().count()` is still 0 and matches `whole_ops == 0`, so a failure there is a real bug.

- [ ] **Step 11: Commit**

```bash
git add crates/manifold/src/store/journal.rs
git commit -m "store: read past damage and repair, keeping the original; .prev rebuilds a lost snapshot

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: Fuzz file-level damage and the sidecar

**Files:**
- Modify: `crates/manifold/src/store/journal.rs` (tests only)

**Interfaces:**
- Consumes: `take_recovery`, `clear_siblings`, `local::path_for`, and the private fields `keep_unknown` and `read_only` (visible to `mod tests`).

- [ ] **Step 1: More kinds of damage.** In `damage`, replace the final `match rng.below(8) { … }` with:

```rust
        let run = |rng: &mut XorShift, len: usize, max: u64| {
            let at = rng.below(len as u64) as usize;
            (at, (at + 1 + rng.below(max) as usize).min(len))
        };
        match rng.below(12) {
            0 => {
                let at = rng.below(out.len() as u64) as usize;
                out[at] ^= 1 << rng.below(8);
            }
            1 => out.truncate(rng.below(out.len() as u64) as usize),
            2 => {
                // Zeros, as from writes persisted out of order.
                let (at, end) = run(rng, out.len(), 64);
                out[at..end].fill(0);
            }
            3 => {
                let (at, end) = run(rng, out.len(), 64);
                for byte in &mut out[at..end] {
                    *byte = rng.next() as u8;
                }
            }
            4 => {
                // A run copied elsewhere, as from a misdirected write: it may
                // duplicate whole frames, snapshots included.
                let (from, end) = run(rng, out.len(), 256);
                let copied = out[from..end].to_vec();
                let at = rng.below(out.len() as u64 + 1) as usize;
                out.splice(at..at, copied);
            }
            _ => {}
        }
```

Update `damage`'s doc comment to: `/// Damages one frame's message (keeping its CRC valid, so the damage reaches the decoders), and sometimes the file itself: a bit, a cut, a zeroed or random run, or a run copied elsewhere.`

- [ ] **Step 2: After a compacting close, the file is clean.** In `use_damaged`, replace the `if open { … }` block with:

```rust
        if open {
            std::fs::write(path, bytes).unwrap();
            if let Ok(mut journal) = Journal::open(path, ActorId(3)) {
                if let Ok(mut txn) = journal.begin() {
                    let (_, create) = create_tab(&mut txn, "https://z.example/");
                    let _ = txn.commit(vec![create]);
                }
                let may_compact = !journal.keep_unknown && !journal.read_only;
                if journal.close().is_ok() && may_compact {
                    let mut again = Journal::open(path, ActorId(4)).expect("a repaired file opens");
                    assert!(again.take_recovery().is_none(), "a compacting close left damage behind");
                }
            }
            clear_siblings(path);
        }
```

- [ ] **Step 3: The sidecar's own fuzz test**

```rust
    #[test]
    fn damaged_personal_state_never_panics_or_reads_wrong() {
        let dir = temp_dir("local-damaged");
        let path = dir.join("session.mnf");
        let journal = Journal::open(&path, ActorId(1)).unwrap();
        journal.write_local(b"some personal state").unwrap();
        let clean = std::fs::read(local::path_for(&path)).unwrap();
        let mut rng = XorShift(0x10CA1);
        for _ in 0..2000 {
            let mut bytes = clean.clone();
            for _ in 0..1 + rng.below(3) {
                if bytes.is_empty() {
                    break;
                }
                let at = rng.below(bytes.len() as u64) as usize;
                match rng.below(3) {
                    0 => bytes[at] ^= 1 << rng.below(8),
                    1 => bytes.truncate(at),
                    _ => bytes.insert(at, rng.next() as u8),
                }
            }
            std::fs::write(local::path_for(&path), &bytes).unwrap();
            // The frame's CRC means any state read is the one written.
            if let Ok(Some(state)) = journal.read_local() {
                assert_eq!(state, b"some personal state");
            }
        }
    }
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -q -p manifold -j 12 store::`
Expected: 62 passed. Clippy: nothing.

If `damaged_files_never_panic` fails, it keeps the file as `crash-<i>.mnf` in its temp directory. Reproduce it with `Journal::open` on that file. Treat the failure as a real bug in Task 7's code, not in the fuzz test.

- [ ] **Step 5: Commit**

```bash
git add crates/manifold/src/store/journal.rs
git commit -m "store: fuzz zeroed, random and copied runs; a compacting close leaves a clean file

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: Record the outcome and Plan B's contracts

**Files:**
- Modify: `docs/superpowers/plans/2026-09-25-session-store-core.md` (the "Outcome" section)
- Modify: this plan (append an "Outcome" section)

- [ ] **Step 1: Contracts.** In the old plan's "Contracts Plan B must honour":
  - Replace the `**\`Local\` frames.**` bullet with:

```markdown
- **Personal state.** It is in `session.mnf.local` (format revision, Plan A′).
  - Call `read_local()` once after `open`. Treat `Err` or `None` as a fresh start.
  - Call `write_local()` only when the state changed: at most every 30 s, 2.5 s after a Recently Closed change, and at exit before `close()`.
  - Settled positions count as changed only past 1 px.
  - `save_copy(path, Some(state))` gives the copy its state.
- **Missing means deleted.** A node id in `remote` that is missing from `doc()` was deleted; compaction may have pruned it.
  - Never name a non-live node as the group or `parent` of a new op; use its effective group, or none.
  - After `begin`, apply the `remote` changes to the live graph before diffing, so a pruned node is never re-emitted.
- **Repairs.** After `open` and after each `begin`, call `take_recovery()`. When it returns `Some`, tell the user what was lost and where the original was saved (`saved`).
```

  - In "Deferred, with reasons", delete the **Plan-level risk** bullet: Plan A′ Task 7 resolved it.
  - Replace `\`catch_up\` reads the whole file on every save.` with `\`catch_up\` reads the whole file on every save (incremental parsing, but a whole-file read).`

- [ ] **Step 2: Outcome.** Append to this plan:

```markdown
## Outcome (executed YYYY-MM-DD)

Landed on `dev` as <first>..<last>: N `store::` tests; clippy clean. Deviations from the blocks above: <list, or "none">.
```

Fill in the real commits, the count and the deviations.

- [ ] **Step 3: Commit**

```bash
git add docs/superpowers/plans/2026-09-25-session-store-core.md docs/superpowers/plans/2026-09-25-session-store-format-revision.md
git commit -m "Plan: record the format revision's outcome and Plan B contracts

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Outcome (executed 2026-09-26)

Landed on `dev` in merge 6910e0c (branch `store-format-revision`, 587babf..a6bf90d): 66 `store::` tests; clippy clean. Deviations from the blocks above:
- Task 1: kept the spec's two unknown-frame-type bullets that the brief's replacement range would have deleted; also changed a leftover "written to `Local`" to the personal-state file.
- Global Constraints: the per-task clippy check became the pre-commit hook's (`-D warnings`), not a grep of `src\store`.
- Task 4: rewrapped `close`'s doc comment (308ab38).
- Task 7: review found that a failed save of damaged bytes still let compaction or an append destroy them; fixed with an `unsaved_damage` flag and an `ensure_saved` guard before every rewrite or truncation, plus two tests (1c5b2fd). Hence 64 tests, not 62.
- Final review: `save_copy` removes a stale `.prev`; a clean reload clears `unsaved_damage`; spec and contract wording aligned; two tests. Hence 66 tests, not 64.
- Otherwise the blocks were applied as written.
