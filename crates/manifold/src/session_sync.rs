//! The live graph and the session store (spec "Local edits become ops"). This file holds the pure half: a node as
//! the store holds it (`Snapshot`), the `Create`s for queued new nodes, the field codec and the one link resolver, plus
//! the session lifecycle (`restore_session`, `poll_store`, `close_store`). The rest is split by direction:
//! `writes` saves local edits as ops (`PendingWrites`, `build_ops`, `save_session`), `remote` shows the store in the
//! live graph (`materialize`, `apply_remote`), `edits` is the `edit_*` writers. Personal-state apply is in
//! `local_state.rs`; JSON import/export and session replace are in `session_files.rs`.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use manifold_view_protocol::HostToView;

use crate::local_state::LocalNode;
use crate::node::{DownloadPayload, JsonPayload, NotePayload, Runtime, UnloadedState, UnloadedTabPayload};
use crate::process_manager::ViewKind;
use crate::store::journal::{JournalError, Recovery};
use crate::store::model::{
    decode_field, encode_field, field, field_def, Doc, FieldType, FieldValue, Kind, Node, NodeId, TYPE_TAB,
};
use crate::store::ops::OpBody;
use crate::store::tree::{self, Leaf, TreePath, Trees, TREE_FIELDS};
use crate::tabs::TabSpawn;
use crate::{App, ViewMode};

mod edits;
mod remote;
mod writes;

pub(crate) use crate::local_state::{local_from, LocalUnread};
pub(crate) use crate::session_files::ImportRoot;
pub(crate) use writes::{Life, PendingWrites};

/// How long after a save that didn't commit `poll_store` tries again:
/// soon enough that an edit isn't left unsaved, slow enough that a failing
/// disk isn't retried every frame.
pub(crate) const SAVE_RETRY: std::time::Duration = std::time::Duration::from_secs(1);

/// How long after a host-started load last reported loading its tab's url
/// changes stay local (`App::shares_url_change`), when the engine doesn't
/// say how they started: a script redirect can come just after the page
/// reports loaded.
/// The least time between two looks at the store file for another instance's edits (none under test, which polls by hand).
const STORE_STAT_GAP: std::time::Duration =
    if cfg!(test) { std::time::Duration::ZERO } else { std::time::Duration::from_millis(100) };

const HOST_LOAD_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// A node's fields by number: every field of its kind (`Kind::fields`) but `group`, absent values included, so a
/// field cleared reads as a change.
pub(crate) type Fields = BTreeMap<u32, FieldValue>;

/// A node as the store holds it, or is about to (`App::snapshot`): its type string as stored, its group and its
/// fields of that type's kind (`Kind::fields`, every one present). A `Create` writes one, a Duplicate exports
/// one, and a tab's spawn starts from one (`App::spawning`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Snapshot {
    /// The stored `type` (`Node::ty`, `Mirror::ty`), kept as written: a type this build doesn't know included.
    pub ty: String,
    pub group: Option<NodeId>,
    pub fields: Fields,
    /// The node's tree fields (`PROPS`), non-empty ones only. Travels wherever `fields` does.
    pub trees: Trees,
}

impl Snapshot {
    /// What the document shows for its node `id`: its type, its effective group and its fields (`fields_of`).
    pub(crate) fn of(doc: &Doc, id: NodeId, node: &Node) -> Snapshot {
        Snapshot {
            ty: node.ty().to_owned(),
            group: doc.effective_group(id),
            fields: fields_of(node),
            trees: node.trees(),
        }
    }

    /// The kind its type means (`Kind::from_type`).
    pub(crate) fn kind(&self) -> Kind {
        Kind::from_type(&self.ty)
    }
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
/// A tab's runtime is one type whether it is loaded or asleep; a group has none; a type this build
/// doesn't know reads as `TYPE_UNKNOWN` (`card_kind::canonical_type`).
#[cfg(debug_assertions)]
fn runtime_has_type(runtime: &Runtime, ty: &str) -> bool {
    use crate::store::model::{
        TYPE_DOWNLOAD, TYPE_GROUP, TYPE_IMAGE, TYPE_JAR, TYPE_JSON, TYPE_NOTE, TYPE_PDF, TYPE_TAB, TYPE_UNKNOWN,
    };
    match runtime {
        Runtime::Unknown => ty == TYPE_UNKNOWN,
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

/// The one resolver of the links a commit writes: `target` stands if the document shows it live
/// and this commit (`deleted`) doesn't delete it, or this commit (`created`) creates it; otherwise it is written as
/// none (contract "Missing means deleted"). A closed target is still valid: its node is only gone once
/// its delete commits.
fn resolve_id(
    doc: &Doc,
    created: &BTreeSet<NodeId>,
    deleted: &BTreeSet<NodeId>,
    target: Option<NodeId>,
) -> Option<NodeId> {
    target.filter(|t| (doc.nodes.get(t).is_some_and(Node::is_live) && !deleted.contains(t)) || created.contains(t))
}

/// The `Create`s for `new`, the queued nodes the document has no entry for, each with what it shows,
/// each group's chain of new ancestors before the nodes inside it. Every link a `Create` carries
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
        ops.push(OpBody::create(id, &node.ty, enc));
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

/// What another replica's change leaves a node as (spec decision 3): shown with this snapshot, hidden under or as a
/// closed node (kept, out of the graph), or deleted.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Remote<T> {
    Shown(T),
    Hidden,
    Deleted,
}

impl<T> Remote<T> {
    pub(crate) fn shown_mut(&mut self) -> Option<&mut T> {
        match self {
            Remote::Shown(t) => Some(t),
            _ => None,
        }
    }
}

/// One node another replica changed: what the document shows for it (`Snapshot::of`), or that it is hidden or no
/// longer live (deleted or pruned).
pub(crate) type RemoteChange = (NodeId, Remote<Snapshot>);

fn str_of(fields: &Fields, no: u32) -> String {
    fields.get(&no).and_then(FieldValue::as_str).unwrap_or_default().to_owned()
}

/// The URL field as `Option<String>`: absent (empty string) is `None`.
fn url_of(fields: &Fields) -> Option<String> {
    Some(str_of(fields, field::URL)).filter(|u| !u.is_empty())
}

/// The document pos `fields` hold; `None` when unplaced (`FieldValue::as_pos`).
pub(crate) fn pos_in(fields: &Fields) -> Option<egui::Vec2> {
    fields.get(&field::POS)?.as_pos().map(|(x, y)| egui::vec2(x, y))
}

/// A stored node's document pos, or, for one that has none (an old jar from before jars were cards), a jittered
/// spawn around the world center: the rule `App::placement` uses for a new node with no position.
pub(crate) fn pos_of(id: NodeId, fields: &Fields) -> egui::Vec2 {
    pos_in(fields).unwrap_or_else(|| crate::graph_layout::GRAPH_WORLD_CENTER + crate::graph_layout::spawn_jitter(id))
}

/// `pos` as the store keeps it: never non-finite (contract "Fields"), so a non-finite one is unplaced.
pub(crate) fn pos_value(pos: egui::Vec2) -> FieldValue {
    FieldValue::Pos((pos.x.is_finite() && pos.y.is_finite()).then_some((pos.x, pos.y)))
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
    jar: Option<NodeId>,
) -> Snapshot {
    let fields = Fields::from([
        (field::POS, pos.map_or(FieldValue::Pos(None), pos_value)),
        (field::URL, FieldValue::Str(url.to_owned())),
        (field::VIEW_KIND, FieldValue::ViewCode(view_kind)),
        (field::PINNED, FieldValue::Bool(spawn.carry.pinned)),
        (field::PARENT, FieldValue::Id(spawn.parent)),
        (field::JAR, FieldValue::Id(jar)),
    ]);
    Snapshot { ty: TYPE_TAB.to_owned(), group: spawn.group, fields, trees: Trees::new() }
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

impl App {
    /// Startup: shows the document, then applies personal state
    /// (`session.mnf.local`) on top. `false` when it has no live node, so
    /// the caller starts fresh. Closes past `MAX_CLOSED` are evicted at load (spec decision 6).
    pub(crate) fn restore_session(&mut self) -> bool {
        let Some(journal) = self.doc.journal.as_ref() else { return false };
        let doc = journal.doc();
        let mut nodes: Vec<(NodeId, Snapshot)> = doc.shown().map(|(id, n)| (id, Snapshot::of(doc, id, n))).collect();
        self.take_store_recovery();
        // Contract: `None` (no file, another document's, or a newer format
        // version) or a damaged file is a fresh start, same as a session
        // with no personal state at all, and the next write replaces it. A
        // read that failed (the lock stayed busy, an I/O error) is not: the
        // file may hold this profile's state, so it isn't written until
        // `poll_store` has read it.
        let read = self.doc.journal.as_ref().map(|j| j.read_local());
        let unread = matches!(read, Some(Err(ref err)) if !matches!(err, JournalError::Corrupt(_)));
        if let Some(Err(err)) = read.as_ref().filter(|_| unread) {
            self.log_local_unread(err);
        }
        let local = local_from(read);
        let toasts = self.toasts.save_toast_queue.len();
        // Links and labels ride along: stored, never graph nodes.
        for (id, s) in nodes.iter().filter(|(_, s)| !s.kind().card()) {
            self.materialize(*id, s, None);
        }
        nodes.retain(|(_, s)| s.kind().card());
        // Only jars (the Default at least): a fresh start, which keeps them where they are.
        if nodes.iter().all(|(_, s)| s.kind() == Kind::Jar) {
            for (id, s) in &nodes {
                self.materialize(*id, s, None);
            }
            self.keep_hidden(local.nodes);
            self.evict_closed();
            self.arm_local(unread, true);
            return false;
        }
        nodes.sort_by_key(|(_, s)| s.kind() != Kind::Group);
        let pass = self.begin_content_pass();
        for (id, s) in &nodes {
            // The store's own `pos` still reaches the mirror inside
            // `materialize`; only the live placement is overridden.
            let entry = local.nodes.get(id);
            self.materialize(*id, s, entry);
        }
        self.end_content_pass(pass);
        self.collapse_toasts_since(toasts);
        // Graph view with the first tab focused, re-fit once restored tabs
        // land; `apply_local` below overrides both from personal state when
        // it has them.
        self.focused = nodes.iter().find(|(_, s)| s.kind() == Kind::Tab).map(|(id, _)| *id);
        self.view_mode = ViewMode::Graph;
        self.camera.frame_restored = true;
        self.apply_local(local);
        self.wake_focused();
        self.evict_closed();
        self.arm_local(unread, false);
        true
    }

    /// Each frame: pick up other instances' edits, retry a save that didn't
    /// commit, and flush a debounced fsync (contract "Flushing").
    pub(crate) fn poll_store(&mut self) {
        let Some(journal) = self.doc.journal.as_mut() else { return };
        let synced = journal.sync_if_due();
        // A stat is a system call; other instances' edits can wait a tick, not a frame.
        let now = std::time::Instant::now();
        let stat_due = self.doc.writes.store_statted_at.is_none_or(|at| now.duration_since(at) >= STORE_STAT_GAP);
        let changed = stat_due && journal.changed_on_disk();
        if stat_due {
            self.doc.writes.store_statted_at = Some(now);
        }
        if let Err(err) = synced {
            self.store_error(err);
        }
        let retry_due = self.doc.writes.save_retry_at.is_some_and(|at| std::time::Instant::now() >= at);
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
        self.flush_histories();
        if let Some(journal) = self.doc.journal.as_mut() {
            if let Err(err) = journal.close() {
                eprintln!("session: closing the store failed: {err}");
            }
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
        let Some(recovery) = self.doc.journal.as_mut().and_then(|j| j.take_recovery()) else { return };
        self.doc.store_recovered = true;
        self.notice(recovery_notice(&recovery));
    }

    /// Logs and toasts `msg`, unless it was shown since the last commit: a
    /// failing store or a damage report repeats at each save.
    fn notice(&mut self, msg: String) {
        if self.doc.writes.store_notices.contains(&msg) {
            return;
        }
        eprintln!("session: {msg}");
        self.queue_toast(msg.clone());
        self.doc.writes.store_notices.insert(msg);
    }
}

#[cfg(test)]
pub(crate) mod tests;
