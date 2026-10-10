//! The session file (spec "File and frames", "Journal"): a header, then
//! CRC-checked frames, each readable on its own. Reading resumes past
//! damage and the file is repaired, keeping the original beside it. A
//! torn tail is cut by the next append. Every read and write happens
//! under a lock on a separate `.lock` file, so two processes on one
//! profile take turns and each merges what the other appended.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use super::crc32c::{crc32c, crc32c_append};
use super::local;
use super::model::{now_ms, ActorId, Clock, Doc, DocId, Hlc, Node, NodeId, Reg, Stamp};
use super::ops::{Op, OpBody};
use super::tlv::{Reader, TlvError, Writer};
use super::varint;

pub const MAGIC: [u8; 4] = *b"MNF\0";
/// 2: a node's kind is its `type` register (field 19); 1 held a numeric
/// kind, and a version-1 file is refused (no migration).
pub const FORMAT_VERSION: u64 = 2;

/// Frame types. 0 is invalid, so a zeroed region never parses.
pub mod frame {
    pub const SNAPSHOT: u64 = 2;
    pub const OPS: u64 = 3;
    /// Never reused: 1 was a file-wide actor table (each frame now
    /// carries its own), 4 was personal state (now `<session>.local`).
    #[cfg(test)]
    pub const RETIRED: [u64; 2] = [1, 4];
}

/// How long an append may wait for its fsync (spec "Journal").
const SYNC_DEBOUNCE: Duration = Duration::from_secs(1);

/// Frames appended since the last snapshot before compacting: twice the
/// snapshot, and at least this.
const COMPACT_MIN: u64 = 64 * 1024;

#[derive(Debug)]
pub enum JournalError {
    Io(io::Error),
    /// No `MNF\0` magic, or the header is cut short.
    NotAJournal,
    /// A message that doesn't decode (personal state; a frame is skipped instead).
    Corrupt(String),
    /// Written by a newer format version; this build only reads it.
    ReadOnly,
    /// A read or write failed, so this replica may not match its file;
    /// `begin` re-reads it.
    Stale,
}

impl From<io::Error> for JournalError {
    fn from(err: io::Error) -> Self {
        JournalError::Io(err)
    }
}

impl From<TlvError> for JournalError {
    fn from(err: TlvError) -> Self {
        JournalError::Corrupt(err.to_string())
    }
}

impl std::fmt::Display for JournalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JournalError::Io(err) => write!(f, "{err}"),
            JournalError::NotAJournal => write!(f, "not a manifold session file"),
            JournalError::Corrupt(what) => write!(f, "damaged session file: {what}"),
            JournalError::ReadOnly => write!(f, "written by a newer manifold; opened read-only"),
            JournalError::Stale => write!(f, "the session file must be read again first"),
        }
    }
}

// ---- Frames -------------------------------------------------------------

/// Why `scan` stopped before the end of the buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// The frame's length runs past the end of the file.
    Overrun,
    BadCrc,
    ZeroLength,
    ZeroType,
}

/// One whole frame found by `scan`.
#[derive(Debug, Clone, Copy)]
pub struct RawFrame<'a> {
    /// Offset of its first byte in the file.
    pub offset: usize,
    /// Bytes it takes, header included.
    pub size: usize,
    pub ty: u64,
    pub message: &'a [u8],
}

pub struct Scan<'a> {
    pub frames: Vec<RawFrame<'a>>,
    /// Where the whole frames end: the end of the file, or a torn tail.
    pub end: usize,
    /// Why reading stopped at `end`, when that isn't the end of the file.
    pub stop: Option<Stop>,
}

pub fn encode_frame(out: &mut Vec<u8>, ty: u64, message: &[u8]) {
    let mut body = Vec::with_capacity(message.len() + 1);
    varint::write(&mut body, ty);
    body.extend_from_slice(message);
    let len = u32::try_from(body.len()).expect("a frame under 4 GiB");
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&frame_crc(len, &body).to_le_bytes());
    out.extend_from_slice(&body);
}

/// CRC-32C over `len ‖ frame_type ‖ message`, so a zeroed region fails.
fn frame_crc(len: u32, body: &[u8]) -> u32 {
    crc32c_append(crc32c(&len.to_le_bytes()), body)
}

fn frame_at(buf: &[u8]) -> Result<(u64, &[u8], usize), Stop> {
    let word = |at: usize| buf.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().expect("4 bytes")));
    let (Some(len), Some(crc)) = (word(0), word(4)) else { return Err(Stop::Overrun) };
    let body = buf.get(8..8 + len as usize).ok_or(Stop::Overrun)?;
    if len == 0 {
        return Err(Stop::ZeroLength);
    }
    if frame_crc(len, body) != crc {
        return Err(Stop::BadCrc);
    }
    let (ty, n) = varint::read(body).map_err(|_| Stop::ZeroType)?;
    if ty == 0 {
        return Err(Stop::ZeroType);
    }
    Ok((ty, &body[n..], 8 + len as usize))
}

/// Splits `buf` (the file from offset `base` on) into whole frames.
pub fn scan(buf: &[u8], base: usize) -> Scan<'_> {
    let mut frames = Vec::new();
    let mut pos = 0;
    let stop = loop {
        if pos == buf.len() {
            break None;
        }
        match frame_at(&buf[pos..]) {
            Ok((ty, message, size)) => {
                frames.push(RawFrame { offset: base + pos, size, ty, message });
                pos += size;
            }
            Err(stop) => break Some(stop),
        }
    };
    Scan { frames, end: base + pos, stop }
}

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
    // A body starting with a zero byte has frame type 0, which `frame_at`
    // refuses: skipping it here saves the checksum, not a result.
    let valid = |p: usize| buf.get(p + 8).is_some_and(|&b| b != 0) && frame_at(&buf[p..]).is_ok();
    let claimed = buf.get(at..at + 4).map(|b| at + 8 + u32::from_le_bytes(b.try_into().expect("4 bytes")) as usize);
    if let Some(p) = claimed.filter(|&p| p > at + 8 && valid(p)) {
        return Some(p);
    }
    (at + 1..buf.len()).find(|&p| valid(p))
}

// ---- Header ---------------------------------------------------------------

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
    let word = |i: usize| {
        buf.get(i..i + 8).map(|b| u64::from_le_bytes(b.try_into().expect("8 bytes"))).ok_or(JournalError::NotAJournal)
    };
    let (generation, prev_generation) = (word(at)?, word(at + 8)?);
    let crc = buf.get(at + 16..at + 20).ok_or(JournalError::NotAJournal)?;
    let damaged = crc32c(&buf[..at + 16]) != u32::from_le_bytes(crc.try_into().expect("4 bytes"));
    Ok(Header { version, generation, prev_generation, damaged, len: at + 20 })
}

/// `read_header`, refusing as `NotAJournal` a file older than
/// `FORMAT_VERSION`: version 1 held a numeric kind and no `type`, so read
/// as this format every node would be typeless and compact away (no
/// migration). The one rule for what this build reads: `load`, the
/// set-aside check and `--check` all use it.
pub fn read_readable_header(buf: &[u8]) -> Result<Header, JournalError> {
    let header = read_header(buf)?;
    if header.version < FORMAT_VERSION {
        return Err(JournalError::NotAJournal);
    }
    Ok(header)
}

/// The most a header can take: magic (4), a varint version (at most 9,
/// `varint`'s longest), two u64 generations and a CRC (4).
const HEADER_MAX: u64 = 4 + 9 + 16 + 4;

/// The header, the file's length and its bytes from `from` on: what an
/// incremental catch-up needs, without reading the snapshot before `from`.
/// An empty tail when the file is shorter than `from`.
fn read_tail(path: &Path, from: u64) -> Result<(Header, u64, Vec<u8>), JournalError> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let mut head = Vec::new();
    (&mut file).take(HEADER_MAX).read_to_end(&mut head)?;
    let header = read_header(&head)?;
    let mut tail = Vec::new();
    if from <= len {
        file.seek(SeekFrom::Start(from))?;
        file.read_to_end(&mut tail)?;
    }
    Ok((header, len, tail))
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

// ---- Messages -------------------------------------------------------------

/// A frame's actor table: the frame names actors by their index here.
#[derive(Debug, Clone, Default)]
pub struct Actors {
    list: Vec<ActorId>,
    index: BTreeMap<ActorId, u64>,
}

impl Actors {
    fn push(&mut self, actor: ActorId) {
        if !self.index.contains_key(&actor) {
            self.index.insert(actor, self.list.len() as u64);
            self.list.push(actor);
        }
    }

    fn index(&self, actor: ActorId) -> u64 {
        self.index[&actor]
    }

    fn get(&self, index: u64) -> Result<ActorId, JournalError> {
        let bad = || JournalError::Corrupt(format!("actor index {index} not in the table"));
        self.list.get(usize::try_from(index).map_err(|_| bad())?).copied().ok_or_else(bad)
    }
}

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

/// A node id in a frame's own structure: `{1: actor index, 2: counter}`.
fn id_message(id: NodeId, actors: &Actors) -> Writer {
    let mut m = Writer::new();
    let index = actors.index(id.actor);
    if index != 0 {
        m.varint(1, index);
    }
    if id.counter != 0 {
        m.varint(2, id.counter);
    }
    m
}

fn decode_id(buf: &[u8], actors: &Actors) -> Result<NodeId, JournalError> {
    // The `_ => {}` arms in this file's decoders skip fields a newer build wrote (forward
    // compatibility). Nothing is lost: a frame that doesn't re-encode to itself sets
    // `keep_unknown`, which stops the journal being compacted or rewritten (`should_compact`).
    let (mut index, mut counter) = (0, 0);
    for field in Reader::new(buf) {
        let field = field?;
        match field.no {
            1 => index = field.value.u64()?,
            2 => counter = field.value.u64()?,
            _ => {}
        }
    }
    Ok(NodeId { actor: actors.get(index)?, counter })
}

fn hlc_message(hlc: Hlc) -> Writer {
    let mut m = Writer::new();
    if hlc.ms != 0 {
        m.varint(1, hlc.ms);
    }
    if hlc.logical != 0 {
        m.varint(2, u64::from(hlc.logical));
    }
    m
}

fn decode_hlc(buf: &[u8]) -> Result<Hlc, JournalError> {
    let mut hlc = Hlc::default();
    for field in Reader::new(buf) {
        let field = field?;
        match field.no {
            1 => hlc.ms = field.value.u64()?,
            2 => hlc.logical = field.value.u32()?,
            _ => {}
        }
    }
    Ok(hlc)
}

/// `{1: ms, 2: logical, 3: actor index}`.
fn stamp_message(stamp: Stamp, actors: &Actors) -> Writer {
    let mut m = hlc_message(stamp.hlc);
    let index = actors.index(stamp.actor);
    if index != 0 {
        m.varint(3, index);
    }
    m
}

fn decode_stamp(buf: &[u8], actors: &Actors) -> Result<Stamp, JournalError> {
    let (mut hlc, mut index) = (Hlc::default(), 0);
    for field in Reader::new(buf) {
        let field = field?;
        match field.no {
            1 => hlc.ms = field.value.u64()?,
            2 => hlc.logical = field.value.u32()?,
            3 => index = field.value.u64()?,
            _ => {}
        }
    }
    Ok(Stamp { hlc, actor: actors.get(index)? })
}

/// Every actor `doc`'s structure names, sorted: a compacted file's table.
fn doc_actors(doc: &Doc) -> Actors {
    let mut set = BTreeSet::new();
    set.extend(doc.version.keys().copied());
    for (id, node) in &doc.nodes {
        set.insert(id.actor);
        set.extend(node.group.map(|g| g.actor));
        set.extend(node.fields.values().map(|reg| reg.stamp.actor));
    }
    let mut actors = Actors::default();
    for actor in set {
        actors.push(actor);
    }
    actors
}

/// Snapshot: `{1: doc id, 2: node*, 3: version*, 4: reserved for a
/// retained Move-log tail, 5: clock, 6: actor*}`. A node is `{1: id, 3:
/// deleted, 4: group, 5: register*}` (2, the numeric kind, is retired: the
/// kind is the `type` register since format 2) and a register `{1: field,
/// 2: stamp, 3: value encoding}`. The actor table is sorted, and nodes and
/// registers are in id and field order, so equal documents encode to
/// equal bytes.
fn snapshot_message(doc: &Doc) -> Vec<u8> {
    let actors = doc_actors(doc);
    let actors = &actors;
    let mut w = Writer::new();
    w.bytes(1, &doc.id.0);
    for (id, node) in &doc.nodes {
        let mut n = Writer::new();
        n.message(1, &id_message(*id, actors));
        if node.deleted {
            n.varint(3, 1);
        }
        if let Some(group) = node.group {
            n.message(4, &id_message(group, actors));
        }
        for (no, reg) in &node.fields {
            let mut r = Writer::new();
            r.varint(1, u64::from(*no));
            r.message(2, &stamp_message(reg.stamp, actors));
            if !reg.enc.is_empty() {
                r.bytes(3, &reg.enc);
            }
            n.message(5, &r);
        }
        w.message(2, &n);
    }
    for (actor, next) in &doc.version {
        let mut v = Writer::new();
        let index = actors.index(*actor);
        if index != 0 {
            v.varint(1, index);
        }
        if *next != 0 {
            v.varint(2, *next);
        }
        w.message(3, &v);
    }
    let clock = hlc_message(doc.clock);
    if !clock.is_empty() {
        w.message(5, &clock);
    }
    write_table(&mut w, 6, actors);
    w.into_bytes()
}

fn decode_snapshot(buf: &[u8]) -> Result<Doc, JournalError> {
    let actors = read_table(buf, 6)?;
    let actors = &actors;
    let mut doc = Doc::new(DocId([0; 16]));
    for field in Reader::new(buf) {
        let field = field?;
        match field.no {
            1 => {
                let bytes = field.value.bytes()?;
                doc.id = DocId(bytes.try_into().map_err(|_| JournalError::Corrupt("document id length".into()))?);
            }
            2 => {
                let mut id = None;
                let mut node = Node::placeholder();
                for f in Reader::new(field.value.bytes()?) {
                    let f = f?;
                    match f.no {
                        1 => id = Some(decode_id(f.value.bytes()?, actors)?),
                        3 => node.deleted = f.value.bool()?,
                        4 => node.group = Some(decode_id(f.value.bytes()?, actors)?),
                        5 => {
                            let (mut no, mut stamp, mut enc) = (0, None, Vec::new());
                            for r in Reader::new(f.value.bytes()?) {
                                let r = r?;
                                match r.no {
                                    1 => no = r.value.u32()?,
                                    2 => stamp = Some(decode_stamp(r.value.bytes()?, actors)?),
                                    3 => enc = r.value.bytes()?.to_vec(),
                                    _ => {}
                                }
                            }
                            let stamp =
                                stamp.ok_or_else(|| JournalError::Corrupt("register without a stamp".into()))?;
                            node.fields.insert(no, Reg { stamp, enc });
                        }
                        _ => {}
                    }
                }
                let id = id.ok_or_else(|| JournalError::Corrupt("node without an id".into()))?;
                doc.nodes.insert(id, node);
            }
            3 => {
                let (mut index, mut next) = (0, 0);
                for f in Reader::new(field.value.bytes()?) {
                    let f = f?;
                    match f.no {
                        1 => index = f.value.u64()?,
                        2 => next = f.value.u64()?,
                        _ => {}
                    }
                }
                doc.version.insert(actors.get(index)?, next);
            }
            5 => doc.clock = decode_hlc(field.value.bytes()?)?,
            _ => {}
        }
    }
    Ok(doc)
}

/// The snapshot frame for `doc`, as a compacted file holds it. Equal
/// documents give equal bytes.
pub fn snapshot_bytes(doc: &Doc) -> Vec<u8> {
    let mut out = Vec::new();
    encode_frame(&mut out, frame::SNAPSHOT, &snapshot_message(doc));
    out
}

/// The actors an op names in its structure.
fn op_actors(op: &Op) -> Vec<ActorId> {
    let mut actors = vec![op.id.actor];
    match &op.body {
        OpBody::Create { node, .. } | OpBody::Set { node, .. } | OpBody::Delete { node } => actors.push(node.actor),
        OpBody::Move { node, group } => {
            actors.push(node.actor);
            actors.extend(group.map(|g| g.actor));
        }
    }
    actors
}

/// Ops: `{1: actor*, 2: op*}`, the frame's table with the author first
/// (index 0) and then every other actor its ops name, in first-use order.
// The body field numbers of an op message, named once for `ops_message` and
// `decode_ops` so a new op kind can't fall through to another's decoding.
const OP_CREATE: u32 = 4;
const OP_SET: u32 = 5;
const OP_MOVE: u32 = 6;
const OP_DELETE: u32 = 7;

/// An op is `{1: counter delta, 2: ms delta (both zigzag, from the
/// previous op in the frame), 3: logical, then one body field: 4 Create
/// {1: node, 3: fields} (2, the numeric kind, is retired), 5 Set {1: node, 2: field, 3: value
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
    let (mut counter, mut ms) = (0u64, 0u64);
    for op in ops {
        assert_eq!(op.id.actor, author, "one author per ops frame");
        let mut o = Writer::new();
        let delta = |a: u64, b: u64| varint::zigzag(a.wrapping_sub(b) as i64);
        if delta(op.id.counter, counter) != 0 {
            o.varint(1, delta(op.id.counter, counter));
        }
        if delta(op.hlc.ms, ms) != 0 {
            o.varint(2, delta(op.hlc.ms, ms));
        }
        if op.hlc.logical != 0 {
            o.varint(3, u64::from(op.hlc.logical));
        }
        (counter, ms) = (op.id.counter, op.hlc.ms);
        let mut b = Writer::new();
        let field = match &op.body {
            OpBody::Create { node, fields } => {
                b.message(1, &id_message(*node, actors));
                if !fields.is_empty() {
                    b.bytes(3, fields);
                }
                OP_CREATE
            }
            OpBody::Set { node, field, enc } => {
                b.message(1, &id_message(*node, actors));
                b.varint(2, u64::from(*field));
                if !enc.is_empty() {
                    b.bytes(3, enc);
                }
                OP_SET
            }
            OpBody::Move { node, group } => {
                b.message(1, &id_message(*node, actors));
                if let Some(group) = group {
                    b.message(2, &id_message(*group, actors));
                }
                OP_MOVE
            }
            OpBody::Delete { node } => {
                b.message(1, &id_message(*node, actors));
                OP_DELETE
            }
        };
        o.message(field, &b);
        w.message(2, &o);
    }
    w.into_bytes()
}

/// The ops in an ops frame. Op kinds this build doesn't know are skipped
/// (`take` notices: the frame then doesn't re-encode to itself).
fn decode_ops(buf: &[u8]) -> Result<Vec<Op>, JournalError> {
    let actors = read_table(buf, 1)?;
    let actors = &actors;
    let author = actors.get(0)?;
    let mut ops = Vec::new();
    let (mut counter, mut ms) = (0u64, 0u64);
    for field in Reader::new(buf) {
        let field = field?;
        if field.no != 2 {
            continue;
        }
        let (mut logical, mut body) = (0, None);
        let (mut counter_delta, mut ms_delta) = (0, 0);
        for f in Reader::new(field.value.bytes()?) {
            let f = f?;
            match f.no {
                1 => counter_delta = f.value.u64()?,
                2 => ms_delta = f.value.u64()?,
                3 => logical = f.value.u32()?,
                OP_CREATE..=OP_DELETE => body = Some((f.no, f.value.bytes()?)),
                _ => {}
            }
        }
        counter = counter.wrapping_add(varint::unzigzag(counter_delta) as u64);
        ms = ms.wrapping_add(varint::unzigzag(ms_delta) as u64);
        if counter == u64::MAX {
            return Err(JournalError::Corrupt("op counter".into())); // `version` holds counter + 1
        }
        let Some((kind, bytes)) = body else {
            continue;
        };
        let (mut node, mut num, mut blob, mut group) = (None, 0, Vec::new(), None);
        for f in Reader::new(bytes) {
            let f = f?;
            match (kind, f.no) {
                (_, 1) => node = Some(decode_id(f.value.bytes()?, actors)?),
                (OP_SET, 2) => num = f.value.u64()?,
                (OP_MOVE, 2) => group = Some(decode_id(f.value.bytes()?, actors)?),
                (OP_CREATE | OP_SET, 3) => blob = f.value.bytes()?.to_vec(),
                _ => {}
            }
        }
        let node = node.ok_or_else(|| JournalError::Corrupt("op without a node".into()))?;
        let body = match kind {
            OP_CREATE => OpBody::Create { node, fields: blob },
            OP_SET => OpBody::Set {
                node,
                field: u32::try_from(num).map_err(|_| JournalError::Corrupt("field number".into()))?,
                enc: blob,
            },
            OP_MOVE => OpBody::Move { node, group },
            OP_DELETE => OpBody::Delete { node },
            _ => unreachable!("the body field is one of OP_CREATE..=OP_DELETE"),
        };
        ops.push(Op { id: NodeId { actor: author, counter }, hlc: Hlc { ms, logical }, body });
    }
    Ok(ops)
}

// ---- The journal ----------------------------------------------------------

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

/// One replica's handle on a session file: the document as of the last
/// read, this launch's actor, clock and id counter.
pub struct Journal {
    path: PathBuf,
    actor: ActorId,
    clock: Clock,
    counter: u64,
    generation: u64,
    /// End of the last whole frame read: where the next append goes.
    end: u64,
    doc: Doc,
    /// Size of the latest snapshot frame, and of the frames after it.
    snapshot_len: u64,
    since_snapshot: u64,
    /// A frame type, op kind, field or format version this build doesn't
    /// know was read: never compact, since that would drop it.
    keep_unknown: bool,
    /// A read or write failed partway, so this state (actor table, `end`,
    /// document) may not match the file: the next `begin` re-reads it whole.
    stale: bool,
    /// A compaction failed; appends go on and `close` tries again.
    compaction_failed: bool,
    /// The file holds more than one clean snapshot: frames after it, a torn
    /// tail, or damage. `close` compacts only then.
    dirty: bool,
    /// The file on disk still holds the damage `load` found; cleared once
    /// a rewrite succeeds.
    damaged_on_disk: bool,
    /// A repair's save of the damaged original failed: no rewrite or
    /// truncating append may proceed until a retried save succeeds.
    unsaved_damage: bool,
    /// Repairs since `take_recovery` last took them.
    recovery: Option<Recovery>,
    /// The header's link to `.prev`, kept by a repair's rewrite.
    prev_generation: u64,
    read_only: bool,
    /// Length and modified time last seen, for `changed_on_disk`.
    seen: Option<(u64, SystemTime)>,
    last_sync: Instant,
    unsynced: bool,
    /// `begin` calls, for tests that count saves that reached the store.
    #[cfg(test)]
    pub(crate) begins: usize,
}

/// Holds the profile lock; commit through it. Dropping it unlocks.
pub struct Txn<'a> {
    journal: &'a mut Journal,
    _lock: File,
    /// Nodes other processes changed since this replica last read.
    pub remote: Vec<NodeId>,
}

impl Journal {
    fn blank(path: &Path, actor: ActorId) -> Journal {
        Journal {
            path: path.to_path_buf(),
            actor,
            clock: Clock::default(),
            counter: 0,
            generation: 0,
            end: 0,
            doc: Doc::new(DocId([0; 16])),
            snapshot_len: 0,
            since_snapshot: 0,
            keep_unknown: false,
            stale: false,
            compaction_failed: false,
            dirty: false,
            damaged_on_disk: false,
            unsaved_damage: false,
            recovery: None,
            prev_generation: 0,
            read_only: false,
            seen: None,
            last_sync: Instant::now(),
            unsynced: false,
            #[cfg(test)]
            begins: 0,
        }
    }

    /// Opens `path`, first creating an empty document there if there is
    /// none, and repairs damage (spec "File and frames"). `NotAJournal`
    /// leaves the file as it was.
    pub fn open(path: &Path, actor: ActorId) -> Result<Journal, JournalError> {
        let mut journal = Journal::blank(path, actor);
        let _lock = journal.lock()?;
        // `exists()` is false on any error; a transient access error must not
        // replace the user's session with an empty one.
        if !path.try_exists()? {
            write_compacted(path, &Doc::new(DocId::random()), 0, false)?;
        }
        let bytes = std::fs::read(path)?;
        journal.load(&bytes)?;
        journal.repair(&bytes);
        journal.note_seen();
        Ok(journal)
    }

    /// `open`, but a file that isn't a journal this build reads (not one at
    /// all, or an older format: `read_readable_header`) is renamed to
    /// `<path>.unreadable-<secs>[-<n>]`, with its `.prev` and `.local`
    /// beside it (`set_aside_unless_journal`), and a fresh journal is
    /// created in its place. Refusing used to leave the app without a
    /// store, so nothing more was saved that run. Returns where the old
    /// file went.
    pub fn open_or_set_aside(path: &Path, actor: ActorId) -> Result<(Journal, Option<PathBuf>), JournalError> {
        match Journal::open(path, actor) {
            Err(JournalError::NotAJournal) => {
                let aside = set_aside_unless_journal(path)?;
                Ok((Journal::open(path, actor)?, aside))
            }
            other => other.map(|journal| (journal, None)),
        }
    }

    /// Reads a session file's document, as `open` would, without a lock or
    /// an actor. For `--dump` and tests.
    pub fn read(bytes: &[u8]) -> Result<Doc, JournalError> {
        let mut journal = Journal::blank(Path::new(""), ActorId(0));
        journal.load(bytes)?;
        Ok(journal.doc)
    }

    /// `read`, but `None` when loading had to repair anything: a lost
    /// snapshot, damaged regions or skipped frames.
    pub fn read_intact(bytes: &[u8]) -> Result<Option<Doc>, JournalError> {
        let mut journal = Journal::blank(Path::new(""), ActorId(0));
        journal.load(bytes)?;
        Ok(journal.recovery.is_none().then_some(journal.doc))
    }

    pub fn doc(&self) -> &Doc {
        &self.doc
    }

    /// The session file this journal reads and writes.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A newer build wrote the file: commits are refused, and this build may not read all of it.
    pub fn read_only(&self) -> bool {
        self.read_only
    }

    /// This profile's personal state for this document (spec "Personal
    /// state"). `None` when there is none, or it is another document's.
    pub fn read_local(&self) -> Result<Option<Vec<u8>>, JournalError> {
        self.read_local_within(LOCK_WAIT)
    }

    /// `read_local`, giving up with `WouldBlock` once the lock has been
    /// busy for `wait` (`Duration::ZERO`: a single try, for a retry that
    /// mustn't stall a frame).
    pub fn read_local_within(&self, wait: Duration) -> Result<Option<Vec<u8>>, JournalError> {
        let _lock = lock_file_within(&self.path, wait)?;
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

    /// A fresh id from this launch's counter, for a new node.
    pub fn next_id(&mut self) -> NodeId {
        let id = NodeId { actor: self.actor, counter: self.counter };
        self.counter += 1;
        id
    }

    fn lock(&self) -> Result<File, JournalError> {
        lock_file(&self.path)
    }

    /// Takes the lock and reads what other processes appended since.
    pub fn begin(&mut self) -> Result<Txn<'_>, JournalError> {
        #[cfg(test)]
        {
            self.begins += 1;
        }
        let lock = self.lock()?;
        let remote = self.guard(Journal::catch_up)?;
        Ok(Txn { journal: self, _lock: lock, remote })
    }

    /// Cheap check, no lock: has the file changed since this replica last
    /// read or wrote it? If so, `begin` picks the changes up.
    pub fn changed_on_disk(&self) -> bool {
        file_seen(&self.path) != self.seen
    }

    fn note_seen(&mut self) {
        self.seen = file_seen(&self.path);
    }

    /// Replaces this replica's state with `bytes`, a whole file, read past
    /// damage. Fails only on a header it can't read.
    fn load(&mut self, bytes: &[u8]) -> Result<(), JournalError> {
        let header = read_readable_header(bytes)?;
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
        self.dirty = self.damaged_on_disk
            || !(scan.stop.is_none() && scan.frames.len() == 1 && scan.frames[0].ty == frame::SNAPSHOT);
        if self.damaged_on_disk {
            self.recovery = Some(found);
        } else {
            // A file with no damage has nothing left to save.
            self.unsaved_damage = false;
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
            _ => {
                self.keep_unknown = true; // a newer build's frame: kept, not read
                Vec::new()
            }
        };
        self.since_snapshot += frame.size as u64;
        Ok(touched)
    }

    /// Reads the frames other processes appended, or the whole file again
    /// if another process compacted it or damage turned up. Returns the
    /// nodes they touched. After a whole-file read, that is the nodes that
    /// differ from before (plus those the tail's frames touched before the
    /// damage that caused it), so another replica's compaction alone
    /// reports nothing (pruning a tombstone changes nothing a replica shows).
    fn catch_up(&mut self) -> Result<Vec<NodeId>, JournalError> {
        let (header, len, tail) = read_tail(&self.path, self.end)?;
        let mut touched = Vec::new();
        let mut reload = self.stale || header.generation != self.generation || len < self.end;
        if !reload {
            let base = self.end as usize;
            let scan = scan(&tail, base);
            for f in &scan.frames {
                // A snapshot after other frames, or a frame that doesn't
                // decode, is damage: `load` skips it.
                let taken = if f.ty == frame::SNAPSHOT {
                    Err(JournalError::Corrupt("a snapshot after other frames".into()))
                } else {
                    self.take(f)
                };
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
            reload |=
                scan.stop == Some(Stop::BadCrc) || (scan.stop.is_some() && resync(&tail, scan.end - base).is_some());
            if !reload {
                self.end = scan.end as u64;
                self.dirty |= !scan.frames.is_empty() || scan.stop.is_some();
            }
        }
        if reload {
            // Only now the whole file: another process compacted it, or damage turned up.
            let bytes = std::fs::read(&self.path)?;
            let before = std::mem::take(&mut self.doc.nodes);
            self.load(&bytes)?;
            self.repair(&bytes);
            let after = &self.doc.nodes;
            let changed = |(id, old): (&NodeId, &Node)| match after.get(id) {
                Some(new) => new != old,
                None => old.is_live(),
            };
            // Added to what the tail frames applied before the damage touched:
            // `before` already holds those changes, so the diff alone misses them.
            touched.extend(before.iter().filter(|&n| changed(n)).map(|(id, _)| *id));
            touched.extend(after.keys().filter(|id| !before.contains_key(id)));
        }
        self.note_seen();
        Ok(self.doc.with_descendants(touched.into_iter().collect()).into_iter().collect())
    }

    fn append(&mut self, bytes: &[u8]) -> Result<(), JournalError> {
        self.ensure_saved()?;
        let mut file = OpenOptions::new().write(true).open(&self.path)?;
        file.set_len(self.end)?; // cuts a torn tail
        file.seek(SeekFrom::Start(self.end))?;
        file.write_all(bytes)?;
        self.end += bytes.len() as u64;
        self.since_snapshot += bytes.len() as u64;
        self.dirty = true;
        if self.last_sync.elapsed() >= SYNC_DEBOUNCE {
            file.sync_data()?;
            self.last_sync = Instant::now();
            self.unsynced = false;
        } else {
            self.unsynced = true;
        }
        drop(file);
        self.note_seen();
        Ok(())
    }

    /// Flushes an append whose fsync was debounced, once it is due. Call it
    /// now and then (the app does, every frame it draws).
    pub fn sync_if_due(&mut self) -> Result<(), JournalError> {
        if self.unsynced && self.last_sync.elapsed() >= SYNC_DEBOUNCE {
            sync_data(&self.path)?;
            self.last_sync = Instant::now();
            self.unsynced = false;
        }
        Ok(())
    }

    fn should_compact(&self) -> bool {
        !self.keep_unknown && !self.compaction_failed && self.since_snapshot > (2 * self.snapshot_len).max(COMPACT_MIN)
    }

    /// Rewrites the file as one pruned snapshot (spec "Compact"), keeping
    /// the old file as `.prev` unless it is damaged. Holds the lock.
    fn compact(&mut self) -> Result<(), JournalError> {
        self.rewrite(!self.damaged_on_disk)
    }

    fn rewrite(&mut self, rotate: bool) -> Result<(), JournalError> {
        self.ensure_saved()?;
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
    /// A read-only file, or one holding something this build doesn't know
    /// (`keep_unknown`), still gets its damaged copy saved but is never
    /// rewritten. `.prev`, the good copy, is left as it is. If the save
    /// fails, the file is left untouched (`unsaved_damage`), and
    /// `ensure_saved` retries before any later rewrite or truncating
    /// append; if the rewrite fails, the next `begin` tries again.
    fn repair(&mut self, bytes: &[u8]) {
        if !self.damaged_on_disk || self.save_original(bytes).is_err() {
            return;
        }
        if !self.keep_unknown && !self.read_only && self.rewrite(false).is_err() {
            (self.compaction_failed, self.stale) = (true, true);
        }
    }

    /// Saves `bytes` (the file as read, damaged) aside if it isn't already
    /// there, tracking whether that succeeded in `unsaved_damage` and, when
    /// there is a pending `Recovery`, its `saved` path.
    fn save_original(&mut self, bytes: &[u8]) -> io::Result<PathBuf> {
        let saved = damaged_path(&self.path, bytes);
        let result = save_damaged(&saved, bytes);
        self.unsaved_damage = result.is_err();
        if result.is_ok() {
            if let Some(recovery) = &mut self.recovery {
                recovery.saved = Some(saved.clone());
            }
        }
        result.map(|()| saved)
    }

    /// Retries saving the damaged original once before anything that would
    /// rewrite or truncate the live file, so a save that failed in `repair`
    /// never lets the damage be lost to a later write. Fails, leaving the
    /// file untouched, only if the retry also fails.
    fn ensure_saved(&mut self) -> Result<(), JournalError> {
        if !self.unsaved_damage {
            return Ok(());
        }
        let bytes = std::fs::read(&self.path)?;
        self.save_original(&bytes)?;
        Ok(())
    }

    /// The repairs since the last call: the app tells the user once.
    pub fn take_recovery(&mut self) -> Option<Recovery> {
        self.recovery.take()
    }

    /// Clean exit: catch up, then compact if the file changed since it was last
    /// compacted, or just fsync when this build must not compact this file.
    pub fn close(&mut self) -> Result<(), JournalError> {
        if self.read_only {
            return Ok(());
        }
        let _lock = self.lock()?;
        self.guard(|j| {
            j.catch_up()?;
            if j.keep_unknown {
                sync_data(&j.path)?;
                j.unsynced = false;
                Ok(())
            } else if j.dirty {
                j.compact()
            } else {
                Ok(())
            }
        })
    }

    /// Runs `f`, marking this replica stale if it fails.
    fn guard<T>(&mut self, f: impl FnOnce(&mut Journal) -> Result<T, JournalError>) -> Result<T, JournalError> {
        let result = f(self);
        self.stale |= result.is_err();
        result
    }

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
        doc.prune();
        write_compacted(path, &doc, 0, false)?;
        let _ = std::fs::remove_file(prev_path(path));
        if let Some(state) = local {
            local::write(path, doc.id, state)?;
        }
        Ok(())
    }
}

impl Txn<'_> {
    pub fn doc(&self) -> &Doc {
        &self.journal.doc
    }

    pub fn next_id(&mut self) -> NodeId {
        self.journal.next_id()
    }

    /// A newer build compacted the file: commits are refused. Check it
    /// after `begin` (contract "Read-only").
    pub fn read_only(&self) -> bool {
        self.journal.read_only
    }

    /// Stamps `bodies` as this replica's next ops, applies them, and
    /// appends them to the file. After an error the ops may or may not be
    /// in the file; the next `begin` re-reads it, so diff again against
    /// `doc()` rather than retrying the same bodies.
    pub fn commit(self, bodies: Vec<OpBody>) -> Result<(), JournalError> {
        let Txn { journal, _lock, .. } = self;
        journal.guard(|j| j.write_ops(bodies))
    }
}

impl Journal {
    fn write_ops(&mut self, bodies: Vec<OpBody>) -> Result<(), JournalError> {
        let j = self;
        if j.read_only {
            return Err(JournalError::ReadOnly);
        }
        if bodies.is_empty() {
            return Ok(());
        }
        let now = now_ms();
        let ops: Vec<Op> = bodies
            .into_iter()
            .map(|body| {
                let id = j.next_id();
                Op { id, hlc: j.clock.tick(now), body }
            })
            .collect();
        let mut out = Vec::new();
        encode_frame(&mut out, frame::OPS, &ops_message(&ops));
        j.append(&out)?;
        for op in &ops {
            j.doc.apply(op);
        }
        if j.should_compact() && j.compact().is_err() {
            // The ops are in the file and a failed rewrite leaves it as it
            // was: the commit stands. Re-read once; `close` tries again.
            (j.compaction_failed, j.stale) = (true, true);
        }
        Ok(())
    }
}

/// fsyncs `path`. Opened for writing: on Windows, `FlushFileBuffers` on a
/// read-only handle fails with "Access is denied".
fn sync_data(path: &Path) -> io::Result<()> {
    OpenOptions::new().write(true).open(path)?.sync_data()
}

/// How long taking the session lock may wait for another instance before
/// the caller gets a "busy" error (and the app's usual save-failed toast)
/// instead of a frozen window.
const LOCK_WAIT: Duration = Duration::from_secs(5);

/// Takes the exclusive lock beside `path` (`<path>.lock`), creating it.
fn lock_file(path: &Path) -> Result<File, JournalError> {
    lock_file_within(path, LOCK_WAIT)
}

/// `lock_file`, giving up after `wait` with `WouldBlock`.
fn lock_file_within(path: &Path, wait: Duration) -> Result<File, JournalError> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let file = OpenOptions::new().create(true).truncate(false).write(true).open(with_suffix(path, ".lock"))?;
    let deadline = Instant::now() + wait;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(JournalError::Io(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "the session is busy in another window",
                )))
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
        }
    }
}

/// `open_or_set_aside`'s move, under the lock. The file is read again
/// first: another process starting at the same time may already have set
/// it aside and started a journal, which is kept. Its `.prev` (the best
/// copy to recover from, which the new journal's first compaction would
/// replace) and `.local` move with it, to `<aside>.prev` and
/// `<aside>.local`. Returns where it went.
fn set_aside_unless_journal(path: &Path) -> Result<Option<PathBuf>, JournalError> {
    const COMPANIONS: [&str; 2] = [".prev", ".local"];
    let _lock = lock_file(path)?;
    match std::fs::read(path) {
        Ok(bytes) if matches!(read_readable_header(&bytes), Err(JournalError::NotAJournal)) => {}
        _ => return Ok(None),
    }
    let secs = SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let taken = |aside: &Path| -> io::Result<bool> {
        for suffix in [""].into_iter().chain(COMPANIONS) {
            if with_suffix(aside, suffix).try_exists()? {
                return Ok(true);
            }
        }
        Ok(false)
    };
    let mut aside = with_suffix(path, &format!(".unreadable-{secs}"));
    let mut n = 1;
    while taken(&aside)? {
        aside = with_suffix(path, &format!(".unreadable-{secs}-{n}"));
        n += 1;
    }
    // Companions first: if one can't move, nothing has, and the error
    // leaves the user's files as they were.
    let mut moved = Vec::new();
    for suffix in COMPANIONS {
        match std::fs::rename(with_suffix(path, suffix), with_suffix(&aside, suffix)) {
            Ok(()) => moved.push(suffix),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => {
                undo_companions(path, &aside, &moved);
                return Err(err.into());
            }
        }
    }
    if let Err(err) = std::fs::rename(path, &aside) {
        undo_companions(path, &aside, &moved);
        return Err(err.into());
    }
    Ok(Some(aside))
}

/// Puts back what `set_aside_unless_journal` moved before a later step failed. Best effort.
fn undo_companions(path: &Path, aside: &Path, moved: &[&str]) {
    for suffix in moved {
        let _ = std::fs::rename(with_suffix(aside, suffix), with_suffix(path, suffix));
    }
}

/// `path` with `suffix` appended to its file name (`session.mnf` →
/// `session.mnf.lock`).
pub(crate) fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn file_seen(path: &Path) -> Option<(u64, SystemTime)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()?))
}

/// Writes a compacted file (header, snapshot) beside `path`, fsyncs it,
/// and renames it over `path`. With `rotate`, the file it replaces is kept
/// as `<path>.prev` first, and `prev_generation` is that file's
/// generation. Returns its bytes.
fn write_compacted(path: &Path, doc: &Doc, prev_generation: u64, rotate: bool) -> Result<Vec<u8>, JournalError> {
    let mut bytes = header_bytes(getrandom::u64().expect("the OS random source failed"), prev_generation);
    bytes.extend(snapshot_bytes(doc));
    super::atomic::replace_with(path, &bytes, false, || {
        if rotate {
            keep_prev(path);
        }
    })?;
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
    if path.try_exists()? {
        return Ok(());
    }
    super::atomic::replace(path, bytes)
}

// ---- Inspection (`--dump`, `--check`) --------------------------------------

pub struct Inspect {
    pub version: u64,
    pub generation: u64,
    pub prev_generation: u64,
    pub header_damaged: bool,
    pub file_len: usize,
    /// `(offset, type, size)` of every whole frame.
    pub frames: Vec<(usize, u64, usize)>,
    /// `(start, end)` of each damaged region.
    pub damaged: Vec<(usize, usize)>,
    /// Where a torn tail starts, and why reading stopped there.
    pub torn: Option<(usize, Stop)>,
}

pub fn inspect(bytes: &[u8]) -> Result<Inspect, JournalError> {
    let header = read_header(bytes)?;
    let scan = scan_all(&bytes[header.len..], header.len);
    Ok(Inspect {
        version: header.version,
        generation: header.generation,
        prev_generation: header.prev_generation,
        header_damaged: header.damaged,
        file_len: bytes.len(),
        frames: scan.frames.iter().map(|f| (f.offset, f.ty, f.size)).collect(),
        damaged: scan.damaged,
        torn: scan.stop.map(|stop| (scan.end, stop)),
    })
}

#[cfg(test)]
mod tests;
