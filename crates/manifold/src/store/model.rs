//! The shared document's data model (spec "Data model"): ids, the hybrid
//! logical clock, node kinds, the field table, and nodes whose fields are
//! last-writer-wins registers holding their field's own TLV encoding.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use super::ops::MoveRec;
use super::tlv::{Reader, TlvError, Writer};
use super::tree::TreePath;

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

/// Serde spells a `NodeId` as its `Display` text (also usable as a JSON map key); the store's own codec does not
/// go through serde.
impl serde::Serialize for NodeId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for NodeId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = <std::borrow::Cow<str>>::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
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
pub const TYPE_LABEL: &str = "application/vnd.manifold.label";
pub const TYPE_NOTE: &str = "text/markdown";
pub const TYPE_PDF: &str = "application/pdf";
pub const TYPE_JSON: &str = "application/json";
/// What a created image is stored as until sniffed types arrive (plan 2c); any `image/*` reads back as an image.
pub const TYPE_IMAGE: &str = "image/png";
pub const TYPE_DOWNLOAD: &str = "application/octet-stream";
/// Every virtual card's type starts with this: a card drawn from other state, with no node (spec
/// 2026-10-16-virtual-cards-design.md).
pub const TYPE_VIRTUAL_PREFIX: &str = "application/vnd.manifold.virtual.";
/// Never stored: a page of the focused tab's trail strip (`trail_strip`). It sits under `TYPE_VIRTUAL_PREFIX`, so only
/// its exact row's abilities (drag-out) apply.
pub const TYPE_TRAIL_PAGE: &str = "application/vnd.manifold.virtual.trail-page";
/// Never stored: the kind of a JSON level's cards (`level::level_items_from`), drawn by `draw_json_level`. Its row
/// grants no ability, so the shared chrome (selection, drop targets, the marquee) leaves them alone.
pub const TYPE_JSON_LEVEL: &str = "application/vnd.manifold.json-level";
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
    /// A name and a colour cards can carry (`PROPS/labels`): a stored node, never a graph card.
    Label,
    /// No `type` yet (an op named the node before its `Create` arrived), or a type a newer build wrote. The node
    /// keeps its type string (`Node::ty`); the app reads only its generic fields (`GENERIC`).
    Unknown,
}

/// The type table: each kind, the type string it is stored under, and the fields a node of it carries, `group`
/// aside (spec, "The shared document"). `parent` ("opened from") is on every kind a page can make: a tab, a download,
/// and content a tab became. Older builds don't write it for the non-tab kinds, but keep it (`Node` holds every
/// register, and a `Create` only writes the fields listed here). Fields a type does not list stay raw in the document
/// untouched.
#[rustfmt::skip]
pub(crate) const TYPES: [(Kind, &str, &[u32]); 10] = {
    use field::*;
    [
        (Kind::Tab, TYPE_TAB, &[POS, PARENT, URL, VIEW_KIND, ANCHOR, JAR, NAME, CLOSED]),
        (Kind::Group, TYPE_GROUP, &[POS, ANCHOR, NAME, JAR, CLOSED, HELD]),
        (Kind::Note, TYPE_NOTE, &[POS, ANCHOR, PARENT, URL, TEXT, NAME, CLOSED]),
        (Kind::Image, TYPE_IMAGE, &[POS, ANCHOR, PARENT, URL, PATH, NAME, CLOSED]),
        (Kind::Pdf, TYPE_PDF, &[POS, ANCHOR, PARENT, URL, PATH, NAME, CLOSED]),
        (Kind::Json, TYPE_JSON, &[POS, ANCHOR, PARENT, URL, PATH, VIEW, NAME, CLOSED]),
        (Kind::Download, TYPE_DOWNLOAD, &[POS, ANCHOR, PARENT, URL, PATH, NAME, CLOSED]),
        (Kind::Jar, TYPE_JAR, &[POS, ANCHOR, COLOR, NAME, EPHEMERAL, IS_DEFAULT, CLOSED]),
        (Kind::Link, TYPE_LINK, &[NAME, FROM, TO, CLOSED]),
        (Kind::Label, TYPE_LABEL, &[NAME, COLOR, CLOSED]),
    ]
};

/// The fields every card carries whatever its type: all the app reads, mirrors and copies of a node of a type this
/// build doesn't know (`Kind::Unknown`, spec decision 3). Its other fields stay raw in the document.
pub(crate) const GENERIC: &[u32] = &[field::POS, field::ANCHOR, field::PARENT, field::NAME, field::CLOSED];

impl Kind {
    /// The type string a created node of this kind is stored under.
    pub fn type_str(self) -> Option<&'static str> {
        TYPES.iter().find(|t| t.0 == self).map(|t| t.1)
    }

    /// Whether a node of this kind is a graph card; a stored-only kind (a link, a label) never is.
    pub fn card(self) -> bool {
        !matches!(self, Kind::Link | Kind::Label)
    }

    /// The fields a node of this kind carries, `group` aside (`TYPES`): what a `Create` writes, what another
    /// replica's change hands the app, and what a snapshot of the node holds. `GENERIC` for
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

/// Engine codes for the `view_kind` field: code `n` is `VIEW_KINDS[n - 1]`,
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
    /// `field::HELD`: the group a fold sent its members to and the members, `{1: dest id, 2: member id*}`; absent
    /// is the top level and no members.
    Held,
    /// `field::ANCHOR`: a card held at a point, `{1: x, 2: y, 3: group id, 4: explicit}` (`Anchor`); absent is none.
    Anchor,
}

#[derive(Debug)]
pub struct FieldDef {
    pub no: u32,
    pub name: &'static str,
    pub ty: FieldType,
}

/// `field::ANCHOR`'s value: a card held at `pos`, in its group's local coordinates (as `POS`), written while the card
/// was in `group`. `explicit` is a user Pin, which only Unpin releases; otherwise it is a drop, which Tidy
/// and a local move release, and which holds only while the card is still in `group` (`Mirror::held`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Anchor {
    pub pos: [f32; 2],
    pub group: Option<NodeId>,
    pub explicit: bool,
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
    // Never reuse 7 (the retired `pinned` bool, replaced by `ANCHOR`).
    pub const NAME: u32 = 8;
    // Never reuse 9, 12 and 13 (the retired JSON leaf's `from_json`, `key` and `value`). A document that still holds
    // them keeps them raw, like any field a type does not list.
    pub const TEXT: u32 = 10;
    pub const PATH: u32 = 11;
    /// A Json node's view overrides (`json_view::JsonView`), compact JSON; "" when none.
    pub const VIEW: u32 = 14;
    /// The `jar` edge of a tab or group: the `Kind::Jar` node it uses; absent to inherit (`App::effective_jar`).
    /// Never reuse 16 and 17 (`RESERVED`).
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
    /// On a group closed by Ungroup or Dissolve Group: the ids of the members it had, space separated, so reopening
    /// it puts them back (`App::regroup_held`); "" otherwise.
    pub const HELD: u32 = 24;
    /// Tree fields (no `FieldDef`: `Doc::apply` keeps them raw, paths included). PROPS: a card's properties; BODY: an
    /// inline Json card's document.
    pub const PROPS: u32 = 25;
    pub const BODY: u32 = 26;
    /// A card's hold (`Anchor`): an explicit Pin or a drop. Replaces the retired `pinned` bool (7).
    pub const ANCHOR: u32 = 27;
}

pub static FIELDS: [FieldDef; 19] = [
    FieldDef { no: field::POS, name: "pos", ty: FieldType::Pos },
    FieldDef { no: field::COLOR, name: "color", ty: FieldType::Color },
    FieldDef { no: field::GROUP, name: "group", ty: FieldType::Id },
    FieldDef { no: field::PARENT, name: "parent", ty: FieldType::Id },
    FieldDef { no: field::URL, name: "url", ty: FieldType::Str },
    FieldDef { no: field::VIEW_KIND, name: "view_kind", ty: FieldType::ViewCode },
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
    FieldDef { no: field::HELD, name: "held", ty: FieldType::Held },
    FieldDef { no: field::ANCHOR, name: "anchor", ty: FieldType::Anchor },
];

/// Retired: 7, the `pinned` bool (replaced by `ANCHOR`); 16 and 17, a tab's `jar_set` and a group's `default_jar`
/// (replaced by the `jar` edge). Never reused; old documents keep them raw (checked by a test).
#[cfg(test)]
pub const RESERVED: [u32; 3] = [7, 16, 17];

pub fn field_def(no: u32) -> Option<&'static FieldDef> {
    FIELDS.iter().find(|f| f.no == no)
}

pub fn field_by_name(name: &str) -> Option<&'static FieldDef> {
    FIELDS.iter().find(|f| f.name == name)
}

#[derive(Debug, Clone, PartialEq)]
pub enum FieldValue {
    /// A card's document position; `None`: unplaced (it spawns at a jitter from its id, `session_sync::pos_of`).
    Pos(Option<(f32, f32)>),
    Color(Option<u32>),
    Id(Option<NodeId>),
    Str(String),
    Bool(bool),
    ViewCode(u64),
    Replica(u64),
    /// What a fold took out of a group: where its members went (`None`: the top level) and who they were.
    Held {
        dest: Option<NodeId>,
        members: Vec<NodeId>,
    },
    /// A card's hold; `None`: not held.
    Anchor(Option<Anchor>),
}

impl FieldValue {
    /// What a missing field decodes to.
    pub fn absent(ty: FieldType) -> FieldValue {
        match ty {
            FieldType::Pos => FieldValue::Pos(None),
            FieldType::Color => FieldValue::Color(None),
            FieldType::Id => FieldValue::Id(None),
            FieldType::Str => FieldValue::Str(String::new()),
            FieldType::Bool => FieldValue::Bool(false),
            FieldType::ViewCode => FieldValue::ViewCode(0),
            FieldType::Replica => FieldValue::Replica(0),
            FieldType::Held => FieldValue::Held { dest: None, members: Vec::new() },
            FieldType::Anchor => FieldValue::Anchor(None),
        }
    }

    /// The text of a `Str`; `None` for any other variant.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            FieldValue::Str(s) => Some(s),
            _ => None,
        }
    }

    /// The node an `Id` names; `None` for no node or any other variant.
    pub fn as_id(&self) -> Option<NodeId> {
        match self {
            FieldValue::Id(id) => *id,
            _ => None,
        }
    }

    /// Whether this is `Bool(true)`.
    pub fn as_bool(&self) -> bool {
        matches!(self, FieldValue::Bool(true))
    }

    /// A placed `Pos`'s coordinates; `None` for an unplaced one or any other variant.
    pub fn as_pos(&self) -> Option<(f32, f32)> {
        match self {
            FieldValue::Pos(pos) => *pos,
            _ => None,
        }
    }

    /// The stored view code of a `ViewCode`; 0 (none) for any other variant.
    pub fn as_view_code(&self) -> u64 {
        match self {
            FieldValue::ViewCode(code) => *code,
            _ => 0,
        }
    }

    /// A set `Anchor`; `None` for none or any other variant.
    pub fn as_anchor(&self) -> Option<Anchor> {
        match self {
            FieldValue::Anchor(anchor) => *anchor,
            _ => None,
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
        // A placed position is written even at the origin (an empty message), so it never reads as unplaced.
        FieldValue::Pos(Some((x, y))) => {
            let mut m = Writer::new();
            if *x != 0.0 {
                m.f32(1, *x);
            }
            if *y != 0.0 {
                m.f32(2, *y);
            }
            w.message(no, &m);
        }
        FieldValue::Color(Some(rgb)) => w.varint(no, u64::from(*rgb)),
        FieldValue::Id(Some(id)) => w.message(no, &id_message(*id)),
        FieldValue::Str(s) if !s.is_empty() => w.str(no, s),
        FieldValue::Bool(true) => w.varint(no, 1),
        FieldValue::ViewCode(code) | FieldValue::Replica(code) if *code != 0 => w.varint(no, *code),
        FieldValue::Held { dest, members } if dest.is_some() || !members.is_empty() => {
            let mut m = Writer::new();
            if let Some(dest) = dest {
                m.message(1, &id_message(*dest));
            }
            for member in members {
                m.message(2, &id_message(*member));
            }
            w.message(no, &m);
        }
        // A set anchor is written even at the top-level origin with no bit (an empty message), so it never reads as
        // absent.
        FieldValue::Anchor(Some(a)) => {
            let mut m = Writer::new();
            if a.pos[0] != 0.0 {
                m.f32(1, a.pos[0]);
            }
            if a.pos[1] != 0.0 {
                m.f32(2, a.pos[1]);
            }
            if let Some(group) = a.group {
                m.message(3, &id_message(group));
            }
            if a.explicit {
                m.varint(4, 1);
            }
            w.message(no, &m);
        }
        // The empty or zero value of each type is "absent": nothing is written.
        FieldValue::Pos(None)
        | FieldValue::Color(None)
        | FieldValue::Id(None)
        | FieldValue::Str(_)
        | FieldValue::Bool(false)
        | FieldValue::ViewCode(_)
        | FieldValue::Replica(_)
        | FieldValue::Held { .. }
        | FieldValue::Anchor(None) => {}
    }
    *out = w.into_bytes();
}

pub(super) fn id_message(id: NodeId) -> Writer {
    let mut m = Writer::new();
    if id.actor.0 != 0 {
        m.fixed64(1, id.actor.0);
    }
    if id.counter != 0 {
        m.varint(2, id.counter);
    }
    m
}

pub(super) fn decode_id(buf: &[u8]) -> Result<NodeId, TlvError> {
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
/// last occurrence wins). A non-finite position or anchor point is not one: it
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
                FieldValue::Pos((x.is_finite() && y.is_finite()).then_some((x, y)))
            }
            FieldType::Color => FieldValue::Color(Some(v.u32()?)),
            FieldType::Id => FieldValue::Id(Some(decode_id(v.bytes()?)?)),
            FieldType::Str => FieldValue::Str(v.str()?.to_owned()),
            FieldType::Bool => FieldValue::Bool(v.bool()?),
            FieldType::ViewCode => FieldValue::ViewCode(v.u64()?),
            FieldType::Replica => FieldValue::Replica(v.u64()?),
            FieldType::Held => {
                let (mut dest, mut members) = (None, Vec::new());
                for f in Reader::new(v.bytes()?) {
                    let f = f?;
                    match f.no {
                        1 => dest = Some(decode_id(f.value.bytes()?)?),
                        2 => members.push(decode_id(f.value.bytes()?)?),
                        _ => {}
                    }
                }
                FieldValue::Held { dest, members }
            }
            FieldType::Anchor => {
                let (mut x, mut y, mut group, mut explicit) = (0.0f32, 0.0f32, None, false);
                for f in Reader::new(v.bytes()?) {
                    let f = f?;
                    match f.no {
                        1 => x = f.value.f32()?,
                        2 => y = f.value.f32()?,
                        3 => group = Some(decode_id(f.value.bytes()?)?),
                        4 => explicit = f.value.bool()?,
                        _ => {}
                    }
                }
                FieldValue::Anchor((x.is_finite() && y.is_finite()).then_some(Anchor { pos: [x, y], group, explicit }))
            }
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

/// Where a register lives in a node: a field and a path inside that field's tree. Every scalar field sits at the
/// root path; only tree fields (`field::PROPS`, `field::BODY`, and numbers this build doesn't know) have deeper ones.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RegKey {
    pub field: u32,
    pub path: TreePath,
}

impl RegKey {
    pub fn of(no: u32) -> RegKey {
        RegKey { field: no, path: TreePath::default() }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    /// Every field but `group`, by field and path, `type` included, and numbers this
    /// build doesn't know (kept raw so compaction writes them back).
    pub fields: BTreeMap<RegKey, Reg>,
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

    /// Field `no`'s register at the root path: every scalar field's only one.
    pub fn reg(&self, no: u32) -> Option<&Reg> {
        self.fields.get(&RegKey::of(no))
    }

    /// Created: an op wrote its `type`.
    pub fn has_type(&self) -> bool {
        self.reg(field::TYPE).is_some()
    }

    /// Its stored `type` string as written ("" before its `Create` arrives): the last occurrence in the register,
    /// as `decode_field` reads it.
    pub fn ty(&self) -> &str {
        let Some(reg) = self.reg(field::TYPE) else { return "" };
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
        Node { fields: BTreeMap::from([(RegKey::of(field::TYPE), reg)]), group: None, deleted: false }
    }

    /// Field `no`'s value; its default when unset or undecodable.
    pub fn get(&self, no: u32) -> FieldValue {
        let def = field_def(no).expect("a field number from `field`");
        self.reg(no).and_then(|reg| decode_field(def, &reg.enc).ok()).unwrap_or_else(|| FieldValue::absent(def.ty))
    }

    /// This node as a plain node message, `group` included: what a
    /// `Create` carries.
    #[cfg(test)]
    pub fn message(&self) -> Vec<u8> {
        let mut w = Writer::new();
        let group = encode_field(field::GROUP, &FieldValue::Id(self.group));
        let mut group_written = false;
        for (key, reg) in self.fields.iter().filter(|(k, _)| k.path.is_root()) {
            if key.field > field::GROUP && !group_written {
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
    /// (checked first; any node, group or not), else `Shown(effective_group)`. `id`'s own `closed` is
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

    /// Shown in the graph: live, not itself closed, and not under a closed node. A live link is always shown:
    /// the mirror keeps every one, and drawing asks for both ends.
    pub fn is_shown(&self, id: NodeId) -> bool {
        let Some(node) = self.nodes.get(&id).filter(|n| n.is_live()) else { return false };
        !node.kind().card() || (!node.closed() && self.placement(id) != Placement::Hidden)
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
            .filter_map(|(id, n)| Some((id, n.reg(field::CLOSED)?.stamp)))
            .collect();
        roots.sort_by_key(|&(id, stamp)| std::cmp::Reverse((stamp, id)));
        roots
    }

    /// `nodes` and every node below one of them: where a node is shown
    /// depends on each group above it (`effective_group`).
    pub fn with_descendants(&self, nodes: BTreeSet<NodeId>) -> BTreeSet<NodeId> {
        let mut out = nodes.clone();
        for &id in self.nodes.keys() {
            if crate::walk::ancestors(id, |k| self.nodes.get(k).and_then(|n| n.group)).any(|a| nodes.contains(&a)) {
                out.insert(id);
            }
        }
        out
    }

    /// Whether `ancestor` is on the path from `of` to the root, `of`
    /// excluded. Walks deleted nodes too, so every replica answers alike.
    pub fn is_ancestor(&self, ancestor: NodeId, of: NodeId) -> bool {
        let mut above = crate::walk::ancestors(of, |k| self.nodes.get(k).and_then(|n| n.group));
        above.any(|a| a == ancestor) || above.looped() // a cycle; `apply_move` never makes one
    }
}

#[cfg(test)]
mod tests;
