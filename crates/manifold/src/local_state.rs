//! Personal state (spec "Personal state"): what this profile shows and
//! how, never merged. JSON inside `session.mnf.local`'s state bytes
//! (plan ruling 4).

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::node::Runtime;
use crate::session_sync::SAVE_RETRY;
use crate::store::journal::JournalError;
use crate::store::model::{Node, NodeId};
use crate::{App, ViewMode};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct LocalState {
    #[serde(default)]
    pub focused: Option<String>,
    /// Item view (a tab or image full-size) rather than Graph view.
    #[serde(default)]
    pub view_item: bool,
    #[serde(default)]
    pub current_group: Option<String>,
    #[serde(default)]
    pub viewed_image: Option<String>,
    #[serde(default)]
    pub split_tab: Option<String>,
    #[serde(default)]
    pub groups_collapsed: bool,
    /// The camera rect `[min x, min y, max x, max y]` and the level it was
    /// saved at (plan ruling 5).
    #[serde(default, deserialize_with = "lenient_camera")]
    pub camera: Option<([f32; 4], Option<String>)>,
    /// Per node, by live id.
    #[serde(default)]
    pub nodes: BTreeMap<String, LocalNode>,
    /// Copy path on a JSON level gives the accessor form
    /// (`users[0].name`) rather than a JSON Pointer.
    #[serde(default)]
    pub json_accessor_paths: bool,
    /// The JSON outline panel is hidden.
    #[serde(default)]
    pub json_outline_hidden: bool,
    /// The JSON outline panel's width, once resized.
    #[serde(default)]
    pub json_outline_width: Option<f32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct LocalNode {
    /// The settled position, so the graph reopens as it was left.
    #[serde(deserialize_with = "lenient_pos")]
    pub pos: [f32; 2],
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub muted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zoom: Option<f64>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub editing: bool,
    /// A tab's current url, when its page moved on from the shared `url`
    /// without sharing (a redirect, a script's navigation).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_url: Option<LiveUrl>,
}

/// See `LocalNode::live_url`. A restart reopens `url` while the shared
/// `url` is still `from`; once another replica has shared a new one, that
/// wins.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct LiveUrl {
    pub url: String,
    pub from: String,
}

fn is_false(b: &bool) -> bool {
    !b
}

/// A `null` coordinate (an older build wrote NaN that way) reads as 0.
fn lenient_pos<'de, D: serde::Deserializer<'de>>(d: D) -> Result<[f32; 2], D::Error> {
    let [x, y] = <[Option<f32>; 2]>::deserialize(d)?;
    Ok([x.unwrap_or(0.0), y.unwrap_or(0.0)])
}

/// A camera rect with a `null` in it reads as no saved camera.
#[allow(clippy::type_complexity, reason = "mirrors the field's own type")]
fn lenient_camera<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<([f32; 4], Option<String>)>, D::Error> {
    let raw = Option::<([Option<f32>; 4], Option<String>)>::deserialize(d)?;
    Ok(raw.and_then(|(r, level)| Some(([r[0]?, r[1]?, r[2]?, r[3]?], level))))
}

/// Whether `a` and `b` differ enough to write: settled positions within
/// 1 px count as equal, so physics jitter never causes a write.
pub(crate) fn differs(a: &LocalState, b: &LocalState) -> bool {
    let strip = |s: &LocalState| {
        let mut s = s.clone();
        s.nodes.values_mut().for_each(|n| n.pos = [0.0, 0.0]);
        // A non-finite camera is never written (`to_bytes`), so it must not read as a change either.
        if s.camera.as_ref().is_some_and(|(r, _)| r.iter().any(|v| !v.is_finite())) {
            s.camera = None;
        }
        s
    };
    if strip(a) != strip(b) {
        return true;
    }
    a.nodes.iter().any(|(id, n)| {
        let m = &b.nodes[id]; // same keys: `strip` compared them
        (n.pos[0] - m.pos[0]).abs() > 1.0 || (n.pos[1] - m.pos[1]).abs() > 1.0
    })
}
/// The personal-state file's write bookkeeping (`App::local_persist`).
pub(crate) struct LocalPersist {
    /// Personal state (spec "Personal state") last written to
    /// `session.mnf.local`, or read at startup: `maybe_write_local` skips
    /// writing when the current state compares equal, so an idle session
    /// never rewrites the file.
    pub(crate) local_written: Option<LocalState>,
    /// Set while startup couldn't read `session.mnf.local` (another
    /// window held the lock, or the read failed): nothing writes it, so
    /// this profile's saved state isn't replaced by defaults, and
    /// `poll_store` retries the read.
    pub(crate) local_unread: Option<crate::session_sync::LocalUnread>,
    /// When `local_written` was last written (or, at startup, `Instant::now`
    /// so the first `EVERY` window starts from launch, not from `UNIX_EPOCH`).
    pub(crate) local_written_at: std::time::Instant,
    /// Personal-state entries for nodes `apply_local` found still spawning
    /// (no live node yet): applied once each one's node is inserted
    /// (`handle_tab_spawn_result`), then removed. The view fields are resolved once, in `apply_local`
    /// itself, since spawning is enough for those to take effect.
    pub(crate) pending_local: Option<HashMap<NodeId, LocalNode>>,
}

impl Default for LocalPersist {
    fn default() -> Self {
        Self {
            local_written: None,
            local_unread: None,
            local_written_at: std::time::Instant::now(),
            pending_local: None,
        }
    }
}

impl LocalState {
    /// This state with ids renamed by `map` (an import gave nodes fresh
    /// ids); ids `map` lacks are dropped.
    pub(crate) fn remap(&self, map: &HashMap<String, String>) -> LocalState {
        let id = |old: &Option<String>| old.as_ref().and_then(|o| map.get(o)).cloned();
        LocalState {
            focused: id(&self.focused),
            current_group: id(&self.current_group),
            viewed_image: id(&self.viewed_image),
            split_tab: id(&self.split_tab),
            camera: self
                .camera
                .clone()
                .filter(|(_, level)| level.is_none() || id(level).is_some())
                .map(|(r, level)| (r, id(&level))),
            nodes: self.nodes.iter().filter_map(|(k, v)| Some((map.get(k)?.clone(), v.clone()))).collect(),
            ..self.clone()
        }
    }

    fn all_finite(&self) -> bool {
        self.nodes.values().all(|n| n.pos.iter().all(|v| v.is_finite()))
            && self.camera.as_ref().is_none_or(|(r, _)| r.iter().all(|v| v.is_finite()))
            && self.json_outline_width.is_none_or(f32::is_finite)
    }

    /// serde_json writes NaN and infinity as `null`, which a plain `f32`
    /// can't read back: one bad value used to make the whole file unreadable,
    /// Recently Closed included. A non-finite position is zeroed, any other non-finite value dropped.
    pub(crate) fn to_bytes(&self) -> Vec<u8> {
        if self.all_finite() {
            return serde_json::to_vec(self).expect("personal state serializes");
        }
        let mut clean = self.clone();
        clean.nodes.values_mut().flat_map(|n| n.pos.iter_mut()).filter(|v| !v.is_finite()).for_each(|v| *v = 0.0);
        if clean.camera.as_ref().is_some_and(|(r, _)| r.iter().any(|v| !v.is_finite())) {
            clean.camera = None;
        }
        clean.json_outline_width = clean.json_outline_width.filter(|w| w.is_finite());
        serde_json::to_vec(&clean).expect("personal state serializes")
    }

    /// `None` for bytes that aren't this shape: treated as a fresh start.
    pub(crate) fn from_bytes(bytes: &[u8]) -> Option<LocalState> {
        serde_json::from_slice(bytes).ok()
    }
}

/// Personal state startup couldn't read (`App::local_persist`.`local_unread`): when
/// `poll_store` next tries, and whether startup was a fresh start (no
/// graph node), when nothing is applied from what it reads.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LocalUnread {
    pub(crate) retry_at: std::time::Instant,
    pub(crate) fresh: bool,
}

/// Personal state from a `read_local`: the defaults for no store, no
/// state, one that doesn't decode, or a read that failed (which the caller
/// checks for itself first).
pub(crate) fn local_from(read: Option<Result<Option<Vec<u8>>, JournalError>>) -> LocalState {
    match read {
        Some(Ok(Some(bytes))) => LocalState::from_bytes(&bytes).unwrap_or_default(),
        _ => LocalState::default(),
    }
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

impl App {
    /// The end of `restore_session`: personal state as read is what the
    /// file holds, so `maybe_write_local` skips an unchanged write; one
    /// that couldn't be read is retried instead, and never written over.
    pub(crate) fn arm_local(&mut self, unread: bool, fresh: bool) {
        if unread {
            self.doc.local_persist.local_written = None;
            self.doc.local_persist.local_unread =
                Some(LocalUnread { retry_at: std::time::Instant::now() + SAVE_RETRY, fresh });
        } else {
            self.doc.local_persist.local_written = Some(self.local_state());
            self.doc.local_persist.local_unread = None;
        }
    }

    /// Logs why startup couldn't read personal state, once (it's marked
    /// unread only there), unless another instance just held the lock:
    /// that is routine and the retry sees to it. Anything else (permission
    /// denied, say) may never clear, and personal state is then never
    /// written for this session. Names only the file, not its folder.
    pub(crate) fn log_local_unread(&self, err: &JournalError) {
        if matches!(err, JournalError::Io(io) if io.kind() == std::io::ErrorKind::WouldBlock) {
            return;
        }
        let file = self
            .doc
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
    /// applied as startup would have, and the next due write saves the result. Only a
    /// state that decodes is applied: none, or a damaged one, leaves the
    /// live view as the user has had it since startup. At exit nothing is
    /// woken (the read only feeds the write that follows).
    pub(crate) fn retry_read_local(&mut self, force: bool) {
        let Some(unread) = self.doc.local_persist.local_unread else { return };
        let Some(journal) = self.doc.journal.as_ref() else { return };
        let read = if force {
            journal.read_local()
        } else if std::time::Instant::now() >= unread.retry_at {
            journal.read_local_within(std::time::Duration::ZERO)
        } else {
            return;
        };
        if matches!(read, Err(ref err) if !matches!(err, JournalError::Corrupt(_))) {
            self.doc.local_persist.local_unread =
                Some(LocalUnread { retry_at: std::time::Instant::now() + SAVE_RETRY, ..unread });
            return;
        }
        self.doc.local_persist.local_unread = None;
        // Unknown: the file holds what was read, not this merge (or nothing).
        self.doc.local_persist.local_written = None;
        let Some(local) = read.ok().flatten().and_then(|bytes| LocalState::from_bytes(&bytes)) else { return };
        if unread.fresh {
            self.keep_hidden_local(local.nodes);
        } else {
            self.apply_local(local);
            if !force {
                self.wake_focused();
            }
        }
    }

    /// `id`'s personal state as the live graph shows it: its settled position, a tab's title, muted, zoom and
    /// unshared live url, a note's `editing`.
    pub(crate) fn local_node(&self, id: NodeId, node: &crate::GraphNode) -> LocalNode {
        let pos = self.layout_pos(id);
        let mut local = LocalNode { pos: [pos.x, pos.y], ..LocalNode::default() };
        if let Some(tab) = tab_common(&node.runtime) {
            local.title = tab.title.to_string();
            local.muted = tab.muted;
            local.zoom = (tab.zoom_factor != self.settings.zoom_factor).then_some(tab.zoom_factor);
            // Against the shared url the mirror holds (Ruling 9): a url this replica shares is edited (`edit_url`)
            // into it, so only an unshared move (a redirect, a script) is recorded.
            let from = self.doc.mirror.url(id);
            local.live_url = (from != tab.url).then(|| LiveUrl { url: tab.url.to_string(), from: from.to_owned() });
        } else if let Runtime::Note(n) = &node.runtime {
            local.editing = n.editing;
        }
        local
    }

    /// This profile's personal state as the live graph currently shows it
    /// (spec "Personal state"): what to write to `session.mnf.local`. A
    /// node still spawning keeps its `pending_local` entry, so a quit
    /// before restored tabs land doesn't lose their placement and zoom.
    pub(crate) fn local_state(&self) -> LocalState {
        let item = self.view_mode == ViewMode::Item;
        let mut nodes: BTreeMap<String, LocalNode> =
            self.nodes.iter().map(|(id, node)| (id.to_string(), self.local_node(*id, node))).collect();
        if let Some(pending) = &self.doc.local_persist.pending_local {
            for (id, node) in pending {
                let key = id.to_string();
                if self.spawns.spawning.contains_key(id) && !nodes.contains_key(&key) {
                    nodes.insert(key, node.clone());
                }
            }
        }
        // Hidden nodes keep theirs while the document still holds them (a Delete not yet seen is dropped).
        if let Some(doc) = self.doc.journal.as_ref().map(|j| j.doc()) {
            for (id, node) in &self.doc.hidden_local {
                if doc.nodes.get(id).is_some_and(Node::is_live) {
                    nodes.entry(id.to_string()).or_insert_with(|| node.clone());
                }
            }
        }
        let rect = self.camera.rect;
        let text = |id: Option<NodeId>| id.map(|id| id.to_string());
        LocalState {
            focused: text(self.focused),
            view_item: item,
            current_group: text(self.current_group),
            viewed_image: text(self.viewed_image.filter(|_| item)),
            split_tab: text(self.split_tab),
            groups_collapsed: self.groups_collapsed,
            camera: Some(([rect.min.x, rect.min.y, rect.max.x, rect.max.y], text(self.current_group))),
            nodes,
            json_accessor_paths: self.json_ui.json_accessor_paths,
            json_outline_hidden: !self.json_ui.json_outline_visible,
            json_outline_width: Some(self.json_ui.json_outline_width),
        }
    }

    /// Applies `state` (personal state, from `session.mnf.local` or an
    /// imported/opened file) on top of the just-restored/imported live
    /// graph. A view field naming a node takes hold only when that node
    /// exists, live or still spawning; otherwise the caller's own default
    /// stands. A node still spawning keeps its entry in `self.doc.local_persist.pending_local`
    /// for `handle_tab_spawn_result` to apply once it lands.
    pub(crate) fn apply_local(&mut self, state: LocalState) {
        let parsed = |id: &Option<String>| id.as_deref().and_then(|id| id.parse::<NodeId>().ok());
        let known = |id: &Option<String>| {
            parsed(id).filter(|id| self.nodes.contains_key(id) || self.spawns.spawning.contains_key(id))
        };
        if let Some(id) = known(&state.focused) {
            self.focused = Some(id);
        }
        // A level that is hidden now falls back to its nearest shown ancestor (spec decision 3).
        let level = match (self.doc.journal.as_ref(), parsed(&state.current_group)) {
            (Some(j), Some(g)) => j.doc().shown_level(g),
            (_, group) => group,
        };
        if let Some(level) = level.filter(|id| self.nodes.contains_key(id) || self.spawns.spawning.contains_key(id)) {
            self.current_group = Some(level);
        }
        if let Some(id) = known(&state.viewed_image) {
            self.viewed_image = Some(id);
        }
        if let Some(id) = known(&state.split_tab) {
            self.split_tab = Some(id);
        }
        self.view_mode = if state.view_item { ViewMode::Item } else { ViewMode::Graph };
        self.groups_collapsed = state.groups_collapsed;
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
            if let Some(id) = self.focused {
                if !self.nodes.get(&id).is_some_and(|n| n.as_tab().is_some()) {
                    self.spawns.pending_respawn_focus = Some(id);
                }
            }
        }

        let mut pending = HashMap::new();
        let mut leftover = Vec::new();
        for (id, node) in state.nodes {
            let Ok(id) = id.parse::<NodeId>() else { continue };
            if let Some(live) = self.nodes.get_mut(&id) {
                Self::apply_local_node(live, &node);
                self.snap_layout(id, egui::vec2(node.pos[0], node.pos[1]));
            } else if self.spawns.spawning.contains_key(&id) {
                pending.insert(id, node);
            } else {
                leftover.push((id, node));
            }
        }
        self.keep_hidden(leftover);
        if !pending.is_empty() {
            self.doc.local_persist.pending_local = Some(pending);
        }

        if let Some((rect, level)) = state.camera {
            if level == self.current_group.map(|g| g.to_string()) {
                self.camera.rect = egui::Rect::from_min_max(egui::pos2(rect[0], rect[1]), egui::pos2(rect[2], rect[3]));
                self.camera.frame_restored = false;
            }
        }
    }

    /// Personal-state entries for nodes the document holds but the graph doesn't show (closed, or under a closed
    /// node): kept in `hidden_local` for a reopen, not dropped as stale.
    pub(crate) fn keep_hidden_local(&mut self, entries: impl IntoIterator<Item = (String, LocalNode)>) {
        self.keep_hidden(entries.into_iter().filter_map(|(id, node)| Some((id.parse().ok()?, node))));
    }

    fn keep_hidden(&mut self, entries: impl IntoIterator<Item = (NodeId, LocalNode)>) {
        let Some(doc) = self.doc.journal.as_ref().map(|j| j.doc()) else { return };
        let held = |id: &NodeId| doc.nodes.get(id).is_some_and(Node::is_live);
        let here = |id: &NodeId| self.nodes.contains_key(id) || self.spawns.spawning.contains_key(id);
        let keep: Vec<(NodeId, LocalNode)> = entries.into_iter().filter(|(id, _)| held(id) && !here(id)).collect();
        self.doc.hidden_local.extend(keep);
    }

    /// One node's share of `apply_local` beyond its settled position (the caller's `snap_layout`): whatever its
    /// payload kind keeps (a tab's title/muted/zoom, a note's editing).
    pub(crate) fn apply_local_node(live: &mut crate::GraphNode, node: &LocalNode) {
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
    pub(crate) fn apply_pending_local(&mut self, id: NodeId) {
        let Some(pending) = &mut self.doc.local_persist.pending_local else { return };
        let Some(node) = pending.remove(&id) else { return };
        if pending.is_empty() {
            self.doc.local_persist.pending_local = None;
        }
        if let Some(live) = self.nodes.get_mut(&id) {
            if let Runtime::Tab(_) | Runtime::Unloaded(_) | Runtime::Note(_) = &live.runtime {
                Self::apply_local_node(live, &node)
            }
            self.snap_layout(id, egui::vec2(node.pos[0], node.pos[1]));
        }
    }

    /// Writes personal state when it changed (contract "Personal state"):
    /// at most every 30 s, and at exit (`force`). Never while the file is unread (`App::local_persist`.`local_unread`).
    pub(crate) fn maybe_write_local(&mut self, force: bool) {
        const EVERY: std::time::Duration = std::time::Duration::from_secs(30);
        let due = force || self.doc.local_persist.local_written_at.elapsed() >= EVERY;
        if !due || self.doc.journal.is_none() || self.doc.local_persist.local_unread.is_some() {
            return;
        }
        let state = self.local_state();
        self.doc.local_persist.local_written_at = std::time::Instant::now();
        if self.doc.local_persist.local_written.as_ref().is_some_and(|w| !crate::local_state::differs(w, &state)) {
            return;
        }
        match self.doc.journal.as_ref().unwrap().write_local(&state.to_bytes()) {
            Ok(()) => self.doc.local_persist.local_written = Some(state),
            Err(err) => self.store_error(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_pos(x: f32) -> LocalState {
        let node = LocalNode { pos: [x, 0.0], ..LocalNode::default() };
        LocalState { nodes: BTreeMap::from([("a".to_owned(), node)]), ..LocalState::default() }
    }

    #[test]
    fn jitter_under_a_pixel_is_not_a_change() {
        assert!(!differs(&with_pos(10.0), &with_pos(10.9)));
        assert!(differs(&with_pos(10.0), &with_pos(11.5)));
        assert!(differs(&with_pos(10.0), &LocalState::default()), "a node appearing is a change");
    }

    #[test]
    fn remap_renames_ids_and_drops_unknown_ones() {
        let mut state = with_pos(1.0);
        state.focused = Some("a".into());
        state.current_group = Some("gone".into());
        let map = HashMap::from([("a".to_owned(), "b".to_owned())]);
        let out = state.remap(&map);
        assert_eq!(out.focused.as_deref(), Some("b"));
        assert_eq!(out.current_group, None);
        assert!(out.nodes.contains_key("b"));
    }

    #[test]
    fn bytes_round_trip_and_garbage_is_a_fresh_start() {
        let state = with_pos(3.0);
        assert_eq!(LocalState::from_bytes(&state.to_bytes()), Some(state));
        assert_eq!(LocalState::from_bytes(b"\x00not json"), None);
    }

    #[test]
    fn a_non_finite_value_costs_only_itself() {
        let mut state = with_pos(f32::NAN);
        state.nodes.insert("b".into(), LocalNode { pos: [5.0, 6.0], title: "kept".into(), ..LocalNode::default() });
        state.camera = Some(([0.0, f32::INFINITY, 1.0, 1.0], None));
        state.json_outline_width = Some(f32::NAN);
        let back = LocalState::from_bytes(&state.to_bytes()).expect("still readable");
        assert_eq!(back.nodes["a"].pos, [0.0, 0.0], "the bad position is zeroed, the node kept");
        assert_eq!(back.nodes["b"].title, "kept");
        assert_eq!((back.camera, back.json_outline_width), (None, None));
    }

    #[test]
    fn a_non_finite_camera_is_no_change() {
        let mut state = with_pos(1.0);
        state.camera = Some(([0.0, f32::NAN, 1.0, 1.0], None));
        assert!(!differs(&state, &state.clone()), "it is never written, so it never differs");
    }

    /// Files written before the fix hold `null` where the value was NaN.
    #[test]
    fn nulls_written_by_older_builds_read_back() {
        let bytes = br#"{"camera":[[0.0,null,1.0,1.0],null],"nodes":{"a":{"pos":[null,2.0]}},"closed":[{"nodes":[]}]}"#;
        let state = LocalState::from_bytes(bytes).expect("lenient");
        assert_eq!(state.nodes["a"].pos, [0.0, 2.0]);
        assert_eq!(state.camera, None);
        assert_eq!(state.nodes.len(), 1, "an old file's Recently Closed list is ignored, not an error");
    }

    #[test]
    fn json_level_preferences_round_trip_and_default_off() {
        let state = LocalState {
            json_accessor_paths: true,
            json_outline_hidden: true,
            json_outline_width: Some(310.0),
            ..LocalState::default()
        };
        let back = LocalState::from_bytes(&state.to_bytes()).unwrap();
        assert_eq!(
            (back.json_accessor_paths, back.json_outline_hidden, back.json_outline_width),
            (true, true, Some(310.0))
        );
        let old = LocalState::from_bytes(br#"{}"#).unwrap();
        assert_eq!((old.json_accessor_paths, old.json_outline_hidden, old.json_outline_width), (false, false, None));
    }
}
