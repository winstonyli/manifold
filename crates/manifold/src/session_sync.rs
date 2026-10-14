//! The live graph and the session store (spec "Local edits become ops").
//! The first half is pure: a node as the store holds it (`Snapshot`), the
//! `Create`s for queued new nodes and the one link resolver. The `impl App`
//! half after it restores the session, saves local edits (the queued
//! creates, moves, sets and deletes) and shows other replicas' ops.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use ipc_protocol::HostToView;

use crate::local_state::{LiveUrl, LocalNode, LocalState};
use crate::node::{
    DownloadPayload, ImagePayload, JsonPayload, NotePayload, PdfPayload, Runtime, UnloadedState, UnloadedTabPayload,
};
use crate::process_manager::ViewKind;
use crate::store::journal::{JournalError, Recovery};
use crate::store::json::{Imported, JsonDoc};
use crate::store::model::{
    decode_field, encode_field, field, field_def, Doc, FieldType, FieldValue, Kind, Node, NodeId,
};
use crate::store::ops::OpBody;
use crate::tabs::TabSpawn;
use crate::{App, ClosedEntry, ViewMode, MAX_CLOSED};

/// How long after a save that didn't commit `poll_store` tries again:
/// soon enough that an edit isn't left unsaved, slow enough that a failing
/// disk isn't retried every frame.
const SAVE_RETRY: std::time::Duration = std::time::Duration::from_secs(1);

/// How long after a host-started load last reported loading its tab's url
/// changes stay local (`App::shares_url_change`), when the engine doesn't
/// say how they started: a script redirect can come just after the page
/// reports loaded.
const HOST_LOAD_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// Personal state startup couldn't read (`App::local_persist`.`local_unread`): when
/// `poll_store` next tries, and whether startup was a fresh start (no
/// graph node), when only Recently Closed is taken from what it reads.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LocalUnread {
    pub(crate) retry_at: std::time::Instant,
    pub(crate) fresh: bool,
}

/// A node's fields by number: every field of its kind (`Kind::fields`) but `group`, absent values included, so a
/// field cleared reads as a change.
pub(crate) type Fields = BTreeMap<u32, FieldValue>;

/// Personal state from a `read_local`: the defaults for no store, no
/// state, one that doesn't decode, or a read that failed (which the caller
/// checks for itself first).
fn local_from(read: Option<Result<Option<Vec<u8>>, JournalError>>) -> LocalState {
    match read {
        Some(Ok(Some(bytes))) => LocalState::from_bytes(&bytes).unwrap_or_default(),
        _ => LocalState::default(),
    }
}

/// A node as the store holds it, or is about to (`App::snapshot`): its kind, its group and its fields of that kind
/// (`Kind::fields`, every one present). A `Create` writes one, Recently Closed records one, and a tab's spawn starts
/// from one (`App::spawning`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Snapshot {
    pub kind: Kind,
    pub group: Option<NodeId>,
    pub fields: Fields,
}

/// A document node's fields for its kind, shaped like `Snapshot::fields`.
pub(crate) fn fields_of(node: &Node) -> Fields {
    node.kind().fields().iter().map(|&no| (no, node.get(no))).collect()
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

/// Whether a card of stored type `ty` can run on `runtime`: the pairing `place_node` and the landings establish.
/// A tab's runtime is one type whether it is loaded or asleep; a group has none (2b-6 Ruling 8).
#[cfg(debug_assertions)]
fn runtime_has_type(runtime: &Runtime, ty: &str) -> bool {
    use crate::store::model::{
        TYPE_DOWNLOAD, TYPE_GROUP, TYPE_IMAGE, TYPE_JAR, TYPE_JSON, TYPE_NOTE, TYPE_PDF, TYPE_TAB,
    };
    match runtime {
        Runtime::Jar => ty == TYPE_JAR,
        Runtime::Tab(_) | Runtime::Unloaded(_) => ty == TYPE_TAB,
        Runtime::Download(_) => ty == TYPE_DOWNLOAD,
        Runtime::Note(_) => ty == TYPE_NOTE,
        Runtime::Image(_) => ty == TYPE_IMAGE,
        Runtime::Pdf(_) => ty == TYPE_PDF,
        Runtime::Json(_) => ty == TYPE_JSON,
        Runtime::None => ty == TYPE_GROUP,
    }
}

/// `store::model::ENGINES` order: code `n` is `ENGINES[n - 1]`.
pub(crate) fn view_code(kind: ViewKind) -> u64 {
    kind.desc().code
}

/// The engine for a stored code. `0` is an absent field (a tab whose engine
/// was never set), which is Servo, the first engine.
pub(crate) fn view_kind_of(code: u64) -> ViewKind {
    ViewKind::from_code(code).unwrap_or(ViewKind::Servo)
}

/// The one resolver of the links a commit writes (2b-5 Ruling 12): `target` stands if the document shows it live
/// and this commit (`deleted`) doesn't delete it, or this commit (`created`) creates it; otherwise it is written as
/// none (contract "Missing means deleted"; Ruling 5). A closed target is still valid: its node is only gone once
/// its delete commits.
fn resolve_id(
    doc: &Doc,
    created: &BTreeSet<NodeId>,
    deleted: &BTreeSet<NodeId>,
    target: Option<NodeId>,
) -> Option<NodeId> {
    target.filter(|t| (doc.nodes.get(t).is_some_and(Node::is_live) && !deleted.contains(t)) || created.contains(t))
}

/// The `Create`s for `new`, the queued nodes the document has no entry for, each with what it shows (Ruling 3),
/// each group's chain of new ancestors before the nodes inside it (Ruling 13). Every link a `Create` carries
/// (`group`, `parent`, a tab's `jar`, a group's `default_jar`) goes through `resolve_id`, against this commit's
/// `deleted`. A group created later in the batch (only a cycle does that) would make a placeholder: the node is
/// created at the top level instead.
fn create_ops(doc: &Doc, new: &BTreeMap<NodeId, Snapshot>, deleted: &BTreeSet<NodeId>) -> Vec<OpBody> {
    let in_doc = |id: &NodeId| doc.nodes.get(id).is_some_and(Node::is_live);
    let all: BTreeSet<NodeId> = new.keys().copied().collect();
    let valid = |target: Option<NodeId>| resolve_id(doc, &all, deleted, target);
    let mut order = Vec::new();
    let mut queued = BTreeSet::new();
    for &start in new.keys() {
        let mut chain = Vec::new();
        let mut at = Some(start);
        while let Some(id) = at.filter(|id| new.contains_key(id) && !queued.contains(id) && !chain.contains(id)) {
            chain.push(id);
            at = new[&id].group;
        }
        for id in chain.into_iter().rev() {
            queued.insert(id);
            order.push(id);
        }
    }
    let mut ops = Vec::new();
    let mut created = BTreeSet::new();
    for id in order {
        let node = &new[&id];
        let group = valid(node.group).filter(|g| in_doc(g) || created.contains(g));
        let mut fields = node.fields.clone();
        for value in fields.values_mut() {
            if let FieldValue::Id(target) = value {
                *target = valid(*target);
            }
        }
        fields.insert(field::GROUP, FieldValue::Id(group));
        let enc = fields.iter().flat_map(|(&no, value)| encode_field(no, value)).collect();
        ops.push(OpBody::create(id, node.kind, enc));
        created.insert(id);
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

/// A stored node's document pos, or, for one that has none (an old jar from before jars were cards), a jittered
/// spawn around the world center: the rule `App::placement` uses for a new node with no position. A missing `POS`
/// decodes as (0, 0) (`FieldValue::absent`), so the exact origin reads as unplaced, as `Mirror::pos` reads it.
pub(crate) fn pos_of(id: &str, fields: &Fields) -> egui::Vec2 {
    match fields.get(&field::POS) {
        Some(FieldValue::Pos(x, y)) if (*x, *y) != (0.0, 0.0) => egui::vec2(*x, *y),
        _ => crate::graph_layout::GRAPH_WORLD_CENTER + crate::graph_layout::spawn_jitter(id),
    }
}

fn id_of(fields: &Fields, no: u32) -> Option<String> {
    match fields.get(&no) {
        Some(FieldValue::Id(Some(id))) => Some(id.to_string()),
        _ => None,
    }
}

fn view_field(fields: &Fields) -> ViewKind {
    view_kind_of(match fields.get(&field::VIEW_KIND) {
        Some(FieldValue::ViewCode(code)) => *code,
        _ => 0,
    })
}

/// `pos` as the store keeps it: never non-finite (contract "Fields").
pub(crate) fn pos_value(pos: egui::Vec2) -> FieldValue {
    if pos.x.is_finite() && pos.y.is_finite() {
        FieldValue::Pos(pos.x, pos.y)
    } else {
        FieldValue::Pos(0.0, 0.0)
    }
}

/// A parent or group as a store link: `Some(None)` for none, `None` when it names something that isn't a store id.
fn parent_id(parent: &Option<String>) -> Option<Option<NodeId>> {
    match parent {
        None => Some(None),
        Some(id) => id.parse().ok().map(Some),
    }
}

fn parse_id(id: &Option<String>) -> Option<NodeId> {
    link_id(id.as_deref())
}

/// A mirror link as a store id: `None` for none or an id that isn't a store id.
pub(crate) fn link_id(id: Option<&str>) -> Option<NodeId> {
    id.and_then(|id| id.parse().ok())
}

/// The tab `spawn` starts, as a `Snapshot`: what `save_session` writes for a
/// tab whose spawn hasn't landed yet (`App::spawning`). `pos`, `url`, `view_kind` (a
/// stored code) and `jar` are its shared ones (`App::spawn_tab_async`): the
/// mirror's for a tab that has an entry, which a respawn's live `spawn.url`
/// may not be.
pub(crate) fn spawn_snapshot(
    spawn: &TabSpawn,
    pos: Option<egui::Vec2>,
    url: &str,
    view_kind: u64,
    jar: crate::jar::JarChoice,
) -> Snapshot {
    let pos = pos.map_or(FieldValue::Pos(0.0, 0.0), pos_value);
    let mut fields = Fields::from([(field::POS, pos), (field::COLOR, color_value(spawn.carry.color))]);
    let (pinned, parent) = (spawn.carry.pinned, parse_id(&spawn.parent));
    let (url, view_kind) = (FieldValue::Str(url.to_owned()), FieldValue::ViewCode(view_kind));
    let kind = tab_fields(&mut fields, url, view_kind, pinned, parent, jar);
    Snapshot { kind, group: parse_id(&spawn.group), fields }
}

/// A tab's fields beyond `pos` and `color`, live or failed. `url`, `view_kind` and `jar` are the shared ones
/// (`Mirror::url`, the mirror's raw engine code, `Mirror::jar_choice`).
fn tab_fields(
    fields: &mut Fields,
    url: FieldValue,
    view_kind: FieldValue,
    pinned: bool,
    parent: Option<NodeId>,
    jar: crate::jar::JarChoice,
) -> Kind {
    fields.insert(field::URL, url);
    fields.insert(field::VIEW_KIND, view_kind);
    fields.insert(field::PINNED, FieldValue::Bool(pinned));
    fields.insert(field::PARENT, FieldValue::Id(parent));
    fields.insert(field::JAR, FieldValue::Id(jar.jar));
    fields.insert(field::JAR_SET, FieldValue::Bool(jar.set));
    Kind::Tab
}

/// `raw` (an imported node's field values, as `store::json::import` gave
/// them) shaped like `Snapshot::fields`: exactly `kind.fields()`, with
/// any field the file left out filled in as absent, for
/// `App::materialize_imported`.
pub(crate) fn imported_fields(kind: Kind, raw: &BTreeMap<u32, FieldValue>) -> Fields {
    kind.fields()
        .iter()
        .map(|&no| {
            let absent = || FieldValue::absent(field_def(no).expect("a known field").ty);
            (no, raw.get(&no).cloned().unwrap_or_else(absent))
        })
        .collect()
}

/// The fields a live tab and an unloaded one share, read alike for personal
/// state (`local_state`).
struct TabCommon<'a> {
    title: &'a str,
    muted: bool,
    zoom_factor: f64,
    url: &'a str,
}

fn tab_common(payload: &Runtime) -> Option<TabCommon<'_>> {
    match payload {
        Runtime::Tab(t) => Some(TabCommon { title: &t.title, muted: t.muted, zoom_factor: t.zoom_factor, url: &t.url }),
        Runtime::Unloaded(u) => {
            Some(TabCommon { title: &u.title, muted: u.muted, zoom_factor: u.zoom_factor, url: &u.url })
        }
        _ => None,
    }
}

/// The session's write-back bookkeeping (`App::writes`): what the next save commits besides `App::pending`, and when
/// saves happened or are owed.
#[derive(Default)]
pub(crate) struct PendingWrites {
    /// Nodes placed here that the next save creates (2b-5 Ruling 3): every way a node enters the live graph
    /// (`place_node`, `spawn_tab_async`, a jar's `add_jar`) queues its id; the save creates each the document lacks,
    /// from the mirror or the spawn's snapshot (`snapshot`). One the document already holds (a restore, another
    /// replica's node) is dropped. Cleared of created ids by a successful commit.
    pub(crate) pending_creates: BTreeSet<NodeId>,
    /// Nodes removed here that the next save deletes (Ruling 4: `queue_delete`). Only a local removal queues: a
    /// node another replica deleted is already gone from the document, and a respawn, swap or reload only takes
    /// the node out of `nodes` for a while. Cleared by a successful commit.
    pub(crate) pending_deletes: BTreeSet<NodeId>,
    /// Local group moves the next save commits as `Move`s, by node: the target group, `None` for the top level.
    /// Cleared by a successful commit.
    pub(crate) pending_moves: BTreeMap<NodeId, Option<NodeId>>,
    /// Another replica's text for a note being typed in, held back from its mirror and box until the blur
    /// (`end_note_edit`, 2b-4 Ruling 7), by node. Dropped when this replica commits its own text after it.
    pub(crate) held_text: HashMap<String, String>,
    /// Store errors and repair notices toasted since the last commit
    /// (`store_error`, `take_store_recovery`), each shown once: a read-only
    /// file with a damaged tail reports both at every reload.
    pub(crate) store_notices: BTreeSet<String>,
    /// When a save that didn't commit (another instance kept writing, or
    /// the write failed) is tried again by `poll_store`; `None` when
    /// nothing is owed. Without it an edit could sit unsaved until the next
    /// local action or remote write.
    pub(crate) save_retry_at: Option<std::time::Instant>,
    /// When `save_session` last committed to the store -- shown in the
    /// settings popup's Sessions section next to a "Save Now" button, so a
    /// save that's actually continuous (every mutation calls
    /// `save_session`, not a timer) has some visible confirmation instead
    /// of being purely invisible. `None` until the first save this run.
    pub(crate) last_session_save: Option<std::time::Instant>,
}

impl App {
    /// What the store holds for `id`, or is about to, as a `Snapshot`: a graph node's fields of its kind as the mirror
    /// holds them (2b-5: every stored field of a graph node is mirror state), or the snapshot a
    /// tab's spawn started with (`spawning`, for one not shown yet). A tab's url is the shared one, not the live one
    /// its payload holds (2b-3 Ruling 1); its engine is the one a swap in progress already shows as its target
    /// (`start_swap_to`); a file card's path is the stored one, not the one its payload resolved here (2b-5 Ruling
    /// 5). Its kind is its type (`Mirror::ty`, the one source of a node's type). `None` for an in-progress download
    /// (it can't resume, so it isn't created until it completes), for a node of a type this build doesn't know (never
    /// placed today: every way in seeds a known type), and for an id that is none of these.
    pub(crate) fn snapshot(&self, id: NodeId) -> Option<Snapshot> {
        let key = id.to_string();
        if self.nodes.contains_key(&key) {
            if self.is_running_download(&key) {
                return None;
            }
            let kind = Kind::from_type(self.mirror.ty(&key));
            if kind == Kind::Unknown {
                return None;
            }
            let fields = kind
                .fields()
                .iter()
                .map(|&no| match no {
                    field::PARENT => (no, FieldValue::Id(link_id(self.mirror.parent(&key)))),
                    _ => (no, self.mirror.get(&key, no)),
                })
                .collect();
            return Some(Snapshot { kind, group: link_id(self.mirror.group(&key)), fields });
        }
        if self.mirror.is_link(id) {
            let fields = Kind::Link.fields().iter().map(|&no| (no, self.mirror.get(&key, no))).collect();
            return Some(Snapshot { kind: Kind::Link, group: None, fields });
        }
        self.spawns.spawning.get(&key).cloned()
    }

    /// Whether card `key` is a download still in progress (or failed): placed, but not stored until it completes
    /// (`snapshot`), so nothing stored may name it yet (`check_connect`).
    pub(crate) fn is_running_download(&self, key: &str) -> bool {
        self.nodes.get(key).and_then(|n| n.as_download()).is_some_and(|dl| !dl.completed)
    }

    /// Queues `id`, a node that just entered the live graph (`place_node`, `spawn_tab_async`, a jar's `add_jar`),
    /// for the next save to create (Ruling 3), and withdraws a pending delete of it: it is shown again. One the
    /// document already has an entry for (shown again from a restore or another replica's op, or a tombstone) needs
    /// no create; an id that isn't a store id is never written.
    pub(crate) fn queue_create(&mut self, id: &str) {
        let Ok(node) = id.parse::<NodeId>() else { return };
        self.writes.pending_deletes.remove(&node);
        if self.journal.as_ref().is_some_and(|j| j.doc().nodes.contains_key(&node)) {
            return;
        }
        self.writes.pending_creates.insert(node);
    }

    /// Queues `id`, a node removed here, for the next save to delete (Ruling 4). The save deletes it if the document
    /// still shows it live, so a node never created is just forgotten. Until then another replica's edit to it is
    /// dropped rather than showing it again (`materialize`). Only a local removal queues (a remote delete queues
    /// only the deletes of the links naming the node, `remove_remotely_deleted`): a node another replica
    /// deleted is already gone from the document, and a crash respawn, an engine swap
    /// and a content reload take a node out of `nodes` only for a while.
    pub(crate) fn queue_delete(&mut self, id: &str) {
        let Ok(node) = id.parse::<NodeId>() else { return };
        self.writes.pending_creates.remove(&node);
        self.writes.pending_deletes.insert(node);
        // A link goes with either end. `delete_link` forgets the link before it queues the delete, so a link naming
        // a link (remote data can) is found once at most: the recursion ends.
        for link in self.links_naming(id) {
            self.delete_link(&link);
        }
    }

    /// Makes live node `id` show `fields`. A new node is inserted; a new
    /// tab arrives asleep (spec "Lazy tabs"), whatever created it: a
    /// restore, another replica, Recently Closed or an import. A node
    /// whose spawn is in flight keeps the change
    /// for when it lands (`remote_in_flight`). An existing node is updated
    /// in place, but only in the fields where the store differs from what
    /// the mirror shows (`mirror_remote`, which takes them), so an unsaved local
    /// value in another field survives (it is held, and keeps the mirror's value):
    /// - a tab's url change navigates, unless a local url edit is pending
    ///   (it wins);
    /// - an engine change is the mirror's; a live tab swaps to it, unless a
    ///   local engine edit is pending or a swap is in flight (they win);
    /// - a jar change is the mirror's alone: `apply_remote`'s
    ///   `sweep_jar_switches` then switches the live tabs it moved.
    ///
    /// A node or jar whose delete is pending (`queue_delete`) was closed here
    /// and the save deletes it: another replica's edit to it is dropped
    /// rather than bringing it back (Ruling 9).
    ///
    /// A kind mismatch between the live node and `kind` is ignored (the
    /// store never changes a node's kind). A file card whose file can't be
    /// loaded is kept, showing its error: the node stays in the store.
    ///
    /// `local`, personal state's entry for a new node, lays it out at its
    /// settled position instead of `fields`'s stored `pos` (which the mirror
    /// holds), and reopens a tab at its live url (`LocalNode::live_url`).
    /// Every caller but `restore_session` passes `None`.
    pub(crate) fn materialize(
        &mut self,
        id: String,
        kind: Kind,
        fields: &Fields,
        group: Option<String>,
        local: Option<&LocalNode>,
    ) {
        // A node or jar deleted here stays gone: a remote edit arriving before the save commits the queued delete
        // would otherwise re-show it, nameless from a partial change or whole (2b-5 Ruling 9, final review M1).
        if id.parse::<NodeId>().is_ok_and(|node| self.writes.pending_deletes.contains(&node)) {
            return;
        }
        // A link is stored and mirrored, and is never a graph node.
        if kind == Kind::Link {
            let Ok(link) = id.parse::<NodeId>() else { return };
            self.mirror.add_link(link);
            self.queue_create(&id);
            self.mirror_remote(&id, fields, None);
            return;
        }
        if self.spawns.spawning.contains_key(&id) {
            self.spawns.remote_in_flight.insert(id, Some((kind, fields.clone(), group)));
            return;
        }
        let (held, changed) = self.mirror_remote(&id, fields, group.as_deref());
        // The store never changes a node's type, so the first one seen stands (2b-5 Ruling 10).
        if let Some(ty) = kind.type_str() {
            self.mirror.init_field(&id, field::TYPE, FieldValue::Str(ty.to_owned()));
        }
        let pos = local.map_or_else(|| pos_of(&id, fields), |l| egui::vec2(l.pos[0], l.pos[1]));
        if self.update_live_node(&id, fields, pos, &changed, &held) {
            return;
        }
        self.insert_materialized(id, kind, fields, local);
    }

    /// Applies `fields` to the live node `id` in the fields `mirror_remote` found changed (`changed`); the ones it
    /// held at a local edit (`held`) are left out: their live effects follow the mirror's verdict (2b-3 Ruling 3),
    /// so a tab whose url edit is pending is not navigated away from it, and a card whose pos edit is pending is not
    /// moved. A new pos snaps the layout to `pos`, at rest, except for a card being dragged: the drag keeps it under
    /// the pointer, and its release then wins (2b-4 Ruling 4). False when there is no such node.
    fn update_live_node(&mut self, id: &str, fields: &Fields, pos: egui::Vec2, changed: &[u32], held: &[u32]) -> bool {
        let in_flight = self.swap_in_flight(id);
        let changed = |no: u32| !held.contains(&no) && changed.contains(&no);
        if !self.nodes.contains_key(id) {
            return false;
        }
        if changed(field::POS) && !self.is_being_dragged(id) {
            self.snap_layout(id, pos);
        }
        // Only a jar has `ephemeral`. The mirror already holds the new flag (`mirror_remote`), so this applies its
        // effects without comparing (`after_jar_ephemeral_changed`).
        if changed(field::EPHEMERAL) {
            if let Ok(jar) = id.parse::<NodeId>() {
                let on = matches!(fields.get(&field::EPHEMERAL), Some(FieldValue::Bool(true)));
                self.after_jar_ephemeral_changed(jar, on);
            }
        }
        let Some(node) = self.nodes.get_mut(id) else {
            return false;
        };
        let (mut swap, mut reassert) = (None, None);
        let mut store_load = false;
        match &mut node.runtime {
            Runtime::Tab(tab) => {
                let url = str_of(fields, field::URL);
                if changed(field::URL) && tab.url != url {
                    tab.url = url.clone();
                    tab.tab.send(&HostToView::Navigate { url, referrer: None });
                    store_load = true;
                }
                // A swap in flight here (`swap_in_flight`, parked for cookies too) wins: `mirror_remote` held the
                // remote engine back while the swap's edit is unsaved; once saved, its target is written again
                // (`reassert`), so the document ends on the engine this tab lands on.
                let want = view_field(fields);
                if changed(field::VIEW_KIND) {
                    match in_flight {
                        Some(target) if target != want => reassert = Some(target),
                        Some(_) => {}
                        None if tab.tab.kind != want => swap = Some(want),
                        None => {}
                    }
                }
            }
            Runtime::Unloaded(tab) => {
                if changed(field::URL) {
                    tab.url = str_of(fields, field::URL);
                }
            }
            Runtime::Note(n) => {
                // The box shows the mirror's new text. Not for a note being typed in: `mirror_remote` held it.
                if changed(field::TEXT) {
                    n.text = str_of(fields, field::TEXT);
                }
            }
            // A group's fields (it has no runtime), a jar's, a content node's url and a Json node's view are the
            // mirror's alone (`mirror_remote`; the level shown is relaid out below). Paths, keys and values never
            // change after creation.
            Runtime::None
            | Runtime::Jar
            | Runtime::Download(_)
            | Runtime::Image(_)
            | Runtime::Pdf(_)
            | Runtime::Json(_) => {}
        }
        // Another replica changed the view of the level being shown:
        // relaid out on the next sync.
        if changed(field::VIEW) {
            if let Some(level) = self.json_level.as_mut().filter(|l| l.node_id == id) {
                level.stale = true;
            }
        }
        if store_load {
            self.note_host_load(id);
            // The replica that navigated converts it, if it's content;
            // converting here too would add a second node.
            self.spawns.skip_convert.insert(id.to_string(), false);
        }
        if let Some(target) = swap {
            self.start_swap_to(id, target);
        }
        if let Some(target) = reassert {
            self.edit_view_kind(id, target);
        }
        true
    }

    /// Starts a pass that materializes many nodes: one content-dir scan
    /// answers all of them (`content_index`). `true` when this call set the
    /// index, so only that caller's `end_content_pass` clears it.
    fn begin_content_pass(&mut self) -> bool {
        if self.content_index.is_some() {
            return false;
        }
        self.content_index = Some(crate::ContentIndex::scan(crate::profile::content_dir().as_deref()));
        true
    }

    fn end_content_pass(&mut self, began: bool) {
        if began {
            self.content_index = None;
        }
    }

    /// Creates the node `id` of `kind` from `fields` (a file card whose file is gone gets a placeholder).
    fn insert_materialized(&mut self, id: String, kind: Kind, fields: &Fields, local: Option<&LocalNode>) {
        let pos = local.map_or_else(|| pos_of(&id, fields), |l| egui::vec2(l.pos[0], l.pos[1]));
        // Resolved once up front for the arms below and the missing-file
        // refetch check: this machine's newest `<id>.*` in the content dir,
        // else `stored` (`ContentIndex`): the pass's own index when one is
        // running (`content_index`), so a restore reads the dir once.
        let content_path = matches!(kind, Kind::Image | Kind::Pdf | Kind::Json).then(|| {
            let stored = str_of(fields, field::PATH);
            match &self.content_index {
                Some(index) => index.resolve(&id, &stored),
                None => crate::ContentIndex::scan(crate::profile::content_dir().as_deref()).resolve(&id, &stored),
            }
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
                // Title, mute and zoom come from personal state (`apply_local`); the engine is the mirror's.
                let zoom_factor = self.settings.zoom_factor;
                let (title, muted, state) = (String::new(), false, UnloadedState::Sleeping);
                let payload = UnloadedTabPayload { url, title, muted, zoom_factor, state };
                self.insert_placeholder_node(id.clone(), pos, kind, Runtime::Unloaded(payload));
            }
            Kind::Group => {
                // Its name, `from_json` and default jar are already the mirror's (`mirror_remote`); it has no runtime.
                self.insert_placeholder_node(id.clone(), pos, kind, Runtime::None);
            }
            Kind::Note => {
                let payload = NotePayload { text: str_of(fields, field::TEXT), editing: false };
                self.insert_placeholder_node(id.clone(), pos, kind, Runtime::Note(payload));
            }
            Kind::Image => {
                let path = content_path.clone().unwrap_or_default();
                if missing_with_url {
                    let payload = ImagePayload { path, width: 0, height: 0, error: None };
                    self.insert_placeholder_node(id.clone(), pos, kind, Runtime::Image(payload));
                } else {
                    self.insert_image_node_with_id(id.clone(), path, pos, None);
                }
            }
            Kind::Pdf => {
                let path = content_path.clone().unwrap_or_default();
                if missing_with_url {
                    let payload = PdfPayload { path, outcome: None };
                    self.insert_placeholder_node(id.clone(), pos, kind, Runtime::Pdf(payload));
                } else {
                    self.insert_pdf_node_with_id(id.clone(), path, pos, None);
                }
            }
            Kind::Json => {
                let path = content_path.clone().unwrap_or_default();
                let error = if missing_with_url {
                    None
                } else {
                    self.insert_json_node_with_id(id.clone(), path.clone(), pos, None).err()
                };
                if missing_with_url || error.is_some() {
                    let payload = JsonPayload { path, value: Default::default(), error };
                    self.insert_placeholder_node(id.clone(), pos, kind, Runtime::Json(payload));
                }
            }
            Kind::Download => {
                let path = str_of(fields, field::PATH);
                let bytes = std::fs::metadata(&path).map_or(0, |m| m.len());
                let payload = DownloadPayload {
                    path,
                    bytes_received: bytes,
                    total_bytes: Some(bytes),
                    completed: true,
                    failed: false,
                };
                self.insert_placeholder_node(id.clone(), pos, kind, Runtime::Download(payload));
            }
            Kind::Jar => {
                self.insert_placeholder_node(id.clone(), pos, kind, Runtime::Jar);
                // A jar first seen already ephemeral (made on another device, or restored) has no file of its own:
                // a leftover one goes, as a toggle on would delete it (`update_live_node` does that for a change).
                if let (Ok(jar), Some(FieldValue::Bool(true))) = (id.parse::<NodeId>(), fields.get(&field::EPHEMERAL)) {
                    self.after_jar_ephemeral_changed(jar, true);
                }
            }
            Kind::Link | Kind::Unknown => return,
        }
        // A content node's url, its stored path and a Json node's view are already the mirror's (`mirror_remote`).
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
                (id, live.map(|n| (n.kind(), fields_of(n), doc.effective_group(id))))
            })
            .collect()
    }

    /// Shows other replicas' changes, before the next commit (contract
    /// "Missing means deleted"). A deleted node is removed without a
    /// Recently Closed entry and without queueing a delete of its own
    /// (`remove_remotely_deleted`: the document already lacks it; it does queue the deletes of the links naming
    /// the node), and a
    /// tab's engine host is shut down. Groups are materialized before the
    /// nodes inside them, so a new member never names a group that doesn't
    /// exist yet. A new tab arrives asleep (`materialize`, spec "Lazy
    /// tabs"): another replica opening a big session mustn't start an
    /// engine here for every tab in it.
    pub(crate) fn apply_remote(&mut self, mut changes: Vec<RemoteChange>) {
        changes.sort_by_key(|(_, c)| match c {
            Some((Kind::Group, ..)) => 0,
            _ => 1,
        });
        let toasts = self.toasts.save_toast_queue.len();
        let pass = self.begin_content_pass();
        for (id, change) in changes {
            let key = id.to_string();
            match change {
                Some((kind, fields, group)) => self.materialize(key, kind, &fields, group.map(|g| g.to_string()), None),
                None if self.spawns.spawning.contains_key(&key) => {
                    self.spawns.remote_in_flight.insert(key, None);
                }
                None => self.remove_remotely_deleted(&key),
            }
        }
        self.end_content_pass(pass);
        self.collapse_toasts_since(toasts);
        // A remote jar edit follows the same table (spec "Live tabs when `jar` changes"); once the whole batch is
        // in, so a tab is judged by where the batch leaves it.
        self.sweep_jar_switches();
    }

    /// Removes a node another replica deleted, recording nothing and saving
    /// nothing (but see the links below). Its tab children move to its parent. A group's members move
    /// to the group's own group (`dissolve_group`, which also leaves the
    /// level if it was showing): a member the store still has is in
    /// `remote` too (it lists descendants) and gets its effective group
    /// from there, and one only this replica has lands where the store
    /// would show it. Either way no live node names a removed group.
    pub(crate) fn remove_remotely_deleted(&mut self, id: &str) {
        // The links naming a node another replica deleted go with it. Unlike the node, a link may still be live in the
        // document (the deleting replica did not know it, or an older build left it), and a link forgotten here
        // while live would fail `check_graph_matches_document`: so these deletes are queued (`delete_link`).
        for link in self.links_naming(id) {
            self.delete_link(&link);
        }
        if let Some(link) = id.parse::<NodeId>().ok().filter(|l| self.mirror.is_link(*l)) {
            self.mirror.forget_link(link);
            return;
        }
        let Some(node) = self.nodes.get(id) else { return };
        let up = self.mirror.group(id).map(str::to_owned);
        let is_group = self.is_container(id);
        let is_tab = node.runtime.saved_tab().is_some();
        let parent = self.mirror.parent(id).map(str::to_owned);
        let children: Vec<String> = self.children_of(id).map(str::to_owned).collect();
        for child in children {
            self.follow_remote_parent(&child, parent.clone());
        }
        if is_group {
            // A change waiting for a spawn may name this group too.
            for (_, _, group) in self.spawns.remote_in_flight.values_mut().flatten() {
                if group.as_deref() == Some(id) {
                    *group = up.clone();
                }
            }
            self.dissolve_group_quietly(id);
        } else if is_tab {
            self.drop_tab(id);
        } else {
            self.drop_node(id);
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
        self.spawns.wakes_in_flight.remove(id);
    }

    /// Applies what other replicas changed while `id`'s spawn was in flight,
    /// now that its node exists (`handle_tab_spawn_result`).
    pub(crate) fn land_remote_in_flight(&mut self, id: &str, remote: Option<Option<RemoteNode>>) {
        match remote {
            Some(Some((kind, fields, group))) => self.materialize(id.to_string(), kind, &fields, group, None),
            Some(None) => self.remove_remotely_deleted(id),
            None => {}
        }
    }

    /// Tab `id` is loading a url the host gave it (another replica's `url`,
    /// a restore, or a typed url, which `navigate_tab` shares itself): its
    /// unlabelled url changes stay local until the load settles
    /// (`shares_url_change`).
    pub(crate) fn note_host_load(&mut self, id: &str) {
        self.spawns.host_loads.insert(id.to_string(), std::time::Instant::now());
    }

    /// Starts the engines a restored session starts (spec "Lazy tabs"): the
    /// focused tab's, and the saved split tab's too when the session was
    /// saved split -- otherwise `apply_local` restores `split_tab` onto a
    /// card that never wakes, leaving the split pane empty forever.
    pub(crate) fn wake_focused(&mut self) {
        let asleep = |id: &str, this: &Self| {
            let card = this.nodes.get(id).and_then(|n| n.as_unloaded());
            card.is_some_and(|t| t.state == UnloadedState::Sleeping).then(|| this.mirror.view_kind(id))
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
    /// typed url, which `navigate_tab` shares itself, or another replica's)
    /// isn't. Otherwise, not while it follows a load the host started, nor
    /// for `HOST_LOAD_GRACE` after the tab was last seen loading. Either
    /// way a site that redirects to a fresh url on every load (a nonce, a
    /// session token) can't have two instances navigate each other back
    /// and forth forever: the url stays local.
    pub(crate) fn shares_url_change(&mut self, id: &str, user_started: Option<bool>) -> bool {
        match user_started {
            Some(true) => {
                self.spawns.host_loads.remove(id);
                return true;
            }
            Some(false) => return false,
            None => {}
        }
        let Some(since) = self.spawns.host_loads.get_mut(id) else { return true };
        if self.nodes.get(id).and_then(|n| n.as_tab()).is_some_and(|t| t.loading) {
            *since = std::time::Instant::now();
            return false;
        }
        if since.elapsed() < HOST_LOAD_GRACE {
            return false;
        }
        self.spawns.host_loads.remove(id);
        true
    }

    /// The one writer of mirrored fields (node model spec, 2b "Read model"): the mirror shows `value` at once, and
    /// the next save commits it as a `Set`. An id that isn't a store id (a hand-built test node) only updates the
    /// mirror. A value the mirror already shows queues nothing.
    pub(crate) fn edit_field(&mut self, id: &str, no: u32, value: FieldValue) {
        debug_assert!(!matches!(no, field::GROUP | field::PARENT), "a link: `edit_group` / `edit_parent`");
        if self.mirror.get(id, no) == value {
            return;
        }
        if let Ok(node) = id.parse::<NodeId>() {
            self.pending.insert((node, no), encode_field(no, &value));
        }
        self.mirror.set(id, no, value);
    }

    /// The one writer of a tab's shared url (2b-3 Ruling 4): called with the url at the moment it is shared (a typed
    /// navigation, or a report `shares_url_change` accepts). A move that is not shared (a report labelled not
    /// user-started, or a redirect during a load the host started, `note_host_load`) changes only the live url; an
    /// unlabelled redirect report after a link click is shared.
    pub(crate) fn edit_url(&mut self, id: &str, url: &str) {
        self.edit_field(id, field::URL, FieldValue::Str(url.to_owned()));
    }

    /// The one writer of a note's text (2b-4 Ruling 6): what its box holds, committed at the blur (`end_note_edit`)
    /// and by every save (`commit_note_buffers`), never per keystroke, since a note can reach 1 MB. Text the mirror
    /// already shows queues nothing and copies nothing.
    pub(crate) fn edit_text(&mut self, id: &str, text: &str) {
        if self.mirror.text(id) != text {
            self.edit_field(id, field::TEXT, FieldValue::Str(text.to_owned()));
        }
    }

    /// The one writer of a Json node's view (2b-4 Ruling 8): `view` is the stored text (`JsonView::to_field`). The
    /// mirror's decoded cache (`Mirror::view`) follows at once. Written by a level edit (`edit_json_view`) and by a
    /// reload that prunes overrides its new value can't resolve (`reload_fetched_node`).
    pub(crate) fn edit_view(&mut self, id: &str, view: &str) {
        self.edit_field(id, field::VIEW, FieldValue::Str(view.to_owned()));
    }

    /// The one writer of a group's name (2b-4 Ruling 9): the rename commit (`NodeAction::CommitRename`), with the
    /// name already cleaned. A new group's is seeded at placement instead (`place_group`). A jar's is `commit_rename`'s.
    pub(crate) fn edit_name(&mut self, id: &str, name: &str) {
        // name rule: compares stored names
        if self.mirror.name(id) != name {
            self.edit_field(id, field::NAME, FieldValue::Str(name.to_owned()));
        }
    }

    /// What a note's card shows, borrowed (Ruling 6): the box's edit buffer while it is typed in, the mirror's text
    /// otherwise. "" for a node that isn't a note.
    pub(crate) fn note_text(&self, id: &str) -> &str {
        match self.nodes.get(id).and_then(|n| n.as_note()) {
            Some(note) if note.editing => &note.text,
            _ => self.mirror.text(id),
        }
    }

    /// Whether node `id` is known by its stored name alone (a group, a jar): no derived label to fall back on, so
    /// the name is its own text.
    pub(crate) fn named_by_name(&self, id: &str) -> bool {
        matches!(self.nodes.get(id).map(|n| &n.runtime), Some(Runtime::None | Runtime::Jar))
    }

    /// The stored text a card is known by, borrowed: a note's (`note_text`), a group's or a jar's raw name
    /// (`Mirror::name`), which the caller draws through a name rule or matches as stored. "" for other kinds.
    pub(crate) fn own_text(&self, id: &str) -> &str {
        if self.named_by_name(id) {
            // name rule: raw, for `accessible_name` (display_name) and the filter (matches the stored text)
            return self.mirror.name(id);
        }
        self.note_text(id)
    }

    /// Whether `id` is a note being typed in: its box holds the text, and another replica's is held (Ruling 7).
    fn is_typed_in(&self, id: &str) -> bool {
        self.nodes.get(id).and_then(|n| n.as_note()).is_some_and(|n| n.editing)
    }

    /// Commits every note's box that differs from the mirror (`edit_text`), so a save commits what is being typed.
    /// A note nobody is typing in has its box equal to the mirror, so it compares and copies nothing.
    fn commit_note_buffers(&mut self) {
        let typed: Vec<(String, String)> = self
            .nodes
            .iter()
            .filter_map(|(id, n)| Some((id, n.as_note()?)))
            .filter(|(id, note)| note.text != self.mirror.text(id))
            .map(|(id, note)| (id.clone(), note.text.clone()))
            .collect();
        for (id, text) in typed {
            self.edit_text(&id, &text);
        }
    }

    /// Ends the edit of note `id` (its box lost focus, or stopped being laid out: `end_note_edits`). What the user
    /// typed since the last commit wins, as a pending local edit does, and another replica's text held meanwhile
    /// (`held_text`) is dropped; with nothing typed, the held text applies to the mirror and the box (Ruling 7). The
    /// caller saves.
    pub(crate) fn end_note_edit(&mut self, id: &str) {
        let held = self.writes.held_text.remove(id);
        let Some(note) = self.nodes.get_mut(id).and_then(|n| n.as_note_mut()) else { return };
        note.editing = false;
        let buffer = std::mem::take(&mut note.text);
        let pending = id.parse::<NodeId>().is_ok_and(|node| self.pending.contains_key(&(node, field::TEXT)));
        let text = match held {
            Some(theirs) if !pending && buffer == self.mirror.text(id) => {
                self.mirror.set(id, field::TEXT, FieldValue::Str(theirs.clone()));
                theirs
            }
            _ => {
                self.edit_text(id, &buffer);
                buffer
            }
        };
        if let Some(note) = self.nodes.get_mut(id).and_then(|n| n.as_note_mut()) {
            note.text = text;
        }
    }

    /// A note's box lost focus: its edit ends (`end_note_edit`) and is saved.
    pub(crate) fn blur_note(&mut self, id: &str) {
        self.end_note_edit(id);
        self.save_session();
    }

    /// The one writer of a node's document pos (2b-4 Ruling 2): a drag release, with where the card was dropped
    /// (`release_drag`). A new node's is seeded at placement instead (`place_node`, `seed_landing`). Physics never
    /// calls it: a settle moves only `App::layout`. `None` (and a non-finite pos) writes the absent value, Pos (0,0).
    pub(crate) fn edit_pos(&mut self, id: &str, pos: Option<egui::Vec2>) {
        self.edit_field(id, field::POS, pos.map_or(FieldValue::Pos(0.0, 0.0), pos_value));
    }

    /// The one writer of a tab's jar choice (2b-3 Ruling 7): `JAR` and `JAR_SET` go through `edit_field`, so only the
    /// one that changed is queued, and re-picking the same jar queues nothing. A pending `JAR` naming a jar that is
    /// gone by the save is written as none (`save_session`, Ruling 8). The caller sweeps (`sweep_jar_switches`).
    pub(crate) fn edit_jar(&mut self, id: &str, choice: crate::jar::JarChoice) {
        self.edit_field(id, field::JAR, FieldValue::Id(choice.jar));
        self.edit_field(id, field::JAR_SET, FieldValue::Bool(choice.set));
    }

    /// The one writer of a group's default jar (2b-5 Task 1): `set_group_default_jar` and `clear_group_default_jar`.
    /// Resolved at commit like a tab's `JAR` (2b-5 Ruling 6): a jar deleted by the save is written
    /// as none, and one another replica deleted earlier stays stored, dangling, until a new edit.
    pub(crate) fn edit_default_jar(&mut self, group: &str, jar: Option<NodeId>) {
        self.edit_field(group, field::DEFAULT_JAR, FieldValue::Id(jar));
    }

    /// The one writer of a tab's engine (2b-3 Ruling 5): called when a switch starts (`start_swap_to`, a wake into
    /// another engine), and with the old engine when a switch fails and falls back. Compared as decoded, so asking for
    /// the engine the mirror already shows queues nothing and keeps a code this build doesn't know (Ruling 6).
    pub(crate) fn edit_view_kind(&mut self, id: &str, kind: ViewKind) {
        if self.mirror.view_kind(id) != kind {
            self.edit_field(id, field::VIEW_KIND, FieldValue::ViewCode(view_code(kind)));
        }
    }

    /// The one writer of a node's parent link: the mirror shows it at once and the next save commits a `Set PARENT`.
    /// A non-store id or target only updates the mirror.
    pub(crate) fn edit_parent(&mut self, id: &str, parent: Option<String>) {
        if self.mirror.parent(id) == parent.as_deref() {
            return;
        }
        if let Ok(node) = id.parse::<NodeId>() {
            match parent_id(&parent) {
                Some(target) => {
                    let enc = encode_field(field::PARENT, &FieldValue::Id(target));
                    self.pending.insert((node, field::PARENT), enc);
                }
                // A target that isn't a store id is mirror-only: an earlier queued target must not commit.
                None => {
                    self.pending.remove(&(node, field::PARENT));
                }
            }
        }
        self.mirror.set_parent(id, parent);
    }

    /// The one writer of a node's group (Ruling 2): shown at once, committed as a `Move` by the next save.
    /// A non-store id or target only updates the mirror.
    pub(crate) fn edit_group(&mut self, id: &str, group: Option<String>) {
        if self.mirror.group(id) == group.as_deref() {
            return;
        }
        if let Ok(node) = id.parse::<NodeId>() {
            // A target that isn't a store id is mirror-only: an earlier queued move must not commit.
            match parent_id(&group) {
                Some(target) => self.writes.pending_moves.insert(node, target),
                None => self.writes.pending_moves.remove(&node),
            };
        }
        self.mirror.set_group(id, group);
    }

    /// Whether node `id` is shown at the level being viewed.
    pub(crate) fn in_level(&self, id: &str) -> bool {
        self.mirror.group(id) == self.current_group.as_deref()
    }

    /// Every node whose mirror group is `group` (`linked_ids`).
    pub(crate) fn members_of<'a>(&'a self, group: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.linked_ids().filter(move |id| self.mirror.group(id) == Some(group))
    }

    /// The ids a cascade (a dissolve, merge or removal; a jar re-snapshot, default or delete) must reach: every live
    /// node, and every tab mid-respawn. A respawn takes the tab out of `nodes` until it lands but keeps its mirror
    /// entry, which the landing keeps (`Mirror::seed`), so a cascade that skipped it would leave it naming a removed
    /// node or an old jar. A new spawn has no mirror entry yet and is left out: it lands from its `TabSpawn`
    /// (`seed_landing`), and a cascade writing to it would make a partial entry that stops that seed.
    pub(crate) fn linked_ids(&self) -> impl Iterator<Item = &str> {
        let respawning = self.spawns.spawning.keys().filter(|id| !self.nodes.contains_key(*id) && self.mirror.has(id));
        self.nodes.keys().chain(respawning).map(String::as_str)
    }

    /// Whether `id` is one of `linked_ids`.
    pub(crate) fn is_linked(&self, id: &str) -> bool {
        self.nodes.contains_key(id) || (self.spawns.spawning.contains_key(id) && self.mirror.has(id))
    }

    /// "Remove from Group" (the tab card's eject button, dragging a card out through the way back, and the tab and
    /// item menus): moves `id` up one level, into the group of the group it's in, since groups nest. `None` (the
    /// top level) when that group isn't itself in one.
    pub(crate) fn remove_from_group(&mut self, id: &str) {
        if !self.nodes.contains_key(id) {
            return;
        }
        let group = self.mirror.group(id).map(str::to_owned);
        let up = group.as_deref().and_then(|g| self.mirror.group(g)).map(str::to_owned);
        self.edit_group(id, up);
    }

    /// A remote removal's cascade: the document already shows `id`'s new parent (a node only this replica has
    /// carries it in its own `Create`), so only the mirror changes; queuing it would revert a concurrent remote
    /// re-parent. A local edit still pending for the link is rewritten instead, so it keeps winning.
    pub(crate) fn follow_remote_parent(&mut self, id: &str, parent: Option<String>) {
        if id.parse::<NodeId>().is_ok_and(|node| self.pending.contains_key(&(node, field::PARENT))) {
            self.edit_parent(id, parent);
        } else {
            self.mirror.set_parent(id, parent);
        }
    }

    /// `follow_remote_parent` for the group: a remote group delete's members move up in the mirror only, so a
    /// concurrent remote merge or move of them stands.
    pub(crate) fn follow_remote_group(&mut self, id: &str, group: Option<String>) {
        if id.parse::<NodeId>().is_ok_and(|node| self.writes.pending_moves.contains_key(&node)) {
            self.edit_group(id, group);
        } else {
            self.mirror.set_group(id, group);
        }
    }

    /// Every node whose mirror parent is `id` (`linked_ids`).
    pub(crate) fn children_of<'a>(&'a self, id: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.linked_ids().filter(move |child| self.mirror.parent(child) == Some(id))
    }

    /// Shows another replica's values of `fields` (the node's type's fields, Ruling 11) and its effective `group`,
    /// except where this replica has an edit waiting for a save: it commits next and wins (last writer, per field or
    /// link). Returns `(held, changed)`: the fields held back that way, whose live effects `update_live_node` then
    /// skips, and the fields whose value differs from what the mirror showed (Ruling 9), whose live effects it
    /// applies. A note's text is held too while the note is typed in, and remembered in `held_text` for the blur
    /// (2b-4 Ruling 7). The links (`parent`, `group`) are set but reported in neither list.
    fn mirror_remote(&mut self, id: &str, fields: &Fields, group: Option<&str>) -> (Vec<u32>, Vec<u32>) {
        let node = id.parse::<NodeId>().ok();
        let (mut held, mut changed) = (Vec::new(), Vec::new());
        for (&no, value) in fields {
            if no == field::PARENT {
                continue;
            }
            if node.is_some_and(|node| self.pending.contains_key(&(node, no))) {
                held.push(no);
                continue;
            }
            if no == field::TEXT && self.is_typed_in(id) {
                held.push(no);
                match value {
                    FieldValue::Str(theirs) if theirs != self.mirror.text(id) => {
                        self.writes.held_text.insert(id.to_owned(), theirs.clone());
                    }
                    // Back to what the mirror shows: nothing left to apply.
                    _ => {
                        self.writes.held_text.remove(id);
                    }
                }
                continue;
            }
            if self.mirror.get(id, no) != *value {
                changed.push(no);
                self.mirror.set(id, no, value.clone());
            }
        }
        if !node.is_some_and(|node| self.pending.contains_key(&(node, field::PARENT))) {
            self.mirror.set_parent(id, id_of(fields, field::PARENT));
        }
        if !node.is_some_and(|node| self.writes.pending_moves.contains_key(&node)) {
            self.mirror.set_group(id, group.map(str::to_owned));
        }
        (held, changed)
    }

    /// Saves browsing history to disk if `history_dirty` is set, then clears
    /// the flag. Called from `save_session` and `close_store` on every return
    /// path. History doesn't depend on the store, so it's saved regardless of
    /// store success or failure.
    fn flush_history(&mut self) {
        if self.history_ui.history_dirty {
            crate::history::save(&self.history);
            self.history_ui.history_dirty = false;
        }
        let nodes = &self.nodes;
        crate::nav_history::save(&mut self.nav_history, |id| nodes.contains_key(id));
    }

    /// Saves local edits (spec "Local edits become ops"): take the lock,
    /// show what other replicas wrote, and commit the queued edits as one
    /// transaction, in the order Creates (`pending_creates`, groups first),
    /// Moves (`pending_moves`), Sets (`pending`), Deletes (`pending_deletes`).
    /// On error the queues stay for the next save (contract "Transactions").
    /// `true` when the store now holds every local edit: `false` read-only,
    /// on error, and when another instance kept writing. The last two are
    /// tried again by `poll_store` after `SAVE_RETRY`.
    pub(crate) fn save_session(&mut self) -> bool {
        // Each round applies what others wrote since the last; four rounds
        // of fresh remote frames means another instance is busy: try again
        // later (`save_retry_at`) rather than spin.
        // What is being typed is committed too (2b-4 Ruling 6).
        self.commit_note_buffers();
        for _ in 0..4 {
            // Projected before `begin`, since the `Txn` borrows the journal: what each queued node shows (one that
            // shows nothing yet, an in-progress download, waits).
            let candidates: BTreeMap<NodeId, Snapshot> =
                self.writes.pending_creates.iter().filter_map(|&id| Some((id, self.snapshot(id)?))).collect();
            let Some(journal) = self.journal.as_mut() else {
                self.flush_history();
                return false;
            };
            let txn = match journal.begin() {
                Ok(txn) => txn,
                Err(err) => {
                    self.store_error(err);
                    self.writes.save_retry_at = Some(std::time::Instant::now() + SAVE_RETRY);
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
            let doc = txn.doc();
            // A queued node is created only if the document has no entry for it at all: a tombstone is never
            // recreated (ids are never reused), and one another replica created meanwhile stands.
            let new: BTreeMap<NodeId, Snapshot> =
                candidates.into_iter().filter(|(id, _)| !doc.nodes.contains_key(id)).collect();
            // A queued delete applies to a node the document still shows live (Ruling 4); one it never had, or
            // another replica deleted meanwhile, is just forgotten. Nothing queued is shown here: `materialize`
            // drops a remote edit to it, and showing it again locally withdraws the delete (`queue_create`).
            let deleted: BTreeSet<NodeId> = self
                .writes
                .pending_deletes
                .iter()
                .copied()
                .filter(|id| doc.nodes.get(id).is_some_and(Node::is_live))
                .collect();
            let mut ops = create_ops(doc, &new, &deleted);
            let created: BTreeSet<NodeId> = new.keys().copied().collect();
            let resolve = |target: Option<NodeId>| resolve_id(doc, &created, &deleted, target);
            // Edits to mirrored fields and moves. A node this commit creates carries its values in the `Create`,
            // and one it deletes needs none; one the document doesn't show live would get a placeholder from a
            // `Set`. Those edits are dropped.
            let editable = |node: &NodeId| {
                !created.contains(node) && !deleted.contains(node) && doc.nodes.get(node).is_some_and(Node::is_live)
            };
            // Each pending link's target as this commit leaves it: what the mirror shows once it lands. A moved
            // node's group is read back from the document after the commit instead: the store may refuse the move.
            let mut moved: BTreeSet<NodeId> = BTreeSet::new();
            let mut links: BTreeMap<(NodeId, u32), Option<NodeId>> = BTreeMap::new();
            // Moves before sets (Ruling 4). A move to where the document already shows the node is left out.
            for (&node, &group) in &self.writes.pending_moves {
                if !editable(&node) {
                    continue;
                }
                let group = resolve(group);
                moved.insert(node);
                if group != doc.effective_group(node) {
                    ops.push(OpBody::Move { node, group });
                }
            }
            for (&(node, no), enc) in &self.pending {
                if !editable(&node) {
                    continue;
                }
                // Every link (an `Id` field: `parent`, a tab's `jar`) is resolved alike (2b-3 Ruling 8).
                let def = field_def(no);
                let enc = if def.is_some_and(|def| def.ty == FieldType::Id) {
                    let target = match def.and_then(|def| decode_field(def, enc).ok()) {
                        Some(FieldValue::Id(target)) => resolve(target),
                        _ => None,
                    };
                    links.insert((node, no), target);
                    encode_field(no, &FieldValue::Id(target))
                } else {
                    enc.clone()
                };
                ops.push(OpBody::Set { node, field: no, enc });
            }
            ops.extend(deleted.iter().map(|&node| OpBody::Delete { node }));
            let result = if ops.is_empty() {
                drop(txn);
                Ok(())
            } else {
                txn.commit(ops)
            };
            self.take_store_recovery();
            match result {
                Ok(()) => {
                    // The mirror already shows what this commit wrote (it is what the ops were built from); only
                    // the links the commit resolved can differ from it.
                    if let Some(journal) = &self.journal {
                        let doc = journal.doc();
                        // A node this commit created shows the links its `Create` resolved (`resolve_id`: a
                        // tab's jar, a group's default jar, any Id field), as an edited one does. Only one
                        // with an entry: a new spawn's lands from its `TabSpawn` (`linked_ids`). `parent` lives
                        // in the mirror's links and `group` is the moves' (below).
                        for id in &created {
                            let Some(node) = doc.nodes.get(id) else { continue };
                            let key = id.to_string();
                            if !self.mirror.has(&key) {
                                continue;
                            }
                            for &no in node.kind().fields() {
                                let is_id = field_def(no).is_some_and(|def| def.ty == FieldType::Id);
                                if !is_id || no == field::PARENT {
                                    continue;
                                }
                                let stored = node.get(no);
                                if self.mirror.get(&key, no) != stored {
                                    self.mirror.set(&key, no, stored);
                                }
                            }
                        }
                        // A moved node shows the group the document holds: the target, none for one this commit
                        // deleted (Ruling 5), or where it was when the store refused the move as a cycle (a
                        // concurrent move raced it), which the mirror would otherwise keep showing.
                        for &node in &moved {
                            let (id, group) = (node.to_string(), doc.effective_group(node));
                            if link_id(self.mirror.group(&id)) != group {
                                self.mirror.set_group(&id, group.map(|g| g.to_string()));
                            }
                        }
                        // Created ids are done; one still queued shows nothing yet (an in-progress download) and
                        // waits for the save after it does.
                        self.writes.pending_creates.retain(|id| !doc.nodes.contains_key(id));
                    }
                    self.writes.pending_deletes.clear();
                    // A link target this commit wrote as none (Ruling 5: deleted here) is none in the mirror too.
                    for (&(node, no), &target) in &links {
                        let id = node.to_string();
                        if no == field::PARENT {
                            if link_id(self.mirror.parent(&id)) != target {
                                self.mirror.set_parent(&id, target.map(|p| p.to_string()));
                            }
                        } else {
                            self.mirror.set(&id, no, FieldValue::Id(target));
                        }
                    }
                    // A remote value parked for a tab still spawning (`remote_in_flight`) predates what this
                    // commit wrote; its landing must not replay it over the local edit.
                    for &(node, no) in self.pending.keys() {
                        let id = node.to_string();
                        if let Some(Some((_, fields, _))) = self.spawns.remote_in_flight.get_mut(&id) {
                            let value = match no {
                                field::PARENT => FieldValue::Id(link_id(self.mirror.parent(&id))),
                                _ => self.mirror.get(&id, no),
                            };
                            fields.insert(no, value);
                        }
                    }
                    for node in self.writes.pending_moves.keys() {
                        let id = node.to_string();
                        if let Some(Some((_, _, group))) = self.spawns.remote_in_flight.get_mut(&id) {
                            *group = self.mirror.group(&id).map(str::to_owned);
                        }
                    }
                    // Text committed here is newer than another replica's held before it: the held text is dropped.
                    for &(node, no) in self.pending.keys() {
                        if no == field::TEXT {
                            self.writes.held_text.remove(&node.to_string());
                        }
                    }
                    self.pending.clear();
                    self.writes.pending_moves.clear();
                    // A tab still spawning keeps its entry, and its layout: a respawn has removed it from `nodes`
                    // until it lands, and `Mirror::init` then leaves an existing entry alone.
                    let (nodes, spawning) = (&self.nodes, &self.spawns.spawning);
                    self.mirror.retain(|id| nodes.contains_key(id) || spawning.contains_key(id));
                    self.layout.retain(|id, _| nodes.contains_key(id) || spawning.contains_key(id));
                    self.writes
                        .held_text
                        .retain(|id, _| nodes.get(id).and_then(|n| n.as_note()).is_some_and(|n| n.editing));
                    self.writes.store_notices.clear();
                    self.writes.last_session_save = Some(std::time::Instant::now());
                    self.writes.save_retry_at = None;
                    self.flush_history();
                    #[cfg(debug_assertions)]
                    self.check_graph_matches_document();
                    return true;
                }
                Err(err) => {
                    self.store_error(err);
                    self.writes.save_retry_at = Some(std::time::Instant::now() + SAVE_RETRY);
                    self.flush_history();
                    return false;
                }
            }
        }
        self.writes.save_retry_at = Some(std::time::Instant::now() + SAVE_RETRY);
        self.flush_history();
        false
    }

    /// Ruling 14, test and debug builds: after a successful save the live graph and the document agree. Every store
    /// id in `nodes` (an in-progress download aside) and every jar is live in the document; a tab whose spawn is
    /// in flight has an entry (live, or a tombstone another replica left while it was in flight, which the landing
    /// applies); and every live document node is one of them. A failure names the funnel that was missed: a node
    /// that entered the graph without `place_node`, `spawn_tab_async` or `add_jar` (a missed create), or left it
    /// without `queue_delete` (a missed delete). Each node's stored type is one its runtime runs, so a create that
    /// passed a mismatched kind (an image type on a pdf runtime) is caught before a painter dispatches on it.
    #[cfg(debug_assertions)]
    fn check_graph_matches_document(&self) {
        let Some(journal) = &self.journal else { return };
        let doc = journal.doc();
        let live = |id: &NodeId| doc.nodes.get(id).is_some_and(Node::is_live);
        for (key, node) in &self.nodes {
            let Ok(id) = key.parse::<NodeId>() else { continue };
            if node.as_download().is_some_and(|dl| !dl.completed) || self.spawns.spawning.contains_key(key) {
                continue;
            }
            assert!(live(&id), "node {key} is shown but not live in the document: a create funnel was missed");
            let ty = self.card_kind(key).map(|kind| kind.ty);
            assert!(
                ty.is_some_and(|ty| runtime_has_type(&node.runtime, ty)),
                "node {key} has type {ty:?} that its runtime does not run: a create funnel passed a mismatched kind"
            );
        }
        for id in self.mirror.link_ids() {
            assert!(live(&id), "link {id} is shown but not live in the document: a create funnel was missed");
        }
        for key in self.spawns.spawning.keys() {
            let Ok(id) = key.parse::<NodeId>() else { continue };
            assert!(
                doc.nodes.contains_key(&id),
                "tab {key} is spawning but the document has no entry: a create funnel was missed"
            );
        }
        for (id, _) in doc.live() {
            let key = id.to_string();
            let shown =
                self.nodes.contains_key(&key) || self.spawns.spawning.contains_key(&key) || self.mirror.is_link(id);
            assert!(shown, "node {key} is live in the document but not shown: a delete funnel was missed");
        }
    }

    /// Startup: shows the document, then applies personal state
    /// (`session.mnf.local`) on top. `false` when it has no live node, so
    /// the caller starts fresh; Recently Closed is still restored then,
    /// or the next personal-state write would drop it.
    pub(crate) fn restore_session(&mut self) -> bool {
        let Some(journal) = self.journal.as_ref() else { return false };
        let doc = journal.doc();
        let mut nodes: Vec<(NodeId, Kind, Fields, Option<NodeId>)> =
            doc.live().map(|(id, n)| (id, n.kind(), fields_of(n), doc.effective_group(id))).collect();
        self.take_store_recovery();
        // Contract: `None` (no file, another document's, or a newer format
        // version) or a damaged file is a fresh start, same as a session
        // with no personal state at all, and the next write replaces it. A
        // read that failed (the lock stayed busy, an I/O error) is not: the
        // file may hold this profile's state, so it isn't written until
        // `poll_store` has read it.
        let read = self.journal.as_ref().map(|j| j.read_local());
        let unread = matches!(read, Some(Err(ref err)) if !matches!(err, JournalError::Corrupt(_)));
        if let Some(Err(err)) = read.as_ref().filter(|_| unread) {
            self.log_local_unread(err);
        }
        let local = local_from(read);
        let toasts = self.toasts.save_toast_queue.len();
        // Links ride along: stored, never graph nodes.
        for (id, kind, fields, _) in nodes.iter().filter(|(_, kind, ..)| *kind == Kind::Link) {
            self.materialize(id.to_string(), *kind, fields, None, None);
        }
        nodes.retain(|(_, kind, ..)| *kind != Kind::Link);
        // Only jars (with Task 3, the Default at least): a fresh start, which keeps them where they are.
        if nodes.iter().all(|(_, kind, ..)| *kind == Kind::Jar) {
            for (id, kind, fields, group) in &nodes {
                self.materialize(id.to_string(), *kind, fields, group.map(|g| g.to_string()), None);
            }
            self.restore_closed(local.closed);
            self.arm_local(unread, true);
            return false;
        }
        nodes.sort_by_key(|(_, kind, ..)| *kind != Kind::Group);
        let pass = self.begin_content_pass();
        for (id, kind, fields, group) in &nodes {
            let group = group.map(|g| g.to_string());
            // The store's own `pos` still reaches the mirror inside
            // `materialize`; only the live placement is overridden.
            let entry = local.nodes.get(&id.to_string());
            self.materialize(id.to_string(), *kind, fields, group, entry);
        }
        self.end_content_pass(pass);
        self.collapse_toasts_since(toasts);
        // Graph view with the first tab focused, re-fit once restored tabs
        // land; `apply_local` below overrides both from personal state when
        // it has them.
        self.focused = nodes.iter().find(|(_, kind, ..)| *kind == Kind::Tab).map(|(id, ..)| id.to_string());
        self.view_mode = ViewMode::Graph;
        self.camera.frame_restored = true;
        self.apply_local(local);
        self.wake_focused();
        self.arm_local(unread, false);
        true
    }

    /// The end of `restore_session`: personal state as read is what the
    /// file holds, so `maybe_write_local` skips an unchanged write; one
    /// that couldn't be read is retried instead, and never written over.
    fn arm_local(&mut self, unread: bool, fresh: bool) {
        if unread {
            self.local_persist.local_written = None;
            self.local_persist.local_unread =
                Some(LocalUnread { retry_at: std::time::Instant::now() + SAVE_RETRY, fresh });
        } else {
            self.local_persist.local_written = Some(self.local_state());
            self.local_persist.local_unread = None;
        }
    }

    /// Logs why startup couldn't read personal state, once (it's marked
    /// unread only there), unless another instance just held the lock:
    /// that is routine and the retry sees to it. Anything else (permission
    /// denied, say) may never clear, and personal state is then never
    /// written for this session. Names only the file, not its folder.
    fn log_local_unread(&self, err: &JournalError) {
        if matches!(err, JournalError::Io(io) if io.kind() == std::io::ErrorKind::WouldBlock) {
            return;
        }
        let file = self
            .journal
            .as_ref()
            .map(|j| crate::store::local::path_for(j.path()))
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
        eprintln!(
            "session: couldn't read personal state ({}): {err}; it stays unwritten until a retry reads it",
            file.as_deref().unwrap_or("?")
        );
    }

    /// Retries reading the personal state startup couldn't
    /// (`App::local_persist`.`local_unread`), at most every `SAVE_RETRY` and without
    /// waiting on the lock, so a busy peer never stalls a frame; `force`
    /// (at exit) tries now and waits as a save does. Once read, it's
    /// applied as startup would have, keeping what was closed since in
    /// Recently Closed, and the next due write saves the result. Only a
    /// state that decodes is applied: none, or a damaged one, leaves the
    /// live view as the user has had it since startup. At exit nothing is
    /// woken (the read only feeds the write that follows).
    fn retry_read_local(&mut self, force: bool) {
        let Some(unread) = self.local_persist.local_unread else { return };
        let Some(journal) = self.journal.as_ref() else { return };
        let read = if force {
            journal.read_local()
        } else if std::time::Instant::now() >= unread.retry_at {
            journal.read_local_within(std::time::Duration::ZERO)
        } else {
            return;
        };
        if matches!(read, Err(ref err) if !matches!(err, JournalError::Corrupt(_))) {
            self.local_persist.local_unread =
                Some(LocalUnread { retry_at: std::time::Instant::now() + SAVE_RETRY, ..unread });
            return;
        }
        self.local_persist.local_unread = None;
        // Unknown: the file holds what was read, not this merge (or nothing).
        self.local_persist.local_written = None;
        let Some(mut local) = read.ok().flatten().and_then(|bytes| LocalState::from_bytes(&bytes)) else { return };
        local.closed.extend(self.closed.iter().map(|e| e.doc.clone()));
        if unread.fresh {
            self.restore_closed(local.closed);
        } else {
            self.apply_local(local);
            if !force {
                self.wake_focused();
            }
        }
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
                let pos = self.layout_pos(id);
                let mut local = LocalNode { pos: [pos.x, pos.y], ..LocalNode::default() };
                // Against the shared url the mirror holds (Ruling 9): a url this replica shares is edited
                // (`edit_url`) into it, so only an unshared move (a redirect, a script) is recorded.
                let live_url = |url: &str| {
                    let from = self.mirror.url(id);
                    (from != url).then(|| LiveUrl { url: url.to_string(), from: from.to_owned() })
                };
                if let Some(tab) = tab_common(&node.runtime) {
                    local.title = tab.title.to_string();
                    local.muted = tab.muted;
                    local.zoom = zoom(tab.zoom_factor);
                    local.live_url = live_url(tab.url);
                } else if let Runtime::Note(n) = &node.runtime {
                    local.editing = n.editing;
                }
                (id.clone(), local)
            })
            .collect();
        if let Some(pending) = &self.local_persist.pending_local {
            for (id, node) in &pending.nodes {
                if self.spawns.spawning.contains_key(id) && !nodes.contains_key(id) {
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
            json_accessor_paths: self.json_ui.json_accessor_paths,
            json_outline_hidden: !self.json_ui.json_outline_visible,
            json_outline_width: Some(self.json_ui.json_outline_width),
        }
    }

    /// Applies `state` (personal state, from `session.mnf.local` or an
    /// imported/opened file) on top of the just-restored/imported live
    /// graph. A view field naming a node takes hold only when that node
    /// exists, live or still spawning; otherwise the caller's own default
    /// stands. A node still spawning keeps its entry in `self.local_persist.pending_local`
    /// for `handle_tab_spawn_result` to apply once it lands.
    pub(crate) fn apply_local(&mut self, state: LocalState) {
        let exists = |id: &Option<String>| {
            id.as_deref().is_some_and(|id| self.nodes.contains_key(id) || self.spawns.spawning.contains_key(id))
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
        self.json_ui.json_accessor_paths = state.json_accessor_paths;
        self.json_ui.json_outline_visible = !state.json_outline_hidden;
        if let Some(width) = state.json_outline_width {
            self.json_ui.json_outline_width = width;
        }
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
                    self.spawns.pending_respawn_focus = Some(id.clone());
                }
            }
        }

        let mut pending = BTreeMap::new();
        for (id, node) in state.nodes {
            if let Some(live) = self.nodes.get_mut(&id) {
                Self::apply_local_node(live, &node);
                self.snap_layout(&id, egui::vec2(node.pos[0], node.pos[1]));
            } else if self.spawns.spawning.contains_key(&id) {
                pending.insert(id, node);
            }
        }
        if !pending.is_empty() {
            self.local_persist.pending_local = Some(LocalState { nodes: pending, ..LocalState::default() });
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

    /// One node's share of `apply_local` beyond its settled position (the caller's `snap_layout`): whatever its
    /// payload kind keeps (a tab's title/muted/zoom, a note's editing).
    fn apply_local_node(live: &mut crate::GraphNode, node: &LocalNode) {
        match &mut live.runtime {
            Runtime::Tab(tab) => {
                tab.title = node.title.clone();
                tab.muted = node.muted;
                if let Some(zoom) = node.zoom {
                    tab.zoom_factor = zoom;
                }
            }
            Runtime::Unloaded(tab) => {
                tab.title = node.title.clone();
                tab.muted = node.muted;
                if let Some(zoom) = node.zoom {
                    tab.zoom_factor = zoom;
                }
            }
            Runtime::Note(n) => n.editing = node.editing,
            _ => {}
        }
    }

    /// Applies a spawning node's personal-state entry once its node exists
    /// (`handle_tab_spawn_result`), removing it from `pending_local`. An
    /// unloaded card now takes title/muted/zoom from personal state just
    /// like a live tab; every other kind only takes its position.
    pub(crate) fn apply_pending_local(&mut self, id: &str) {
        let Some(pending) = &mut self.local_persist.pending_local else { return };
        let Some(node) = pending.nodes.remove(id) else { return };
        if pending.nodes.is_empty() {
            self.local_persist.pending_local = None;
        }
        if let Some(live) = self.nodes.get_mut(id) {
            if let Runtime::Tab(_) | Runtime::Unloaded(_) | Runtime::Note(_) = &live.runtime {
                Self::apply_local_node(live, &node)
            }
            self.snap_layout(id, egui::vec2(node.pos[0], node.pos[1]));
        }
    }

    /// Writes personal state when it changed (contract "Personal state"):
    /// at most every 30 s, 2.5 s after a Recently Closed change, and at
    /// exit (`force`). Never while the file is unread (`App::local_persist`.`local_unread`).
    pub(crate) fn maybe_write_local(&mut self, force: bool) {
        const EVERY: std::time::Duration = std::time::Duration::from_secs(30);
        const AFTER_CLOSED: std::time::Duration = std::time::Duration::from_millis(2500);
        let due = force
            || self.local_persist.closed_changed_at.is_some_and(|t| t.elapsed() >= AFTER_CLOSED)
            || self.local_persist.local_written_at.elapsed() >= EVERY;
        if !due || self.journal.is_none() || self.local_persist.local_unread.is_some() {
            return;
        }
        let state = self.local_state();
        self.local_persist.local_written_at = std::time::Instant::now();
        if self.local_persist.local_written.as_ref().is_some_and(|w| !crate::local_state::differs(w, &state)) {
            self.local_persist.closed_changed_at = None;
            return;
        }
        match self.journal.as_ref().unwrap().write_local(&state.to_bytes()) {
            Ok(()) => {
                self.local_persist.local_written = Some(state);
                self.local_persist.closed_changed_at = None;
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
        let retry_due = self.writes.save_retry_at.is_some_and(|at| std::time::Instant::now() >= at);
        if changed || retry_due {
            self.save_session();
        }
        self.retry_read_local(false);
        self.maybe_write_local(false);
    }

    /// Clean exit: save, write personal state, then close (compacts only if
    /// something changed).
    pub(crate) fn close_store(&mut self) {
        self.save_session();
        self.retry_read_local(true);
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
    /// nodes are exported. `None` without a store or for an
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
        self.leave_json_level();
        let next_id = || self.alloc_node_id().parse().expect("a store id");
        let imported = match crate::store::json::import(json, next_id, crate::store::json::Trust::Foreign) {
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
        let pos = crate::graph_layout::GRAPH_WORLD_CENTER + crate::graph_layout::spawn_jitter(&root.to_string());
        let root = ImportRoot { group: self.insertion_group(), pos: Some(pos) };
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
    /// - A jar in the file is always a new jar under its fresh id, never
    ///   matched to an existing one by name (jar name labels, retiring
    ///   per-tab jars deviation 4): `import` already rewrote the file's
    ///   tabs' `jar` and groups' `default_jar` to that id (and, for a
    ///   `Trust::Foreign` file, dropped any naming a jar outside it).
    ///   Same-named jars
    ///   are told apart by their labels (`jar_defs::named_jar_label`).
    ///
    /// - A Link is created only when its `from` and `to` each name a card in
    ///   the same batch or a live one; otherwise it is skipped, so no junk
    ///   link reaches the store (`import` already dropped ends outside a
    ///   foreign file, or naming a jar, a link or the link itself).
    ///
    /// Tabs arrive asleep (`materialize`). Toasts from missing files are
    /// collapsed into one.
    pub(crate) fn materialize_imported(
        &mut self,
        imported: Vec<Imported>,
        root: Option<ImportRoot>,
        keep_live_parents: bool,
    ) {
        let mut imported = imported;
        let groups: BTreeSet<NodeId> = imported.iter().filter(|i| i.kind == Kind::Group).map(|i| i.id).collect();
        let ids: BTreeSet<NodeId> = imported.iter().map(|i| i.id).collect();
        let root_id = root.as_ref().and(imported.first()).map(|i| i.id);
        let (dest, root_pos) = root.map_or((None, None), |r| (r.group, r.pos));
        imported.sort_by_key(|i| match i.kind {
            Kind::Group => 1,
            Kind::Link => 3, // after the cards it joins
            _ => 2,
        });
        let toasts = self.toasts.save_toast_queue.len();
        let pass = self.begin_content_pass();
        for node in imported {
            // A link whose end did not resolve, or names no card in this batch or live, has nothing to join.
            if crate::store::json::is_half_made_link(&node, |n| {
                ids.contains(&n) || self.nodes.contains_key(&n.to_string())
            }) {
                continue;
            }
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
        self.end_content_pass(pass);
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
                crate::store::atomic::replace(path, text.as_bytes()).map_err(|e| e.to_string())
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
    /// session. `trust` is `Own` for a named session this profile saved,
    /// `Foreign` for an imported file (see `store::json::Trust`).
    pub(crate) fn replace_running_session(
        &mut self,
        json: &JsonDoc,
        trust: crate::store::json::Trust,
    ) -> HashMap<String, String> {
        let next_id = || self.alloc_node_id().parse().expect("a store id");
        let imported = match crate::store::json::import(json, next_id, trust) {
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
        // Personal state startup couldn't read is the old session's: a late
        // read may still bring back its Recently Closed, but not its view
        // state, which names nodes that are gone.
        if let Some(unread) = self.local_persist.local_unread.as_mut() {
            unread.fresh = true;
        }
        self.wake_focused();
        self.save_session();
        map
    }

    /// Clears the running session's nodes and the state naming them, and
    /// queues a delete for every card the document holds (a jar included: it is a card;
    /// links go below). Each
    /// tab's `ViewTab::Drop` shuts its engine host down, as `destroy_tab`
    /// relies on, so clearing the map is enough; spawns still in flight are
    /// dropped when they land (`session_generation`), and a PDF's render
    /// not yet started is skipped, as `remove_node` does.
    fn teardown_running_session(&mut self) {
        for (id, _) in self.nodes.iter().filter(|(_, node)| node.as_pdf().is_some()) {
            self.pdf_renderer.cancel(id);
        }
        let old: Vec<NodeId> =
            self.journal.as_ref().map(|j| j.doc().live().map(|(id, _)| id).collect()).unwrap_or_default();
        for id in old {
            self.queue_delete(&id.to_string());
        }
        // A link is not a card, so no endpoint's cascade is certain to reach it (a dangling one has none): the
        // replaced session's links all go, as its cards do.
        for link in self.mirror.link_ids().map(|l| l.to_string()).collect::<Vec<_>>() {
            self.delete_link(&link);
        }
        // The old session's nodes not created yet are not created at all.
        self.writes.pending_creates.clear();
        // Each old node's per-id state and GPU textures (`forget_id`), including tabs whose spawn is still in flight
        // (no `nodes` entry yet, but cookie and popup state already). The session-wide clears below stay.
        let old_ids: Vec<String> = self.nodes.keys().chain(self.spawns.spawning.keys()).cloned().collect();
        for id in old_ids {
            self.forget_id(&id);
        }
        self.begin_view_shutdown(); // the tabs below drop together: one SHUTDOWN_FLUSH wait, not one each
        self.nodes.clear();
        self.node_order.clear();
        self.layout.clear();
        self.writes.held_text.clear();
        self.focused = None;
        self.viewed_image = None;
        self.split_tab = None;
        self.current_group = None;
        self.interaction.dragging = None;
        self.interaction.drag_moved.clear();
        self.link.end();
        self.link.line_press = None;
        self.interaction.hovered = None;
        self.interaction.selected.clear();
        self.interaction.rect_select_start = None;
        self.graph_settled_key = None;
        self.spawns.reset();
        self.local_persist.pending_local = None;
    }

    /// Collapses the toasts queued since `mark` into one, so a bulk restore
    /// or import with many missing files toasts once, not once per card.
    fn collapse_toasts_since(&mut self, mark: usize) {
        let extra = self.toasts.save_toast_queue.len().saturating_sub(mark + 1);
        if extra > 0 {
            let first = self.toasts.save_toast_queue.drain(mark..).next().expect("toasts past the mark");
            self.toasts.save_toast_queue.push_back(format!("{first} (and {extra} more)"));
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
        self.store_recovered = true;
        self.notice(recovery_notice(&recovery));
    }

    /// Logs and toasts `msg`, unless it was shown since the last commit: a
    /// failing store or a damage report repeats at each save.
    fn notice(&mut self, msg: String) {
        if self.writes.store_notices.contains(&msg) {
            return;
        }
        eprintln!("session: {msg}");
        self.queue_toast(msg.clone());
        self.writes.store_notices.insert(msg);
    }
}

#[cfg(test)]
pub(crate) mod tests;
