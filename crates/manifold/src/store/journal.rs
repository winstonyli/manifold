//! The session file (spec "File and frames", "Journal"): a header, then
//! CRC-checked frames, each readable on its own. Reading resumes past
//! damage and the file is repaired, keeping the original beside it. A
//! torn tail is cut by the next append. Every read and write happens
//! under a lock on a separate `.lock` file, so two processes on one
//! profile take turns and each merges what the other appended.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use super::crc32c::crc32c;
use super::local;
use super::model::{now_ms, ActorId, Clock, Doc, DocId, Hlc, Kind, Node, NodeId, Reg, Stamp};
use super::ops::{Op, OpBody};
use super::tlv::{Reader, TlvError, Writer};
use super::varint;

pub const MAGIC: [u8; 4] = *b"MNF\0";
pub const FORMAT_VERSION: u64 = 1;

/// Frame types. 0 is invalid, so a zeroed region never parses.
pub mod frame {
    pub const SNAPSHOT: u64 = 2;
    pub const OPS: u64 = 3;
    /// Never reused: 1 was a file-wide actor table (each frame now
    /// carries its own), 4 was personal state (now `<session>.local`).
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
    let mut input = Vec::with_capacity(4 + body.len());
    input.extend_from_slice(&len.to_le_bytes());
    input.extend_from_slice(body);
    crc32c(&input)
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
    let valid = |p: usize| p < buf.len() && frame_at(&buf[p..]).is_ok();
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
    let hlc = decode_hlc(buf)?;
    let mut index = 0;
    for field in Reader::new(buf) {
        let field = field?;
        if field.no == 3 {
            index = field.value.u64()?;
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
/// retained Move-log tail, 5: clock, 6: actor*}`. A node is `{1: id, 2:
/// kind, 3: deleted, 4: group, 5: register*}` and a register `{1: field,
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
        if node.kind.code() != 0 {
            n.varint(2, node.kind.code());
        }
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
                        2 => node.kind = Kind::from_code(f.value.u64()?),
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
            OpBody::Create { node, kind, fields } => {
                b.message(1, &id_message(*node, actors));
                b.varint(2, kind.code());
                if !fields.is_empty() {
                    b.bytes(3, fields);
                }
                4
            }
            OpBody::Set { node, field, enc } => {
                b.message(1, &id_message(*node, actors));
                b.varint(2, u64::from(*field));
                if !enc.is_empty() {
                    b.bytes(3, enc);
                }
                5
            }
            OpBody::Move { node, group } => {
                b.message(1, &id_message(*node, actors));
                if let Some(group) = group {
                    b.message(2, &id_message(*group, actors));
                }
                6
            }
            OpBody::Delete { node } => {
                b.message(1, &id_message(*node, actors));
                7
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
                4..=7 => body = Some((f.no, f.value.bytes()?)),
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
                (4 | 5, 2) => num = f.value.u64()?,
                (6, 2) => group = Some(decode_id(f.value.bytes()?, actors)?),
                (4 | 5, 3) => blob = f.value.bytes()?.to_vec(),
                _ => {}
            }
        }
        let node = node.ok_or_else(|| JournalError::Corrupt("op without a node".into()))?;
        let body = match kind {
            4 => OpBody::Create { node, kind: Kind::from_code(num), fields: blob },
            5 => OpBody::Set {
                node,
                field: u32::try_from(num).map_err(|_| JournalError::Corrupt("field number".into()))?,
                enc: blob,
            },
            6 => OpBody::Move { node, group },
            _ => OpBody::Delete { node },
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
        }
    }

    /// Opens `path`, first creating an empty document there if there is
    /// none, and repairs damage (spec "File and frames"). `NotAJournal`
    /// leaves the file as it was.
    pub fn open(path: &Path, actor: ActorId) -> Result<Journal, JournalError> {
        let mut journal = Journal::blank(path, actor);
        let _lock = journal.lock()?;
        if !path.exists() {
            write_compacted(path, &Doc::new(DocId::random()), 0, false)?;
        }
        let bytes = std::fs::read(path)?;
        journal.load(&bytes)?;
        journal.repair(&bytes);
        journal.note_seen();
        Ok(journal)
    }

    /// Reads a session file's document, as `open` would, without a lock or
    /// an actor. For `--dump` and tests.
    pub fn read(bytes: &[u8]) -> Result<Doc, JournalError> {
        let mut journal = Journal::blank(Path::new(""), ActorId(0));
        journal.load(bytes)?;
        Ok(journal.doc)
    }

    pub fn doc(&self) -> &Doc {
        &self.doc
    }

    pub fn actor(&self) -> ActorId {
        self.actor
    }

    pub fn read_only(&self) -> bool {
        self.read_only
    }

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
    /// differ from before, so another replica's compaction alone reports
    /// nothing (pruning a tombstone changes nothing a replica shows).
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
            reload |= scan.stop == Some(Stop::BadCrc) || (scan.stop.is_some() && resync(&bytes, scan.end).is_some());
            if !reload {
                self.end = scan.end as u64;
                self.dirty |= !scan.frames.is_empty() || scan.stop.is_some();
            }
        }
        if reload {
            let before = std::mem::take(&mut self.doc.nodes);
            self.load(&bytes)?;
            self.repair(&bytes);
            let after = &self.doc.nodes;
            let changed = |(id, old): (&NodeId, &Node)| match after.get(id) {
                Some(new) => new != old,
                None => old.is_live(),
            };
            touched = before.iter().filter(|&n| changed(n)).map(|(id, _)| *id).collect();
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

/// Takes the exclusive lock beside `path` (`<path>.lock`), creating it.
fn lock_file(path: &Path) -> Result<File, JournalError> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let file = OpenOptions::new().create(true).truncate(false).write(true).open(with_suffix(path, ".lock"))?;
    file.lock()?;
    Ok(file)
}

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
mod tests {
    use super::*;
    use crate::store::model::{encode_field, field, FieldValue, Node, FIELDS};
    use crate::store::testutil::{shown, temp_dir, XorShift};

    fn create_tab(txn: &mut Txn, url: &str) -> (NodeId, OpBody) {
        let node = txn.next_id();
        let fields = encode_field(field::URL, &FieldValue::Str(url.into()));
        (node, OpBody::Create { node, kind: Kind::Tab, fields })
    }

    fn url(doc: &Doc, id: NodeId) -> FieldValue {
        doc.nodes[&id].get(field::URL)
    }

    #[test]
    fn frames_round_trip_and_damage_stops_the_scan() {
        let mut bytes = Vec::new();
        encode_frame(&mut bytes, frame::SNAPSHOT, b"one");
        encode_frame(&mut bytes, frame::OPS, b"");
        let scan1 = scan(&bytes, 0);
        assert_eq!(scan1.frames.len(), 2);
        assert_eq!((scan1.frames[0].ty, scan1.frames[0].message), (frame::SNAPSHOT, &b"one"[..]));
        assert_eq!((scan1.end, scan1.stop), (bytes.len(), None));

        let mut flipped = bytes.clone();
        flipped[9] ^= 1;
        assert_eq!(scan(&flipped, 0).stop, Some(Stop::BadCrc));
        assert_eq!(scan(&[0u8; 64], 0).stop, Some(Stop::ZeroLength));
        assert!(scan(&[0u8; 64], 0).frames.is_empty());
        let mut zero_type = Vec::new();
        encode_frame(&mut zero_type, 0, b"x");
        assert_eq!(scan(&zero_type, 0).stop, Some(Stop::ZeroType));
        assert_eq!(scan(&bytes[..bytes.len() - 1], 0).stop, Some(Stop::Overrun));
    }

    #[test]
    fn a_cut_at_any_byte_keeps_exactly_the_whole_frames_before_it() {
        let dir = temp_dir("torn");
        let path = dir.join("session.mnf");
        let mut journal = Journal::open(&path, ActorId(1)).unwrap();
        for i in 0..5 {
            let mut txn = journal.begin().unwrap();
            let (_, create) = create_tab(&mut txn, &format!("https://example.com/{i}"));
            txn.commit(vec![create]).unwrap();
        }
        let full = std::fs::read(&path).unwrap();
        let header = read_header(&full).unwrap();
        let whole = scan(&full[header.len..], header.len).frames;
        for cut in header.len..=full.len() {
            let got = scan(&full[header.len..cut], header.len);
            let expected = whole.iter().filter(|f| f.offset + f.size <= cut).count();
            assert_eq!(got.frames.len(), expected, "cut at {cut}");
            // Each commit is one ops frame creating one tab.
            let whole_ops = whole.iter().filter(|f| f.ty == frame::OPS && f.offset + f.size <= cut).count();
            let doc = Journal::read(&full[..cut]).unwrap();
            assert_eq!(doc.live().count(), whole_ops, "cut at {cut}");
        }
    }

    #[test]
    fn reopening_gives_back_the_document() {
        let dir = temp_dir("reopen");
        let path = dir.join("session.mnf");
        let mut journal = Journal::open(&path, ActorId(1)).unwrap();
        let mut txn = journal.begin().unwrap();
        let (tab, create) = create_tab(&mut txn, "https://a.example/");
        txn.commit(vec![create]).unwrap();
        let doc_id = journal.doc().id;
        drop(journal);

        let mut reopened = Journal::open(&path, ActorId(2)).unwrap();
        assert_eq!(reopened.doc().id, doc_id);
        assert_eq!(url(reopened.doc(), tab), FieldValue::Str("https://a.example/".into()));
        // The new replica's ops are stamped after everything in the file.
        let seen = reopened.doc().clock;
        let txn = reopened.begin().unwrap();
        txn.commit(vec![OpBody::Delete { node: tab }]).unwrap();
        assert!(reopened.doc().clock > seen);
    }

    #[test]
    fn a_torn_tail_is_cut_before_the_next_append() {
        let dir = temp_dir("cut");
        let path = dir.join("session.mnf");
        drop(Journal::open(&path, ActorId(1)).unwrap());
        let clean_len = std::fs::metadata(&path).unwrap().len();
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(&[7, 0, 0, 0, 1, 2]).unwrap(); // half a frame
        drop(file);

        let mut journal = Journal::open(&path, ActorId(2)).unwrap();
        assert!(journal.take_recovery().is_none()); // a torn tail is no damage
        let mut txn = journal.begin().unwrap();
        let (tab, create) = create_tab(&mut txn, "https://b.example/");
        txn.commit(vec![create]).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let header = read_header(&bytes).unwrap();
        let scanned = scan(&bytes[header.len..], header.len);
        assert_eq!((scanned.end, scanned.stop), (bytes.len(), None));
        // The new frames start where the half frame did: it was cut.
        assert!(scanned.frames.iter().any(|f| f.offset as u64 == clean_len));
        let doc = Journal::read(&bytes).unwrap();
        assert!(doc.nodes[&tab].is_live());
    }

    #[test]
    fn two_replicas_on_one_file_merge_each_others_ops() {
        let dir = temp_dir("two");
        let path = dir.join("session.mnf");
        let mut a = Journal::open(&path, ActorId(1)).unwrap();
        let mut b = Journal::open(&path, ActorId(2)).unwrap();

        let mut txn = a.begin().unwrap();
        let group = txn.next_id();
        txn.commit(vec![OpBody::Create { node: group, kind: Kind::Group, fields: Vec::new() }]).unwrap();
        let mut txn = b.begin().unwrap();
        assert!(txn.remote.contains(&group));
        let (tab, create) = create_tab(&mut txn, "https://c.example/");
        txn.commit(vec![create, OpBody::Move { node: tab, group: Some(group) }]).unwrap();

        assert!(!b.changed_on_disk() && a.changed_on_disk());
        let txn = a.begin().unwrap();
        assert!(txn.remote.contains(&tab));
        drop(txn);
        assert_eq!(a.doc().nodes, b.doc().nodes);
        assert_eq!(a.doc().nodes[&tab].group, Some(group));
    }

    #[test]
    fn a_compaction_by_one_replica_is_picked_up_by_the_other() {
        let dir = temp_dir("compact");
        let path = dir.join("session.mnf");
        let mut a = Journal::open(&path, ActorId(1)).unwrap();
        let mut b = Journal::open(&path, ActorId(2)).unwrap();
        let mut txn = a.begin().unwrap();
        let (one, create) = create_tab(&mut txn, "https://one.example/");
        txn.commit(vec![create]).unwrap();
        a.close().unwrap(); // compacts while `b` has the file open

        let mut txn = b.begin().unwrap();
        assert!(txn.remote.contains(&one));
        let (two, create) = create_tab(&mut txn, "https://two.example/");
        txn.commit(vec![create]).unwrap();

        let reopened = Journal::open(&path, ActorId(3)).unwrap();
        assert!(reopened.doc().nodes[&one].is_live() && reopened.doc().nodes[&two].is_live());
    }

    #[test]
    fn enough_appends_compact_and_nothing_is_lost() {
        let dir = temp_dir("many");
        let path = dir.join("session.mnf");
        let mut journal = Journal::open(&path, ActorId(1)).unwrap();
        let mut txn = journal.begin().unwrap();
        let (tab, create) = create_tab(&mut txn, "https://0.example/");
        txn.commit(vec![create]).unwrap();
        let long = "x".repeat(1000);
        for i in 0..200 {
            let text = FieldValue::Str(format!("{i}{long}"));
            let txn = journal.begin().unwrap();
            txn.commit(vec![OpBody::Set { node: tab, field: field::URL, enc: encode_field(field::URL, &text) }])
                .unwrap();
        }
        // 200 KB of appends against a tiny snapshot: compacted at least once.
        assert!(std::fs::metadata(&path).unwrap().len() < 100 * 1024);
        let doc = Journal::read(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(url(&doc, tab), FieldValue::Str(format!("199{long}")));
    }

    #[test]
    fn a_failed_compaction_does_not_fail_the_commit() {
        let dir = temp_dir("compact-fail");
        let path = dir.join("session.mnf");
        let mut tmp = path.clone().into_os_string();
        tmp.push(".tmp");
        let mut journal = Journal::open(&path, ActorId(1)).unwrap();
        std::fs::create_dir(&tmp).unwrap(); // the rewrite's temp file can't be created
        let mut txn = journal.begin().unwrap();
        let (tab, create) = create_tab(&mut txn, "https://0.example/");
        txn.commit(vec![create]).unwrap();
        let long = "x".repeat(1000);
        for i in 0..200 {
            let text = FieldValue::Str(format!("{i}{long}"));
            let txn = journal.begin().unwrap();
            txn.commit(vec![OpBody::Set { node: tab, field: field::URL, enc: encode_field(field::URL, &text) }])
                .unwrap();
        }
        assert!(std::fs::metadata(&path).unwrap().len() > 150 * 1024); // never compacted
        assert!(journal.close().is_err());
        std::fs::remove_dir(&tmp).unwrap();
        journal.close().unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() < 100 * 1024);
        let doc = Journal::read(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(url(&doc, tab), FieldValue::Str(format!("199{long}")));
    }

    #[test]
    fn a_newer_builds_frames_and_fields_survive() {
        let dir = temp_dir("newer");
        let path = dir.join("session.mnf");
        let mut journal = Journal::open(&path, ActorId(1)).unwrap();
        let mut txn = journal.begin().unwrap();
        let (tab, create) = create_tab(&mut txn, "https://d.example/");
        let mut future = Writer::new();
        future.str(99, "from the future");
        let future = future.into_bytes();
        txn.commit(vec![create, OpBody::Set { node: tab, field: 99, enc: future.clone() }]).unwrap();
        journal.close().unwrap(); // compacts: the unknown field is written back

        let doc = Journal::read(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(doc.nodes[&tab].fields[&99].enc, future);

        // A frame type from the future: kept, and the file never compacts.
        let mut frame = Vec::new();
        encode_frame(&mut frame, 50, b"new");
        OpenOptions::new().append(true).open(&path).unwrap().write_all(&frame).unwrap();
        let mut journal = Journal::open(&path, ActorId(2)).unwrap();
        let txn = journal.begin().unwrap();
        txn.commit(vec![OpBody::Delete { node: tab }]).unwrap();
        journal.close().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let header = read_header(&bytes).unwrap();
        assert!(scan(&bytes[header.len..], header.len).frames.iter().any(|f| f.ty == 50));
    }

    #[test]
    fn a_newer_builds_structural_fields_block_compaction() {
        let dir = temp_dir("structural");
        let path = dir.join("session.mnf");
        let mut journal = Journal::open(&path, ActorId(1)).unwrap();
        let mut txn = journal.begin().unwrap();
        let (tab, create) = create_tab(&mut txn, "https://e.example/");
        txn.commit(vec![create]).unwrap();
        drop(journal);

        // An ops frame whose top level carries a field this build doesn't know.
        let op = Op {
            id: NodeId { actor: ActorId(1), counter: 100 },
            hlc: Hlc { ms: now_ms() + 1000, logical: 0 },
            body: OpBody::Delete { node: tab },
        };
        let mut message = ops_message(&[op]);
        let mut extra = Writer::new();
        extra.varint(9, 1);
        message.extend(extra.into_bytes());
        let mut frame = Vec::new();
        encode_frame(&mut frame, frame::OPS, &message);
        OpenOptions::new().append(true).open(&path).unwrap().write_all(&frame).unwrap();
        let mut journal = Journal::open(&path, ActorId(2)).unwrap();
        assert!(journal.doc().nodes[&tab].deleted);
        journal.close().unwrap();
        assert!(std::fs::read(&path).unwrap().ends_with(&frame));

        // A snapshot with the reserved Move-log tail (field 4) filled in.
        let doc = Journal::read(&std::fs::read(&path).unwrap()).unwrap();
        let mut bytes = header_bytes(7, 0);
        let mut snapshot = snapshot_message(&doc);
        let mut tail = Writer::new();
        tail.bytes(4, b"tail");
        snapshot.extend(tail.into_bytes());
        encode_frame(&mut bytes, frame::SNAPSHOT, &snapshot);
        std::fs::write(&path, &bytes).unwrap();
        let mut journal = Journal::open(&path, ActorId(3)).unwrap();
        journal.close().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }

    #[test]
    fn a_failed_commit_rereads_the_file_and_keeps_it_readable() {
        let dir = temp_dir("failed");
        let path = dir.join("session.mnf");
        let set_readonly = |readonly: bool| {
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_readonly(readonly);
            std::fs::set_permissions(&path, perms).unwrap();
        };
        let mut journal = Journal::open(&path, ActorId(1)).unwrap();
        set_readonly(true);
        let mut txn = journal.begin().unwrap();
        let (_, create) = create_tab(&mut txn, "https://lost.example/");
        assert!(txn.commit(vec![create]).is_err()); // the file is read-only
        assert!(matches!(journal.save_copy(&dir.join("copy.mnf"), None), Err(JournalError::Stale)));
        set_readonly(false);
        let mut txn = journal.begin().unwrap();
        let (tab, create) = create_tab(&mut txn, "https://kept.example/");
        txn.commit(vec![create]).unwrap();
        let doc = Journal::read(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(doc.nodes.len(), 1);
        assert_eq!(url(&doc, tab), FieldValue::Str("https://kept.example/".into()));
        assert_eq!(journal.doc().nodes, doc.nodes);
    }

    #[test]
    fn compaction_prunes_and_other_replicas_hear_of_it() {
        let dir = temp_dir("prune");
        let path = dir.join("session.mnf");
        let mut a = Journal::open(&path, ActorId(1)).unwrap();
        let mut b = Journal::open(&path, ActorId(2)).unwrap();
        let mut c = Journal::open(&path, ActorId(3)).unwrap();
        let mut txn = a.begin().unwrap();
        let (gone, create_gone) = create_tab(&mut txn, "https://gone.example/");
        let (kept, create_kept) = create_tab(&mut txn, "https://kept.example/");
        txn.commit(vec![create_gone, create_kept]).unwrap();
        drop(c.begin().unwrap()); // c holds `gone` live
        let txn = a.begin().unwrap();
        txn.commit(vec![OpBody::Delete { node: gone }]).unwrap();
        drop(b.begin().unwrap()); // b holds the tombstone
        a.close().unwrap();
        let doc = Journal::read(&std::fs::read(&path).unwrap()).unwrap();
        assert!(!doc.nodes.contains_key(&gone) && doc.nodes[&kept].is_live());
        let txn = c.begin().unwrap();
        assert_eq!(txn.remote, vec![gone]); // missing from the document: deleted
        assert!(!txn.doc().nodes.contains_key(&gone));
        drop(txn);
        // b saw the delete already: the prune shows it nothing new.
        let txn = b.begin().unwrap();
        assert_eq!(txn.remote, vec![]);
        assert!(!txn.doc().nodes.contains_key(&gone));
    }

    /// A session file holding every frame type, op kind and field type:
    /// the seed `damaged_files_never_panic` mutates.
    fn rich_file(path: &Path) -> Vec<u8> {
        let mut journal = Journal::open(path, ActorId(1)).unwrap();
        let mut txn = journal.begin().unwrap();
        let group = txn.next_id();
        let (tab, create) = create_tab(&mut txn, "https://a.example/");
        let note = txn.next_id();
        let mut note_fields = encode_field(field::POS, &FieldValue::Pos(1.5, -2.0));
        note_fields.extend(encode_field(field::COLOR, &FieldValue::Color(Some(0x12ab34))));
        note_fields.extend(encode_field(field::GROUP, &FieldValue::Id(Some(group))));
        note_fields.extend(encode_field(field::TEXT, &FieldValue::Str("hi".into())));
        let set = |node, no, value| OpBody::Set { node, field: no, enc: encode_field(no, &value) };
        let bodies = vec![
            OpBody::Create {
                node: group,
                kind: Kind::Group,
                fields: encode_field(field::NAME, &FieldValue::Str("g".into())),
            },
            create,
            OpBody::Create { node: note, kind: Kind::Note, fields: note_fields },
            set(tab, field::ENGINE, FieldValue::Engine(2)),
            set(tab, field::PINNED, FieldValue::Bool(true)),
            OpBody::Move { node: tab, group: Some(group) },
        ];
        txn.commit(bodies).unwrap();
        journal.close().unwrap(); // a snapshot

        let mut journal = Journal::open(path, ActorId(2)).unwrap();
        let mut txn = journal.begin().unwrap();
        let leaf = txn.next_id();
        let mut leaf_fields = encode_field(field::PARENT, &FieldValue::Id(Some(tab)));
        leaf_fields.extend(encode_field(field::KEY, &FieldValue::Str("k".into())));
        leaf_fields.extend(encode_field(field::VALUE, &FieldValue::Str("v".into())));
        let bodies = vec![
            OpBody::Create { node: leaf, kind: Kind::JsonLeaf, fields: leaf_fields },
            OpBody::Delete { node: group },
            OpBody::Move { node: note, group: None },
            set(note, field::COLOR, FieldValue::Color(None)),
        ];
        txn.commit(bodies).unwrap();
        drop(journal);
        std::fs::read(path).unwrap()
    }

    /// Damages one frame's message (keeping its CRC valid, so the damage
    /// reaches the decoders), and sometimes the file itself: a bit, a cut, a
    /// zeroed or random run, or a run copied elsewhere.
    fn damage(rng: &mut XorShift, bytes: &[u8]) -> Vec<u8> {
        let header = read_header(bytes).unwrap();
        let frames = scan(&bytes[header.len..], header.len).frames;
        let target = rng.below(frames.len() as u64) as usize;
        let mut out = bytes[..header.len].to_vec();
        for (i, frame) in frames.iter().enumerate() {
            let (mut ty, mut message) = (frame.ty, frame.message.to_vec());
            for _ in 0..if i == target { 1 + rng.below(4) } else { 0 } {
                let at = rng.below(message.len() as u64 + 1) as usize;
                match rng.below(6) {
                    0 if at < message.len() => message[at] ^= 1 << rng.below(8),
                    1 if at < message.len() => message[at] = rng.next() as u8,
                    2 => message.insert(at, rng.next() as u8),
                    3 if at < message.len() => drop(message.remove(at)),
                    4 => message.truncate(at),
                    5 => ty = 1 + rng.below(4),
                    _ => {}
                }
            }
            encode_frame(&mut out, ty, &message);
        }
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
        out
    }

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

    /// Everything a reader of `bytes` might do. Errors are fine; panics
    /// are not, and what this build reads it can write and read back.
    fn use_damaged(bytes: &[u8], path: &Path, open: bool) {
        let _ = inspect(bytes);
        if let Ok(doc) = Journal::read(bytes) {
            doc.with_descendants(doc.nodes.keys().copied().collect());
            for (&id, node) in &doc.nodes {
                doc.effective_group(id);
                node.message();
                for def in &FIELDS {
                    node.get(def.no);
                }
            }
            let mut again = header_bytes(1, 0);
            again.extend(snapshot_bytes(&doc));
            let back = Journal::read(&again).expect("a decoded document's snapshot reads back");
            assert_eq!((&back.nodes, &back.version), (&doc.nodes, &doc.version));
            let text = serde_json::to_string(&crate::store::json::doc_to_json(&doc)).unwrap();
            let json: crate::store::json::JsonDoc = serde_json::from_str(&text).unwrap();
            let mut counter = 0;
            let fresh = || {
                counter += 1;
                NodeId { actor: ActorId(9), counter }
            };
            crate::store::json::import(&json, fresh).expect("an export imports");
        }
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
    }

    #[test]
    fn damaged_files_never_panic() {
        let dir = temp_dir("damaged");
        let seed = rich_file(&dir.join("seed.mnf"));
        let path = dir.join("session.mnf");
        let mut rng = XorShift(0x5EED);
        for i in 0..10_000 {
            let bytes = damage(&mut rng, &seed);
            let run =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| use_damaged(&bytes, &path, i % 20 == 0)));
            if run.is_err() {
                let keep = dir.join(format!("crash-{i}.mnf"));
                std::fs::write(&keep, &bytes).unwrap();
                panic!("damaged file {i} panicked; kept at {}", keep.display());
            }
        }
    }

    #[test]
    fn a_newer_format_version_opens_read_only() {
        let dir = temp_dir("version");
        let path = dir.join("session.mnf");
        drop(Journal::open(&path, ActorId(1)).unwrap());
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[4] = 2; // format_version 2
        let crc = crc32c(&bytes[..21]);
        bytes[21..25].copy_from_slice(&crc.to_le_bytes());
        std::fs::write(&path, &bytes).unwrap();
        let mut journal = Journal::open(&path, ActorId(2)).unwrap();
        assert!(journal.read_only());
        let txn = journal.begin().unwrap();
        assert!(matches!(txn.commit(vec![]), Err(JournalError::ReadOnly)));
        assert!(matches!(journal.write_local(b"x"), Err(JournalError::ReadOnly)));
        assert!(!local::path_for(&path).exists());
        journal.close().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }

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

        // Save As keeps no `.prev` and links none, dropping a stale one.
        let copy = dir.join("copy.mnf");
        std::fs::write(prev_path(&copy), b"stale").unwrap();
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

    #[test]
    fn not_a_journal_is_refused_and_left_alone() {
        let dir = temp_dir("json");
        let path = dir.join("session.mnf");
        std::fs::write(&path, b"{\"tabs\": []}").unwrap();
        assert!(matches!(Journal::open(&path, ActorId(1)), Err(JournalError::NotAJournal)));
        assert_eq!(std::fs::read(&path).unwrap(), b"{\"tabs\": []}");
    }

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
        txn.commit(vec![OpBody::Create { node: group, kind: Kind::Group, fields: Vec::new() }]).unwrap();
        let mut txn = b.begin().unwrap();
        let (tab, create) = create_tab(&mut txn, "https://f.example/");
        txn.commit(vec![create, OpBody::Move { node: tab, group: Some(group) }]).unwrap();

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

    #[test]
    fn inspect_reports_frames_and_the_torn_tail() {
        let dir = temp_dir("inspect");
        let path = dir.join("session.mnf");
        drop(Journal::open(&path, ActorId(1)).unwrap());
        let mut bytes = std::fs::read(&path).unwrap();
        let clean = bytes.len();
        bytes.extend_from_slice(&[0; 16]);
        let report = inspect(&bytes).unwrap();
        assert_eq!(report.frames.iter().map(|f| f.1).collect::<Vec<_>>(), [frame::SNAPSHOT]);
        assert_eq!(report.torn, Some((clean, Stop::ZeroLength)));
    }

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
        let frames = |path: &Path| {
            inspect(&std::fs::read(path).unwrap()).unwrap().frames.iter().map(|f| f.1).collect::<Vec<_>>()
        };

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
    fn a_prev_with_the_wrong_generation_is_not_used() {
        let dir = temp_dir("prev-mismatch");
        let path = dir.join("session.mnf");
        let bytes = layered_file(&path);
        let original = Journal::read(&bytes).unwrap();
        let (offset, size) = frame_spans(&bytes)[0];
        let mut damaged = bytes.clone();
        damaged[offset + size - 1] ^= 1;
        std::fs::write(&path, &damaged).unwrap();

        // `.prev` from another journal entirely: a different generation,
        // and its tabs are foreign to this document.
        let foreign_dir = temp_dir("prev-mismatch-foreign");
        let foreign_bytes = layered_file(&foreign_dir.join("other.mnf"));
        std::fs::write(prev_path(&path), &foreign_bytes).unwrap();

        let mut journal = Journal::open(&path, ActorId(3)).unwrap();
        let recovery = journal.take_recovery().unwrap();
        assert!(!recovery.from_prev && recovery.new_document);
        assert_ne!(journal.doc().id, original.id);
        assert_ne!(journal.doc().id, Journal::read(&foreign_bytes).unwrap().id);
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

    #[test]
    fn a_failed_save_blocks_rewrite_and_truncation_until_retried() {
        let dir = temp_dir("unsaved-damage");
        let path = dir.join("session.mnf");
        let bytes = layered_file(&path);
        let (offset, size) = frame_spans(&bytes)[1]; // not the file's last frame
        let mut damaged = bytes.clone();
        damaged[offset + size - 1] ^= 1;
        std::fs::write(&path, &damaged).unwrap();

        // Block `save_damaged`'s temp file with a directory of the same name.
        let saved = damaged_path(&path, &damaged);
        let tmp = with_suffix(&saved, ".tmp");
        std::fs::create_dir_all(&tmp).unwrap();

        let mut journal = Journal::open(&path, ActorId(3)).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), damaged); // the repair's rewrite never ran
        let recovery = journal.take_recovery().unwrap();
        assert!(recovery.saved.is_none());

        // A commit is refused rather than truncating the unsaved original away.
        let mut txn = journal.begin().unwrap();
        let (_, create) = create_tab(&mut txn, "https://blocked.example/");
        assert!(txn.commit(vec![create]).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), damaged);

        // Once the save can succeed, the next operation retries it and proceeds.
        std::fs::remove_dir(&tmp).unwrap();
        let mut txn = journal.begin().unwrap();
        let (_, create) = create_tab(&mut txn, "https://ok.example/");
        txn.commit(vec![create]).unwrap();
        assert_eq!(std::fs::read(&saved).unwrap(), damaged);
        assert_ne!(std::fs::read(&path).unwrap(), damaged);
    }

    #[test]
    fn a_clean_reload_clears_unsaved_damage() {
        let dir = temp_dir("unsaved-damage-cleared");
        let path = dir.join("session.mnf");
        let bytes = layered_file(&path);
        let (offset, size) = frame_spans(&bytes)[1]; // not the file's last frame
        let mut damaged = bytes.clone();
        damaged[offset + size - 1] ^= 1;
        std::fs::write(&path, &damaged).unwrap();

        // Block this journal's own save, so it opens with the damage
        // unsaved and never rewritten.
        let saved = damaged_path(&path, &damaged);
        let tmp = with_suffix(&saved, ".tmp");
        std::fs::create_dir_all(&tmp).unwrap();
        let mut journal = Journal::open(&path, ActorId(3)).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), damaged);
        journal.take_recovery();

        // Another replica repairs the file cleanly.
        std::fs::remove_dir(&tmp).unwrap();
        drop(Journal::open(&path, ActorId(4)).unwrap());
        assert_eq!(std::fs::read(&saved).unwrap(), damaged);
        assert_ne!(std::fs::read(&path).unwrap(), damaged);
        let count = |dir: &Path| {
            std::fs::read_dir(dir)
                .unwrap()
                .filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().contains(".damaged-"))
                .count()
        };
        let before = count(&dir);

        // The stale journal's next commit sees the file is now clean and
        // doesn't save a spurious, already-clean copy aside.
        let mut txn = journal.begin().unwrap();
        let (_, create) = create_tab(&mut txn, "https://ok.example/");
        txn.commit(vec![create]).unwrap();
        assert_eq!(count(&dir), before);
    }

    #[test]
    fn take_recovery_returns_none_the_second_time() {
        let dir = temp_dir("recovery-once");
        let path = dir.join("session.mnf");
        let bytes = layered_file(&path);
        let (offset, size) = frame_spans(&bytes)[1];
        let mut damaged = bytes.clone();
        damaged[offset + size - 1] ^= 1;
        std::fs::write(&path, &damaged).unwrap();
        let mut journal = Journal::open(&path, ActorId(3)).unwrap();
        assert!(journal.take_recovery().is_some());
        assert!(journal.take_recovery().is_none());
    }

    // Tests with real processes. Each child is this test binary running the
    // ignored `child_writer` test, told what to do through the environment.

    /// Long enough that a few hundred commits pass `COMPACT_MIN`, so the
    /// writers compact while the other one is running.
    fn child_url(actor: u64, i: u64) -> String {
        format!("https://p{actor}.example/{i}/{}", "x".repeat(400))
    }

    fn spawn_writer(path: &Path, actor: u64, count: u64) -> std::process::Child {
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["store::journal::tests::child_writer", "--exact", "--ignored", "--nocapture", "--test-threads=1"])
            .env("MANIFOLD_STORE_CHILD", path)
            .env("MANIFOLD_STORE_ACTOR", actor.to_string())
            .env("MANIFOLD_STORE_COUNT", count.to_string())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap()
    }

    fn live_urls(doc: &Doc) -> std::collections::BTreeSet<String> {
        doc.live()
            .filter_map(|(_, node)| match node.get(field::URL) {
                FieldValue::Str(url) => Some(url),
                _ => None,
            })
            .collect()
    }

    /// Commits one tab at a time and prints `committed <i>` after each
    /// commit returns; `count` 0 means until killed. Run only as a child.
    #[test]
    #[ignore = "a child process of the multi-process tests"]
    fn child_writer() {
        let var = |name| std::env::var(name).unwrap();
        let path = PathBuf::from(var("MANIFOLD_STORE_CHILD"));
        let actor: u64 = var("MANIFOLD_STORE_ACTOR").parse().unwrap();
        let count: u64 = var("MANIFOLD_STORE_COUNT").parse().unwrap();
        let mut journal = Journal::open(&path, ActorId(actor)).unwrap();
        let mut i = 0;
        while count == 0 || i < count {
            let mut txn = journal.begin().unwrap();
            let (_, create) = create_tab(&mut txn, &child_url(actor, i));
            txn.commit(vec![create]).unwrap();
            println!("committed {i}");
            i += 1;
        }
        journal.close().unwrap();
    }

    #[test]
    fn two_processes_writing_one_file_lose_nothing() {
        let dir = temp_dir("processes");
        let path = dir.join("session.mnf");
        Journal::open(&path, ActorId(1)).unwrap().close().unwrap();
        let children = [spawn_writer(&path, 2, 200), spawn_writer(&path, 3, 200)];
        for child in children {
            assert!(child.wait_with_output().unwrap().status.success());
        }
        let mut journal = Journal::open(&path, ActorId(4)).unwrap();
        assert_eq!(journal.take_recovery(), None);
        let expected = (0..200).flat_map(|i| [child_url(2, i), child_url(3, i)]).collect();
        assert_eq!(live_urls(journal.doc()), expected);
        // 400 tabs of ~450 bytes: both writers compacted along the way.
        assert!(prev_path(&path).exists());
    }

    #[test]
    fn a_writer_killed_mid_commit_loses_at_most_that_commit() {
        use std::io::BufRead;
        let dir = temp_dir("killed");
        let path = dir.join("session.mnf");
        let mut rng = XorShift(0x4B11);
        let mut committed = std::collections::BTreeSet::new();
        for round in 0..8u64 {
            let actor = 10 + round;
            let mut child = spawn_writer(&path, actor, 0);
            let mut lines = std::io::BufReader::new(child.stdout.take().unwrap()).lines();
            // The harness prints `test child_writer ... ` without a newline, so
            // `committed 0` shares its line: parse indices rather than count lines.
            let index = |line: &str| line.rsplit_once("committed ").and_then(|(_, i)| i.parse::<u64>().ok());
            let mut reported = 0;
            let stop_after = 1 + rng.below(40);
            while reported < stop_after {
                reported = reported.max(index(&lines.next().unwrap().unwrap()).map_or(0, |i| i + 1));
            }
            child.kill().unwrap();
            child.wait().unwrap();
            // Commits printed after `stop_after` but before the process died.
            reported = lines.map_while(Result::ok).filter_map(|l| index(&l)).fold(reported, |n, i| n.max(i + 1));
            committed.extend((0..reported).map(|i| child_url(actor, i)));

            let mut journal = Journal::open(&path, ActorId(99)).unwrap();
            let urls = live_urls(journal.doc());
            assert!(urls.is_superset(&committed), "round {round}: a reported commit is missing");
            assert!(urls.len() <= committed.len() + 1, "round {round}: more than the in-flight commit landed");
            // The in-flight commit, if it landed, stays.
            committed = urls;
            // A kill leaves at most a torn tail, never damage worth a report.
            assert_eq!(journal.take_recovery(), None, "round {round}");
            journal.close().unwrap();
        }
    }

    // --- Save-path costs (Plan B measures these before wiring saves) ---

    /// Best and median of `n` timings of `f`.
    fn timed(n: usize, mut f: impl FnMut()) -> (Duration, Duration) {
        let mut times: Vec<Duration> = (0..n)
            .map(|_| {
                let start = Instant::now();
                f();
                start.elapsed()
            })
            .collect();
        times.sort();
        (times[0], times[n / 2])
    }

    fn report(what: &str, (best, median): (Duration, Duration)) {
        println!("  {what:<44} best {best:>10.1?}  median {median:>10.1?}");
    }

    /// What each save costs at realistic session sizes, with the file at
    /// its largest (just below compaction), since `catch_up` reads the
    /// whole file on every `begin`. Timing, not correctness, so ignored:
    /// `cargo test --release -j 12 -p manifold -- --ignored --nocapture save_path_costs`
    #[test]
    #[ignore]
    fn save_path_costs() {
        const N: usize = 40;
        for tabs in [300usize, 3000] {
            let dir = temp_dir(&format!("cost-{tabs}"));
            let path = dir.join("session.mnf");
            let mut a = Journal::open(&path, ActorId::random()).unwrap();
            let mut txn = a.begin().unwrap();
            let groups: Vec<NodeId> = (0..tabs / 10).map(|_| txn.next_id()).collect();
            let mut bodies: Vec<OpBody> = groups
                .iter()
                .enumerate()
                .map(|(i, &node)| {
                    let fields = encode_field(field::NAME, &FieldValue::Str(format!("Group {i}")));
                    OpBody::Create { node, kind: Kind::Group, fields }
                })
                .collect();
            let mut ids = Vec::new();
            for i in 0..tabs {
                let node = txn.next_id();
                let url =
                    format!("https://www.example{}.com/articles/2026/09/some-long-article-slug-{i}?ref=home", i % 50);
                let mut fields = encode_field(field::URL, &FieldValue::Str(url));
                fields.extend(encode_field(field::POS, &FieldValue::Pos(i as f32 * 3.5, i as f32 * -2.25)));
                fields.extend(encode_field(field::ENGINE, &FieldValue::Engine(1 + (i % 4) as u64)));
                fields.extend(encode_field(field::GROUP, &FieldValue::Id(Some(groups[i % groups.len()]))));
                bodies.push(OpBody::Create { node, kind: Kind::Tab, fields });
                ids.push(node);
            }
            txn.commit(bodies).unwrap();
            let txn = a.begin().unwrap();
            txn.journal.compact().unwrap();
            drop(txn);
            let snapshot = std::fs::metadata(&path).unwrap().len();

            // Grow the tail with single-op saves (a drag end each) to just
            // below the compaction threshold: the largest file `begin` reads.
            let set_pos = |node: NodeId, x: f32| OpBody::Set {
                node,
                field: field::POS,
                enc: encode_field(field::POS, &FieldValue::Pos(x, x)),
            };
            let margin = 8 * 1024;
            let mut k = 0;
            while a.since_snapshot + margin < (2 * a.snapshot_len).max(COMPACT_MIN) {
                let txn = a.begin().unwrap();
                txn.commit(vec![set_pos(ids[k % tabs], k as f32)]).unwrap();
                k += 1;
            }
            let largest = std::fs::metadata(&path).unwrap().len();
            println!("{tabs} tabs: snapshot {snapshot} B, largest file {largest} B ({k} saves in the tail)");

            report("open (startup)", timed(10, || drop(Journal::open(&path, ActorId::random()).unwrap())));
            report("changed_on_disk, unchanged (per frame)", timed(N, || assert!(!a.changed_on_disk())));
            report("begin, nothing new (per save)", timed(N, || drop(a.begin().unwrap())));
            // Stay below compaction: ~30 B per op against an 8 KB margin.
            let mut x = 0.0;
            report(
                "begin + commit one Set, fsync debounced",
                timed(N, || {
                    x += 1.0;
                    a.begin().unwrap().commit(vec![set_pos(ids[0], x)]).unwrap();
                }),
            );
            let mut synced: Vec<Duration> = (0..5)
                .map(|_| {
                    std::thread::sleep(SYNC_DEBOUNCE); // the next append fsyncs
                    x += 1.0;
                    let start = Instant::now();
                    a.begin().unwrap().commit(vec![set_pos(ids[1], x)]).unwrap();
                    start.elapsed()
                })
                .collect();
            synced.sort();
            report("begin + commit one Set, with fsync", (synced[0], synced[2]));

            // Another instance's edits: B appends one op, A catches up.
            let mut b = Journal::open(&path, ActorId::random()).unwrap();
            let mut incremental = Vec::new();
            let mut reload = Vec::new();
            let mut compact = Vec::new();
            for i in 0..10 {
                x += 1.0;
                b.begin().unwrap().commit(vec![set_pos(ids[2], x)]).unwrap();
                let start = Instant::now();
                let txn = a.begin().unwrap();
                incremental.push(start.elapsed());
                assert_eq!(txn.remote, vec![ids[2]], "sample {i}");
                drop(txn);

                // B compacts: A reloads the whole file.
                let txn = b.begin().unwrap();
                let start = Instant::now();
                txn.journal.compact().unwrap();
                compact.push(start.elapsed());
                drop(txn);
                let start = Instant::now();
                let txn = a.begin().unwrap();
                reload.push(start.elapsed());
                assert_eq!(txn.remote, vec![], "a compaction alone changes nothing A shows");
            }
            for (what, mut v) in [
                ("begin after another appended one op", incremental),
                ("compact (under the lock)", compact),
                ("begin after another compacted (reload)", reload),
            ] {
                v.sort();
                report(what, (v[0], v[v.len() / 2]));
            }
            let bytes = std::fs::read(&path).unwrap();
            let header = read_header(&bytes).unwrap();
            let snapshot = &scan_all(&bytes[header.len..], header.len).frames[0];
            report("  of which: decode the snapshot", timed(10, || drop(decode_snapshot(snapshot.message).unwrap())));
            let doc = decode_snapshot(snapshot.message).unwrap();
            report("  of which: re-encode it (unknown-field check)", timed(10, || drop(snapshot_message(&doc))));
        }
    }
}
