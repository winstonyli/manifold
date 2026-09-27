//! The live graph and the session store (spec "Local edits become ops").
//! The first half is pure: each live node's shared fields (`Shared`), and
//! the diff that turns local edits into ops. The `impl App` half after it
//! restores the session, saves local edits, and shows other replicas' ops.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use ipc_protocol::HostToEngine;

use crate::local_state::{LiveUrl, LocalNode, LocalState};
use crate::node::{
    DownloadPayload, GroupPayload, ImagePayload, JsonPayload, NodePayload, NotePayload, PdfPayload, TabPayload,
    UnloadedState, UnloadedTabPayload,
};
use crate::process_manager::EngineKind;
use crate::store::journal::{JournalError, Recovery};
use crate::store::json::{Imported, JsonDoc};
use crate::store::model::{encode_field, encode_field_into, field, field_def, Doc, FieldValue, Kind, Node, NodeId};
use crate::store::ops::OpBody;
use crate::{App, ClosedEntry, TabSpawn, ViewMode, MAX_CLOSED};

/// How long after a save that didn't commit `poll_store` tries again:
/// soon enough that an edit isn't left unsaved, slow enough that a failing
/// disk isn't retried every frame.
const SAVE_RETRY: std::time::Duration = std::time::Duration::from_secs(1);

/// How long after a host-started load last reported loading its tab's url
/// changes stay local (`App::shares_url_change`), when the engine doesn't
/// say how they started: a script redirect can come just after the page
/// reports loaded.
const HOST_LOAD_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// A node's shared fields by number: every field of its kind but `group`.
pub(crate) type Fields = BTreeMap<u32, FieldValue>;

/// The fields each kind carries, `group` aside (spec, "The shared document").
pub(crate) fn kind_fields(kind: Kind) -> &'static [u32] {
    use field::*;
    match kind {
        Kind::Tab => &[POS, COLOR, PARENT, URL, ENGINE, PINNED],
        Kind::Group => &[POS, COLOR, NAME, FROM_JSON],
        Kind::Note => &[POS, COLOR, URL, TEXT],
        Kind::Image | Kind::Pdf | Kind::Json => &[POS, COLOR, URL, PATH],
        Kind::Download => &[POS, COLOR, PATH],
        Kind::JsonLeaf => &[POS, COLOR, PATH, KEY, VALUE],
        Kind::Unknown(_) => &[],
    }
}

/// One live node as the store sees it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Shared {
    pub kind: Kind,
    pub group: Option<NodeId>,
    /// Exactly `kind_fields(kind)`, absent values included, so a field
    /// cleared live reads as a change.
    pub fields: Fields,
}

/// A document node's fields for its kind, shaped like `Shared::fields`.
pub(crate) fn fields_of(node: &Node) -> Fields {
    kind_fields(node.kind).iter().map(|&no| (no, node.get(no))).collect()
}

pub(crate) fn color_value(color: Option<egui::Color32>) -> FieldValue {
    FieldValue::Color(color.map(|c| u32::from_be_bytes([0, c.r(), c.g(), c.b()])))
}

pub(crate) fn color_of(value: &FieldValue) -> Option<egui::Color32> {
    match value {
        FieldValue::Color(Some(rgb)) => {
            let [_, r, g, b] = rgb.to_be_bytes();
            Some(egui::Color32::from_rgb(r, g, b))
        }
        _ => None,
    }
}

/// `store::model::ENGINES` order: code `n` is `ENGINES[n - 1]`.
pub(crate) fn engine_code(kind: EngineKind) -> u64 {
    match kind {
        EngineKind::Servo => 1,
        EngineKind::Cef => 2,
        EngineKind::Webkit => 3,
        EngineKind::Webview2 => 4,
    }
}

/// Unknown codes fall back to Servo, as `engine_kind_serde` does for names.
pub(crate) fn engine_of(code: u64) -> EngineKind {
    match code {
        2 => EngineKind::Cef,
        3 => EngineKind::Webkit,
        4 => EngineKind::Webview2,
        _ => EngineKind::Servo,
    }
}

/// The ops that make `doc` show `live` (spec "Local edits become ops").
/// - A live node the document lacks is created, each group before the
///   nodes inside it. A live id the document holds as a tombstone is
///   never recreated (ids are never reused).
/// - A node shown in another group moves. Its group is compared with
///   `effective_group`, so re-homing out of a deleted group is not a move.
/// - A diffed field that differs is `Set`. An explicit field (`pos`,
///   `url`) is `Set` only when `explicit` names it.
/// - A document node missing from `live` is deleted, unless `keep` names
///   it (a tab whose spawn is in flight).
///
/// A group or parent naming a node that is neither live in the document
/// nor created here is written as none (contract "Missing means deleted").
pub(crate) fn diff(
    doc: &Doc,
    live: &BTreeMap<NodeId, Shared>,
    explicit: &BTreeSet<(NodeId, u32)>,
    keep: &BTreeSet<NodeId>,
) -> Vec<OpBody> {
    let in_doc = |id: &NodeId| doc.nodes.get(id).is_some_and(Node::is_live);
    let new: BTreeSet<NodeId> = live.keys().filter(|id| !doc.nodes.contains_key(id)).copied().collect();
    let valid = |id: Option<NodeId>| id.filter(|id| in_doc(id) || new.contains(id));
    let mut ops = Vec::new();

    // Creates, each group's chain of new ancestors first.
    let mut order = Vec::new();
    let mut queued = BTreeSet::new();
    for &start in &new {
        let mut chain = Vec::new();
        let mut at = Some(start);
        while let Some(id) = at.filter(|id| new.contains(id) && !queued.contains(id) && !chain.contains(id)) {
            chain.push(id);
            at = live[&id].group;
        }
        for id in chain.into_iter().rev() {
            queued.insert(id);
            order.push(id);
        }
    }
    let mut created = BTreeSet::new();
    for id in order {
        let node = &live[&id];
        // A group created later in this batch (only a cycle does that)
        // would make a placeholder: create at the top level instead.
        let group = valid(node.group).filter(|g| in_doc(g) || created.contains(g));
        let mut all = node.fields.clone();
        all.insert(field::GROUP, FieldValue::Id(group));
        if let Some(FieldValue::Id(parent)) = all.get_mut(&field::PARENT) {
            *parent = valid(*parent);
        }
        let fields = all.iter().flat_map(|(&no, value)| encode_field(no, value)).collect();
        ops.push(OpBody::Create { node: id, kind: node.kind, fields });
        created.insert(id);
    }

    // Existing nodes: moves and sets. Each field is encoded into `enc` and
    // copied out only when it differs.
    let mut enc = Vec::new();
    for (&id, node) in live {
        let Some(current) = doc.nodes.get(&id).filter(|n| n.is_live()) else { continue };
        let group = valid(node.group);
        if group != doc.effective_group(id) {
            ops.push(OpBody::Move { node: id, group });
        }
        for (&no, value) in &node.fields {
            if field_def(no).is_some_and(|def| def.explicit) && !explicit.contains(&(id, no)) {
                continue;
            }
            match value {
                FieldValue::Id(parent) => encode_field_into(&mut enc, no, &FieldValue::Id(valid(*parent))),
                other => encode_field_into(&mut enc, no, other),
            }
            if current.fields.get(&no).map_or(&[][..], |reg| &reg.enc[..]) != &enc[..] {
                ops.push(OpBody::Set { node: id, field: no, enc: enc.clone() });
            }
        }
    }

    for (id, _) in doc.live() {
        if !live.contains_key(&id) && !keep.contains(&id) {
            ops.push(OpBody::Delete { node: id });
        }
    }
    ops
}

// ---- Store notices and repairs -------------------------------------------

/// What to tell the user about a repair (contract "Repairs").
pub(crate) fn recovery_notice(r: &Recovery) -> String {
    let what = if r.new_document {
        "The session file's start was lost; what survived was kept as a new session".to_owned()
    } else if r.from_prev {
        "The session file was damaged and rebuilt from its previous copy".to_owned()
    } else {
        let n = r.damaged.len().max(r.skipped_frames);
        format!("The session file was damaged; {n} damaged part{} skipped", if n == 1 { " was" } else { "s were" })
    };
    match &r.saved {
        Some(path) => format!("{what}. The original is saved at {}", path.display()),
        None => format!("{what}. The original could not be saved, so nothing new is written until it can be"),
    }
}

// ---- The live graph and the store -----------------------------------------

/// One node another replica changed: its kind, fields and effective
/// group, or `None` when it is no longer live (deleted or pruned).
pub(crate) type RemoteChange = (NodeId, Option<(Kind, Fields, Option<NodeId>)>);

/// Where `App::materialize_imported` puts an imported file's root (its
/// first node): the level it's brought into, and optionally a fresh
/// position there.
pub(crate) struct ImportRoot {
    pub group: Option<String>,
    pub pos: Option<egui::Vec2>,
}

/// A node's kind, fields and group, as `materialize` takes them.
pub(crate) type RemoteNode = (Kind, Fields, Option<String>);

fn str_of(fields: &Fields, no: u32) -> String {
    match fields.get(&no) {
        Some(FieldValue::Str(s)) => s.clone(),
        _ => String::new(),
    }
}

/// The URL field as `Option<String>`: absent (empty string) is `None`.
fn url_of(fields: &Fields) -> Option<String> {
    Some(str_of(fields, field::URL)).filter(|u| !u.is_empty())
}

fn bool_of(fields: &Fields, no: u32) -> bool {
    matches!(fields.get(&no), Some(FieldValue::Bool(true)))
}

fn pos_of(fields: &Fields) -> egui::Vec2 {
    match fields.get(&field::POS) {
        Some(FieldValue::Pos(x, y)) => egui::vec2(*x, *y),
        _ => egui::Vec2::ZERO,
    }
}

fn id_of(fields: &Fields, no: u32) -> Option<String> {
    match fields.get(&no) {
        Some(FieldValue::Id(Some(id))) => Some(id.to_string()),
        _ => None,
    }
}

fn engine_field(fields: &Fields) -> EngineKind {
    engine_of(match fields.get(&field::ENGINE) {
        Some(FieldValue::Engine(code)) => *code,
        _ => 0,
    })
}

/// `pos` as the store keeps it: never non-finite (contract "Fields").
fn pos_value(pos: egui::Vec2) -> FieldValue {
    if pos.x.is_finite() && pos.y.is_finite() {
        FieldValue::Pos(pos.x, pos.y)
    } else {
        FieldValue::Pos(0.0, 0.0)
    }
}

fn parse_id(id: &Option<String>) -> Option<NodeId> {
    id.as_deref().and_then(|id| id.parse().ok())
}

/// The tab `spawn` starts, as `Shared`: what `save_session` writes for a
/// tab whose spawn hasn't landed yet (`App::spawning`).
pub(crate) fn spawn_shared(spawn: &TabSpawn) -> Shared {
    let mut fields = Fields::from([(field::POS, pos_value(spawn.pos)), (field::COLOR, color_value(spawn.carry.color))]);
    let kind = tab_fields(&mut fields, &spawn.url, spawn.kind, spawn.carry.pinned, parse_id(&spawn.parent));
    Shared { kind, group: parse_id(&spawn.group), fields }
}

/// A tab's fields beyond `pos` and `color`, live or failed.
fn tab_fields(fields: &mut Fields, url: &str, engine: EngineKind, pinned: bool, parent: Option<NodeId>) -> Kind {
    fields.insert(field::URL, FieldValue::Str(url.to_owned()));
    fields.insert(field::ENGINE, FieldValue::Engine(engine_code(engine)));
    fields.insert(field::PINNED, FieldValue::Bool(pinned));
    fields.insert(field::PARENT, FieldValue::Id(parent));
    Kind::Tab
}

/// `raw` (an imported node's field values, as `store::json::import` gave
/// them) shaped like `Shared::fields`: exactly `kind_fields(kind)`, with
/// any field the file left out filled in as absent, for
/// `App::materialize_imported`.
pub(crate) fn imported_fields(kind: Kind, raw: &BTreeMap<u32, FieldValue>) -> Fields {
    kind_fields(kind)
        .iter()
        .map(|&no| {
            let absent = || FieldValue::absent(field_def(no).expect("a known field").ty);
            (no, raw.get(&no).cloned().unwrap_or_else(absent))
        })
        .collect()
}

impl App {
    /// Every live node the store keeps, as `Shared`. Skipped: an
    /// in-progress download (it can't resume), and a node whose id isn't a
    /// store id (only hand-built test nodes).
    pub(crate) fn shared_nodes(&self) -> BTreeMap<NodeId, Shared> {
        let str = |s: &str| FieldValue::Str(s.to_owned());
        let mut out = BTreeMap::new();
        for (key, node) in &self.nodes {
            let Ok(id) = key.parse::<NodeId>() else { continue };
            let mut fields = Fields::from([(field::POS, pos_value(node.pos)), (field::COLOR, color_value(node.color))]);
            let kind = match &node.payload {
                // A swap in progress already shows its target engine, so a
                // save meanwhile doesn't write the old one back.
                NodePayload::Tab(TabPayload { tab, url, pinned, swap_target, .. }) => {
                    tab_fields(&mut fields, url, swap_target.unwrap_or(tab.kind), *pinned, parse_id(&node.parent))
                }
                NodePayload::Unloaded(UnloadedTabPayload { url, kind, pinned, .. }) => {
                    tab_fields(&mut fields, url, *kind, *pinned, parse_id(&node.parent))
                }
                NodePayload::Download(dl) if dl.completed => {
                    fields.insert(field::PATH, str(&dl.path));
                    Kind::Download
                }
                NodePayload::Download(_) => continue,
                NodePayload::Group(g) => {
                    fields.insert(field::NAME, str(&g.name));
                    fields.insert(field::FROM_JSON, FieldValue::Bool(g.from_json));
                    Kind::Group
                }
                NodePayload::Note(n) => {
                    fields.insert(field::URL, str(n.url.as_deref().unwrap_or("")));
                    fields.insert(field::TEXT, str(&n.text));
                    Kind::Note
                }
                NodePayload::Image(i) => {
                    fields.insert(field::URL, str(i.url.as_deref().unwrap_or("")));
                    fields.insert(field::PATH, str(&i.path));
                    Kind::Image
                }
                NodePayload::Pdf(p) => {
                    fields.insert(field::URL, str(p.url.as_deref().unwrap_or("")));
                    fields.insert(field::PATH, str(&p.path));
                    Kind::Pdf
                }
                NodePayload::Json(j) => {
                    fields.insert(field::PATH, str(&j.path));
                    match &j.key {
                        None => {
                            fields.insert(field::URL, str(j.url.as_deref().unwrap_or("")));
                            Kind::Json
                        }
                        Some(key) => {
                            fields.insert(field::KEY, str(key));
                            fields.insert(field::VALUE, FieldValue::Str(j.value.to_string()));
                            Kind::JsonLeaf
                        }
                    }
                }
            };
            debug_assert_eq!(fields.keys().copied().collect::<Vec<_>>(), {
                let mut want = kind_fields(kind).to_vec();
                want.sort();
                want
            });
            out.insert(id, Shared { kind, group: parse_id(&node.group), fields });
        }
        out
    }

    /// Makes live node `id` show `fields`. A new node is inserted; a new
    /// tab arrives asleep (spec "Lazy tabs"), whatever created it: a
    /// restore, another replica, Recently Closed or an import. A node
    /// whose spawn is in flight keeps the change
    /// for when it lands (`remote_in_flight`). An existing node is updated
    /// in place, but only in the fields (and group) where the store
    /// differs from what this replica last saw of it (`last_shared`), so
    /// an unsaved local value in another field survives:
    /// - a tab's url change navigates without marking `url`;
    /// - an engine change swaps engines.
    ///
    /// A kind mismatch between the live node and `kind` is ignored (the
    /// store never changes a node's kind). A file card whose file can't be
    /// loaded is kept, showing its error, so the next diff doesn't delete
    /// it from the store.
    ///
    /// `local`, personal state's entry for a new node, places it at its
    /// settled position instead of `fields`'s stored `pos`, and reopens a
    /// tab at its live url (`LocalNode::live_url`); `last_shared` still
    /// records the real stored fields, so a later save doesn't diff the
    /// overrides back into the store. Every caller but `restore_session`
    /// passes `None`.
    pub(crate) fn materialize(
        &mut self,
        id: String,
        kind: Kind,
        fields: &Fields,
        group: Option<String>,
        local: Option<&LocalNode>,
    ) {
        if self.spawning.contains_key(&id) {
            self.remote_in_flight.insert(id, Some((kind, fields.clone(), group)));
            return;
        }
        let pos = local.map_or_else(|| pos_of(fields), |l| egui::vec2(l.pos[0], l.pos[1]));
        let color = fields.get(&field::COLOR).and_then(color_of);
        let now = Shared { kind, group: group.as_deref().and_then(|g| g.parse().ok()), fields: fields.clone() };
        let seen = self.last_shared.insert(id.clone(), now.clone());
        if let Some(node) = self.nodes.get_mut(&id) {
            let changed = |no: u32| seen.as_ref().is_none_or(|s| s.fields.get(&no) != now.fields.get(&no));
            if seen.as_ref().is_none_or(|s| s.group != now.group) {
                node.group = group;
            }
            if changed(field::COLOR) {
                node.color = color;
            }
            if changed(field::POS) {
                node.pos = pos;
                node.vel = egui::Vec2::ZERO;
            }
            if kind == Kind::Tab && changed(field::PARENT) {
                node.parent = id_of(fields, field::PARENT);
            }
            let mut swap = None;
            let mut store_load = false;
            match &mut node.payload {
                NodePayload::Tab(tab) => {
                    if changed(field::PINNED) {
                        tab.pinned = bool_of(fields, field::PINNED);
                    }
                    let url = str_of(fields, field::URL);
                    if changed(field::URL) && tab.url != url {
                        tab.url = url.clone();
                        tab.tab.send(&HostToEngine::Navigate { url, referrer: None });
                        store_load = true;
                    }
                    let want = engine_field(fields);
                    if changed(field::ENGINE) && tab.swap_target.unwrap_or(tab.tab.kind) != want {
                        swap = Some(want);
                    }
                }
                NodePayload::Unloaded(tab) => {
                    if changed(field::PINNED) {
                        tab.pinned = bool_of(fields, field::PINNED);
                    }
                    if changed(field::URL) {
                        tab.url = str_of(fields, field::URL);
                    }
                    if changed(field::ENGINE) {
                        tab.kind = engine_field(fields);
                    }
                }
                NodePayload::Group(g) => {
                    if changed(field::NAME) {
                        g.name = str_of(fields, field::NAME);
                    }
                    if changed(field::FROM_JSON) {
                        g.from_json = bool_of(fields, field::FROM_JSON);
                    }
                }
                NodePayload::Note(n) => {
                    if changed(field::TEXT) {
                        n.text = str_of(fields, field::TEXT);
                    }
                    if changed(field::URL) {
                        n.url = url_of(fields);
                    }
                }
                NodePayload::Image(p) if changed(field::URL) => p.url = url_of(fields),
                NodePayload::Pdf(p) if changed(field::URL) => p.url = url_of(fields),
                NodePayload::Json(p) if p.key.is_none() && changed(field::URL) => p.url = url_of(fields),
                // Paths, keys and values never change after creation.
                NodePayload::Download(_) | NodePayload::Image(_) | NodePayload::Pdf(_) | NodePayload::Json(_) => {}
            }
            if store_load {
                self.note_host_load(&id);
            }
            if let Some(target) = swap {
                self.start_swap_to(&id, target);
            }
            return;
        }
        // Resolved once up front (used by three of the arms below and by
        // the missing-file refetch check after them): `resolve_content_path`
        // prefers `stored` when it exists, else this machine's own copy in
        // the content dir, named by id.
        let content_path = matches!(kind, Kind::Image | Kind::Pdf | Kind::Json).then(|| {
            crate::resolve_content_path(crate::profile::content_dir().as_deref(), &id, &str_of(fields, field::PATH))
        });
        // Missing file, URL known (spec "Where fetched files live"):
        // refetched instead of shown as an error, whether it's genuinely
        // missing here or this is a node synced from another replica whose
        // path was never on this machine. Checked before the insert
        // helpers below run, so a known miss never reaches their own
        // "Couldn't open ..." toast (or, for Pdf, starts a doomed render)
        // in the first place -- a placeholder goes in directly instead,
        // and `refresh_node` (after this match) replaces it once the fetch
        // lands.
        let missing_with_url =
            content_path.as_deref().is_some_and(|p| !std::path::Path::new(p).exists()) && url_of(fields).is_some();
        match kind {
            Kind::Tab => {
                let shared = str_of(fields, field::URL);
                let live = local.and_then(|l| l.live_url.as_ref()).filter(|l| l.from == shared);
                let url = live.map_or(shared, |l| l.url.clone());
                let (kind, pinned, parent) =
                    (engine_field(fields), bool_of(fields, field::PINNED), id_of(fields, field::PARENT));
                // Title, mute and zoom come from personal state (`apply_local`).
                let zoom_factor = self.settings.zoom_factor;
                let (title, muted, state) = (String::new(), false, UnloadedState::Sleeping);
                let payload = UnloadedTabPayload { url, title, kind, pinned, muted, zoom_factor, state };
                self.insert_placeholder_node(id.clone(), pos, NodePayload::Unloaded(payload));
                if let Some(node) = self.nodes.get_mut(&id) {
                    node.parent = parent;
                }
            }
            Kind::Group => {
                let payload =
                    GroupPayload { name: str_of(fields, field::NAME), from_json: bool_of(fields, field::FROM_JSON) };
                self.insert_placeholder_node(id.clone(), pos, NodePayload::Group(payload));
            }
            Kind::Note => {
                let payload = NotePayload { text: str_of(fields, field::TEXT), editing: false, url: None };
                self.insert_placeholder_node(id.clone(), pos, NodePayload::Note(payload));
            }
            Kind::Image => {
                let path = content_path.clone().unwrap_or_default();
                if missing_with_url {
                    let payload = ImagePayload { path, width: 0, height: 0, error: None, url: None };
                    self.insert_placeholder_node(id.clone(), pos, NodePayload::Image(payload));
                } else if let Err(error) = self.insert_image_node_with_id(id.clone(), path.clone(), pos, None) {
                    let payload = ImagePayload { path, width: 0, height: 0, error: Some(error), url: None };
                    self.insert_placeholder_node(id.clone(), pos, NodePayload::Image(payload));
                }
            }
            Kind::Pdf => {
                let path = content_path.clone().unwrap_or_default();
                if missing_with_url {
                    let payload = PdfPayload { path, outcome: None, url: None };
                    self.insert_placeholder_node(id.clone(), pos, NodePayload::Pdf(payload));
                } else {
                    self.insert_pdf_node_with_id(id.clone(), path, pos, None);
                }
            }
            Kind::Json => {
                let path = content_path.clone().unwrap_or_default();
                if missing_with_url {
                    let payload =
                        JsonPayload { path, value: serde_json::Value::Null, key: None, error: None, url: None };
                    self.insert_placeholder_node(id.clone(), pos, NodePayload::Json(payload));
                } else if let Err(error) = self.insert_json_node_with_id(id.clone(), path.clone(), pos, None) {
                    let payload =
                        JsonPayload { path, value: serde_json::Value::Null, key: None, error: Some(error), url: None };
                    self.insert_placeholder_node(id.clone(), pos, NodePayload::Json(payload));
                }
            }
            Kind::JsonLeaf => {
                let text = str_of(fields, field::VALUE);
                let value = serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text));
                let key = Some(str_of(fields, field::KEY));
                let payload = JsonPayload { path: str_of(fields, field::PATH), value, key, error: None, url: None };
                self.insert_placeholder_node(id.clone(), pos, NodePayload::Json(payload));
            }
            Kind::Download => {
                let path = str_of(fields, field::PATH);
                let bytes = std::fs::metadata(&path).map_or(0, |m| m.len());
                let payload =
                    DownloadPayload { path, bytes_received: bytes, total_bytes: Some(bytes), completed: true };
                self.insert_placeholder_node(id.clone(), pos, NodePayload::Download(payload));
            }
            Kind::Unknown(_) => return,
        }
        if let Some(node) = self.nodes.get_mut(&id) {
            node.group = group;
            node.color = color;
            if let Some(url) = node.source_url_mut() {
                *url = url_of(fields);
            }
        }
        // The placeholder just inserted above (see `missing_with_url`)
        // shows until this fetch lands.
        if missing_with_url {
            self.refresh_node(&id);
        }
    }

    fn remote_changes(doc: &Doc, remote: &[NodeId]) -> Vec<RemoteChange> {
        remote
            .iter()
            .map(|&id| {
                let live = doc.nodes.get(&id).filter(|n| n.is_live());
                (id, live.map(|n| (n.kind, fields_of(n), doc.effective_group(id))))
            })
            .collect()
    }

    /// Shows other replicas' changes, before the next diff (contract
    /// "Missing means deleted"): the diff would otherwise delete what they
    /// created and recreate what they pruned. A deleted node is removed
    /// without a Recently Closed entry (`remove_remotely_deleted`), and a
    /// tab's engine host is shut down. Groups are materialized before the
    /// nodes inside them, so a new member never names a group that doesn't
    /// exist yet. A new tab arrives asleep (`materialize`, spec "Lazy
    /// tabs"): another replica opening a big session mustn't start an
    /// engine here for every tab in it.
    pub(crate) fn apply_remote(&mut self, mut changes: Vec<RemoteChange>) {
        changes.sort_by_key(|(_, c)| !matches!(c, Some((Kind::Group, ..))));
        let toasts = self.save_toast_queue.len();
        for (id, change) in changes {
            let key = id.to_string();
            match change {
                Some((kind, fields, group)) => self.materialize(key, kind, &fields, group.map(|g| g.to_string()), None),
                None if self.spawning.contains_key(&key) => {
                    self.remote_in_flight.insert(key, None);
                }
                None => {
                    self.last_shared.remove(&key);
                    self.remove_remotely_deleted(&key);
                }
            }
        }
        self.collapse_toasts_since(toasts);
    }

    /// Removes a node another replica deleted, recording nothing and saving
    /// nothing. Its tab children move to its parent. A group's members move
    /// to the group's own group (`dissolve_group`, which also leaves the
    /// level if it was showing): a member the store still has is in
    /// `remote` too (it lists descendants) and gets its effective group
    /// from there, and one only this replica has lands where the store
    /// would show it. Either way no live node names a removed group.
    pub(crate) fn remove_remotely_deleted(&mut self, id: &str) {
        let Some(node) = self.nodes.get(id) else { return };
        let (parent, up) = (node.parent.clone(), node.group.clone());
        let is_group = node.as_group().is_some();
        let is_tab = node.payload.saved_tab().is_some();
        for node in self.nodes.values_mut().filter(|n| n.parent.as_deref() == Some(id)) {
            node.parent = parent.clone();
        }
        if is_group {
            // A change waiting for a spawn may name this group too.
            for (_, _, group) in self.remote_in_flight.values_mut().flatten() {
                if group.as_deref() == Some(id) {
                    *group = up.clone();
                }
            }
            self.dissolve_group(id);
        } else if is_tab {
            self.remove_tab(id);
        } else {
            self.destroy_node(id); // saves nothing for these kinds
        }
        if self.viewed_image.as_deref() == Some(id) {
            self.viewed_image = None;
            if self.view_mode == ViewMode::Item {
                self.view_mode = ViewMode::Graph;
            }
        }
        if self.focused.as_deref() == Some(id) {
            self.focused = None;
        }
        self.selected_tabs.remove(id);
        self.wakes_in_flight.remove(id);
    }

    /// Applies what other replicas changed while `id`'s spawn was in flight,
    /// now that its node exists (`handle_tab_spawn_result`).
    pub(crate) fn land_remote_in_flight(&mut self, id: &str, remote: Option<Option<RemoteNode>>) {
        match remote {
            Some(Some((kind, fields, group))) => self.materialize(id.to_string(), kind, &fields, group, None),
            Some(None) => {
                self.last_shared.remove(id);
                self.remove_remotely_deleted(id);
            }
            None => {}
        }
    }

    /// Tab `id` is loading a url the host gave it (another replica's `url`,
    /// a restore, or a typed url, which `navigate_tab` marks itself): its
    /// unlabelled url changes stay local until the load settles
    /// (`shares_url_change`).
    pub(crate) fn note_host_load(&mut self, id: &str) {
        self.host_loads.insert(id.to_string(), std::time::Instant::now());
    }

    /// Starts the engines a restored session starts (spec "Lazy tabs"): the
    /// focused tab's, and the saved split tab's too when the session was
    /// saved split -- otherwise `apply_local` restores `split_tab` onto a
    /// card that never wakes, leaving the split pane empty forever.
    pub(crate) fn wake_focused(&mut self) {
        let asleep = |id: &str, this: &Self| {
            this.nodes
                .get(id)
                .and_then(|n| n.as_unloaded())
                .filter(|t| t.state == UnloadedState::Sleeping)
                .map(|t| t.kind)
        };
        if let Some(id) = self.focused.clone() {
            if let Some(kind) = asleep(&id, self) {
                self.wake_tab(&id, kind);
            }
        }
        if let Some(id) = self.split_tab.clone() {
            if let Some(kind) = asleep(&id, self) {
                self.wake_tab(&id, kind);
            }
        }
    }

    /// Whether tab `id`'s reported url change is shared (ruling 3), given
    /// how the engine says its navigation started (`user_started`). One a
    /// person started is; a page script's or the host's own `Navigate` (a
    /// typed url, which `navigate_tab` marks itself, or another replica's)
    /// isn't. Otherwise, not while it follows a load the host started, nor
    /// for `HOST_LOAD_GRACE` after the tab was last seen loading. Either
    /// way a site that redirects to a fresh url on every load (a nonce, a
    /// session token) can't have two instances navigate each other back
    /// and forth forever: the url stays local.
    pub(crate) fn shares_url_change(&mut self, id: &str, user_started: Option<bool>) -> bool {
        match user_started {
            Some(true) => {
                self.host_loads.remove(id);
                return true;
            }
            Some(false) => return false,
            None => {}
        }
        let Some(since) = self.host_loads.get_mut(id) else { return true };
        if self.nodes.get(id).and_then(|n| n.as_tab()).is_some_and(|t| t.loading) {
            *since = std::time::Instant::now();
            return false;
        }
        if since.elapsed() < HOST_LOAD_GRACE {
            return false;
        }
        self.host_loads.remove(id);
        true
    }

    /// Marks explicit field `field` (`pos` or `url`) of `id` for the next
    /// save (spec "Local edits become ops"). New nodes need no mark: their
    /// `Create` carries every field.
    pub(crate) fn mark(&mut self, id: &str, field: u32) {
        if let Ok(id) = id.parse() {
            self.explicit.insert((id, field));
        }
    }

    /// Saves browsing history to disk if `history_dirty` is set, then clears
    /// the flag. Called from `save_session` and `close_store` on every return
    /// path. History doesn't depend on the store, so it's saved regardless of
    /// store success or failure.
    fn flush_history(&mut self) {
        if self.history_dirty {
            crate::history::save(&self.history);
            self.history_dirty = false;
        }
    }

    /// Saves local edits (spec "Local edits become ops"): take the lock,
    /// show what other replicas wrote, diff the live graph against the
    /// document, and commit. On error the explicit marks stay for the next
    /// save, which re-diffs (contract "Transactions"). `true` when the
    /// store now holds every local edit: `false` read-only, on error, and
    /// when another instance kept writing. The last two are tried again
    /// by `poll_store` after `SAVE_RETRY`.
    pub(crate) fn save_session(&mut self) -> bool {
        // Each round applies what others wrote since the last; four rounds
        // of fresh remote frames means another instance is busy: try again
        // later (`save_retry_at`) rather than spin.
        for _ in 0..4 {
            // Projected before `begin`, since the `Txn` borrows the journal.
            let mut live = self.shared_nodes();
            let keep: BTreeSet<NodeId> = self.spawning.keys().filter_map(|id| id.parse().ok()).collect();
            let Some(journal) = self.journal.as_mut() else {
                self.flush_history();
                return false;
            };
            let txn = match journal.begin() {
                Ok(txn) => txn,
                Err(err) => {
                    self.store_error(err);
                    self.save_retry_at = Some(std::time::Instant::now() + SAVE_RETRY);
                    self.flush_history();
                    return false;
                }
            };
            if !txn.remote.is_empty() {
                let changes = Self::remote_changes(txn.doc(), &txn.remote);
                drop(txn);
                self.take_store_recovery();
                self.apply_remote(changes);
                continue;
            }
            if txn.read_only() {
                drop(txn);
                self.notice("This session was written by a newer manifold; changes aren't saved".to_string());
                self.take_store_recovery();
                self.flush_history();
                return false;
            }
            // A tab still spawning that the store doesn't hold yet is
            // created as it was started; one it does hold is only kept, so
            // other replicas' edits to it stand.
            for (id, shared) in &self.spawning {
                let Ok(id) = id.parse::<NodeId>() else { continue };
                if !txn.doc().nodes.contains_key(&id) {
                    live.entry(id).or_insert_with(|| shared.clone());
                }
            }
            let ops = diff(txn.doc(), &live, &self.explicit, &keep);
            // The nodes whose stored state this commit changes. A delete can
            // also change other nodes' effective group, so it refreshes all.
            let touched: Option<BTreeSet<NodeId>> = ops
                .iter()
                .map(|op| match op {
                    OpBody::Create { node, .. } | OpBody::Set { node, .. } | OpBody::Move { node, .. } => Some(*node),
                    OpBody::Delete { .. } => None,
                })
                .collect();
            let result = if ops.is_empty() {
                drop(txn);
                Ok(())
            } else {
                txn.commit(ops)
            };
            self.take_store_recovery();
            match result {
                Ok(()) => {
                    // What the store now holds is what this replica last
                    // saw: every diffed field as live, but an explicit one
                    // only if it was marked (an unmarked `pos` stays). Only
                    // `touched` nodes can differ from `last_shared`: every
                    // other store change (a remote op, a restore) already
                    // updated it in `materialize`.
                    if let Some(journal) = &self.journal {
                        let doc = journal.doc();
                        let ids: Box<dyn Iterator<Item = &NodeId>> = match &touched {
                            Some(touched) => Box::new(touched.iter()),
                            None => Box::new(live.keys()),
                        };
                        for &id in ids {
                            let Some(node) = doc.nodes.get(&id).filter(|n| n.is_live()) else { continue };
                            let seen =
                                Shared { kind: node.kind, group: doc.effective_group(id), fields: fields_of(node) };
                            self.last_shared.insert(id.to_string(), seen);
                        }
                    }
                    self.explicit.clear();
                    self.store_notices.clear();
                    self.last_session_save = Some(std::time::Instant::now());
                    self.save_retry_at = None;
                    self.flush_history();
                    return true;
                }
                Err(err) => {
                    self.store_error(err);
                    self.save_retry_at = Some(std::time::Instant::now() + SAVE_RETRY);
                    self.flush_history();
                    return false;
                }
            }
        }
        self.save_retry_at = Some(std::time::Instant::now() + SAVE_RETRY);
        self.flush_history();
        false
    }

    /// Startup: shows the document, then applies personal state
    /// (`session.mnf.local`) on top. `false` when it has no live node, so
    /// the caller starts fresh; Recently Closed is still restored then,
    /// or the next personal-state write would drop it.
    pub(crate) fn restore_session(&mut self) -> bool {
        let Some(journal) = self.journal.as_ref() else { return false };
        let doc = journal.doc();
        let mut nodes: Vec<(NodeId, Kind, Fields, Option<NodeId>)> =
            doc.live().map(|(id, n)| (id, n.kind, fields_of(n), doc.effective_group(id))).collect();
        self.take_store_recovery();
        // Contract: `Err` or `None` (no file, another document's, or a
        // newer format version) is a fresh start, same as a session with no
        // personal state at all.
        let local = match self.journal.as_ref().map(|j| j.read_local()) {
            Some(Ok(Some(bytes))) => LocalState::from_bytes(&bytes).unwrap_or_default(),
            _ => LocalState::default(),
        };
        if nodes.is_empty() {
            self.restore_closed(local.closed);
            self.local_written = Some(self.local_state());
            return false;
        }
        nodes.sort_by_key(|(_, kind, ..)| *kind != Kind::Group);
        let toasts = self.save_toast_queue.len();
        for (id, kind, fields, group) in &nodes {
            let group = group.map(|g| g.to_string());
            // The store's own `pos` still becomes `last_shared` inside
            // `materialize`; only the live placement is overridden.
            let entry = local.nodes.get(&id.to_string());
            self.materialize(id.to_string(), *kind, fields, group, entry);
        }
        self.collapse_toasts_since(toasts);
        // Graph view with the first tab focused, re-fit once restored tabs
        // land; `apply_local` below overrides both from personal state when
        // it has them.
        self.focused = nodes.iter().find(|(_, kind, ..)| *kind == Kind::Tab).map(|(id, ..)| id.to_string());
        self.view_mode = ViewMode::Graph;
        self.camera.frame_restored = true;
        self.apply_local(local);
        self.wake_focused();
        self.local_written = Some(self.local_state());
        true
    }

    /// This profile's personal state as the live graph currently shows it
    /// (spec "Personal state"): what to write to `session.mnf.local`. A
    /// node still spawning keeps its `pending_local` entry, so a quit
    /// before restored tabs land doesn't lose their placement and zoom.
    pub(crate) fn local_state(&self) -> LocalState {
        let item = self.view_mode == ViewMode::Item;
        let zoom = |factor: f64| (factor != self.settings.zoom_factor).then_some(factor);
        let mut nodes: BTreeMap<String, LocalNode> = self
            .nodes
            .iter()
            .filter(|(id, _)| id.parse::<NodeId>().is_ok())
            .map(|(id, node)| {
                let mut local = LocalNode { pos: [node.pos.x, node.pos.y], ..LocalNode::default() };
                // Against what the store held at this replica's last save
                // or load: a url this replica shares is marked and saved.
                let live_url = |url: &str| {
                    let from = str_of(&self.last_shared.get(id)?.fields, field::URL);
                    (from != url).then(|| LiveUrl { url: url.to_string(), from })
                };
                match &node.payload {
                    NodePayload::Tab(tab) => {
                        local.title = tab.title.clone();
                        local.muted = tab.muted;
                        local.zoom = zoom(tab.zoom_factor);
                        local.live_url = live_url(&tab.url);
                    }
                    NodePayload::Unloaded(tab) => {
                        local.title = tab.title.clone();
                        local.muted = tab.muted;
                        local.zoom = zoom(tab.zoom_factor);
                        local.live_url = live_url(&tab.url);
                    }
                    NodePayload::Note(n) => local.editing = n.editing,
                    _ => {}
                }
                (id.clone(), local)
            })
            .collect();
        if let Some(pending) = &self.pending_local {
            for (id, node) in &pending.nodes {
                if self.spawning.contains_key(id) && !nodes.contains_key(id) {
                    nodes.insert(id.clone(), node.clone());
                }
            }
        }
        let rect = self.camera.rect;
        LocalState {
            focused: self.focused.clone(),
            view_item: item,
            current_group: self.current_group.clone(),
            viewed_image: item.then(|| self.viewed_image.clone()).flatten(),
            split_tab: self.split_tab.clone(),
            groups_collapsed: self.groups_collapsed,
            camera: Some(([rect.min.x, rect.min.y, rect.max.x, rect.max.y], self.current_group.clone())),
            nodes,
            closed: self.closed.iter().map(|e| e.doc.clone()).collect(),
        }
    }

    /// Applies `state` (personal state, from `session.mnf.local` or an
    /// imported/opened file) on top of the just-restored/imported live
    /// graph. A view field naming a node takes hold only when that node
    /// exists, live or still spawning; otherwise the caller's own default
    /// stands. A node still spawning keeps its entry in `self.pending_local`
    /// for `handle_tab_spawn_result` to apply once it lands.
    pub(crate) fn apply_local(&mut self, state: LocalState) {
        let exists = |id: &Option<String>| {
            id.as_deref().is_some_and(|id| self.nodes.contains_key(id) || self.spawning.contains_key(id))
        };
        if exists(&state.focused) {
            self.focused = state.focused;
        }
        if exists(&state.current_group) {
            self.current_group = state.current_group;
        }
        if exists(&state.viewed_image) {
            self.viewed_image = state.viewed_image;
        }
        if exists(&state.split_tab) {
            self.split_tab = state.split_tab;
        }
        self.view_mode = if state.view_item { ViewMode::Item } else { ViewMode::Graph };
        self.groups_collapsed = state.groups_collapsed;
        self.restore_closed(state.closed);
        // Item view showing a tab still spawning (`focused` names it, but
        // it's not in `self.nodes` yet -- a tab's restore is always async):
        // `build_tab_view_ui` bounces an unknown-tab Item view back to
        // Graph every frame *unless* `pending_respawn_focus` names it, the
        // same guard a crash respawn or engine swap relies on to stay put
        // while its own tab is missing mid-respawn. Without this, Item view
        // never actually survives a restore -- it bounces to Graph the very
        // first frame after `apply_local`, before the spawn can land.
        if self.view_mode == ViewMode::Item && self.viewed_image.is_none() {
            if let Some(id) = &self.focused {
                if !self.nodes.get(id).is_some_and(|n| n.as_tab().is_some()) {
                    self.pending_respawn_focus = Some(id.clone());
                }
            }
        }

        let mut pending = BTreeMap::new();
        for (id, node) in state.nodes {
            if let Some(live) = self.nodes.get_mut(&id) {
                Self::apply_local_node(live, &node);
            } else if self.spawning.contains_key(&id) {
                pending.insert(id, node);
            }
        }
        if !pending.is_empty() {
            self.pending_local = Some(LocalState { nodes: pending, ..LocalState::default() });
        }

        if let Some((rect, level)) = state.camera {
            if level == self.current_group {
                self.camera.rect = egui::Rect::from_min_max(egui::pos2(rect[0], rect[1]), egui::pos2(rect[2], rect[3]));
                self.camera.frame_restored = false;
            }
        }
    }

    /// Recently Closed from personal state, oldest first like `closed`
    /// itself, capped at `MAX_CLOSED`. A blank entry (a format this build
    /// doesn't recognize inside `doc.nodes`, or one some other bug
    /// produced) is dropped rather than shown as an empty row.
    fn restore_closed(&mut self, closed: Vec<JsonDoc>) {
        let mut closed: Vec<ClosedEntry> =
            closed.into_iter().filter(|doc| !doc.nodes.is_empty()).map(|doc| ClosedEntry { doc }).collect();
        if closed.len() > MAX_CLOSED {
            closed.drain(..closed.len() - MAX_CLOSED);
        }
        self.closed = closed;
    }

    /// One node's share of `apply_local`: settled position, and whatever
    /// its payload kind keeps (a tab's title/muted/zoom, a note's editing).
    fn apply_local_node(live: &mut crate::GraphNode, node: &LocalNode) {
        live.pos = egui::vec2(node.pos[0], node.pos[1]);
        live.vel = egui::Vec2::ZERO;
        match &mut live.payload {
            NodePayload::Tab(tab) => {
                tab.title = node.title.clone();
                tab.muted = node.muted;
                if let Some(zoom) = node.zoom {
                    tab.zoom_factor = zoom;
                }
            }
            NodePayload::Unloaded(tab) => {
                tab.title = node.title.clone();
                tab.muted = node.muted;
                if let Some(zoom) = node.zoom {
                    tab.zoom_factor = zoom;
                }
            }
            NodePayload::Note(n) => n.editing = node.editing,
            _ => {}
        }
    }

    /// Applies a spawning node's personal-state entry once its node exists
    /// (`handle_tab_spawn_result`), removing it from `pending_local`. An
    /// unloaded card now takes title/muted/zoom from personal state just
    /// like a live tab; every other kind only takes its position.
    pub(crate) fn apply_pending_local(&mut self, id: &str) {
        let Some(pending) = &mut self.pending_local else { return };
        let Some(node) = pending.nodes.remove(id) else { return };
        if pending.nodes.is_empty() {
            self.pending_local = None;
        }
        if let Some(live) = self.nodes.get_mut(id) {
            match &live.payload {
                NodePayload::Tab(_) | NodePayload::Unloaded(_) | NodePayload::Note(_) => {
                    Self::apply_local_node(live, &node)
                }
                _ => {
                    live.pos = egui::vec2(node.pos[0], node.pos[1]);
                    live.vel = egui::Vec2::ZERO;
                }
            }
        }
    }

    /// Writes personal state when it changed (contract "Personal state"):
    /// at most every 30 s, 2.5 s after a Recently Closed change, and at
    /// exit (`force`).
    pub(crate) fn maybe_write_local(&mut self, force: bool) {
        const EVERY: std::time::Duration = std::time::Duration::from_secs(30);
        const AFTER_CLOSED: std::time::Duration = std::time::Duration::from_millis(2500);
        let due = force
            || self.closed_changed_at.is_some_and(|t| t.elapsed() >= AFTER_CLOSED)
            || self.local_written_at.elapsed() >= EVERY;
        if !due || self.journal.is_none() {
            return;
        }
        let state = self.local_state();
        self.local_written_at = std::time::Instant::now();
        if self.local_written.as_ref().is_some_and(|w| !crate::local_state::differs(w, &state)) {
            self.closed_changed_at = None;
            return;
        }
        match self.journal.as_ref().unwrap().write_local(&state.to_bytes()) {
            Ok(()) => {
                self.local_written = Some(state);
                self.closed_changed_at = None;
            }
            Err(err) => self.store_error(err),
        }
    }

    /// Each frame: pick up other instances' edits, retry a save that didn't
    /// commit, and flush a debounced fsync (contract "Flushing").
    pub(crate) fn poll_store(&mut self) {
        let Some(journal) = self.journal.as_mut() else { return };
        let synced = journal.sync_if_due();
        let changed = journal.changed_on_disk();
        if let Err(err) = synced {
            self.store_error(err);
        }
        let retry_due = self.save_retry_at.is_some_and(|at| std::time::Instant::now() >= at);
        if changed || retry_due {
            self.save_session();
        }
        self.maybe_write_local(false);
    }

    /// Clean exit: save, write personal state, then close (compacts only if
    /// something changed).
    pub(crate) fn close_store(&mut self) {
        self.save_session();
        self.maybe_write_local(true);
        self.flush_history();
        if let Some(journal) = self.journal.as_mut() {
            if let Err(err) = journal.close() {
                eprintln!("session: closing the store failed: {err}");
            }
        }
    }

    /// The session as a JSON snapshot (Export Session), saved first so it
    /// holds every edit; if that save didn't commit, it exports anyway and
    /// warns. Carries this profile's personal state as `local` (spec
    /// "Personal state"), same as a named session's own `.mnf.local`.
    /// `None` without a store.
    pub(crate) fn export_session_json(&mut self) -> Option<JsonDoc> {
        self.journal.as_ref()?;
        if !self.save_session() {
            self.queue_toast("The export may miss recent edits: the session couldn't be saved first".into());
        }
        let mut json = crate::store::json::doc_to_json(self.journal.as_ref()?.doc());
        json.local = Some(serde_json::to_value(self.local_state()).expect("personal state serializes"));
        Some(json)
    }

    /// A group's own subtree as a JSON snapshot (Export Group; spec "JSON
    /// snapshots"), root first (`store::json::subtree_to_json`'s own
    /// contract), saved first like `export_session_json`. Only *saved*
    /// nodes are exported -- a synthetic "Expand as Graph" group
    /// (`from_json`) is itself a saved node since Task 5, so it exports
    /// too, unlike the old per-group export format, which walked live
    /// nodes and always skipped it. `None` without a store or for an
    /// unknown group; toasted when the store doesn't hold the group live
    /// (its save failed), rather than exporting an empty file.
    pub(crate) fn export_group_json(&mut self, group: &str) -> Option<JsonDoc> {
        self.journal.as_ref()?;
        let root: NodeId = group.parse().ok()?;
        if !self.save_session() {
            self.queue_toast("The export may miss recent edits: the session couldn't be saved first".into());
        }
        let doc = self.journal.as_ref()?.doc();
        if !doc.nodes.get(&root).is_some_and(|n| n.is_live()) {
            self.queue_toast("Couldn't export the group: it isn't saved in the session".into());
            return None;
        }
        Some(crate::store::json::subtree_to_json(doc, root))
    }

    /// Imports a group export at the level currently being viewed (Import
    /// Group): every node gets a fresh id (`store::json::import`), the
    /// file's root (first node -- `subtree_to_json`'s contract; a bare
    /// tab/note file with no group root is imported the same way) lands at
    /// a jittered position around `GRAPH_WORLD_CENTER`, under
    /// `self.current_group`. Every other member keeps its file position
    /// exactly: `pos` is always local to a node's own immediate group, so
    /// a member's position is already relative to the root (or a group
    /// nested under it) and needs no translation, only the re-parented
    /// root itself does. A `group` link only within this file's own nodes
    /// is kept; a member whose group was outside the exported subtree
    /// lands with the root (`materialize_imported`). Ends with
    /// `save_session`, same as `replace_running_session`/`reopen_closed`.
    pub(crate) fn import_group_json(&mut self, json: &JsonDoc) {
        let imported = match crate::store::json::import(json, || self.alloc_node_id().parse().expect("a store id")) {
            Ok(imported) => imported,
            Err(err) => {
                self.queue_toast(format!("Couldn't import the group: {err}"));
                return;
            }
        };
        let Some(root) = imported.first().map(|i| i.id) else { return };
        // A node's `pos` is always within its own immediate group's local
        // space (contract "The graph's world-space center" -- see
        // `GRAPH_WORLD_CENTER`'s own doc comment), so only the root -- the
        // one node whose group is changing, from wherever it was exported
        // to `self.current_group` -- needs a fresh position; every other
        // member's file position is already relative to its own (possibly
        // nested) group and stays as-is, same as the old per-group export
        // format's own import behavior.
        let pos = crate::GRAPH_WORLD_CENTER + crate::graph_layout::spawn_jitter(&root.to_string());
        let root = ImportRoot { group: self.current_group.clone(), pos: Some(pos) };
        self.materialize_imported(imported, Some(root), false);
        self.save_session();
    }

    /// Materializes the nodes `store::json::import` produced, for Import
    /// Session, Import Group and Reopen Closed. Groups come first, so no
    /// member names a group that doesn't exist yet, and a `group` link is
    /// kept only when it names a group in the same file.
    /// - With `root`, the file's first node lands at `root.group` (at
    ///   `root.pos`, when given), and so does every other node whose group
    ///   isn't in the file. Without it (a whole session replacing the old
    ///   one), such a node goes to the top level.
    /// - A `parent` outside the file keeps its id through `import`: it
    ///   names some other session's tab, or an unrelated live one. It's
    ///   dropped, unless `keep_live_parents` and that node is live.
    ///
    /// Tabs arrive asleep (`materialize`). Toasts from missing files are
    /// collapsed into one.
    pub(crate) fn materialize_imported(
        &mut self,
        imported: Vec<Imported>,
        root: Option<ImportRoot>,
        keep_live_parents: bool,
    ) {
        let groups: BTreeSet<NodeId> = imported.iter().filter(|i| i.kind == Kind::Group).map(|i| i.id).collect();
        let ids: BTreeSet<NodeId> = imported.iter().map(|i| i.id).collect();
        let root_id = root.as_ref().and(imported.first()).map(|i| i.id);
        let (dest, root_pos) = root.map_or((None, None), |r| (r.group, r.pos));
        let mut imported = imported;
        imported.sort_by_key(|i| i.kind != Kind::Group);
        let toasts = self.save_toast_queue.len();
        for node in imported {
            let is_root = Some(node.id) == root_id;
            let group = match node.fields.get(&field::GROUP) {
                Some(FieldValue::Id(Some(g))) if !is_root && groups.contains(g) => Some(g.to_string()),
                _ => dest.clone(),
            };
            let mut fields = imported_fields(node.kind, &node.fields);
            if let Some(FieldValue::Id(parent)) = fields.get_mut(&field::PARENT) {
                let live = |p: &NodeId| keep_live_parents && self.nodes.contains_key(&p.to_string());
                *parent = parent.filter(|p| ids.contains(p) || live(p));
            }
            if let (true, Some(pos)) = (is_root, root_pos) {
                fields.insert(field::POS, FieldValue::Pos(pos.x, pos.y));
            }
            self.materialize(node.id.to_string(), node.kind, &fields, group, None);
        }
        self.collapse_toasts_since(toasts);
    }

    /// Writes `export_session_json` to `path` (Export Session, headless
    /// `EXPORTSESSION`). A failure is logged and toasted, and gives `false`.
    pub(crate) fn export_session_to(&mut self, path: &std::path::Path) -> bool {
        let written = match self.export_session_json() {
            None => Err("no session file".to_string()),
            Some(json) => serde_json::to_string_pretty(&json).map_err(|e| e.to_string()).and_then(|text| {
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                std::fs::write(path, text).map_err(|e| e.to_string())
            }),
        };
        if let Err(err) = &written {
            eprintln!("session: couldn't export to {}: {err}", path.display());
            self.queue_toast(format!("Couldn't export the session to {}: {err}", path.display()));
        }
        written.is_ok()
    }

    /// Replaces the running session with `json`'s nodes under fresh ids
    /// (Import Session; Task 9 adds Open). The save at the end deletes
    /// every node that was live from the store. Returns the label map
    /// (file id → new id) for remapping personal state (Task 7). A file
    /// with no tabs changes nothing, so a bad import can't wipe a working
    /// session.
    pub(crate) fn replace_running_session(&mut self, json: &JsonDoc) -> HashMap<String, String> {
        let imported = match crate::store::json::import(json, || self.alloc_node_id().parse().expect("a store id")) {
            Ok(imported) => imported,
            Err(err) => {
                self.queue_toast(format!("Couldn't import the session: {err}"));
                return HashMap::new();
            }
        };
        if !imported.iter().any(|i| i.kind == Kind::Tab) {
            self.queue_toast("The session has no tabs".into());
            return HashMap::new();
        }
        self.teardown_running_session();
        let map: HashMap<String, String> =
            json.nodes.iter().zip(&imported).map(|(node, i)| (node.id.clone(), i.id.to_string())).collect();
        let first_tab = imported.iter().find(|i| i.kind == Kind::Tab).map(|i| i.id.to_string());
        // No root: a group or parent outside the file (import keeps real
        // node ids) names nothing once the old session is gone.
        self.materialize_imported(imported, None, false);
        self.focused = first_tab;
        self.view_mode = ViewMode::Graph;
        self.camera.frame_restored = true;
        if let Some(state) = json.local.clone().and_then(|value| serde_json::from_value::<LocalState>(value).ok()) {
            // Recently Closed is this profile's own history, not document
            // content: an imported/opened file's `local.closed` (if any)
            // never replaces it, so `apply_local`'s own restore of
            // `self.closed` is undone right after.
            let closed = std::mem::take(&mut self.closed);
            self.apply_local(state.remap(&map));
            self.closed = closed;
        }
        self.wake_focused();
        self.save_session();
        map
    }

    /// Clears the running session's nodes and the state naming them. Each
    /// tab's `EngineTab::Drop` shuts its engine host down, as `destroy_tab`
    /// relies on, so clearing the map is enough; spawns still in flight are
    /// dropped when they land (`session_generation`).
    fn teardown_running_session(&mut self) {
        self.nodes.clear();
        self.node_order.clear();
        self.focused = None;
        self.viewed_image = None;
        self.split_tab = None;
        self.current_group = None;
        self.dragging = None;
        self.hovered = None;
        self.selected_tabs.clear();
        self.rect_select_start = None;
        self.graph_settled_key = None;
        self.wakes_in_flight.clear();
        self.spawning.clear();
        self.host_loads.clear();
        self.navigated_from.clear();
        self.remote_in_flight.clear();
        self.last_shared.clear();
        self.pending_local = None;
        self.session_generation += 1;
    }

    /// Collapses the toasts queued since `mark` into one, so a bulk restore
    /// or import with many missing files toasts once, not once per card.
    fn collapse_toasts_since(&mut self, mark: usize) {
        let extra = self.save_toast_queue.len().saturating_sub(mark + 1);
        if extra > 0 {
            let first = self.save_toast_queue.drain(mark..).next().expect("toasts past the mark");
            self.save_toast_queue.push_back(format!("{first} (and {extra} more)"));
        }
    }

    /// Tells the user a store call failed (contract "Read-only" for
    /// `ReadOnly`).
    pub(crate) fn store_error(&mut self, err: JournalError) {
        let msg = match err {
            JournalError::ReadOnly => "This session was written by a newer manifold; changes aren't saved".to_string(),
            _ => format!("Session not saved: {err}"),
        };
        self.notice(msg);
    }

    /// Tells the user what a repair of the session file lost, and where
    /// the original was kept (contract "Repairs").
    pub(crate) fn take_store_recovery(&mut self) {
        let Some(recovery) = self.journal.as_mut().and_then(|j| j.take_recovery()) else { return };
        self.notice(recovery_notice(&recovery));
    }

    /// Logs and toasts `msg`, unless it was shown since the last commit: a
    /// failing store or a damage report repeats at each save.
    fn notice(&mut self, msg: String) {
        if self.store_notices.contains(&msg) {
            return;
        }
        eprintln!("session: {msg}");
        self.queue_toast(msg.clone());
        self.store_notices.insert(msg);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::redirect_fetch::{FetchResult, FetchTarget};
    use crate::store::journal::Journal;
    use crate::store::model::{ActorId, DocId, Hlc};
    use crate::store::ops::Op;
    use crate::store::testutil::{temp_dir, XorShift};
    use crate::SpawnPurpose;

    const A: ActorId = ActorId(0xA);

    /// Drains `app.redirect_fetcher` until it reports something (or gives
    /// up after ~1 s): under test, `RedirectFetcher::new(None)` (no
    /// content dir) still spawns a real background thread per `request`,
    /// but it resolves at once with no I/O, so this is fast in practice.
    fn drain_fetches(app: &App) -> Vec<FetchResult> {
        for _ in 0..200 {
            let results = app.redirect_fetcher.tick();
            if !results.is_empty() {
                return results;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        Vec::new()
    }

    #[test]
    fn alternating_store_notices_are_each_shown_once_until_a_commit() {
        let dir = crate::store::testutil::temp_dir("notices");
        let mut app = app_on(&dir.join("session.mnf"), 1);
        for _ in 0..3 {
            app.notice("read-only".into());
            app.notice("damaged".into());
        }
        assert_eq!(app.save_toast_queue.len(), 2);
        add(&mut app, NodePayload::Note(NotePayload { text: "n".into(), editing: false, url: None }));
        assert!(app.save_session());
        app.notice("read-only".into());
        assert_eq!(app.save_toast_queue.len(), 3, "a commit lets a notice show again");
    }

    #[test]
    fn recovery_notices_say_what_was_lost_and_where_the_original_is() {
        let r = Recovery {
            damaged: vec![(10, 20)],
            saved: Some("C:/x/session.mnf.damaged-1".into()),
            ..Recovery::default()
        };
        assert_eq!(recovery_notice(&r), "The session file was damaged; 1 damaged part was skipped. The original is saved at C:/x/session.mnf.damaged-1");
        let r = Recovery { from_prev: true, saved: None, ..Recovery::default() };
        assert!(recovery_notice(&r).contains("rebuilt from its previous copy"));
    }

    fn id(counter: u64) -> NodeId {
        NodeId { actor: A, counter }
    }

    /// Applies `bodies` to `doc` as actor `A`'s next ops.
    fn apply(doc: &mut Doc, bodies: Vec<OpBody>) {
        for body in bodies {
            let counter = doc.version.get(&A).copied().unwrap_or(0).max(1000);
            let hlc = Hlc { ms: doc.clock.ms + 1, logical: 0 };
            doc.apply(&Op { id: id(counter), hlc, body });
        }
    }

    fn shared(kind: Kind, group: Option<NodeId>, set: &[(u32, FieldValue)]) -> Shared {
        let mut fields: Fields =
            kind_fields(kind).iter().map(|&no| (no, FieldValue::absent(field_def(no).unwrap().ty))).collect();
        for (no, value) in set {
            fields.insert(*no, value.clone());
        }
        Shared { kind, group, fields }
    }

    fn note(text: &str, group: Option<NodeId>) -> Shared {
        shared(Kind::Note, group, &[(field::TEXT, FieldValue::Str(text.into()))])
    }

    fn group(name: &str) -> Shared {
        shared(Kind::Group, None, &[(field::NAME, FieldValue::Str(name.into()))])
    }

    /// What `doc` shows, shaped like `live`.
    fn shown(doc: &Doc) -> BTreeMap<NodeId, Shared> {
        doc.live()
            .map(|(id, node)| (id, Shared { kind: node.kind, group: doc.effective_group(id), fields: fields_of(node) }))
            .collect()
    }

    fn all_explicit(live: &BTreeMap<NodeId, Shared>) -> BTreeSet<(NodeId, u32)> {
        live.keys().flat_map(|&id| [(id, field::POS), (id, field::URL)]).collect()
    }

    #[test]
    fn new_nodes_are_created_groups_first() {
        let doc = Doc::new(DocId([1; 16]));
        // The member's id sorts before its group's.
        let live = BTreeMap::from([(id(0), note("inside", Some(id(1)))), (id(1), group("G"))]);
        let ops = diff(&doc, &live, &all_explicit(&live), &BTreeSet::new());
        assert!(matches!(ops[0], OpBody::Create { node, .. } if node == id(1)));
        let mut doc = doc;
        apply(&mut doc, ops);
        assert_eq!(shown(&doc), live);
    }

    #[test]
    fn an_unchanged_graph_emits_nothing() {
        let mut doc = Doc::new(DocId([1; 16]));
        let live = BTreeMap::from([(id(0), note("a", None))]);
        let ops = diff(&doc, &live, &all_explicit(&live), &BTreeSet::new());
        apply(&mut doc, ops);
        assert_eq!(diff(&doc, &live, &all_explicit(&live), &BTreeSet::new()), vec![]);
    }

    #[test]
    fn explicit_fields_change_only_when_marked() {
        let mut doc = Doc::new(DocId([1; 16]));
        let mut live = BTreeMap::from([(id(0), note("a", None))]);
        let ops = diff(&doc, &live, &all_explicit(&live), &BTreeSet::new());
        apply(&mut doc, ops);
        live.get_mut(&id(0)).unwrap().fields.insert(field::POS, FieldValue::Pos(5.0, 6.0));
        assert_eq!(diff(&doc, &live, &BTreeSet::new(), &BTreeSet::new()), vec![], "physics motion is not an edit");
        let marked = BTreeSet::from([(id(0), field::POS)]);
        assert!(matches!(&diff(&doc, &live, &marked, &BTreeSet::new())[..], [OpBody::Set { field: field::POS, .. }]));
    }

    #[test]
    fn a_missing_node_is_deleted_unless_kept() {
        let mut doc = Doc::new(DocId([1; 16]));
        let live = BTreeMap::from([(id(0), note("a", None))]);
        let ops = diff(&doc, &live, &all_explicit(&live), &BTreeSet::new());
        apply(&mut doc, ops);
        let empty = BTreeMap::new();
        assert_eq!(diff(&doc, &empty, &BTreeSet::new(), &BTreeSet::from([id(0)])), vec![]);
        assert_eq!(diff(&doc, &empty, &BTreeSet::new(), &BTreeSet::new()), vec![OpBody::Delete { node: id(0) }]);
    }

    #[test]
    fn a_tombstone_is_never_recreated() {
        let mut doc = Doc::new(DocId([1; 16]));
        let live = BTreeMap::from([(id(0), note("a", None))]);
        let ops = diff(&doc, &live, &all_explicit(&live), &BTreeSet::new());
        apply(&mut doc, ops);
        apply(&mut doc, vec![OpBody::Delete { node: id(0) }]);
        let ops = diff(&doc, &live, &all_explicit(&live), &BTreeSet::new());
        assert!(!ops.iter().any(|op| matches!(op, OpBody::Create { .. })), "{ops:?}");
    }

    #[test]
    fn re_homing_out_of_a_deleted_group_is_not_a_move() {
        let mut doc = Doc::new(DocId([1; 16]));
        let live = BTreeMap::from([(id(0), note("a", Some(id(1)))), (id(1), group("G"))]);
        let ops = diff(&doc, &live, &all_explicit(&live), &BTreeSet::new());
        apply(&mut doc, ops);
        // Another replica deleted the group only; the note is shown at the top.
        apply(&mut doc, vec![OpBody::Delete { node: id(1) }]);
        let rehomed = BTreeMap::from([(id(0), note("a", None))]);
        assert_eq!(diff(&doc, &rehomed, &BTreeSet::new(), &BTreeSet::new()), vec![]);
    }

    #[test]
    fn a_parent_naming_a_missing_node_is_written_as_none() {
        let doc = Doc::new(DocId([1; 16]));
        let tab = shared(Kind::Tab, None, &[(field::PARENT, FieldValue::Id(Some(id(9))))]);
        let live = BTreeMap::from([(id(0), tab)]);
        let mut doc2 = doc.clone();
        apply(&mut doc2, diff(&doc, &live, &all_explicit(&live), &BTreeSet::new()));
        assert_eq!(doc2.nodes[&id(0)].get(field::PARENT), FieldValue::Id(None));
    }

    #[test]
    fn random_edits_converge_in_one_diff() {
        let mut rng = XorShift(0x5E55);
        let mut doc = Doc::new(DocId([1; 16]));
        let mut live: BTreeMap<NodeId, Shared> = BTreeMap::new();
        let mut next = 0;
        for _ in 0..200 {
            match rng.below(5) {
                0 | 1 => {
                    let groups: Vec<NodeId> =
                        live.iter().filter(|(_, s)| s.kind == Kind::Group).map(|(id, _)| *id).collect();
                    let group = (!groups.is_empty() && rng.below(2) == 0)
                        .then(|| groups[rng.below(groups.len() as u64) as usize]);
                    let node = if rng.below(3) == 0 { group_in("g", group) } else { note("n", group) };
                    live.insert(id(next), node);
                    next += 1;
                }
                2 if !live.is_empty() => {
                    let key = *live.keys().nth(rng.below(live.len() as u64) as usize).unwrap();
                    live.get_mut(&key)
                        .unwrap()
                        .fields
                        .insert(field::COLOR, FieldValue::Color(Some(rng.below(1 << 24) as u32)));
                }
                3 if !live.is_empty() => {
                    let key = *live.keys().nth(rng.below(live.len() as u64) as usize).unwrap();
                    // Deleting a group deletes what it shows, as the app does.
                    let doomed: Vec<NodeId> =
                        live.keys().filter(|&&n| n == key || is_inside(&live, n, key)).copied().collect();
                    for n in doomed {
                        live.remove(&n);
                    }
                }
                4 if !live.is_empty() => {
                    let key = *live.keys().nth(rng.below(live.len() as u64) as usize).unwrap();
                    let groups: Vec<NodeId> =
                        live.iter().filter(|(_, s)| s.kind == Kind::Group).map(|(id, _)| *id).collect();
                    // A move into `key` itself or one of its descendants would create a cycle.
                    let candidates: Vec<Option<NodeId>> = std::iter::once(None)
                        .chain(groups.into_iter().map(Some))
                        .filter(|&g| g != Some(key) && !g.is_some_and(|g| is_inside(&live, g, key)))
                        .collect();
                    let choice = candidates[rng.below(candidates.len() as u64) as usize];
                    live.get_mut(&key).unwrap().group = choice;
                }
                _ => {}
            }
            let ops = diff(&doc, &live, &all_explicit(&live), &BTreeSet::new());
            apply(&mut doc, ops);
            assert_eq!(shown(&doc), live);
            assert_eq!(diff(&doc, &live, &all_explicit(&live), &BTreeSet::new()), vec![]);
        }
    }

    fn group_in(name: &str, parent: Option<NodeId>) -> Shared {
        Shared { group: parent, ..group(name) }
    }

    fn is_inside(live: &BTreeMap<NodeId, Shared>, node: NodeId, ancestor: NodeId) -> bool {
        let mut at = live.get(&node).and_then(|s| s.group);
        while let Some(g) = at {
            if g == ancestor {
                return true;
            }
            at = live.get(&g).and_then(|s| s.group);
        }
        false
    }

    /// Materializes `nodes` into a windowless app, as `apply_remote` would
    /// show another replica's new nodes.
    fn app_with(nodes: &BTreeMap<NodeId, Shared>) -> App {
        let mut app = App::default();
        let changes = nodes.iter().map(|(&id, s)| (id, Some((s.kind, s.fields.clone(), s.group)))).collect();
        app.apply_remote(changes);
        app
    }

    #[test]
    fn a_restored_note_group_and_leaf_project_back_to_their_fields() {
        let red = color_value(Some(egui::Color32::from_rgb(200, 10, 10)));
        let nodes = BTreeMap::from([
            (
                id(0),
                shared(
                    Kind::Note,
                    Some(id(1)),
                    &[
                        (field::TEXT, FieldValue::Str("hello".into())),
                        (field::POS, FieldValue::Pos(3.0, -4.0)),
                        (field::COLOR, red),
                    ],
                ),
            ),
            (
                id(1),
                shared(
                    Kind::Group,
                    None,
                    &[(field::NAME, FieldValue::Str("G".into())), (field::FROM_JSON, FieldValue::Bool(true))],
                ),
            ),
            (
                id(2),
                shared(
                    Kind::JsonLeaf,
                    Some(id(1)),
                    &[
                        (field::PATH, FieldValue::Str("C:/data.json".into())),
                        (field::KEY, FieldValue::Str("answer".into())),
                        (field::VALUE, FieldValue::Str("42".into())),
                    ],
                ),
            ),
        ]);
        assert_eq!(app_with(&nodes).shared_nodes(), nodes);
    }

    #[test]
    fn a_missing_content_file_with_a_known_url_becomes_a_placeholder_that_refetches() {
        let nodes = BTreeMap::from([(
            id(0),
            shared(
                Kind::Json,
                None,
                &[
                    (field::PATH, FieldValue::Str(r"Z:\gone.json".into())),
                    (field::URL, FieldValue::Str("https://x.test/a.json".into())),
                ],
            ),
        )]);
        let app = app_with(&nodes);
        let node_id = id(0).to_string();
        match &app.nodes[&node_id].payload {
            NodePayload::Json(p) => {
                assert!(p.error.is_none(), "a known-URL miss never reaches insert_json_node_with_id's own toast");
                assert_eq!(p.url.as_deref(), Some("https://x.test/a.json"));
            }
            _ => panic!("expected a Json placeholder"),
        }
        assert!(app.save_toast_queue.is_empty() && app.save_toast.is_none(), "no 'Couldn't open' toast");
        let results = drain_fetches(&app);
        assert_eq!(results.len(), 1, "materialize's own refresh_node queued exactly one fetch");
        assert!(matches!(&results[0].job.target, FetchTarget::Node(n) if *n == node_id));
        assert!(results[0].body.is_none(), "no content dir under test, so the fetch itself fails");
    }

    #[test]
    fn retry_missing_file_refetches_a_node_with_a_url_instead_of_retrying_its_local_path() {
        let nodes = BTreeMap::from([(
            id(0),
            shared(
                Kind::Json,
                None,
                &[
                    (field::PATH, FieldValue::Str(r"Z:\gone.json".into())),
                    (field::URL, FieldValue::Str("https://x.test/a.json".into())),
                ],
            ),
        )]);
        let mut app = app_with(&nodes);
        let node_id = id(0).to_string();
        // `materialize` already queued one refetch on its own; drain and
        // discard it so the assertion below reflects only the click.
        drain_fetches(&app);
        app.retry_missing_file(&node_id);
        let results = drain_fetches(&app);
        assert_eq!(results.len(), 1, "the URL branch queued exactly one fetch");
        assert!(matches!(&results[0].job.target, FetchTarget::Node(n) if *n == node_id));
    }

    #[test]
    fn a_remotely_removed_group_leaves_no_member_naming_it() {
        let nodes = BTreeMap::from([
            (id(0), group("Outer")),
            (id(1), shared(Kind::Group, Some(id(0)), &[(field::NAME, FieldValue::Str("Inner".into()))])),
            (id(2), note("stored member", Some(id(1)))),
        ]);
        let mut app = app_with(&nodes);
        // A note only this replica has, not yet saved, inside the group.
        let local = id(3).to_string();
        let payload = NodePayload::Note(NotePayload { text: "local".into(), editing: false, url: None });
        app.insert_placeholder_node(local.clone(), egui::Vec2::ZERO, payload);
        app.nodes.get_mut(&local).unwrap().group = Some(id(1).to_string());
        app.current_group = Some(id(1).to_string());
        // What `remote_changes` reports when another replica deletes Inner:
        // the group gone, its stored member now shown in Outer.
        let member = nodes[&id(2)].fields.clone();
        app.apply_remote(vec![(id(1), None), (id(2), Some((Kind::Note, member, Some(id(0)))))]);
        let inner = id(1).to_string();
        assert!(!app.nodes.contains_key(&inner) && !app.node_order.contains(&inner));
        let outer = Some(id(0).to_string());
        assert_eq!(app.nodes[&id(2).to_string()].group, outer);
        assert_eq!(app.nodes[&local].group, outer, "the local-only member moves to the effective group");
        assert_eq!(app.current_group, outer, "the view leaves the removed level");
        assert!(app.nodes.values().all(|n| n.group.as_deref() != Some(inner.as_str())));
    }

    /// A windowless app saving to `path` as `actor`. `pub(crate)`: also
    /// used by `main.rs`'s own windowless tests (e.g. `closed_entry`'s).
    pub(crate) fn app_on(path: &std::path::Path, actor: u64) -> App {
        App { journal: Some(Journal::open(path, ActorId(actor)).unwrap()), ..App::default() }
    }

    pub(crate) fn add(app: &mut App, payload: NodePayload) -> String {
        let id = app.alloc_node_id();
        app.insert_placeholder_node(id.clone(), egui::Vec2::ZERO, payload);
        id
    }

    /// Another replica sets one field of `node`.
    fn remote_set(other: &mut Journal, node: &str, no: u32, value: FieldValue) {
        let node = node.parse().unwrap();
        other.begin().unwrap().commit(vec![OpBody::Set { node, field: no, enc: encode_field(no, &value) }]).unwrap();
    }

    fn stored(app: &App, node: &str, no: u32) -> FieldValue {
        app.journal.as_ref().unwrap().doc().nodes[&node.parse().unwrap()].get(no)
    }

    #[test]
    fn a_remote_edit_to_one_field_keeps_an_unsaved_local_edit_to_another() {
        let path = temp_dir("sync-fields").join("session.mnf");
        let mut app = app_on(&path, 1);
        let note = add(&mut app, NodePayload::Note(NotePayload { text: "v1".into(), editing: false, url: None }));
        let failed = UnloadedTabPayload {
            url: "https://u0/".into(),
            title: String::new(),
            kind: EngineKind::Servo,
            pinned: false,
            muted: false,
            zoom_factor: 1.0,
            state: UnloadedState::Failed("x".into()),
        };
        let tab = add(&mut app, NodePayload::Unloaded(failed));
        assert!(app.save_session());
        let mut other = Journal::open(&path, ActorId(2)).unwrap();

        // Unsaved here: typing in the note, and a navigation (`url` is
        // explicit and unmarked, so saves don't write it).
        if let NodePayload::Note(n) = &mut app.nodes.get_mut(&note).unwrap().payload {
            n.text = "typing".into();
        }
        if let NodePayload::Unloaded(t) = &mut app.nodes.get_mut(&tab).unwrap().payload {
            t.url = "https://u1/".into();
        }
        let red = color_value(Some(egui::Color32::RED));
        remote_set(&mut other, &note, field::COLOR, red.clone());
        remote_set(&mut other, &tab, field::PINNED, FieldValue::Bool(true));

        // The remote round applies, then the next round commits.
        assert!(app.save_session());
        let live = &app.nodes[&note];
        assert_eq!(live.color, Some(egui::Color32::RED));
        assert!(matches!(&live.payload, NodePayload::Note(n) if n.text == "typing"));
        assert_eq!(stored(&app, &note, field::TEXT), FieldValue::Str("typing".into()));
        assert!(matches!(&app.nodes[&tab].payload, NodePayload::Unloaded(t) if t.pinned && t.url == "https://u1/"));
        assert_eq!(stored(&app, &tab, field::URL), FieldValue::Str("https://u0/".into()));

        // A later remote edit to the same field does apply.
        remote_set(&mut other, &note, field::TEXT, FieldValue::Str("theirs".into()));
        assert!(app.save_session());
        assert!(matches!(&app.nodes[&note].payload, NodePayload::Note(n) if n.text == "theirs"));
        assert_eq!(app.nodes[&note].color, Some(egui::Color32::RED));
    }

    #[test]
    fn url_changes_after_a_host_started_load_stay_local_until_it_settles() {
        let mut app = App::default();
        assert!(app.shares_url_change("7", None), "an ordinary tab shares");
        app.note_host_load("7");
        assert!(!app.shares_url_change("7", None), "a redirect right after the host's navigation");
        assert!(!app.shares_url_change("7", None), "still within the grace");
        app.host_loads.insert("7".into(), std::time::Instant::now() - HOST_LOAD_GRACE);
        assert!(app.shares_url_change("7", None), "settled: the next navigation is shared");
        assert!(app.host_loads.is_empty());
    }

    /// An engine that says how a navigation started overrides the grace.
    #[test]
    fn the_engines_cause_decides_whether_a_url_change_is_shared() {
        let mut app = App::default();
        assert!(!app.shares_url_change("7", Some(false)), "a script's navigation stays local");
        app.note_host_load("7");
        assert!(app.shares_url_change("7", Some(true)), "a click right after the host's load is shared");
        assert!(app.host_loads.is_empty(), "and ends the host's load");
    }

    /// Right after a typed navigation, a report still at the url it left
    /// is from before the engine started it.
    #[test]
    fn a_report_still_at_the_url_a_typed_navigation_left_is_stale() {
        let mut app = App::default();
        let report = |app: &mut App, url: &str| app.reported_url("7", url.into(), "https://typed/".into());
        assert_eq!(report(&mut app, "https://old/"), "https://old/", "no typed navigation: as reported");
        app.navigated_from.insert("7".into(), ("https://old/".into(), std::time::Instant::now()));
        assert_eq!(report(&mut app, "https://old/"), "https://typed/");
        assert_eq!(report(&mut app, "https://typed/"), "https://typed/");
        assert_eq!(report(&mut app, "https://old/"), "https://old/", "after the engine moved on: real (going back)");
        let long_ago = std::time::Instant::now() - std::time::Duration::from_secs(2);
        app.navigated_from.insert("7".into(), ("https://old/".into(), long_ago));
        assert_eq!(report(&mut app, "https://old/"), "https://old/", "the navigation never started");
    }

    #[test]
    fn a_failed_commit_keeps_the_explicit_marks() {
        let path = temp_dir("sync-fail").join("session.mnf");
        let mut app = app_on(&path, 1);
        let note = add(&mut app, NodePayload::Note(NotePayload { text: "n".into(), editing: false, url: None }));
        assert!(app.save_session());
        app.nodes.get_mut(&note).unwrap().pos = egui::vec2(5.0, 6.0);
        let mark = (note.parse().unwrap(), field::POS);
        app.explicit.insert(mark);
        let set_readonly = |readonly: bool| {
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            // Toggles the Windows read-only attribute, which is what makes
            // the append fail; on Unix this would make the file writable
            // by everyone, but it's a temp file this test owns.
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(readonly);
            std::fs::set_permissions(&path, perms).unwrap();
        };
        set_readonly(true);
        let saved = app.save_session();
        set_readonly(false);
        assert!(!saved);
        assert!(app.explicit.contains(&mark), "kept for the next save");
        // Nothing else happens, so only the retry saves it.
        app.poll_store();
        assert!(app.explicit.contains(&mark), "not retried before SAVE_RETRY");
        app.save_retry_at = Some(std::time::Instant::now());
        app.poll_store();
        assert!(app.save_retry_at.is_none());
        assert!(app.explicit.is_empty());
        assert_eq!(stored(&app, &note, field::POS), FieldValue::Pos(5.0, 6.0));
    }

    #[test]
    fn a_marked_pos_is_saved_and_an_unmarked_one_is_not() {
        let path = temp_dir("marks").join("session.mnf");
        let mut app = app_on(&path, 1);
        let note = add(&mut app, NodePayload::Note(NotePayload { text: "n".into(), editing: false, url: None }));
        app.nodes.get_mut(&note).unwrap().pos = egui::vec2(1.0, 1.0);
        assert!(app.save_session());
        app.nodes.get_mut(&note).unwrap().pos = egui::vec2(50.0, 50.0);
        assert!(app.save_session());
        assert_eq!(stored(&app, &note, field::POS), FieldValue::Pos(1.0, 1.0), "physics motion stays local");
        app.mark(&note, field::POS);
        assert!(app.save_session());
        assert_eq!(stored(&app, &note, field::POS), FieldValue::Pos(50.0, 50.0));
    }

    #[test]
    fn a_read_only_store_saves_nothing() {
        let path = temp_dir("sync-readonly").join("session.mnf");
        drop(Journal::open(&path, ActorId(1)).unwrap());
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[4] = 2; // a newer format version, as in `journal`'s own test
        let crc = crate::store::crc32c::crc32c(&bytes[..21]);
        bytes[21..25].copy_from_slice(&crc.to_le_bytes());
        std::fs::write(&path, &bytes).unwrap();
        let mut app = app_on(&path, 2);
        add(&mut app, NodePayload::Note(NotePayload { text: "n".into(), editing: false, url: None }));
        assert!(!app.save_session());
        assert_eq!(app.journal.as_ref().unwrap().doc().live().count(), 0);
    }

    #[test]
    fn engine_codes_follow_the_store_table() {
        for kind in [EngineKind::Servo, EngineKind::Cef, EngineKind::Webkit, EngineKind::Webview2] {
            let name = crate::store::model::ENGINES[engine_code(kind) as usize - 1];
            assert_eq!(serde_json::to_value(Wrap(kind)).unwrap(), serde_json::json!(name));
            assert_eq!(engine_of(engine_code(kind)), kind);
        }
    }

    #[derive(serde::Serialize)]
    struct Wrap(#[serde(with = "crate::process_manager::engine_kind_serde")] EngineKind);

    #[test]
    fn colors_round_trip() {
        let c = egui::Color32::from_rgb(1, 2, 3);
        assert_eq!(color_of(&color_value(Some(c))), Some(c));
        assert_eq!(color_value(Some(egui::Color32::BLACK)), FieldValue::Color(Some(0)), "black is encoded");
        assert_eq!(color_of(&color_value(None)), None);
    }

    /// Item view (`view_item`) restoring onto a tab that's still spawning
    /// (a tab's restore is always async) must not bounce back to Graph the
    /// first frame -- `build_tab_view_ui` (tab_view.rs) does exactly that
    /// for any Item view whose focused id isn't a live tab yet, unless
    /// `pending_respawn_focus` names it (the same guard a crash respawn
    /// uses). `apply_local` sets it for this case; this only re-checks the
    /// state it leaves behind, since the actual bounce-back is UI code this
    /// module can't drive without a window.
    #[test]
    fn item_view_restoring_onto_a_still_spawning_tab_sets_the_respawn_focus_guard() {
        let path = crate::store::testutil::temp_dir("view-item-spawning").join("session.mnf");
        let mut app = app_on(&path, 1);
        let tab = add(
            &mut app,
            NodePayload::Unloaded(UnloadedTabPayload {
                url: "https://x/".into(),
                title: String::new(),
                kind: EngineKind::Servo,
                pinned: false,
                muted: false,
                zoom_factor: 1.0,
                state: UnloadedState::Failed("x".into()),
            }),
        );
        // A node whose id isn't live yet -- `spawning`, not `self.nodes` --
        // is exactly the shape a real tab restore has before its spawn
        // lands (`materialize`'s `Kind::Tab` arm never inserts directly).
        app.nodes.remove(&tab);
        app.spawning.insert(tab.clone(), shared(Kind::Tab, None, &[]));
        let state = LocalState { focused: Some(tab.clone()), view_item: true, ..LocalState::default() };
        app.apply_local(state);
        assert_eq!(app.view_mode, ViewMode::Item);
        assert_eq!(app.pending_respawn_focus.as_deref(), Some(tab.as_str()));
    }

    /// A launch that changes nothing (no user action at all) must rewrite
    /// neither the session nor its personal state -- otherwise every launch
    /// would touch both files even when nothing happened. `insert_note_at`
    /// (main.rs) is private to that module and also saves as a side effect,
    /// so this uses the `add` helper and sets the settled position directly,
    /// same as Task 5's own no-window tests.
    #[test]
    fn an_idle_open_and_close_writes_nothing() {
        let dir = crate::store::testutil::temp_dir("idle-app");
        let path = dir.join("session.mnf");
        {
            let mut app = app_on(&path, 1);
            let id = add(&mut app, NodePayload::Note(NotePayload { text: String::new(), editing: false, url: None }));
            app.nodes.get_mut(&id).unwrap().pos = egui::vec2(3.0, 4.0);
            app.close_store();
        }
        let before = (std::fs::read(&path).unwrap(), std::fs::read(crate::store::local::path_for(&path)).unwrap());
        let mut app = app_on(&path, 2);
        assert!(app.restore_session());
        app.close_store();
        let after = (std::fs::read(&path).unwrap(), std::fs::read(crate::store::local::path_for(&path)).unwrap());
        assert!(before == after, "an idle launch rewrote the session or its personal state");
    }

    /// A note made from a web page keeps its URL through save and restore
    /// (content dispatch spec): `shared_nodes` reports it under `field::URL`
    /// and `restore_session` hands it back via `GraphNode::source_url`.
    #[test]
    fn content_nodes_keep_their_source_url() {
        let path = crate::store::testutil::temp_dir("content-url").join("session.mnf");
        let mut app = app_on(&path, 1);
        let id = app.alloc_node_id();
        let payload = NotePayload { text: "hi".into(), editing: false, url: Some("https://x.test/a.txt".into()) };
        app.insert_placeholder_node(id.clone(), egui::Vec2::ZERO, NodePayload::Note(payload));
        let shared = app.shared_nodes();
        let fields = &shared[&id.parse().unwrap()].fields;
        assert_eq!(fields.get(&field::URL), Some(&FieldValue::Str("https://x.test/a.txt".into())));
        assert!(app.save_session());
        app.close_store();
        let mut restored = app_on(&path, 2);
        assert!(restored.restore_session());
        assert_eq!(restored.nodes[&id].source_url(), Some("https://x.test/a.txt"));
    }

    #[test]
    fn path_is_explicit() {
        assert!(crate::store::model::field_def(field::PATH).unwrap().explicit);
    }

    /// A group export round-trips as a JSON snapshot: `export_group_json`
    /// gives the group and its one member, root first, and `import_group_
    /// json` recreates both under fresh ids, leaving the originals in
    /// place (so the level now holds four nodes, not two).
    #[test]
    fn a_group_export_imports_as_fresh_nodes() {
        let dir = crate::store::testutil::temp_dir("group-json");
        let mut app = app_on(&dir.join("session.mnf"), 1);
        let g = app.alloc_node_id();
        app.insert_group_at(g.clone(), egui::vec2(0.0, 0.0));
        let n = app.alloc_node_id();
        app.insert_note_at(n.clone(), egui::vec2(10.0, 0.0));
        app.nodes.get_mut(&n).unwrap().group = Some(g.clone());
        let json = app.export_group_json(&g).unwrap();
        assert_eq!(json.nodes.len(), 2);
        assert_eq!(json.nodes[0].kind, "group");
        app.import_group_json(&json);
        assert_eq!(app.shared_nodes().len(), 4, "the import added a second group and note");
    }

    fn failed_tab(url: &str, zoom_factor: f64) -> NodePayload {
        NodePayload::Unloaded(UnloadedTabPayload {
            url: url.into(),
            title: String::new(),
            kind: EngineKind::Servo,
            pinned: false,
            muted: false,
            zoom_factor,
            state: UnloadedState::Failed("x".into()),
        })
    }

    /// A tab restored asleep, as `add` inserts it.
    pub(crate) fn sleeping_tab(url: &str, title: &str) -> NodePayload {
        NodePayload::Unloaded(UnloadedTabPayload {
            url: url.into(),
            title: title.into(),
            kind: EngineKind::Servo,
            pinned: false,
            muted: false,
            zoom_factor: 1.0,
            state: UnloadedState::Sleeping,
        })
    }

    /// An unloaded tab's title and mute survive a restart through personal
    /// state, though it has no engine to report either.
    #[test]
    fn personal_state_keeps_an_unloaded_tabs_title_and_mute() {
        let mut app = app_on(&temp_dir("unloaded-local").join("session.mnf"), 1);
        let tab = add(&mut app, sleeping_tab("https://s/", "Saved title"));
        let set = |app: &mut App, title: &str, muted: bool| {
            if let NodePayload::Unloaded(t) = &mut app.nodes.get_mut(&tab).unwrap().payload {
                (t.title, t.muted) = (title.to_string(), muted);
            }
        };
        set(&mut app, "Saved title", true);
        let entry = app.local_state().nodes[&tab].clone();
        assert_eq!((entry.title.as_str(), entry.muted), ("Saved title", true));
        set(&mut app, "", false);
        App::apply_local_node(app.nodes.get_mut(&tab).unwrap(), &entry);
        let t = app.nodes[&tab].as_unloaded().unwrap();
        assert_eq!((t.title.as_str(), t.muted), ("Saved title", true));
    }

    /// Closing an unloaded card moves its children to its parent, as
    /// closing a live tab does, instead of leaving them naming a node
    /// that's gone.
    #[test]
    fn closing_an_unloaded_tab_reparents_its_children() {
        let mut app = app_on(&temp_dir("unloaded-close").join("session.mnf"), 1);
        let root = add(&mut app, sleeping_tab("https://root/", ""));
        let mid = add(&mut app, failed_tab("https://mid/", 1.0));
        let leaf = add(&mut app, sleeping_tab("https://leaf/", ""));
        app.nodes.get_mut(&mid).unwrap().parent = Some(root.clone());
        app.nodes.get_mut(&leaf).unwrap().parent = Some(mid.clone());
        app.close_node(&mid);
        assert!(!app.nodes.contains_key(&mid));
        assert_eq!(app.nodes[&leaf].parent.as_deref(), Some(root.as_str()));
    }

    /// A restore starts only the focused tab; every other tab is a
    /// sleeping card with its saved title.
    #[test]
    fn a_restore_starts_only_the_focused_tab() {
        let path = temp_dir("lazy-restore").join("session.mnf");
        let (a, b, c) = {
            let mut app = app_on(&path, 1);
            let a = add(&mut app, sleeping_tab("https://a/", "A"));
            let b = add(&mut app, sleeping_tab("https://b/", "B"));
            let c = add(&mut app, sleeping_tab("https://c/", "C"));
            app.focused = Some(b.clone());
            assert!(app.save_session());
            app.maybe_write_local(true);
            app.close_store();
            (a, b, c)
        };
        let mut app = app_on(&path, 2);
        assert!(app.restore_session());
        let spawned: Vec<&str> = app.test_spawns.iter().map(|(s, _)| s.id.as_str()).collect();
        assert_eq!(spawned, [b.as_str()]);
        assert!(matches!(app.test_spawns[0].0.purpose, SpawnPurpose::Wake));
        for (id, title) in [(&a, "A"), (&c, "C")] {
            let t = app.nodes[id].as_unloaded().unwrap();
            assert_eq!((t.state.clone(), t.title.as_str()), (UnloadedState::Sleeping, title));
        }
        assert_eq!(app.nodes[&b].as_unloaded().unwrap().state, UnloadedState::Waking);
    }

    /// A restored split session wakes both halves -- the focused tab and
    /// the saved split tab -- leaving every other tab asleep (Fix 2,
    /// final-review: `apply_local` restores `split_tab` even though only
    /// `wake_focused` used to actually start an engine, leaving the split
    /// pane empty).
    #[test]
    fn restoring_a_split_session_wakes_both_the_focused_and_split_tab() {
        let path = temp_dir("lazy-restore-split").join("session.mnf");
        let (a, b, c) = {
            let mut app = app_on(&path, 1);
            let a = add(&mut app, sleeping_tab("https://a/", "A"));
            let b = add(&mut app, sleeping_tab("https://b/", "B"));
            let c = add(&mut app, sleeping_tab("https://c/", "C"));
            app.focused = Some(a.clone());
            app.split_tab = Some(b.clone());
            assert!(app.save_session());
            app.maybe_write_local(true);
            app.close_store();
            (a, b, c)
        };
        let mut app = app_on(&path, 2);
        assert!(app.restore_session());
        let mut spawned: Vec<&str> = app.test_spawns.iter().map(|(s, _)| s.id.as_str()).collect();
        spawned.sort();
        let mut expected = [a.as_str(), b.as_str()];
        expected.sort();
        assert_eq!(spawned, expected);
        assert!(app.test_spawns.iter().all(|(s, _)| matches!(s.purpose, SpawnPurpose::Wake)));
        assert_eq!(app.nodes[&c].as_unloaded().unwrap().state, UnloadedState::Sleeping);
    }

    /// Opening a sleeping tab, by any path, wakes it once: focused, in Tab
    /// view (showing the respawning placeholder), one spawn. A second open
    /// while it wakes issues none.
    #[test]
    fn opening_a_sleeping_tab_wakes_it_once() {
        let mut app = app_on(&temp_dir("lazy-open").join("session.mnf"), 1);
        let a = add(&mut app, sleeping_tab("https://a/", "Alpha"));
        let b = add(&mut app, sleeping_tab("https://b/", "Beta"));
        app.focus_item(&a);
        app.focus_item(&a);
        assert_eq!(app.test_spawns.len(), 1);
        assert_eq!(app.focused.as_deref(), Some(a.as_str()));
        assert_eq!(app.view_mode, ViewMode::Item);
        assert_eq!(app.pending_respawn_focus.as_deref(), Some(a.as_str()));
        app.focus_tab(&b);
        assert_eq!(app.test_spawns.len(), 2);
        assert_eq!(app.nodes[&b].as_unloaded().unwrap().state, UnloadedState::Waking);
    }

    /// Tab lists (Quick Switcher, Ctrl+Tab, graph navigation, Close All,
    /// pinning) include sleeping tabs, and opening one from them wakes it.
    #[test]
    fn tab_lists_include_sleeping_tabs() {
        let mut app = app_on(&temp_dir("lazy-lists").join("session.mnf"), 1);
        let a = add(&mut app, sleeping_tab("https://a/", "Alpha"));
        let b = add(&mut app, sleeping_tab("https://b/", "Beta"));
        assert_eq!(app.quick_switcher_matches("beta"), std::slice::from_ref(&b));
        app.quick_switcher = Some(crate::QuickSwitcher { query: "beta".into(), selected: 0 });
        app.quick_switcher_confirm();
        assert_eq!(app.focused.as_deref(), Some(b.as_str()));
        assert_eq!(app.test_spawns.len(), 1);
        app.cycle_focused_tab(true);
        assert_eq!(app.focused.as_deref(), Some(a.as_str()));
        assert_eq!(app.test_spawns.len(), 2);
        assert_eq!(app.tabs_at_current_level(), [a.clone(), b.clone()]);
        app.set_all_pinned(true);
        assert!(app.nodes[&a].payload.pinned() && app.nodes[&b].payload.pinned());
        app.close_all_at_current_level();
        assert!(app.nodes.is_empty());
    }

    /// A failed wake leaves a failed card, still focused, and ends the
    /// respawning placeholder (Tab view then falls back to Graph).
    #[test]
    fn a_failed_wake_leaves_a_failed_card() {
        let mut app = app_on(&temp_dir("lazy-fail").join("session.mnf"), 1);
        let a = add(&mut app, sleeping_tab("https://a/", "A"));
        app.focus_tab(&a);
        fail_spawns(&mut app);
        assert!(matches!(app.nodes[&a].as_unloaded().unwrap().state, UnloadedState::Failed(_)));
        assert_eq!(app.nodes[&a].as_unloaded().unwrap().title, "A", "the wake carried its title");
        assert_eq!(app.pending_respawn_focus, None);
        assert!(app.wakes_in_flight.is_empty());
    }

    /// Closing a tab while its wake is in flight deletes it from the store
    /// once the (now-moot) spawn result lands -- the landing's "closed
    /// while waking" branch used to return without saving, and a save
    /// while the wake was in flight kept the id anyway (`spawning`), so it
    /// lingered in the store forever (Fix 3, final-review).
    #[test]
    fn closing_a_tab_while_it_wakes_deletes_it_from_the_store() {
        let path = temp_dir("close-waking").join("session.mnf");
        let mut app = app_on(&path, 1);
        let tab = add(&mut app, sleeping_tab("https://a/", "A"));
        assert!(app.save_session());
        app.focus_tab(&tab);
        assert_eq!(app.test_spawns.len(), 1, "the wake is in flight, parked");
        assert!(app.spawning.contains_key(&tab));
        app.destroy_tab(&tab);
        assert!(!app.nodes.contains_key(&tab), "closed");
        // Deliver the parked wake's result -- a failed one, the only kind a
        // unit test can produce (no fake `EngineTab` for a success).
        fail_spawns(&mut app);
        let still_live = app.journal.as_ref().unwrap().doc().live().any(|(id, _)| id.to_string() == tab);
        assert!(!still_live, "should have been deleted once the wake landed on a closed tab");
    }

    /// Closing a waking tab clears `pending_respawn_focus` when it names
    /// the removed id -- otherwise it keeps pointing at a gone node, which
    /// `build_tab_view_ui`'s guard reads to decide whether Item view stays
    /// put during a respawn (Fix 6, final-review).
    #[test]
    fn removing_a_tab_clears_its_stale_pending_respawn_focus() {
        let mut app = app_on(&temp_dir("remove-pending-focus").join("session.mnf"), 1);
        let tab = add(&mut app, sleeping_tab("https://a/", "A"));
        app.focus_tab(&tab);
        assert_eq!(app.pending_respawn_focus.as_deref(), Some(tab.as_str()));
        app.destroy_tab(&tab);
        assert_eq!(app.pending_respawn_focus, None);
    }

    /// "Mute Others" mutes a sleeping or waking card too -- it has no
    /// engine to send `SetMuted` to, but the landing already sends it
    /// (`TabCarry::muted`) and personal state saves it either way, so a
    /// woken tab comes back muted like every other tab "Mute Others" hit
    /// (Fix 8, final-review).
    #[test]
    fn mute_all_except_also_mutes_sleeping_tabs() {
        let mut app = app_on(&temp_dir("mute-others-sleeping").join("session.mnf"), 1);
        let keep = add(&mut app, sleeping_tab("https://keep/", "Keep"));
        let other = add(&mut app, sleeping_tab("https://other/", "Other"));
        app.mute_all_except(&keep);
        assert!(!app.nodes[&keep].as_unloaded().unwrap().muted, "kept tab untouched");
        assert!(app.nodes[&other].as_unloaded().unwrap().muted);
        let entry = app.local_state().nodes[&other].clone();
        assert!(entry.muted, "personal state saves the sleeping card's mute");
    }

    /// Another replica's url change moves a sleeping tab without waking it.
    #[test]
    fn a_remote_url_change_moves_a_sleeping_tab_without_waking_it() {
        let path = temp_dir("lazy-remote").join("session.mnf");
        let mut app = app_on(&path, 1);
        let tab = add(&mut app, sleeping_tab("https://s/", "S"));
        assert!(app.save_session());
        let mut other = Journal::open(&path, ActorId(2)).unwrap();
        remote_set(&mut other, &tab, field::URL, FieldValue::Str("https://moved/".into()));
        assert!(app.save_session());
        assert_eq!(app.nodes[&tab].as_unloaded().unwrap().url, "https://moved/");
        assert!(app.test_spawns.is_empty());
    }

    /// A tab another replica creates arrives asleep at its url, so another
    /// instance opening a big session starts no engines here.
    #[test]
    fn a_remote_new_tab_arrives_asleep() {
        let url = FieldValue::Str("https://new/".into());
        let app = app_with(&BTreeMap::from([(id(1), shared(Kind::Tab, None, &[(field::URL, url)]))]));
        assert!(app.test_spawns.is_empty());
        let tab = app.nodes[&id(1).to_string()].as_unloaded().unwrap();
        assert_eq!((tab.url.as_str(), &tab.state), ("https://new/", &UnloadedState::Sleeping));
    }

    /// Saving a sleeping tab writes what its live tab would: a restore
    /// followed by a save changes nothing in the store.
    #[test]
    fn a_sleeping_tab_saves_as_its_tab() {
        let path = temp_dir("lazy-save").join("session.mnf");
        let tab = {
            let mut app = app_on(&path, 1);
            let tab = add(&mut app, sleeping_tab("https://s/", ""));
            if let NodePayload::Unloaded(t) = &mut app.nodes.get_mut(&tab).unwrap().payload {
                t.pinned = true;
            }
            assert!(app.save_session());
            app.close_store();
            tab
        };
        let mut app = app_on(&path, 2);
        assert!(app.restore_session());
        let engine = stored(&app, &tab, field::ENGINE);
        // The single tab is the focused one, so the restore woke it; fail
        // that wake too: a waking and a failed card save the same way.
        assert!(app.save_session());
        fail_spawns(&mut app);
        assert!(app.save_session());
        assert_eq!(stored(&app, &tab, field::URL), FieldValue::Str("https://s/".into()));
        assert_eq!(stored(&app, &tab, field::PINNED), FieldValue::Bool(true));
        assert_eq!(stored(&app, &tab, field::ENGINE), engine);
    }

    /// Fails every spawn parked in `test_spawns`, as a missing engine would.
    fn fail_spawns(app: &mut App) {
        for (spawn, generation) in std::mem::take(&mut app.test_spawns) {
            let result = Err(std::io::Error::other("no engine"));
            app.handle_tab_spawn_result(crate::TabSpawnOutcome { spawn, generation, result });
        }
    }

    fn json_node(id: &str, kind: &str, fields: serde_json::Value) -> crate::store::json::JsonNode {
        let serde_json::Value::Object(fields) = fields else { panic!("fields are an object") };
        crate::store::json::JsonNode { id: id.into(), kind: kind.into(), fields }
    }

    /// A reopened tab comes back asleep, starting no engine, and keeps its
    /// zoom and title (spec "Lazy tabs").
    #[test]
    fn a_reopened_tab_comes_back_asleep_with_its_zoom_and_title() {
        let mut app = app_on(&temp_dir("reopen-sleep").join("session.mnf"), 1);
        let tab = add(&mut app, sleeping_tab("https://r/", "Page R"));
        if let NodePayload::Unloaded(t) = &mut app.nodes.get_mut(&tab).unwrap().payload {
            t.zoom_factor = 1.5;
        }
        let entry = crate::closed_entry(&app, &tab).unwrap();
        app.nodes.remove(&tab);
        app.node_order.retain(|t| t != &tab);
        app.reopen_closed(entry);
        assert!(app.test_spawns.is_empty());
        let cards: Vec<&UnloadedTabPayload> = app.nodes.values().filter_map(|n| n.as_unloaded()).collect();
        assert!(
            matches!(&cards[..], [t] if t.url == "https://r/" && t.title == "Page R" && t.zoom_factor == 1.5
                && t.state == UnloadedState::Sleeping),
            "no sleeping card"
        );
    }

    /// Same for a tab in an imported group: it sleeps in the new group.
    #[test]
    fn an_imported_groups_tab_arrives_asleep_in_the_new_group() {
        let mut app = app_on(&temp_dir("import-sleep").join("session.mnf"), 1);
        let g = add(&mut app, NodePayload::Group(GroupPayload { name: "G".into(), from_json: false }));
        let tab = add(&mut app, failed_tab("https://i/", 1.0));
        app.nodes.get_mut(&tab).unwrap().group = Some(g.clone());
        let json = app.export_group_json(&g).unwrap();
        app.import_group_json(&json);
        assert!(app.test_spawns.is_empty());
        let (_, copy) = app.nodes.iter().find(|(id, n)| **id != tab && n.as_unloaded().is_some()).expect("a card");
        assert_eq!(copy.as_unloaded().unwrap().state, UnloadedState::Sleeping);
        let group = copy.group.as_deref().expect("in a group");
        assert!(group != g && app.nodes[group].as_group().is_some(), "in the imported group");
    }

    /// A member whose group isn't part of the imported file lands where
    /// the file's root does (the level being viewed), not at the top level.
    #[test]
    fn an_imported_member_whose_group_is_outside_the_file_joins_the_roots_level() {
        let mut app = app_on(&temp_dir("import-outside").join("session.mnf"), 1);
        let level = add(&mut app, NodePayload::Group(GroupPayload { name: "L".into(), from_json: false }));
        let elsewhere = add(&mut app, NodePayload::Group(GroupPayload { name: "E".into(), from_json: false }));
        app.current_group = Some(level.clone());
        let json = JsonDoc {
            nodes: vec![
                json_node("a", "group", serde_json::json!({ "name": "G" })),
                json_node("b", "note", serde_json::json!({ "text": "t", "group": elsewhere })),
            ],
            local: None,
        };
        app.import_group_json(&json);
        let note = app.nodes.values().find(|n| matches!(&n.payload, NodePayload::Note(_))).expect("imported");
        assert_eq!(note.group.as_deref(), Some(level.as_str()));
    }

    /// A session with no nodes still keeps Recently Closed: the launch
    /// starts fresh, but the next personal-state write carries the list.
    #[test]
    fn an_empty_session_keeps_recently_closed_across_a_launch() {
        let path = temp_dir("empty-closed").join("session.mnf");
        let entry = JsonDoc { nodes: vec![json_node("x", "note", serde_json::json!({ "text": "gone" }))], local: None };
        {
            let journal = Journal::open(&path, ActorId(1)).unwrap();
            let state = LocalState { closed: vec![entry.clone()], ..LocalState::default() };
            journal.write_local(&state.to_bytes()).unwrap();
        }
        let mut app = app_on(&path, 2);
        assert!(!app.restore_session(), "no nodes: the caller starts fresh");
        assert_eq!(app.closed.len(), 1);
        add(&mut app, NodePayload::Note(NotePayload { text: "n".into(), editing: false, url: None }));
        app.maybe_write_local(true);
        let bytes = app.journal.as_ref().unwrap().read_local().unwrap().unwrap();
        assert_eq!(LocalState::from_bytes(&bytes).unwrap().closed, vec![entry]);
    }

    /// Quitting while an imported session's tabs are still spawning keeps
    /// them: each is saved as it was started, and the replaced session's
    /// nodes are gone.
    #[test]
    fn tabs_still_spawning_at_quit_are_saved() {
        let path = temp_dir("spawning-quit").join("session.mnf");
        let mut app = app_on(&path, 1);
        add(&mut app, NodePayload::Note(NotePayload { text: "old".into(), editing: false, url: None }));
        assert!(app.save_session());
        let json =
            JsonDoc { nodes: vec![json_node("t", "tab", serde_json::json!({ "url": "https://new/" }))], local: None };
        app.replace_running_session(&json);
        assert_eq!(app.test_spawns.len(), 1, "the tab is still spawning");
        app.save_session();
        app.close_store();
        drop(app);
        let journal = Journal::open(&path, ActorId(3)).unwrap();
        let live: Vec<(Kind, FieldValue)> = journal.doc().live().map(|(_, n)| (n.kind, n.get(field::URL))).collect();
        assert_eq!(live, [(Kind::Tab, FieldValue::Str("https://new/".into()))]);
    }

    /// Open Session (`replace_running_session`) restores every tab asleep,
    /// same as a launch, and wakes only the focused (first) one -- the
    /// existing `tabs_still_spawning_at_quit_are_saved` uses a single tab,
    /// which can't tell "asleep" from "still spawning" apart.
    #[test]
    fn open_session_restores_asleep_and_wakes_only_the_focused_tab() {
        let mut app = app_on(&temp_dir("open-session-lazy").join("session.mnf"), 1);
        let json = JsonDoc {
            nodes: vec![
                json_node("t1", "tab", serde_json::json!({ "url": "https://a/" })),
                json_node("t2", "tab", serde_json::json!({ "url": "https://b/" })),
                json_node("t3", "tab", serde_json::json!({ "url": "https://c/" })),
            ],
            local: None,
        };
        app.replace_running_session(&json);
        assert_eq!(app.test_spawns.len(), 1, "only the focused (first) tab wakes");
        assert!(matches!(app.test_spawns[0].0.purpose, SpawnPurpose::Wake));
        let focused = app.focused.clone().unwrap();
        assert_eq!(app.test_spawns[0].0.id, focused);
        let asleep: Vec<&UnloadedTabPayload> =
            app.nodes.iter().filter(|(id, _)| **id != focused).filter_map(|(_, n)| n.as_unloaded()).collect();
        assert_eq!(asleep.len(), 2);
        assert!(asleep.iter().all(|t| t.state == UnloadedState::Sleeping));
    }

    /// Without personal state, restore focuses the first tab (as it always
    /// has) and wakes only that one.
    #[test]
    fn restore_without_personal_state_focuses_and_wakes_the_first_tab() {
        let path = temp_dir("no-local-restore").join("session.mnf");
        let (a, b) = {
            let mut app = app_on(&path, 1);
            let a = add(&mut app, sleeping_tab("https://a/", "A"));
            let b = add(&mut app, sleeping_tab("https://b/", "B"));
            assert!(app.save_session());
            // No `maybe_write_local`: no `session.mnf.local` at all, the
            // literal "without personal state" case, not just one whose
            // `focused` field happens to be unset.
            (a, b)
        };
        let mut app = app_on(&path, 2);
        assert!(app.restore_session());
        assert_eq!(app.focused.as_deref(), Some(a.as_str()));
        let spawned: Vec<&str> = app.test_spawns.iter().map(|(s, _)| s.id.as_str()).collect();
        assert_eq!(spawned, [a.as_str()]);
        assert_eq!(app.nodes[&b].as_unloaded().unwrap().state, UnloadedState::Sleeping);
    }

    /// Personal state written before a restored tab's wake lands still
    /// keeps theirs (position, title, zoom), and a fresh failed card's zoom
    /// is kept too. A restored tab is an `Unloaded` card at once, so
    /// `apply_local` applies its entry directly; the `pending_local` path
    /// for a node still spawning (a group import, Recently Closed) isn't
    /// covered here.
    #[test]
    fn personal_state_survives_onto_a_still_waking_restored_card_and_a_fresh_failed_cards_zoom() {
        let path = temp_dir("local-spawning").join("session.mnf");
        let tab = {
            let mut app = app_on(&path, 1);
            let tab = add(&mut app, failed_tab("https://s/", 1.0));
            app.close_store();
            tab
        };
        let entry = LocalNode { pos: [7.0, 8.0], title: "T".into(), zoom: Some(2.0), ..LocalNode::default() };
        {
            let journal = Journal::open(&path, ActorId(2)).unwrap();
            let state = LocalState { nodes: BTreeMap::from([(tab.clone(), entry.clone())]), ..LocalState::default() };
            journal.write_local(&state.to_bytes()).unwrap();
        }
        let mut app = app_on(&path, 3);
        assert!(app.restore_session());
        // Still waking, not landed -- `entry` is applied straight onto the
        // live `Unloaded` card (`apply_local_node`), not carried through
        // `pending_local`.
        assert_eq!(app.test_spawns.len(), 1, "the restored tab's wake is still in flight");
        let card = add(&mut app, failed_tab("https://f/", 1.5));
        let state = app.local_state();
        assert_eq!(state.nodes.get(&tab), Some(&entry));
        assert_eq!(state.nodes[&card].zoom, Some(1.5));
    }

    /// A tab its page moved on without sharing (a redirect, a script's
    /// navigation) reopens where it was, unless the shared url has changed
    /// since.
    #[test]
    fn a_restart_reopens_a_tabs_live_url_while_the_shared_url_is_unchanged() {
        let path = temp_dir("live-url").join("session.mnf");
        let tab = {
            let mut app = app_on(&path, 1);
            let tab = add(&mut app, failed_tab("https://s/", 1.0));
            assert!(app.save_session());
            if let NodePayload::Unloaded(t) = &mut app.nodes.get_mut(&tab).unwrap().payload {
                t.url = "https://s/moved".into();
            }
            app.close_store();
            tab
        };
        let mut app = app_on(&path, 2);
        assert!(app.restore_session());
        assert_eq!(app.test_spawns[0].0.url, "https://s/moved");
        assert_eq!(stored(&app, &tab, field::URL), FieldValue::Str("https://s/".into()), "still unshared");
        // The single tab is focused, so it is the one waking.
        assert_eq!(app.nodes[&tab].as_unloaded().unwrap().url, "https://s/moved");
        app.close_store();

        // Moved on from a url the store no longer holds: the shared one wins.
        {
            let journal = Journal::open(&path, ActorId(3)).unwrap();
            let live_url = Some(LiveUrl { url: "https://s/moved".into(), from: "https://elsewhere/".into() });
            let entry = LocalNode { live_url, ..LocalNode::default() };
            let state = LocalState { nodes: BTreeMap::from([(tab.clone(), entry)]), ..LocalState::default() };
            journal.write_local(&state.to_bytes()).unwrap();
        }
        let mut app = app_on(&path, 4);
        assert!(app.restore_session());
        assert_eq!(app.test_spawns[0].0.url, "https://s/");
    }

    /// Export Group of a group the store doesn't hold gives nothing (and
    /// says so) rather than an empty file.
    #[test]
    fn exporting_a_group_the_store_lacks_gives_none() {
        let mut app = app_on(&temp_dir("export-missing").join("session.mnf"), 1);
        let g = app.alloc_node_id();
        assert!(app.export_group_json(&g).is_none());
        assert!(!app.save_toast_queue.is_empty(), "toasted");
    }

    /// `save_session` refreshes `last_shared` only for nodes its ops name
    /// (all of them after a delete), trusting `materialize` for every other
    /// store change. After each kind of save, `last_shared` must still be
    /// exactly what the store holds for every live node.
    #[test]
    fn last_shared_matches_the_store_after_every_save() {
        fn check(app: &App, step: &str) {
            let doc = app.journal.as_ref().unwrap().doc();
            for (id, node) in doc.live() {
                let want = Shared { kind: node.kind, group: doc.effective_group(id), fields: fields_of(node) };
                assert_eq!(app.last_shared.get(&id.to_string()), Some(&want), "{step}: node {id}");
            }
        }
        let dir = temp_dir("last-shared");
        let path = dir.join("session.mnf");
        let mut app = app_on(&path, 1);
        let group = |name: &str| NodePayload::Group(GroupPayload { name: name.into(), from_json: false });
        let outer = add(&mut app, group("outer"));
        let inner = add(&mut app, group("inner"));
        let tab = add(&mut app, failed_tab("https://a/", 1.0));
        let note = add(&mut app, NodePayload::Note(NotePayload { text: String::new(), editing: false, url: None }));
        app.nodes.get_mut(&inner).unwrap().group = Some(outer.clone());
        app.nodes.get_mut(&tab).unwrap().group = Some(inner.clone());
        assert!(app.save_session());
        check(&app, "creates");

        if let NodePayload::Note(n) = &mut app.nodes.get_mut(&note).unwrap().payload {
            n.text = "typed".into();
        }
        assert!(app.save_session());
        check(&app, "a set");

        // Another replica pins the tab and deletes the inner group, so the
        // tab's effective group becomes `outer` with no op naming the tab.
        let mut other = Journal::open(&path, ActorId(2)).unwrap();
        remote_set(&mut other, &tab, field::PINNED, FieldValue::Bool(true));
        other.begin().unwrap().commit(vec![OpBody::Delete { node: inner.parse().unwrap() }]).unwrap();
        assert!(app.save_session());
        check(&app, "remote set and delete");

        app.dissolve_group(&outer);
        assert!(app.save_session());
        check(&app, "a local delete and moves");

        if let NodePayload::Unloaded(t) = &mut app.nodes.get_mut(&tab).unwrap().payload {
            t.url = "https://b/".into();
        }
        app.mark(&tab, field::URL);
        assert!(app.save_session());
        check(&app, "a marked url");
    }

    /// What the app's side of a save costs at realistic session sizes: the
    /// live projection (`shared_nodes`), a whole `save_session` (projection,
    /// `begin`, `diff`, commit, `last_shared` refresh) with nothing and with
    /// one url to save, and the personal-state build and write. The store
    /// side alone is `journal::tests::save_path_costs`. Timing, not
    /// correctness, so ignored:
    /// `cargo test --release -j 12 -p manifold -- --ignored --nocapture app_save_costs`
    #[test]
    #[ignore]
    fn app_save_costs() {
        use std::time::{Duration, Instant};
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
        const N: usize = 40;
        for tabs in [300usize, 3000] {
            let dir = temp_dir(&format!("app-cost-{tabs}"));
            let mut app = app_on(&dir.join("session.mnf"), 1);
            let groups: Vec<String> = (0..tabs / 10)
                .map(|i| {
                    add(&mut app, NodePayload::Group(GroupPayload { name: format!("Group {i}"), from_json: false }))
                })
                .collect();
            let mut ids = Vec::new();
            for i in 0..tabs {
                let url =
                    format!("https://www.example{}.com/articles/2026/09/some-long-article-slug-{i}?ref=home", i % 50);
                let id = add(&mut app, failed_tab(&url, 1.0));
                let node = app.nodes.get_mut(&id).unwrap();
                node.group = Some(groups[i % groups.len()].clone());
                node.pos = egui::vec2(i as f32 * 3.5, i as f32 * -2.25);
                ids.push(id);
            }
            assert!(app.save_session());
            println!("{tabs} tabs, {} groups:", groups.len());

            report("shared_nodes (per save)", timed(N, || drop(app.shared_nodes())));
            let live = app.shared_nodes();
            let doc = app.journal.as_ref().unwrap().doc();
            report(
                "diff, nothing changed",
                timed(N, || assert!(diff(doc, &live, &app.explicit, &BTreeSet::new()).is_empty())),
            );
            report(
                "last_shared refresh (after a commit)",
                timed(N, || {
                    for &id in live.keys() {
                        let node = &doc.nodes[&id];
                        let seen = Shared { kind: node.kind, group: doc.effective_group(id), fields: fields_of(node) };
                        std::hint::black_box(seen);
                    }
                }),
            );
            report("save_session, nothing changed", timed(N, || assert!(app.save_session())));
            let mut k = 0;
            report(
                "save_session, one url changed",
                timed(N, || {
                    let id = &ids[k % tabs];
                    if let NodePayload::Unloaded(f) = &mut app.nodes.get_mut(id).unwrap().payload {
                        f.url = format!("https://moved.example/{k}");
                    }
                    app.mark(id, field::URL);
                    k += 1;
                    assert!(app.save_session());
                }),
            );
            report("local_state (every 30 s, and at exit)", timed(N, || drop(app.local_state())));
            report(
                "maybe_write_local, changed (build + write)",
                timed(N, || {
                    app.nodes.get_mut(&ids[k % tabs]).unwrap().pos.x += 1.0;
                    k += 1;
                    app.maybe_write_local(true);
                }),
            );
            let bytes = app.local_state().to_bytes().len();
            println!("  session.mnf.local: {bytes} B");
        }
    }
}
