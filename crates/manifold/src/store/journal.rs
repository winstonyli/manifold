//! The session file (spec "File and frames", "Journal"): a header, then
//! CRC-checked frames. Reading stops at the first damaged frame, a torn
//! tail, which the next append cuts off. Every read and write happens
//! under a lock on a separate `.lock` file, so two processes on one
//! profile take turns and each merges what the other appended.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use super::crc32c::crc32c;
use super::model::{now_ms, ActorId, Clock, Doc, DocId, Hlc, Kind, Node, NodeId, Reg, Stamp};
use super::ops::{Op, OpBody};
use super::tlv::{Reader, TlvError, Writer};
use super::varint;

pub const MAGIC: [u8; 4] = *b"MNF\0";
pub const FORMAT_VERSION: u64 = 1;

/// Frame types. 0 is invalid, so a zeroed region never parses.
pub mod frame {
    pub const ACTORS: u64 = 1;
    pub const SNAPSHOT: u64 = 2;
    pub const OPS: u64 = 3;
    pub const LOCAL: u64 = 4;
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
    /// A whole, CRC-valid frame whose contents don't decode: a bug.
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

// ---- Header ---------------------------------------------------------------

pub struct Header {
    pub version: u64,
    /// Random per compacted file: a changed generation means another
    /// process replaced the file, so it must be read again from the start.
    pub generation: u64,
    pub len: usize,
}

pub fn read_header(buf: &[u8]) -> Result<Header, JournalError> {
    if buf.get(..4) != Some(&MAGIC[..]) {
        return Err(JournalError::NotAJournal);
    }
    let (version, n) = varint::read(&buf[4..]).map_err(|_| JournalError::NotAJournal)?;
    let at = 4 + n;
    let generation = buf.get(at..at + 8).ok_or(JournalError::NotAJournal)?;
    Ok(Header { version, generation: u64::from_le_bytes(generation.try_into().expect("8 bytes")), len: at + 8 })
}

fn header_bytes(generation: u64) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    varint::write(&mut out, FORMAT_VERSION);
    out.extend_from_slice(&generation.to_le_bytes());
    out
}

// ---- Messages -------------------------------------------------------------

/// The file's actor table: frames name actors by their index here.
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

fn actors_message(actors: &[ActorId]) -> Vec<u8> {
    let mut w = Writer::new();
    for actor in actors {
        w.fixed64(1, actor.0);
    }
    w.into_bytes()
}

fn decode_actors(buf: &[u8]) -> Result<Vec<ActorId>, JournalError> {
    let mut actors = Vec::new();
    for field in Reader::new(buf) {
        let field = field?;
        if field.no == 1 {
            actors.push(ActorId(field.value.fixed64()?));
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
/// retained Move-log tail, 5: clock}`. A node is `{1: id, 2: kind,
/// 3: deleted, 4: group, 5: register*}` and a register `{1: field,
/// 2: stamp, 3: value encoding}`. Nodes and registers are in id and
/// field order, so equal documents encode to equal bytes.
fn snapshot_message(doc: &Doc, actors: &Actors) -> Vec<u8> {
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
    w.into_bytes()
}

fn decode_snapshot(buf: &[u8], actors: &Actors) -> Result<Doc, JournalError> {
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
                            let stamp = stamp.ok_or_else(|| JournalError::Corrupt("register without a stamp".into()))?;
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

/// Actors and snapshot frames for `doc`, as a compacted file holds them.
/// Equal documents give equal bytes.
pub fn snapshot_bytes(doc: &Doc) -> Vec<u8> {
    let actors = doc_actors(doc);
    let mut out = Vec::new();
    encode_frame(&mut out, frame::ACTORS, &actors_message(&actors.list));
    encode_frame(&mut out, frame::SNAPSHOT, &snapshot_message(doc, &actors));
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

/// Ops: `{1: author's actor index, 2: op*}`. An op is `{1: counter delta,
/// 2: ms delta (both zigzag, from the previous op in the frame), 3:
/// logical, then one body field: 4 Create {1: node, 2: kind, 3: fields},
/// 5 Set {1: node, 2: field, 3: value encoding}, 6 Move {1: node, 2:
/// group}, 7 Delete {1: node}}`.
fn ops_message(ops: &[Op], actors: &Actors) -> Vec<u8> {
    let author = ops[0].id.actor;
    let mut w = Writer::new();
    let index = actors.index(author);
    if index != 0 {
        w.varint(1, index);
    }
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
fn decode_ops(buf: &[u8], actors: &Actors) -> Result<Vec<Op>, JournalError> {
    let mut author = 0;
    for field in Reader::new(buf) {
        let field = field?;
        if field.no == 1 {
            author = field.value.u64()?;
        }
    }
    let author = actors.get(author)?;
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
    actors: Actors,
    doc: Doc,
    local: Option<Vec<u8>>,
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
            actors: Actors::default(),
            doc: Doc::new(DocId([0; 16])),
            local: None,
            snapshot_len: 0,
            since_snapshot: 0,
            keep_unknown: false,
            stale: false,
            compaction_failed: false,
            read_only: false,
            seen: None,
            last_sync: Instant::now(),
            unsynced: false,
        }
    }

    /// Opens `path`, first creating an empty document there if there is
    /// none. `NotAJournal` and `Corrupt` leave the file as it was.
    pub fn open(path: &Path, actor: ActorId) -> Result<Journal, JournalError> {
        let mut journal = Journal::blank(path, actor);
        let _lock = journal.lock()?;
        if !path.exists() {
            write_compacted(path, &Doc::new(DocId::random()), None)?;
        }
        journal.load(&std::fs::read(path)?)?;
        journal.note_seen();
        Ok(journal)
    }

    /// Reads a session file's document and personal state, as `open`
    /// would, without a lock or an actor. For `--dump` and tests.
    pub fn read(bytes: &[u8]) -> Result<(Doc, Option<Vec<u8>>), JournalError> {
        let mut journal = Journal::blank(Path::new(""), ActorId(0));
        journal.load(bytes)?;
        Ok((journal.doc, journal.local))
    }

    pub fn doc(&self) -> &Doc {
        &self.doc
    }

    /// The latest personal state (spec "Personal state"), as written.
    pub fn local(&self) -> Option<&[u8]> {
        self.local.as_deref()
    }

    pub fn actor(&self) -> ActorId {
        self.actor
    }

    pub fn read_only(&self) -> bool {
        self.read_only
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

    /// Replaces this replica's state with `bytes`, a whole file.
    fn load(&mut self, bytes: &[u8]) -> Result<(), JournalError> {
        let header = read_header(bytes)?;
        self.generation = header.generation;
        self.read_only = header.version > FORMAT_VERSION;
        self.keep_unknown = self.read_only;
        self.stale = false;
        self.actors = Actors::default();
        self.doc = Doc::new(DocId([0; 16]));
        self.local = None;
        self.snapshot_len = 0;
        self.since_snapshot = 0;
        let scan = scan(&bytes[header.len..], header.len);
        for frame in &scan.frames {
            self.take(frame)?;
        }
        self.end = scan.end as u64;
        Ok(())
    }

    /// Applies one frame read from the file.
    fn take(&mut self, frame: &RawFrame) -> Result<Vec<NodeId>, JournalError> {
        let size = frame.size as u64;
        match frame.ty {
            frame::SNAPSHOT => {
                (self.snapshot_len, self.since_snapshot) = (size, 0);
            }
            _ => self.since_snapshot += size,
        }
        match frame.ty {
            frame::ACTORS => {
                let actors = decode_actors(frame.message)?;
                self.keep_unknown |= actors_message(&actors) != frame.message;
                for actor in actors {
                    self.actors.push(actor);
                }
                Ok(Vec::new())
            }
            frame::SNAPSHOT => {
                let before: Vec<NodeId> = self.doc.nodes.keys().copied().collect();
                self.doc = decode_snapshot(frame.message, &self.actors)?;
                self.keep_unknown |= snapshot_message(&self.doc, &self.actors) != frame.message;
                self.clock.observe(self.doc.clock);
                Ok(before.into_iter().chain(self.doc.nodes.keys().copied()).collect())
            }
            frame::OPS => {
                // Encodings are canonical, so a frame that doesn't re-encode
                // to itself held a field or op kind this build skipped.
                let ops = decode_ops(frame.message, &self.actors)?;
                self.keep_unknown |= ops.is_empty() || ops_message(&ops, &self.actors) != frame.message;
                let mut touched = Vec::new();
                for op in &ops {
                    self.clock.observe(op.hlc);
                    touched.extend(self.doc.apply(op));
                }
                Ok(touched)
            }
            frame::LOCAL => {
                self.local = Some(frame.message.to_vec());
                Ok(Vec::new())
            }
            _ => {
                self.keep_unknown = true; // a newer build's frame: kept, not read
                Ok(Vec::new())
            }
        }
    }

    /// Reads the frames other processes appended, or the whole file again
    /// if another process compacted it. Returns the nodes they touched.
    fn catch_up(&mut self) -> Result<Vec<NodeId>, JournalError> {
        let bytes = std::fs::read(&self.path)?;
        let header = read_header(&bytes)?;
        let touched = if self.stale || header.generation != self.generation || (bytes.len() as u64) < self.end {
            let before: Vec<NodeId> = self.doc.nodes.keys().copied().collect();
            self.load(&bytes)?;
            before.into_iter().chain(self.doc.nodes.keys().copied()).collect()
        } else {
            let scan = scan(&bytes[self.end as usize..], self.end as usize);
            let mut touched = Vec::new();
            for frame in &scan.frames {
                touched.extend(self.take(frame)?);
            }
            self.end = scan.end as u64;
            touched
        };
        self.note_seen();
        Ok(self.doc.with_descendants(touched.into_iter().collect()).into_iter().collect())
    }

    fn append(&mut self, bytes: &[u8]) -> Result<(), JournalError> {
        let mut file = OpenOptions::new().write(true).open(&self.path)?;
        file.set_len(self.end)?; // cuts a torn tail
        file.seek(SeekFrom::Start(self.end))?;
        file.write_all(bytes)?;
        self.end += bytes.len() as u64;
        self.since_snapshot += bytes.len() as u64;
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

    /// Rewrites the file as one snapshot (spec "Compact"). Holds the lock.
    fn compact(&mut self) -> Result<(), JournalError> {
        self.doc.forget_moves();
        let bytes = write_compacted(&self.path, &self.doc, self.local.as_deref())?;
        let doc = std::mem::replace(&mut self.doc, Doc::new(DocId([0; 16])));
        self.load(&bytes)?;
        debug_assert_eq!(self.doc.nodes, doc.nodes);
        self.note_seen();
        self.unsynced = false;
        Ok(())
    }

    /// Clean exit: catch up, then compact, or just fsync when this build
    /// must not compact this file.
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
            } else {
                j.compact()
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
    /// (spec "Named sessions": Save As), under `path`'s lock. Refused while
    /// this replica may not match its own file. Not for its own path.
    pub fn save_copy(&self, path: &Path) -> Result<(), JournalError> {
        if self.stale {
            return Err(JournalError::Stale);
        }
        let _lock = lock_file(path)?;
        let mut doc = self.doc.clone();
        doc.id = DocId::random();
        doc.forget_moves();
        write_compacted(path, &doc, self.local.as_deref())?;
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
    /// appends them, and `local` when given, to the file. After an error
    /// the ops may or may not be in the file; the next `begin` re-reads it,
    /// so diff again against `doc()` rather than retrying the same bodies.
    pub fn commit(self, bodies: Vec<OpBody>, local: Option<Vec<u8>>) -> Result<(), JournalError> {
        let Txn { journal, _lock, .. } = self;
        journal.guard(|j| j.write_ops(bodies, local))
    }
}

impl Journal {
    fn write_ops(&mut self, bodies: Vec<OpBody>, local: Option<Vec<u8>>) -> Result<(), JournalError> {
        let j = self;
        if j.read_only {
            return Err(JournalError::ReadOnly);
        }
        if bodies.is_empty() && local.is_none() {
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
        let mut new_actors: Vec<ActorId> = Vec::new();
        for actor in ops.iter().flat_map(op_actors) {
            if !j.actors.index.contains_key(&actor) && !new_actors.contains(&actor) {
                new_actors.push(actor);
            }
        }
        if !new_actors.is_empty() {
            encode_frame(&mut out, frame::ACTORS, &actors_message(&new_actors));
            for actor in new_actors {
                j.actors.push(actor);
            }
        }
        if !ops.is_empty() {
            encode_frame(&mut out, frame::OPS, &ops_message(&ops, &j.actors));
        }
        if let Some(local) = &local {
            encode_frame(&mut out, frame::LOCAL, local);
        }
        j.append(&out)?;
        for op in &ops {
            j.doc.apply(op);
        }
        if local.is_some() {
            j.local = local;
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
    let mut lock_path = path.to_path_buf().into_os_string();
    lock_path.push(".lock");
    let file = OpenOptions::new().create(true).truncate(false).write(true).open(lock_path)?;
    file.lock()?;
    Ok(file)
}

fn file_seen(path: &Path) -> Option<(u64, SystemTime)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()?))
}

/// Writes a compacted file (header, actors, snapshot, local) beside
/// `path`, fsyncs it, and renames it over `path`. Returns its bytes.
fn write_compacted(path: &Path, doc: &Doc, local: Option<&[u8]>) -> Result<Vec<u8>, JournalError> {
    let mut bytes = header_bytes(getrandom::u64().expect("the OS random source failed"));
    bytes.extend(snapshot_bytes(doc));
    if let Some(local) = local {
        encode_frame(&mut bytes, frame::LOCAL, local);
    }
    let mut tmp = path.to_path_buf().into_os_string();
    tmp.push(".tmp");
    {
        let mut file = File::create(&tmp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(bytes)
}

// ---- Inspection (`--dump`, `--check`) --------------------------------------

pub struct Inspect {
    pub version: u64,
    pub generation: u64,
    pub file_len: usize,
    /// `(offset, type, size)` of every whole frame.
    pub frames: Vec<(usize, u64, usize)>,
    /// Where a torn tail starts, and why reading stopped there.
    pub torn: Option<(usize, Stop)>,
}

pub fn inspect(bytes: &[u8]) -> Result<Inspect, JournalError> {
    let header = read_header(bytes)?;
    let scan = scan(&bytes[header.len..], header.len);
    Ok(Inspect {
        version: header.version,
        generation: header.generation,
        file_len: bytes.len(),
        frames: scan.frames.iter().map(|f| (f.offset, f.ty, f.size)).collect(),
        torn: scan.stop.map(|stop| (scan.end, stop)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::model::{encode_field, field, FieldValue};
    use crate::store::testutil::temp_dir;

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
        encode_frame(&mut bytes, frame::LOCAL, b"one");
        encode_frame(&mut bytes, frame::OPS, b"");
        let scan1 = scan(&bytes, 0);
        assert_eq!(scan1.frames.len(), 2);
        assert_eq!((scan1.frames[0].ty, scan1.frames[0].message), (frame::LOCAL, &b"one"[..]));
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
            txn.commit(vec![create], Some(vec![i])).unwrap();
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
            let (doc, _) = Journal::read(&full[..cut]).unwrap();
            assert_eq!(doc.live().count(), whole_ops, "cut at {cut}");
        }
    }

    #[test]
    fn reopening_gives_back_the_document_and_local_state() {
        let dir = temp_dir("reopen");
        let path = dir.join("session.mnf");
        let mut journal = Journal::open(&path, ActorId(1)).unwrap();
        let mut txn = journal.begin().unwrap();
        let (tab, create) = create_tab(&mut txn, "https://a.example/");
        txn.commit(vec![create], Some(b"view".to_vec())).unwrap();
        let doc_id = journal.doc().id;
        drop(journal);

        let mut reopened = Journal::open(&path, ActorId(2)).unwrap();
        assert_eq!(reopened.doc().id, doc_id);
        assert_eq!(url(reopened.doc(), tab), FieldValue::Str("https://a.example/".into()));
        assert_eq!(reopened.local(), Some(&b"view"[..]));
        // The new replica's ops are stamped after everything in the file.
        let seen = reopened.doc().clock;
        let txn = reopened.begin().unwrap();
        txn.commit(vec![OpBody::Delete { node: tab }], None).unwrap();
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
        let mut txn = journal.begin().unwrap();
        let (tab, create) = create_tab(&mut txn, "https://b.example/");
        txn.commit(vec![create], None).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let header = read_header(&bytes).unwrap();
        let scanned = scan(&bytes[header.len..], header.len);
        assert_eq!((scanned.end, scanned.stop), (bytes.len(), None));
        // The new frames start where the half frame did: it was cut.
        assert!(scanned.frames.iter().any(|f| f.offset as u64 == clean_len));
        let (doc, _) = Journal::read(&bytes).unwrap();
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
        txn.commit(vec![OpBody::Create { node: group, kind: Kind::Group, fields: Vec::new() }], None).unwrap();
        let mut txn = b.begin().unwrap();
        assert!(txn.remote.contains(&group));
        let (tab, create) = create_tab(&mut txn, "https://c.example/");
        txn.commit(vec![create, OpBody::Move { node: tab, group: Some(group) }], None).unwrap();

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
        txn.commit(vec![create], None).unwrap();
        a.close().unwrap(); // compacts while `b` has the file open

        let mut txn = b.begin().unwrap();
        assert!(txn.remote.contains(&one));
        let (two, create) = create_tab(&mut txn, "https://two.example/");
        txn.commit(vec![create], None).unwrap();

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
        txn.commit(vec![create], None).unwrap();
        let long = "x".repeat(1000);
        for i in 0..200 {
            let text = FieldValue::Str(format!("{i}{long}"));
            let txn = journal.begin().unwrap();
            txn.commit(vec![OpBody::Set { node: tab, field: field::URL, enc: encode_field(field::URL, &text) }], None).unwrap();
        }
        // 200 KB of appends against a tiny snapshot: compacted at least once.
        assert!(std::fs::metadata(&path).unwrap().len() < 100 * 1024);
        let (doc, _) = Journal::read(&std::fs::read(&path).unwrap()).unwrap();
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
        txn.commit(vec![create], None).unwrap();
        let long = "x".repeat(1000);
        for i in 0..200 {
            let text = FieldValue::Str(format!("{i}{long}"));
            let txn = journal.begin().unwrap();
            txn.commit(vec![OpBody::Set { node: tab, field: field::URL, enc: encode_field(field::URL, &text) }], None).unwrap();
        }
        assert!(std::fs::metadata(&path).unwrap().len() > 150 * 1024); // never compacted
        assert!(journal.close().is_err());
        std::fs::remove_dir(&tmp).unwrap();
        journal.close().unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() < 100 * 1024);
        let (doc, _) = Journal::read(&std::fs::read(&path).unwrap()).unwrap();
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
        txn.commit(vec![create, OpBody::Set { node: tab, field: 99, enc: future.clone() }], None).unwrap();
        journal.close().unwrap(); // compacts: the unknown field is written back

        let (doc, _) = Journal::read(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(doc.nodes[&tab].fields[&99].enc, future);

        // A frame type from the future: kept, and the file never compacts.
        let mut frame = Vec::new();
        encode_frame(&mut frame, 50, b"new");
        OpenOptions::new().append(true).open(&path).unwrap().write_all(&frame).unwrap();
        let mut journal = Journal::open(&path, ActorId(2)).unwrap();
        let txn = journal.begin().unwrap();
        txn.commit(vec![OpBody::Delete { node: tab }], None).unwrap();
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
        txn.commit(vec![create], None).unwrap();
        drop(journal);

        // An ops frame whose top level carries a field this build doesn't know.
        let mut actors = Actors::default();
        actors.push(ActorId(1));
        let op = Op { id: NodeId { actor: ActorId(1), counter: 100 }, hlc: Hlc { ms: now_ms() + 1000, logical: 0 }, body: OpBody::Delete { node: tab } };
        let mut message = ops_message(&[op], &actors);
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
        let (doc, _) = Journal::read(&std::fs::read(&path).unwrap()).unwrap();
        let actors = doc_actors(&doc);
        let mut bytes = header_bytes(7);
        encode_frame(&mut bytes, frame::ACTORS, &actors_message(&actors.list));
        let mut snapshot = snapshot_message(&doc, &actors);
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
        assert!(txn.commit(vec![create], None).is_err()); // the actor table grew in memory only
        assert!(matches!(journal.save_copy(&dir.join("copy.mnf")), Err(JournalError::Stale)));
        set_readonly(false);
        let mut txn = journal.begin().unwrap();
        let (tab, create) = create_tab(&mut txn, "https://kept.example/");
        txn.commit(vec![create], None).unwrap();
        let (doc, _) = Journal::read(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(doc.nodes.len(), 1);
        assert_eq!(url(&doc, tab), FieldValue::Str("https://kept.example/".into()));
        assert_eq!(journal.doc().nodes, doc.nodes);
    }

    #[test]
    fn a_newer_format_version_opens_read_only() {
        let dir = temp_dir("version");
        let path = dir.join("session.mnf");
        drop(Journal::open(&path, ActorId(1)).unwrap());
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[4] = 2; // format_version 2
        std::fs::write(&path, &bytes).unwrap();
        let mut journal = Journal::open(&path, ActorId(2)).unwrap();
        assert!(journal.read_only());
        let txn = journal.begin().unwrap();
        assert!(matches!(txn.commit(vec![], Some(vec![1])), Err(JournalError::ReadOnly)));
        journal.close().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
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
    fn inspect_reports_frames_and_the_torn_tail() {
        let dir = temp_dir("inspect");
        let path = dir.join("session.mnf");
        drop(Journal::open(&path, ActorId(1)).unwrap());
        let mut bytes = std::fs::read(&path).unwrap();
        let clean = bytes.len();
        bytes.extend_from_slice(&[0; 16]);
        let report = inspect(&bytes).unwrap();
        assert_eq!(report.frames.iter().map(|f| f.1).collect::<Vec<_>>(), [frame::ACTORS, frame::SNAPSHOT]);
        assert_eq!(report.torn, Some((clean, Stop::ZeroLength)));
    }
}
