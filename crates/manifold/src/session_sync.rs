//! The live graph and the session store (spec "Local edits become ops").
//! The first half is pure: each live node's shared fields (`Shared`), and
//! the diff that turns local edits into ops. The `impl App` half after it
//! restores the session, saves local edits, and shows other replicas' ops.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use ipc_protocol::HostToEngine;

use crate::local_state::{LocalNode, LocalState};
use crate::node::{
    DownloadPayload, FailedTabPayload, GroupPayload, ImagePayload, JsonPayload, NodePayload, NotePayload, TabPayload,
};
use crate::process_manager::EngineKind;
use crate::store::journal::JournalError;
use crate::store::json::JsonDoc;
use crate::store::model::{encode_field, field, field_def, Doc, FieldValue, Kind, Node, NodeId};
use crate::store::ops::OpBody;
use crate::{App, ClosedEntry, SpawnPurpose, TabCarry, TabSpawn, ViewMode, MAX_CLOSED};

/// A node's shared fields by number: every field of its kind but `group`.
pub(crate) type Fields = BTreeMap<u32, FieldValue>;

/// The fields each kind carries, `group` aside (spec, "The shared document").
pub(crate) fn kind_fields(kind: Kind) -> &'static [u32] {
    use field::*;
    match kind {
        Kind::Tab => &[POS, COLOR, PARENT, URL, ENGINE, PINNED],
        Kind::Group => &[POS, COLOR, NAME, FROM_JSON],
        Kind::Note => &[POS, COLOR, TEXT],
        Kind::Image | Kind::Pdf | Kind::Json | Kind::Download => &[POS, COLOR, PATH],
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

    // Existing nodes: moves and sets.
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
            let value = match value {
                FieldValue::Id(parent) => FieldValue::Id(valid(*parent)),
                other => other.clone(),
            };
            let enc = encode_field(no, &value);
            if current.fields.get(&no).map_or(&[][..], |reg| &reg.enc[..]) != &enc[..] {
                ops.push(OpBody::Set { node: id, field: no, enc });
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

// ---- The live graph and the store -----------------------------------------

/// One node another replica changed: its kind, fields and effective
/// group, or `None` when it is no longer live (deleted or pruned).
pub(crate) type RemoteChange = (NodeId, Option<(Kind, Fields, Option<NodeId>)>);

/// A node's kind, fields and group, as `materialize` takes them.
pub(crate) type RemoteNode = (Kind, Fields, Option<String>);

fn str_of(fields: &Fields, no: u32) -> String {
    match fields.get(&no) {
        Some(FieldValue::Str(s)) => s.clone(),
        _ => String::new(),
    }
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
/// any field the file left out filled in as absent. Shared by
/// `replace_running_session` and `App::reopen_closed` (main.rs), which
/// both materialize nodes `store::json::import` produced.
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
        let parse = |id: &Option<String>| id.as_deref().and_then(|id| id.parse::<NodeId>().ok());
        let str = |s: &str| FieldValue::Str(s.to_owned());
        let mut out = BTreeMap::new();
        for (key, node) in &self.nodes {
            let Ok(id) = key.parse::<NodeId>() else { continue };
            // Never a non-finite `pos` (contract "Fields").
            let finite = node.pos.x.is_finite() && node.pos.y.is_finite();
            let pos = if finite { FieldValue::Pos(node.pos.x, node.pos.y) } else { FieldValue::Pos(0.0, 0.0) };
            let mut fields = Fields::from([(field::POS, pos), (field::COLOR, color_value(node.color))]);
            let kind = match &node.payload {
                // A swap in progress already shows its target engine, so a
                // save meanwhile doesn't write the old one back.
                NodePayload::Tab(TabPayload { tab, url, pinned, swap_target, .. }) => {
                    tab_fields(&mut fields, url, swap_target.unwrap_or(tab.kind), *pinned, parse(&node.parent))
                }
                NodePayload::FailedTab(FailedTabPayload { url, kind, pinned, .. }) => {
                    tab_fields(&mut fields, url, *kind, *pinned, parse(&node.parent))
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
                    fields.insert(field::TEXT, str(&n.text));
                    Kind::Note
                }
                NodePayload::Image(i) => {
                    fields.insert(field::PATH, str(&i.path));
                    Kind::Image
                }
                NodePayload::Pdf(p) => {
                    fields.insert(field::PATH, str(&p.path));
                    Kind::Pdf
                }
                NodePayload::Json(j) => {
                    fields.insert(field::PATH, str(&j.path));
                    match &j.key {
                        None => Kind::Json,
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
            out.insert(id, Shared { kind, group: parse(&node.group), fields });
        }
        out
    }

    /// Makes live node `id` show `fields`. A new node is inserted, and a
    /// new tab is spawned with `purpose` (it joins `spawning`). A node whose
    /// spawn is in flight keeps the change for when it lands
    /// (`remote_in_flight`). An existing node is updated in place, but only
    /// in the fields (and group) where the store differs from what this
    /// replica last saw of it (`last_shared`), so an unsaved local value in
    /// another field survives:
    /// - a tab's url change navigates without marking `url`;
    /// - an engine change swaps engines.
    ///
    /// A kind mismatch between the live node and `kind` is ignored (the
    /// store never changes a node's kind). A file card whose file can't be
    /// loaded is kept, showing its error, so the next diff doesn't delete
    /// it from the store.
    ///
    /// `initial_pos`, when given, places the node here instead of at
    /// `fields`'s stored `pos` (restore applies personal state's settled
    /// position); `last_shared` still records the real stored `pos`, so a
    /// later save doesn't diff the override back into the store. Every
    /// caller but `restore_session` passes `None`.
    pub(crate) fn materialize(
        &mut self,
        id: String,
        kind: Kind,
        fields: &Fields,
        group: Option<String>,
        purpose: SpawnPurpose,
        initial_pos: Option<egui::Vec2>,
    ) {
        if self.spawning.contains(&id) {
            self.remote_in_flight.insert(id, Some((kind, fields.clone(), group)));
            return;
        }
        let pos = initial_pos.unwrap_or_else(|| pos_of(fields));
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
            match &mut node.payload {
                NodePayload::Tab(tab) => {
                    if changed(field::PINNED) {
                        tab.pinned = bool_of(fields, field::PINNED);
                    }
                    let url = str_of(fields, field::URL);
                    if changed(field::URL) && tab.url != url {
                        tab.url = url.clone();
                        tab.tab.send(&HostToEngine::Navigate { url, referrer: None });
                    }
                    let want = engine_field(fields);
                    if changed(field::ENGINE) && tab.swap_target.unwrap_or(tab.tab.kind) != want {
                        swap = Some(want);
                    }
                }
                NodePayload::FailedTab(tab) => {
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
                NodePayload::Note(n) if changed(field::TEXT) => n.text = str_of(fields, field::TEXT),
                NodePayload::Note(_) => {}
                // Paths, keys and values never change after creation.
                NodePayload::Download(_) | NodePayload::Image(_) | NodePayload::Pdf(_) | NodePayload::Json(_) => {}
            }
            if let Some(target) = swap {
                self.start_swap_to(&id, target);
            }
            return;
        }
        match kind {
            Kind::Tab => {
                let url = str_of(fields, field::URL);
                let spawn = TabSpawn::new(id, url, engine_field(fields), pos, purpose, self.settings.zoom_factor);
                let carry = TabCarry { pinned: bool_of(fields, field::PINNED), color, ..spawn.carry };
                self.spawn_tab_async(TabSpawn { parent: id_of(fields, field::PARENT), group, carry, ..spawn });
                return;
            }
            Kind::Group => {
                let payload =
                    GroupPayload { name: str_of(fields, field::NAME), from_json: bool_of(fields, field::FROM_JSON) };
                self.insert_placeholder_node(id.clone(), pos, NodePayload::Group(payload));
            }
            Kind::Note => {
                let payload = NotePayload { text: str_of(fields, field::TEXT), editing: false };
                self.insert_placeholder_node(id.clone(), pos, NodePayload::Note(payload));
            }
            Kind::Image => {
                let path = str_of(fields, field::PATH);
                if let Err(error) = self.insert_image_node_with_id(id.clone(), path.clone(), pos, None) {
                    let payload = ImagePayload { path, width: 0, height: 0, error: Some(error) };
                    self.insert_placeholder_node(id.clone(), pos, NodePayload::Image(payload));
                }
            }
            Kind::Pdf => self.insert_pdf_node_with_id(id.clone(), str_of(fields, field::PATH), pos, None),
            Kind::Json => {
                let path = str_of(fields, field::PATH);
                if let Err(error) = self.insert_json_node_with_id(id.clone(), path.clone(), pos, None) {
                    let payload = JsonPayload { path, value: serde_json::Value::Null, key: None, error: Some(error) };
                    self.insert_placeholder_node(id.clone(), pos, NodePayload::Json(payload));
                }
            }
            Kind::JsonLeaf => {
                let text = str_of(fields, field::VALUE);
                let value = serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text));
                let key = Some(str_of(fields, field::KEY));
                let payload = JsonPayload { path: str_of(fields, field::PATH), value, key, error: None };
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
    /// exist yet. A new tab spawns as `Restore`, so a spawn failure keeps
    /// it as a failed card rather than dropping it from the store.
    pub(crate) fn apply_remote(&mut self, mut changes: Vec<RemoteChange>) {
        changes.sort_by_key(|(_, c)| !matches!(c, Some((Kind::Group, ..))));
        let toasts = self.save_toast_queue.len();
        for (id, change) in changes {
            let key = id.to_string();
            match change {
                Some((kind, fields, group)) => {
                    self.materialize(key, kind, &fields, group.map(|g| g.to_string()), SpawnPurpose::Restore, None)
                }
                None if self.spawning.contains(&key) => {
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
        let is_tab = node.as_tab().is_some();
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
        self.failed_tab_retries.remove(id);
    }

    /// Applies what other replicas changed while `id`'s spawn was in flight,
    /// now that its node exists (`handle_tab_spawn_result`).
    pub(crate) fn land_remote_in_flight(&mut self, id: &str, remote: Option<Option<RemoteNode>>) {
        match remote {
            Some(Some((kind, fields, group))) => {
                self.materialize(id.to_string(), kind, &fields, group, SpawnPurpose::Restore, None)
            }
            Some(None) => {
                self.last_shared.remove(id);
                self.remove_remotely_deleted(id);
            }
            None => {}
        }
    }

    /// Marks explicit field `field` (`pos` or `url`) of `id` for the next
    /// save (spec "Local edits become ops"). New nodes need no mark: their
    /// `Create` carries every field.
    pub(crate) fn mark(&mut self, id: &str, field: u32) {
        if let Ok(id) = id.parse() {
            self.explicit.insert((id, field));
        }
    }

    /// Saves local edits (spec "Local edits become ops"): take the lock,
    /// show what other replicas wrote, diff the live graph against the
    /// document, and commit. On error the explicit marks stay for the next
    /// save, which re-diffs (contract "Transactions"). `true` when the
    /// store now holds every local edit: `false` read-only, on error, and
    /// when another instance kept writing.
    pub(crate) fn save_session(&mut self) -> bool {
        // Each round applies what others wrote since the last; four rounds
        // of fresh remote frames means another instance is busy: try again
        // at the next save point (or frame: `poll_store`) rather than spin.
        for _ in 0..4 {
            // Projected before `begin`, since the `Txn` borrows the journal.
            let live = self.shared_nodes();
            let keep: BTreeSet<NodeId> = self.spawning.iter().filter_map(|id| id.parse().ok()).collect();
            let Some(journal) = self.journal.as_mut() else { return false };
            let txn = match journal.begin() {
                Ok(txn) => txn,
                Err(err) => {
                    self.store_error(err);
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
                self.take_store_recovery();
                return false;
            }
            let ops = diff(txn.doc(), &live, &self.explicit, &keep);
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
                    // only if it was marked (an unmarked `pos` stays).
                    if let Some(journal) = &self.journal {
                        let doc = journal.doc();
                        for &id in live.keys() {
                            let Some(node) = doc.nodes.get(&id).filter(|n| n.is_live()) else { continue };
                            let seen =
                                Shared { kind: node.kind, group: doc.effective_group(id), fields: fields_of(node) };
                            self.last_shared.insert(id.to_string(), seen);
                        }
                    }
                    self.explicit.clear();
                    self.last_session_save = Some(std::time::Instant::now());
                    return true;
                }
                Err(err) => {
                    self.store_error(err);
                    return false;
                }
            }
        }
        false
    }

    /// Startup: shows the document, then applies personal state
    /// (`session.mnf.local`) on top. `false` when it has no live node, so
    /// the caller starts fresh.
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
            return false;
        }
        nodes.sort_by_key(|(_, kind, ..)| *kind != Kind::Group);
        let toasts = self.save_toast_queue.len();
        for (id, kind, fields, group) in &nodes {
            let group = group.map(|g| g.to_string());
            // The store's own `pos` still becomes `last_shared` inside
            // `materialize`; only the live placement is overridden.
            let initial_pos = local.nodes.get(&id.to_string()).map(|n| egui::vec2(n.pos[0], n.pos[1]));
            self.materialize(id.to_string(), *kind, fields, group, SpawnPurpose::Restore, initial_pos);
        }
        self.collapse_toasts_since(toasts);
        // Graph view with the first tab focused, re-fit once restored tabs
        // land; `apply_local` below overrides both from personal state when
        // it has them.
        self.focused = nodes.iter().find(|(_, kind, ..)| *kind == Kind::Tab).map(|(id, ..)| id.to_string());
        self.view_mode = ViewMode::Graph;
        self.camera.frame_restored = true;
        self.apply_local(local);
        self.local_written = Some(self.local_state());
        true
    }

    /// This profile's personal state as the live graph currently shows it
    /// (spec "Personal state"): what to write to `session.mnf.local`.
    pub(crate) fn local_state(&self) -> LocalState {
        let item = self.view_mode == ViewMode::Item;
        let nodes = self
            .nodes
            .iter()
            .filter(|(id, _)| id.parse::<NodeId>().is_ok())
            .map(|(id, node)| {
                let mut local = LocalNode { pos: [node.pos.x, node.pos.y], ..LocalNode::default() };
                match &node.payload {
                    NodePayload::Tab(tab) => {
                        local.title = tab.title.clone();
                        local.muted = tab.muted;
                        if tab.zoom_factor != self.settings.zoom_factor {
                            local.zoom = Some(tab.zoom_factor);
                        }
                    }
                    NodePayload::Note(n) => local.editing = n.editing,
                    _ => {}
                }
                (id.clone(), local)
            })
            .collect();
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
            id.as_deref().is_some_and(|id| self.nodes.contains_key(id) || self.spawning.contains(id))
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
        // Oldest first, like `state.closed` itself; a blank entry (a
        // format this build doesn't recognize inside `doc.nodes`, or one
        // some other bug produced) is dropped rather than shown as an
        // empty row.
        let mut closed: Vec<ClosedEntry> =
            state.closed.into_iter().filter(|doc| !doc.nodes.is_empty()).map(|doc| ClosedEntry { doc }).collect();
        if closed.len() > MAX_CLOSED {
            closed.drain(..closed.len() - MAX_CLOSED);
        }
        self.closed = closed;
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
            } else if self.spawning.contains(&id) {
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
            NodePayload::Note(n) => n.editing = node.editing,
            _ => {}
        }
    }

    /// Applies a spawning node's personal-state entry once its node exists
    /// (`handle_tab_spawn_result`), removing it from `pending_local`. A
    /// `FailedTab` card has no title/muted/editing fields, so only its
    /// position (and zoom, which it does have) come from personal state;
    /// the rest of the entry is simply dropped rather than lingering for an
    /// id that will never gain those fields.
    pub(crate) fn apply_pending_local(&mut self, id: &str) {
        let Some(pending) = &mut self.pending_local else { return };
        let Some(node) = pending.nodes.remove(id) else { return };
        if pending.nodes.is_empty() {
            self.pending_local = None;
        }
        if let Some(live) = self.nodes.get_mut(id) {
            match &live.payload {
                NodePayload::Tab(_) | NodePayload::Note(_) => Self::apply_local_node(live, &node),
                _ => {
                    live.pos = egui::vec2(node.pos[0], node.pos[1]);
                    live.vel = egui::Vec2::ZERO;
                    if let (NodePayload::FailedTab(tab), Some(zoom)) = (&mut live.payload, node.zoom) {
                        tab.zoom_factor = zoom;
                    }
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

    /// Each frame: pick up other instances' edits, and flush a debounced
    /// fsync (contract "Flushing").
    pub(crate) fn poll_store(&mut self) {
        let Some(journal) = self.journal.as_mut() else { return };
        let synced = journal.sync_if_due();
        let changed = journal.changed_on_disk();
        if let Err(err) = synced {
            self.store_error(err);
        }
        if changed {
            self.save_session();
        }
        self.maybe_write_local(false);
    }

    /// Clean exit: save, write personal state, then close (compacts only if
    /// something changed).
    pub(crate) fn close_store(&mut self) {
        self.save_session();
        self.maybe_write_local(true);
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
    /// unknown group.
    pub(crate) fn export_group_json(&mut self, group: &str) -> Option<JsonDoc> {
        self.journal.as_ref()?;
        let root: NodeId = group.parse().ok()?;
        if !self.save_session() {
            self.queue_toast("The export may miss recent edits: the session couldn't be saved first".into());
        }
        Some(crate::store::json::subtree_to_json(self.journal.as_ref()?.doc(), root))
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
    /// is kept; anything else (the root, or a member whose group was
    /// outside the exported subtree) is dropped to `None`/the new root as
    /// appropriate. Ends with `save_session`, same as
    /// `replace_running_session`/`reopen_closed`.
    pub(crate) fn import_group_json(&mut self, json: &JsonDoc) {
        let imported = match crate::store::json::import(json, || self.alloc_node_id().parse().expect("a store id")) {
            Ok(imported) => imported,
            Err(err) => {
                self.queue_toast(format!("Couldn't import the group: {err}"));
                return;
            }
        };
        let Some(root) = imported.first().map(|i| i.id) else { return };
        let groups: BTreeSet<NodeId> = imported.iter().filter(|i| i.kind == Kind::Group).map(|i| i.id).collect();
        // A node's `pos` is always within its own immediate group's local
        // space (contract "The graph's world-space center" -- see
        // `GRAPH_WORLD_CENTER`'s own doc comment), so only the root -- the
        // one node whose group is changing, from wherever it was exported
        // to `self.current_group` -- needs a fresh position; every other
        // member's file position is already relative to its own (possibly
        // nested) group and stays as-is, same as the old per-group export
        // format's own import behavior.
        let root_pos = crate::GRAPH_WORLD_CENTER + crate::graph_layout::spawn_jitter(&root.to_string());

        let mut imported = imported;
        imported.sort_by_key(|i| i.kind != Kind::Group);
        for node in imported {
            let group = if node.id == root {
                self.current_group.clone()
            } else {
                match node.fields.get(&field::GROUP) {
                    Some(FieldValue::Id(Some(g))) if groups.contains(g) => Some(g.to_string()),
                    _ => None,
                }
            };
            let mut fields = imported_fields(node.kind, &node.fields);
            if node.id == root {
                fields.insert(field::POS, FieldValue::Pos(root_pos.x, root_pos.y));
            }
            self.materialize(node.id.to_string(), node.kind, &fields, group, SpawnPurpose::New, None);
        }
        self.save_session();
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
        // A group or parent outside the file (import keeps real node ids)
        // names nothing once the old session is gone.
        let groups: BTreeSet<NodeId> = imported.iter().filter(|i| i.kind == Kind::Group).map(|i| i.id).collect();
        let ids: BTreeSet<NodeId> = imported.iter().map(|i| i.id).collect();
        let mut imported = imported;
        imported.sort_by_key(|i| i.kind != Kind::Group);
        let first_tab = imported.iter().find(|i| i.kind == Kind::Tab).map(|i| i.id.to_string());
        let toasts = self.save_toast_queue.len();
        for node in imported {
            let group = match node.fields.get(&field::GROUP) {
                Some(FieldValue::Id(Some(g))) if groups.contains(g) => Some(g.to_string()),
                _ => None,
            };
            let mut fields = imported_fields(node.kind, &node.fields);
            if let Some(FieldValue::Id(parent)) = fields.get_mut(&field::PARENT) {
                *parent = parent.filter(|p| ids.contains(p));
            }
            self.materialize(node.id.to_string(), node.kind, &fields, group, SpawnPurpose::Restore, None);
        }
        self.collapse_toasts_since(toasts);
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
        self.failed_tab_retries.clear();
        self.spawning.clear();
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

    /// Tells the user once about a store error (Task 10 refines this).
    pub(crate) fn store_error(&mut self, err: JournalError) {
        self.store_notice(format!("Session not saved: {err}"));
    }

    /// Tells the user what a repair of the session file lost, and where
    /// the original was kept (contract "Repairs").
    pub(crate) fn take_store_recovery(&mut self) {
        let Some(recovery) = self.journal.as_mut().and_then(|j| j.take_recovery()) else { return };
        let mut message = if recovery.new_document {
            "The session file was damaged; started a new one with what could be read".to_string()
        } else if recovery.from_prev {
            "The session file was damaged; restored from its previous copy".to_string()
        } else {
            let parts = recovery.damaged.len() + recovery.skipped_frames + usize::from(recovery.header_damaged);
            format!("The session file was damaged; skipped {parts} damaged part(s)")
        };
        if let Some(saved) = &recovery.saved {
            message.push_str(&format!(" (original kept at {})", saved.display()));
        }
        self.store_notice(message);
    }

    /// Logs and toasts `message`, unless it is the last one shown: a
    /// failing store or a damage report repeats at each save.
    fn store_notice(&mut self, message: String) {
        if self.last_store_notice.as_ref() != Some(&message) {
            eprintln!("session: {message}");
            self.queue_toast(message.clone());
            self.last_store_notice = Some(message);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::store::journal::Journal;
    use crate::store::model::{ActorId, DocId, Hlc};
    use crate::store::ops::Op;
    use crate::store::testutil::{temp_dir, XorShift};

    const A: ActorId = ActorId(0xA);

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
    fn a_remotely_removed_group_leaves_no_member_naming_it() {
        let nodes = BTreeMap::from([
            (id(0), group("Outer")),
            (id(1), shared(Kind::Group, Some(id(0)), &[(field::NAME, FieldValue::Str("Inner".into()))])),
            (id(2), note("stored member", Some(id(1)))),
        ]);
        let mut app = app_with(&nodes);
        // A note only this replica has, not yet saved, inside the group.
        let local = id(3).to_string();
        let payload = NodePayload::Note(NotePayload { text: "local".into(), editing: false });
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
        let note = add(&mut app, NodePayload::Note(NotePayload { text: "v1".into(), editing: false }));
        let failed = FailedTabPayload {
            url: "https://u0/".into(),
            kind: EngineKind::Servo,
            error: "x".into(),
            pinned: false,
            zoom_factor: 1.0,
            retrying: false,
        };
        let tab = add(&mut app, NodePayload::FailedTab(failed));
        assert!(app.save_session());
        let mut other = Journal::open(&path, ActorId(2)).unwrap();

        // Unsaved here: typing in the note, and a navigation (`url` is
        // explicit and unmarked, so saves don't write it).
        if let NodePayload::Note(n) = &mut app.nodes.get_mut(&note).unwrap().payload {
            n.text = "typing".into();
        }
        if let NodePayload::FailedTab(t) = &mut app.nodes.get_mut(&tab).unwrap().payload {
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
        assert!(matches!(&app.nodes[&tab].payload, NodePayload::FailedTab(t) if t.pinned && t.url == "https://u1/"));
        assert_eq!(stored(&app, &tab, field::URL), FieldValue::Str("https://u0/".into()));

        // A later remote edit to the same field does apply.
        remote_set(&mut other, &note, field::TEXT, FieldValue::Str("theirs".into()));
        assert!(app.save_session());
        assert!(matches!(&app.nodes[&note].payload, NodePayload::Note(n) if n.text == "theirs"));
        assert_eq!(app.nodes[&note].color, Some(egui::Color32::RED));
    }

    #[test]
    fn a_failed_commit_keeps_the_explicit_marks() {
        let path = temp_dir("sync-fail").join("session.mnf");
        let mut app = app_on(&path, 1);
        let note = add(&mut app, NodePayload::Note(NotePayload { text: "n".into(), editing: false }));
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
        assert!(app.save_session());
        assert!(app.explicit.is_empty());
        assert_eq!(stored(&app, &note, field::POS), FieldValue::Pos(5.0, 6.0));
    }

    #[test]
    fn a_marked_pos_is_saved_and_an_unmarked_one_is_not() {
        let path = temp_dir("marks").join("session.mnf");
        let mut app = app_on(&path, 1);
        let note = add(&mut app, NodePayload::Note(NotePayload { text: "n".into(), editing: false }));
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
        add(&mut app, NodePayload::Note(NotePayload { text: "n".into(), editing: false }));
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
            NodePayload::FailedTab(FailedTabPayload {
                url: "https://x/".into(),
                kind: EngineKind::Servo,
                error: "x".into(),
                pinned: false,
                zoom_factor: 1.0,
                retrying: false,
            }),
        );
        // A node whose id isn't live yet -- `spawning`, not `self.nodes` --
        // is exactly the shape a real tab restore has before its spawn
        // lands (`materialize`'s `Kind::Tab` arm never inserts directly).
        app.nodes.remove(&tab);
        app.spawning.insert(tab.clone());
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
            let id = add(&mut app, NodePayload::Note(NotePayload { text: String::new(), editing: false }));
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
}
