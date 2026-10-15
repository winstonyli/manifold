//! The shared document's data model (spec "Data model"): ids, the hybrid
//! logical clock, node kinds, the field table, and nodes whose fields are
//! last-writer-wins registers holding their field's own TLV encoding.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use super::ops::MoveRec;
use super::tlv::{Reader, TlvError, Writer};

/// A replica's id for one launch: random, never persisted or reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ActorId(pub u64);

impl ActorId {
    pub fn random() -> Self {
        ActorId(getrandom::u64().expect("the OS random source failed"))
    }
}

/// A node or op id: the creating actor and its per-launch counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId {
    pub actor: ActorId,
    pub counter: u64,
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}-{}", self.actor.0, self.counter)
    }
}

impl std::str::FromStr for NodeId {
    type Err = String;

    /// Parses exactly what `Display` prints.
    fn from_str(s: &str) -> Result<Self, String> {
        let bad = || format!("not a node id: {s:?}");
        let (actor, counter) = s.split_once('-').ok_or_else(bad)?;
        let hex = actor.len() == 16 && actor.bytes().all(|b| b.is_ascii_hexdigit());
        let decimal = !counter.is_empty() && counter.bytes().all(|b| b.is_ascii_digit());
        if !hex || !decimal {
            return Err(bad());
        }
        let actor = u64::from_str_radix(actor, 16).map_err(|_| bad())?;
        let counter = counter.parse().map_err(|_| bad())?;
        Ok(NodeId { actor: ActorId(actor), counter })
    }
}

/// 128 random bits naming a document (spec "Document id"). Its text form
/// is left to sub-project 2, the first thing that shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DocId(pub [u8; 16]);

impl DocId {
    pub fn random() -> Self {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).expect("the OS random source failed");
        DocId(bytes)
    }
}

/// A hybrid logical clock reading: wall-clock milliseconds, then a counter
/// for events within one millisecond.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Hlc {
    pub ms: u64,
    pub logical: u32,
}

/// What orders two writes: the HLC, then the writing actor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Stamp {
    pub hlc: Hlc,
    pub actor: ActorId,
}

#[derive(Debug, Default)]
pub struct Clock {
    last: Hlc,
}

impl Clock {
    /// The next local event's time: never behind `now_ms` or any time seen.
    pub fn tick(&mut self, now_ms: u64) -> Hlc {
        let ms = now_ms.max(self.last.ms);
        self.last = if ms > self.last.ms {
            Hlc { ms, logical: 0 }
        } else {
            match self.last.logical.checked_add(1) {
                Some(logical) => Hlc { ms, logical },
                // 2^32 events in one millisecond: borrow the next one. At the
                // very top of the range, stay there: wrapping to 0 would put
                // every later local write behind every stamp seen.
                None => match ms.checked_add(1) {
                    Some(ms) => Hlc { ms, logical: 0 },
                    None => Hlc { ms, logical: u32::MAX },
                },
            }
        };
        self.last
    }

    /// Moves the clock past a time read from the file or another replica.
    pub fn observe(&mut self, seen: Hlc) {
        self.last = self.last.max(seen);
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

pub const TYPE_TAB: &str = "application/vnd.manifold.tab";
pub const TYPE_GROUP: &str = "application/vnd.manifold.group";
pub const TYPE_JAR: &str = "application/vnd.manifold.jar";
pub const TYPE_LINK: &str = "application/vnd.manifold.link";
pub const TYPE_NOTE: &str = "text/markdown";
pub const TYPE_PDF: &str = "application/pdf";
pub const TYPE_JSON: &str = "application/json";
/// What a created image is stored as until sniffed types arrive (plan 2c); any `image/*` reads back as an image.
pub const TYPE_IMAGE: &str = "image/png";
pub const TYPE_DOWNLOAD: &str = "application/octet-stream";
/// Never stored: the row and edge string a type this build doesn't know reads as (`card_kind::canonical_type`).
pub const TYPE_UNKNOWN: &str = "application/vnd.manifold.unknown";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Tab,
    Group,
    Note,
    Image,
    Pdf,
    Json,
    Download,
    Jar,
    /// A connection between two cards: a stored node, never a graph card, with `from` and `to` ends.
    Link,
    /// No `type` yet (an op named the node before its `Create` arrived), or a type a newer build wrote. The node
    /// keeps its type string (`Node::ty`); the app reads only its generic fields (`GENERIC`).
    Unknown,
}

/// The type table: each kind, the type string it is stored under, and the fields a node of it carries, `group`
/// aside (spec, "The shared document"). `parent` ("opened from") is on every kind a page can make: a tab, a download,
/// and content a tab became. Older builds don't write it for the non-tab kinds, but keep it (`Node` holds every
/// register, and a `Create` only writes the fields listed here). Fields a type does not list stay raw in the document
/// untouched (2b-5 Ruling 2).
#[rustfmt::skip]
pub(crate) const TYPES: [(Kind, &str, &[u32]); 9] = {
    use field::*;
    [
        (Kind::Tab, TYPE_TAB, &[POS, COLOR, PARENT, URL, VIEW_KIND, PINNED, JAR, NAME, CLOSED]),
        (Kind::Group, TYPE_GROUP, &[POS, COLOR, NAME, JAR, CLOSED]),
        (Kind::Note, TYPE_NOTE, &[POS, COLOR, PARENT, URL, TEXT, NAME, CLOSED]),
        (Kind::Image, TYPE_IMAGE, &[POS, COLOR, PARENT, URL, PATH, NAME, CLOSED]),
        (Kind::Pdf, TYPE_PDF, &[POS, COLOR, PARENT, URL, PATH, NAME, CLOSED]),
        (Kind::Json, TYPE_JSON, &[POS, COLOR, PARENT, URL, PATH, VIEW, NAME, CLOSED]),
        (Kind::Download, TYPE_DOWNLOAD, &[POS, COLOR, PARENT, URL, PATH, NAME, CLOSED]),
        (Kind::Jar, TYPE_JAR, &[POS, COLOR, NAME, EPHEMERAL, IS_DEFAULT, CLOSED]),
        (Kind::Link, TYPE_LINK, &[NAME, FROM, TO, CLOSED]),
    ]
};

/// The fields every card carries whatever its type: all the app reads, mirrors and copies of a node of a type this
/// build doesn't know (`Kind::Unknown`, spec decision 3). Its other fields stay raw in the document.
pub(crate) const GENERIC: &[u32] = &[field::POS, field::COLOR, field::PARENT, field::NAME, field::CLOSED];

impl Kind {
    /// The type string a created node of this kind is stored under.
    pub fn type_str(self) -> Option<&'static str> {
        TYPES.iter().find(|t| t.0 == self).map(|t| t.1)
    }

    /// The fields a node of this kind carries, `group` aside (`TYPES`): what a `Create` writes, what another
    /// replica's change hands the app (2b-5 Ruling 11), and what a snapshot of the node holds. `GENERIC` for
    /// `Unknown`.
    pub fn fields(self) -> &'static [u32] {
        TYPES.iter().find(|t| t.0 == self).map_or(GENERIC, |t| t.2)
    }

    /// The kind a stored type string means; any `image/*` is an image.
    pub fn from_type(ty: &str) -> Kind {
        if ty.get(..6).is_some_and(|p| p.eq_ignore_ascii_case("image/")) {
            return Kind::Image;
        }
        TYPES.iter().find(|t| t.1.eq_ignore_ascii_case(ty)).map_or(Kind::Unknown, |t| t.0)
    }
}

/// Engine codes for the `view_kind` field: code `n` is `ENGINES[n - 1]`,
/// spelled as `process_manager::view_kind_serde` spells it.
pub const VIEW_KINDS: [&str; 4] = ["servo", "cef", "webkit", "webview2"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldType {
    /// Two `f32`: `{1: x, 2: y}`.
    Pos,
    /// `0xRRGGBB` as a varint; absent means untagged, so black is `Some(0)`.
    Color,
    /// A node id with its full actor: `{1: actor (fixed64), 2: counter}`.
    /// Never an actor-table index, which is local to one file and would
    /// break inside a field this build writes back without knowing it.
    Id,
    Str,
    Bool,
    /// An `VIEW_KINDS` code.
    ViewCode,
    /// A replica id (`profile::replica_id`): a nonzero `u64` varint; absent or 0 is none.
    Replica,
}

#[derive(Debug)]
pub struct FieldDef {
    pub no: u32,
    pub name: &'static str,
    pub ty: FieldType,
}

/// Field numbers. Never reuse one; retired numbers go in `RESERVED`.
pub mod field {
    pub const POS: u32 = 1;
    pub const COLOR: u32 = 2;
    /// The tree edge: set by `Create` and changed only by `Move`.
    pub const GROUP: u32 = 3;
    pub const PARENT: u32 = 4;
    pub const URL: u32 = 5;
    pub const VIEW_KIND: u32 = 6;
    pub const PINNED: u32 = 7;
    pub const NAME: u32 = 8;
    // 9, 12 and 13 were `from_json`, `key` and `value` (the JSON leaf, retired 2026-10-12): never reuse them. A
    // document that still holds them keeps them raw, like any field a type does not list.
    pub const TEXT: u32 = 10;
    pub const PATH: u32 = 11;
    /// A Json node's view overrides (`json_view::JsonView`), compact JSON; "" when none.
    pub const VIEW: u32 = 14;
    /// The `jar` edge of a tab or group: the `Kind::Jar` node it uses; absent to inherit (`App::effective_jar`).
    /// Numbers 16 and 17 were a tab's `jar_set` and a group's `default_jar` (`RESERVED`).
    pub const JAR: u32 = 15;
    /// A jar with no file: dropped at quit.
    pub const EPHEMERAL: u32 = 18;
    /// A node's MIME type, written by `Create`.
    pub const TYPE: u32 = 19;
    /// A link's start card.
    pub const FROM: u32 = 20;
    /// A link's end card.
    pub const TO: u32 = 21;
    /// A jar that is the built-in Default: exactly one live jar has it once `App::ensure_default_jar` has run.
    pub const IS_DEFAULT: u32 = 22;
    /// Set on the root of a close (spec 2026-10-14-closed-field-design.md) to the id of the replica that closed it
    /// (`profile::replica_id`, persisted: it outlives a launch, unlike the actor); absent or 0 is open. The node and
    /// everything under it are hidden in place, and the node is a Recently Closed entry (`Doc::closed_roots`). Only
    /// the close and reopen verbs write it.
    pub const CLOSED: u32 = 23;
}

pub static FIELDS: [FieldDef; 18] = [
    FieldDef { no: field::POS, name: "pos", ty: FieldType::Pos },
    FieldDef { no: field::COLOR, name: "color", ty: FieldType::Color },
    FieldDef { no: field::GROUP, name: "group", ty: FieldType::Id },
    FieldDef { no: field::PARENT, name: "parent", ty: FieldType::Id },
    FieldDef { no: field::URL, name: "url", ty: FieldType::Str },
    FieldDef { no: field::VIEW_KIND, name: "view_kind", ty: FieldType::ViewCode },
    FieldDef { no: field::PINNED, name: "pinned", ty: FieldType::Bool },
    FieldDef { no: field::NAME, name: "name", ty: FieldType::Str },
    FieldDef { no: field::TEXT, name: "text", ty: FieldType::Str },
    // Machine-local: a replica that refetches a missing file saves it under
    // its own content dir, and that path must not be written back.
    FieldDef { no: field::PATH, name: "path", ty: FieldType::Str },
    FieldDef { no: field::VIEW, name: "view", ty: FieldType::Str },
    FieldDef { no: field::JAR, name: "jar", ty: FieldType::Id },
    FieldDef { no: field::EPHEMERAL, name: "ephemeral", ty: FieldType::Bool },
    FieldDef { no: field::TYPE, name: "type", ty: FieldType::Str },
    FieldDef { no: field::FROM, name: "from", ty: FieldType::Id },
    FieldDef { no: field::TO, name: "to", ty: FieldType::Id },
    FieldDef { no: field::IS_DEFAULT, name: "is_default", ty: FieldType::Bool },
    FieldDef { no: field::CLOSED, name: "closed", ty: FieldType::Replica },
];

/// Retired: a tab's `jar_set` and a group's `default_jar`, replaced by the `jar` edge (jars-as-cards spec). Never
/// reused; old documents keep them raw (checked by a test).
#[cfg(test)]
pub const RESERVED: [u32; 2] = [16, 17];

pub fn field_def(no: u32) -> Option<&'static FieldDef> {
    FIELDS.iter().find(|f| f.no == no)
}

pub fn field_by_name(name: &str) -> Option<&'static FieldDef> {
    FIELDS.iter().find(|f| f.name == name)
}

#[derive(Debug, Clone, PartialEq)]
pub enum FieldValue {
    Pos(f32, f32),
    Color(Option<u32>),
    Id(Option<NodeId>),
    Str(String),
    Bool(bool),
    ViewCode(u64),
    Replica(u64),
}

impl FieldValue {
    /// What a missing field decodes to.
    pub fn absent(ty: FieldType) -> FieldValue {
        match ty {
            FieldType::Pos => FieldValue::Pos(0.0, 0.0),
            FieldType::Color => FieldValue::Color(None),
            FieldType::Id => FieldValue::Id(None),
            FieldType::Str => FieldValue::Str(String::new()),
            FieldType::Bool => FieldValue::Bool(false),
            FieldType::ViewCode => FieldValue::ViewCode(0),
            FieldType::Replica => FieldValue::Replica(0),
        }
    }
}

/// `value` as field `no` of a node message, key and value; empty when it
/// holds what absence decodes to, so equal values encode equally.
pub fn encode_field(no: u32, value: &FieldValue) -> Vec<u8> {
    let mut out = Vec::new();
    encode_field_into(&mut out, no, value);
    out
}

/// `encode_field` into `out` (replacing what it held), reusing its
/// allocation: for comparing many fields against stored bytes.
pub fn encode_field_into(out: &mut Vec<u8>, no: u32, value: &FieldValue) {
    let mut w = Writer::reuse(std::mem::take(out));
    match value {
        FieldValue::Pos(x, y) => {
            let mut m = Writer::new();
            if *x != 0.0 {
                m.f32(1, *x);
            }
            if *y != 0.0 {
                m.f32(2, *y);
            }
            if !m.is_empty() {
                w.message(no, &m);
            }
        }
        FieldValue::Color(Some(rgb)) => w.varint(no, u64::from(*rgb)),
        FieldValue::Id(Some(id)) => w.message(no, &id_message(*id)),
        FieldValue::Str(s) if !s.is_empty() => w.str(no, s),
        FieldValue::Bool(true) => w.varint(no, 1),
        FieldValue::ViewCode(code) | FieldValue::Replica(code) if *code != 0 => w.varint(no, *code),
        // The empty or zero value of each type is "absent": nothing is written.
        FieldValue::Color(None)
        | FieldValue::Id(None)
        | FieldValue::Str(_)
        | FieldValue::Bool(false)
        | FieldValue::ViewCode(_)
        | FieldValue::Replica(_) => {}
    }
    *out = w.into_bytes();
}

fn id_message(id: NodeId) -> Writer {
    let mut m = Writer::new();
    if id.actor.0 != 0 {
        m.fixed64(1, id.actor.0);
    }
    if id.counter != 0 {
        m.varint(2, id.counter);
    }
    m
}

fn decode_id(buf: &[u8]) -> Result<NodeId, TlvError> {
    let mut id = NodeId { actor: ActorId(0), counter: 0 };
    for field in Reader::new(buf) {
        let field = field?;
        match field.no {
            1 => id.actor = ActorId(field.value.fixed64()?),
            2 => id.counter = field.value.u64()?,
            _ => {}
        }
    }
    Ok(id)
}

/// Decodes `encode_field`'s output (or any encoding of field `def`; the
/// last occurrence wins). A non-finite position is not a position: it
/// decodes as absent.
pub fn decode_field(def: &FieldDef, encoded: &[u8]) -> Result<FieldValue, TlvError> {
    let mut value = FieldValue::absent(def.ty);
    for field in Reader::new(encoded) {
        let v = field?.value;
        value = match def.ty {
            FieldType::Pos => {
                let (mut x, mut y) = (0.0, 0.0);
                for f in Reader::new(v.bytes()?) {
                    let f = f?;
                    match f.no {
                        1 => x = f.value.f32()?,
                        2 => y = f.value.f32()?,
                        _ => {}
                    }
                }
                if x.is_finite() && y.is_finite() {
                    FieldValue::Pos(x, y)
                } else {
                    FieldValue::absent(def.ty)
                }
            }
            FieldType::Color => FieldValue::Color(Some(v.u32()?)),
            FieldType::Id => FieldValue::Id(Some(decode_id(v.bytes()?)?)),
            FieldType::Str => FieldValue::Str(v.str()?.to_owned()),
            FieldType::Bool => FieldValue::Bool(v.bool()?),
            FieldType::ViewCode => FieldValue::ViewCode(v.u64()?),
            FieldType::Replica => FieldValue::Replica(v.u64()?),
        };
    }
    Ok(value)
}

/// A last-writer-wins register: the winning write's stamp, and its value
/// as `encode_field` encodes it (empty = absent).
#[derive(Debug, Clone, PartialEq)]
pub struct Reg {
    pub stamp: Stamp,
    pub enc: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    /// Every field but `group`, by number, `type` included, and numbers this
    /// build doesn't know (kept raw so compaction writes them back).
    pub fields: BTreeMap<u32, Reg>,
    /// The tree edge, owned by the move log (`ops.rs`). It may name a
    /// deleted group; see `Doc::effective_group`.
    pub group: Option<NodeId>,
    pub deleted: bool,
}

impl Node {
    /// A node an op named before its `Create` arrived; shown once it does.
    pub fn placeholder() -> Node {
        Node { fields: BTreeMap::new(), group: None, deleted: false }
    }

    /// Created: an op wrote its `type`.
    pub fn has_type(&self) -> bool {
        self.fields.contains_key(&field::TYPE)
    }

    /// Its stored `type` string as written ("" before its `Create` arrives): the last occurrence in the register,
    /// as `decode_field` reads it.
    pub fn ty(&self) -> &str {
        let Some(reg) = self.fields.get(&field::TYPE) else { return "" };
        let mut ty = "";
        for f in Reader::new(&reg.enc).map_while(Result::ok) {
            if let Ok(s) = f.value.str() {
                ty = s;
            }
        }
        ty
    }

    /// What kind of node this is, read from its `type`.
    pub fn kind(&self) -> Kind {
        Kind::from_type(self.ty())
    }

    /// Shown in the graph: created with a type (any, one this build doesn't know included: an unknown-type card),
    /// and not deleted. An empty `type` is no type.
    pub fn is_live(&self) -> bool {
        !self.deleted && !self.ty().is_empty()
    }

    /// A group: the only kind other nodes are shown inside (`Doc::effective_group`).
    pub fn is_group(&self) -> bool {
        self.kind() == Kind::Group
    }

    /// The root of a close: `closed` is set (spec decision 1). Its descendants keep their own `closed` as it was.
    pub fn closed(&self) -> bool {
        self.closer() != 0
    }

    /// The replica that closed this node (`field::CLOSED`); 0 when it is open.
    pub fn closer(&self) -> u64 {
        match self.get(field::CLOSED) {
            FieldValue::Replica(replica) => replica,
            _ => 0,
        }
    }

    /// A live, groupless node of a known `kind`.
    #[cfg(test)]
    pub fn typed(kind: Kind) -> Node {
        Node::with_type(kind.type_str().expect("a known kind"))
    }

    /// A live, groupless node whose `type` is `ty`.
    #[cfg(test)]
    pub fn with_type(ty: &str) -> Node {
        let stamp = Stamp { hlc: Hlc::default(), actor: ActorId(0) };
        let reg = Reg { stamp, enc: encode_field(field::TYPE, &FieldValue::Str(ty.to_owned())) };
        Node { fields: BTreeMap::from([(field::TYPE, reg)]), group: None, deleted: false }
    }

    /// Field `no`'s value; its default when unset or undecodable.
    pub fn get(&self, no: u32) -> FieldValue {
        let def = field_def(no).expect("a field number from `field`");
        self.fields
            .get(&no)
            .and_then(|reg| decode_field(def, &reg.enc).ok())
            .unwrap_or_else(|| FieldValue::absent(def.ty))
    }

    /// This node as a plain node message, `group` included: what a
    /// `Create` carries.
    #[cfg(test)]
    pub fn message(&self) -> Vec<u8> {
        let mut w = Writer::new();
        let group = encode_field(field::GROUP, &FieldValue::Id(self.group));
        let mut group_written = false;
        for (&no, reg) in &self.fields {
            if no > field::GROUP && !group_written {
                w.raw(&group);
                group_written = true;
            }
            w.raw(&reg.enc);
        }
        if !group_written {
            w.raw(&group);
        }
        w.into_bytes()
    }
}

#[derive(Debug, Clone)]
pub struct Doc {
    pub id: DocId,
    pub nodes: BTreeMap<NodeId, Node>,
    /// Per actor, one past the highest op counter applied.
    pub version: BTreeMap<ActorId, u64>,
    /// The latest HLC applied, so a reopened replica's clock starts past it.
    pub clock: Hlc,
    /// Applied moves in stamp order, for undo and redo (`ops.rs`).
    pub(crate) moves: Vec<MoveRec>,
}

/// Where a node is shown (`Doc::placement`): in a group (`None` the top level), or not at all because a closed
/// node sits above it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    Shown(Option<NodeId>),
    Hidden,
}

impl Doc {
    pub fn new(id: DocId) -> Doc {
        Doc { id, nodes: BTreeMap::new(), version: BTreeMap::new(), clock: Hlc::default(), moves: Vec::new() }
    }

    /// The live nodes (typed, not deleted; closed ones included), in id order: the commit gate. What the graph shows
    /// is `shown()`.
    pub fn live(&self) -> impl Iterator<Item = (NodeId, &Node)> {
        self.nodes.iter().filter(|(_, node)| node.is_live()).map(|(id, node)| (*id, node))
    }

    /// The group `id` is shown in: its own, or, when that isn't a live group (deleted, not created yet, or another
    /// card, a type this build doesn't know included), its nearest live group ancestor (spec "Semantics"; unknown-type
    /// card decision 2). The document keeps the raw `group`. `None` is the top level, also for a group missing from
    /// the document or a cycle, which only a damaged snapshot can hold.
    pub fn effective_group(&self, id: NodeId) -> Option<NodeId> {
        let mut group = self.nodes.get(&id)?.group;
        for _ in 0..=self.nodes.len() {
            let node = self.nodes.get(&group?)?;
            if node.is_live() && node.is_group() {
                return group;
            }
            group = node.group;
        }
        None
    }

    /// Where `id` would be shown: `Hidden` when any non-deleted closed node is above it on the raw `group` chain
    /// (checked first; any node, group or not, plan D1), else `Shown(effective_group)`. `id`'s own `closed` is
    /// not read: a closed root's place is its group's (`is_shown` reads it). A missing node or a cycle is
    /// `Shown`, as `effective_group` answers.
    pub fn placement(&self, id: NodeId) -> Placement {
        let mut cur = self.nodes.get(&id).and_then(|n| n.group);
        for _ in 0..=self.nodes.len() {
            let Some(node) = cur.and_then(|c| self.nodes.get(&c)) else { break };
            if !node.deleted && node.closed() {
                return Placement::Hidden;
            }
            cur = node.group;
        }
        Placement::Shown(self.effective_group(id))
    }

    /// Shown in the graph: live, not itself closed, and not under a closed node. A live link is always shown
    /// (plan D2): the mirror keeps every one, and drawing asks for both ends.
    pub fn is_shown(&self, id: NodeId) -> bool {
        let Some(node) = self.nodes.get(&id).filter(|n| n.is_live()) else { return false };
        node.kind() == Kind::Link || (!node.closed() && self.placement(id) != Placement::Hidden)
    }

    /// The shown nodes, in id order: what the graph and an export show. `live()` stays the commit gate.
    pub fn shown(&self) -> impl Iterator<Item = (NodeId, &Node)> {
        self.live().filter(|&(id, _)| self.is_shown(id))
    }

    /// `id` when it is shown, else the nearest shown node above it on the raw `group` chain; `None` (the top level)
    /// when there is none. Where a level falls back to when the one it names is hidden or gone.
    pub fn shown_level(&self, id: NodeId) -> Option<NodeId> {
        let mut cur = Some(id);
        for _ in 0..=self.nodes.len() {
            let at = cur?;
            if self.is_shown(at) {
                return Some(at);
            }
            cur = self.nodes.get(&at).and_then(|n| n.group);
        }
        None
    }

    /// Recently Closed (spec decision 5): the live closed nodes that are not hidden by another closed node, newest
    /// first by their `closed` register's stamp (ties by id, newest first), the same order on every replica.
    pub fn closed_roots(&self) -> Vec<(NodeId, Stamp)> {
        let mut roots: Vec<(NodeId, Stamp)> = self
            .live()
            .filter(|&(id, n)| n.closed() && self.placement(id) != Placement::Hidden)
            .filter_map(|(id, n)| Some((id, n.fields.get(&field::CLOSED)?.stamp)))
            .collect();
        roots.sort_by_key(|&(id, stamp)| std::cmp::Reverse((stamp, id)));
        roots
    }

    /// `nodes` and every node below one of them: where a node is shown
    /// depends on each group above it (`effective_group`).
    pub fn with_descendants(&self, nodes: BTreeSet<NodeId>) -> BTreeSet<NodeId> {
        let mut out = nodes.clone();
        for (&id, node) in &self.nodes {
            let mut cur = node.group;
            for _ in 0..self.nodes.len() {
                let Some(c) = cur else { break };
                if nodes.contains(&c) {
                    out.insert(id);
                    break;
                }
                cur = self.nodes.get(&c).and_then(|n| n.group);
            }
        }
        out
    }

    /// Whether `ancestor` is on the path from `of` to the root, `of`
    /// excluded. Walks deleted nodes too, so every replica answers alike.
    pub fn is_ancestor(&self, ancestor: NodeId, of: NodeId) -> bool {
        let mut cur = self.nodes.get(&of).and_then(|n| n.group);
        for _ in 0..=self.nodes.len() {
            match cur {
                Some(c) if c == ancestor => return true,
                Some(c) => cur = self.nodes.get(&c).and_then(|n| n.group),
                None => return false,
            }
        }
        true // a cycle; `apply_move` never makes one
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_saturates_instead_of_wrapping() {
        let mut clock = Clock::default();
        let top = Hlc { ms: u64::MAX, logical: u32::MAX };
        clock.observe(top);
        assert!(clock.tick(0) >= top, "never behind a time seen");
    }

    #[test]
    fn node_ids_print_and_parse() {
        let id = NodeId { actor: ActorId(0x1f3a_9c00_0000_00ab), counter: 17 };
        assert_eq!(id.to_string(), "1f3a9c00000000ab-17");
        assert_eq!(id.to_string().parse(), Ok(id));
        for bad in ["", "tab-3", "1f3a9c00000000ab-", "1f3a9c00000000ab-+1", "+f3a9c00000000ab-1"] {
            assert!(bad.parse::<NodeId>().is_err(), "{bad}");
        }
    }

    #[test]
    fn the_clock_never_goes_back() {
        let mut clock = Clock::default();
        assert_eq!(clock.tick(10), Hlc { ms: 10, logical: 0 });
        assert_eq!(clock.tick(10), Hlc { ms: 10, logical: 1 });
        assert_eq!(clock.tick(5), Hlc { ms: 10, logical: 2 });
        clock.observe(Hlc { ms: 20, logical: 4 });
        assert_eq!(clock.tick(15), Hlc { ms: 20, logical: 5 });
        clock.observe(Hlc { ms: 30, logical: u32::MAX });
        assert_eq!(clock.tick(0), Hlc { ms: 31, logical: 0 });
    }

    #[test]
    fn field_numbers_are_unique_and_never_reserved() {
        let mut seen = std::collections::BTreeSet::new();
        for def in &FIELDS {
            assert!(seen.insert(def.no), "field {} twice", def.no);
            assert!(!RESERVED.contains(&def.no), "field {} is reserved", def.no);
        }
        assert_eq!(field::VIEW, 14);
        let view = field_by_name("view").unwrap();
        assert_eq!(view.ty, FieldType::Str);
        for (no, name, ty) in [
            (field::JAR, "jar", FieldType::Id),
            (field::EPHEMERAL, "ephemeral", FieldType::Bool),
            (field::IS_DEFAULT, "is_default", FieldType::Bool),
        ] {
            let def = field_by_name(name).unwrap();
            assert_eq!((def.no, def.ty), (no, ty), "{name}");
        }
        assert_eq!((field::JAR, field::EPHEMERAL), (15, 18));
        let ty = field_by_name("type").unwrap();
        assert_eq!((ty.no, ty.ty), (19, FieldType::Str));
    }

    #[test]
    fn values_round_trip_and_defaults_encode_to_nothing() {
        let id = NodeId { actor: ActorId(9), counter: 4 };
        let cases = [
            (field::POS, FieldValue::Pos(1.5, -2.0)),
            (field::POS, FieldValue::Pos(0.0, 3.0)),
            (field::COLOR, FieldValue::Color(Some(0))),
            (field::COLOR, FieldValue::Color(Some(0x12_34_56))),
            (field::PARENT, FieldValue::Id(Some(id))),
            (field::URL, FieldValue::Str("https://example.com/".into())),
            (field::PINNED, FieldValue::Bool(true)),
            (field::VIEW_KIND, FieldValue::ViewCode(4)),
            (field::JAR, FieldValue::Id(Some(id))),
            (field::EPHEMERAL, FieldValue::Bool(true)),
            (field::IS_DEFAULT, FieldValue::Bool(true)),
        ];
        for (no, value) in cases {
            let enc = encode_field(no, &value);
            assert!(!enc.is_empty(), "{value:?}");
            assert_eq!(decode_field(field_def(no).unwrap(), &enc), Ok(value));
        }
        for def in &FIELDS {
            assert!(encode_field(def.no, &FieldValue::absent(def.ty)).is_empty(), "{}", def.name);
            assert_eq!(decode_field(def, &[]), Ok(FieldValue::absent(def.ty)));
        }
        assert_eq!(encode_field(field::POS, &FieldValue::Pos(-0.0, -0.0)), Vec::<u8>::new());
    }

    fn reg(counter: u64, ty: &str) -> Reg {
        let stamp = Stamp { hlc: Hlc { ms: counter, logical: 0 }, actor: ActorId(1) };
        Reg { stamp, enc: encode_field(field::TYPE, &FieldValue::Str(ty.into())) }
    }

    #[test]
    fn every_known_kind_has_a_type_string_that_maps_back() {
        for kind in [
            Kind::Tab,
            Kind::Group,
            Kind::Note,
            Kind::Image,
            Kind::Pdf,
            Kind::Json,
            Kind::Download,
            Kind::Jar,
            Kind::Link,
        ] {
            let ty = kind.type_str().expect("a known kind has a type");
            assert_eq!(Kind::from_type(ty), kind, "{ty}");
        }
        assert_eq!(Kind::Unknown.type_str(), None);
    }

    #[test]
    fn from_type_is_case_insensitive_takes_any_image_and_rejects_the_rest() {
        assert_eq!(Kind::from_type("Application/VND.manifold.TAB"), Kind::Tab);
        assert_eq!(Kind::from_type("image/jpeg"), Kind::Image);
        assert_eq!(Kind::from_type("IMAGE/webp"), Kind::Image);
        assert_eq!(Kind::from_type("video/mp4"), Kind::Unknown);
        // The retired JSON leaf: an old document's leaf is now a type this build doesn't know (an unknown-type card).
        assert_eq!(Kind::from_type("application/vnd.manifold.json-leaf"), Kind::Unknown);
        assert_eq!(Kind::from_type(""), Kind::Unknown);
    }

    #[test]
    fn a_node_reads_its_kind_from_the_last_type_write() {
        let mut node = Node::placeholder();
        assert_eq!(node.kind(), Kind::Unknown, "no type yet");
        assert!(!node.has_type() && !node.is_live());
        node.fields.insert(field::TYPE, reg(1, "application/vnd.manifold.jar"));
        assert_eq!((node.kind(), node.has_type(), node.is_live()), (Kind::Jar, true, true));
        node.fields.insert(field::TYPE, reg(2, "application/x-from-the-future"));
        assert_eq!((node.kind(), node.has_type(), node.is_live()), (Kind::Unknown, true, true), "a card of its own");
        node.fields.insert(field::TYPE, reg(3, ""));
        assert!(node.has_type() && !node.is_live(), "an empty type is no type");
    }

    /// A type this build doesn't know carries the generic fields (spec decision 3): what the app reads, mirrors and
    /// copies of it. Its type string is kept as written.
    #[test]
    fn an_unknown_kind_carries_the_generic_fields() {
        use field::*;
        assert_eq!(Kind::Unknown.fields(), &[POS, COLOR, PARENT, NAME, CLOSED]);
        assert_eq!(Node::with_type("application/x-from-the-future").ty(), "application/x-from-the-future");
        assert_eq!(Node::with_type("Text/Markdown").ty(), "Text/Markdown", "kept as written, case and all");
        assert_eq!(Node::placeholder().ty(), "");
    }

    #[test]
    fn a_non_finite_position_decodes_as_absent() {
        let def = field_def(field::POS).unwrap();
        for bad in [f32::NAN, f32::INFINITY] {
            let mut pos = Writer::new();
            pos.fixed32(1, bad.to_bits());
            let mut enc = Writer::new();
            enc.message(field::POS, &pos);
            assert_eq!(decode_field(def, &enc.into_bytes()).unwrap(), FieldValue::Pos(0.0, 0.0));
        }
    }

    #[test]
    fn nodes_show_at_their_nearest_live_group_never_inside_another_card() {
        let id = |counter| NodeId { actor: ActorId(1), counter };
        let at = |mut node: Node, group: Option<u64>, deleted| {
            node.group = group.map(id);
            node.deleted = deleted;
            node
        };
        let node = |kind, group, deleted| at(Node::typed(kind), group, deleted);
        let mut doc = Doc::new(DocId([0; 16]));
        doc.nodes.insert(id(1), node(Kind::Group, None, false));
        doc.nodes.insert(id(2), node(Kind::Group, Some(1), true)); // deleted, in 1
        doc.nodes.insert(id(3), node(Kind::Tab, Some(2), false)); // shown in 1
        doc.nodes.insert(id(4), node(Kind::Tab, Some(99), false)); // group missing
        doc.nodes.insert(id(5), at(Node::with_type("application/x-future"), Some(1), false)); // a newer build's container: live, not a group
        doc.nodes.insert(id(6), node(Kind::Tab, Some(5), false)); // shown in 1
        doc.nodes.insert(id(7), node(Kind::Group, Some(8), true)); // 7 and 8: a damaged cycle
        doc.nodes.insert(id(8), node(Kind::Group, Some(7), true));
        doc.nodes.insert(id(9), node(Kind::Tab, Some(7), false));
        doc.nodes.insert(id(10), node(Kind::Note, Some(3), false)); // in a tab: shown in 1
        assert_eq!(doc.effective_group(id(3)), Some(id(1)));
        assert_eq!(doc.effective_group(id(4)), None);
        assert_eq!(doc.effective_group(id(6)), Some(id(1)));
        assert_eq!(doc.effective_group(id(9)), None);
        assert_eq!(doc.effective_group(id(10)), Some(id(1)), "a tab is no group");
        assert_eq!(doc.with_descendants(BTreeSet::from([id(2)])), BTreeSet::from([id(2), id(3), id(10)]));
        assert_eq!(doc.with_descendants(BTreeSet::from([id(1)])), BTreeSet::from([1, 2, 3, 5, 6, 10].map(id)));
    }

    /// A doc of `(id, kind, group, closed)` rows, all live; ids are `NodeId { actor: ActorId(1), counter }`.
    fn doc_of(rows: &[(u64, Kind, Option<u64>, bool)]) -> (Doc, impl Fn(u64) -> NodeId) {
        let id = |c: u64| NodeId { actor: ActorId(1), counter: c };
        let mut doc = Doc::new(DocId([0; 16]));
        for &(c, kind, group, closed) in rows {
            let mut node = Node::typed(kind);
            node.group = group.map(id);
            if closed {
                let stamp = Stamp { hlc: Hlc { ms: c, logical: 0 }, actor: ActorId(1) };
                let enc = encode_field(field::CLOSED, &FieldValue::Replica(7));
                node.fields.insert(field::CLOSED, Reg { stamp, enc });
            }
            doc.nodes.insert(id(c), node);
        }
        (doc, id)
    }

    #[test]
    fn a_closed_group_hides_everything_under_it_and_a_deleted_one_passes_members_up() {
        let (mut doc, id) = doc_of(&[
            (1, Kind::Group, None, false),
            (2, Kind::Group, Some(1), true),
            (3, Kind::Tab, Some(2), false),
            (4, Kind::Group, Some(3), false), // under a tab: still hidden by 2 (D1)
            (5, Kind::Note, Some(1), false),
        ]);
        assert!(doc.nodes[&id(2)].closed());
        assert_eq!(doc.placement(id(2)), Placement::Shown(Some(id(1))), "a closed node's own place is its group's");
        assert!(!doc.is_shown(id(2)), "but it is not shown: it is closed");
        assert_eq!(doc.placement(id(3)), Placement::Hidden);
        assert_eq!(doc.placement(id(4)), Placement::Hidden);
        assert_eq!(doc.placement(id(5)), Placement::Shown(Some(id(1))));
        let shown: Vec<NodeId> = doc.shown().map(|(n, _)| n).collect();
        assert_eq!(shown, [id(1), id(5)]);
        // A deleted closed group hides nothing: its members pass up to the nearest shown group.
        doc.nodes.get_mut(&id(2)).unwrap().deleted = true;
        assert_eq!(doc.placement(id(3)), Placement::Shown(Some(id(1))));
        assert!(doc.is_shown(id(3)));
    }

    #[test]
    fn shown_level_is_the_nearest_shown_node_up_the_raw_chain() {
        let (doc, id) =
            doc_of(&[(1, Kind::Group, None, false), (2, Kind::Group, Some(1), true), (3, Kind::Group, Some(2), false)]);
        assert_eq!(doc.shown_level(id(1)), Some(id(1)));
        assert_eq!(doc.shown_level(id(3)), Some(id(1)), "hidden by 2, closed itself: both skipped");
        assert_eq!(doc.shown_level(id(9)), None, "unknown: the top level");
    }

    #[test]
    fn a_live_link_is_shown_whatever_its_group() {
        let (mut doc, id) = doc_of(&[(1, Kind::Group, None, true), (2, Kind::Link, Some(1), false)]);
        assert!(doc.is_shown(id(2)), "the mirror keeps every live link (D2)");
        doc.nodes.get_mut(&id(2)).unwrap().deleted = true;
        assert!(!doc.is_shown(id(2)));
    }

    #[test]
    fn closed_roots_are_newest_first_and_skip_nested_and_deleted_ones() {
        let (mut doc, id) = doc_of(&[
            (1, Kind::Note, None, true),    // stamp 1
            (2, Kind::Group, None, true),   // stamp 2
            (3, Kind::Note, Some(2), true), // stamp 3, nested: part of 2's entry
            (4, Kind::Tab, None, true),     // stamp 4
            (5, Kind::Tab, None, false),
        ]);
        let ids = |doc: &Doc| doc.closed_roots().into_iter().map(|(n, _)| n).collect::<Vec<_>>();
        assert_eq!(ids(&doc), [id(4), id(2), id(1)]);
        doc.nodes.get_mut(&id(4)).unwrap().deleted = true;
        let reg = doc.nodes.get_mut(&id(2)).unwrap().fields.get_mut(&field::CLOSED).unwrap();
        reg.enc = encode_field(field::CLOSED, &FieldValue::Replica(0));
        assert_eq!(ids(&doc), [id(3), id(1)], "clearing the outer one makes the nested one an entry");
    }

    #[test]
    fn closed_is_one_more_generic_field() {
        assert!(GENERIC.contains(&field::CLOSED));
        assert!(TYPES.iter().all(|(_, _, fields)| fields.contains(&field::CLOSED)));
        assert_eq!(field_def(field::CLOSED).map(|d| (d.name, d.ty)), Some(("closed", FieldType::Replica)));
    }
}
