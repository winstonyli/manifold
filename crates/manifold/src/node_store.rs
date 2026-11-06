//! Node storage: id allocation, opening the session store, placing nodes, and removing or forgetting them.

use super::*;
use crate::store::model::{Kind, NodeId};

/// The ids of nodes made while there is no journal (unit tests, a failed open): this launch's own actor and a counter,
/// so they are store ids.
pub(crate) struct NodeIdFallback {
    actor: store::model::ActorId,
    counter: u64,
}

impl Default for NodeIdFallback {
    fn default() -> Self {
        Self { actor: store::model::ActorId::random(), counter: 0 }
    }
}

impl NodeIdFallback {
    fn next_id(&mut self) -> store::model::NodeId {
        let id = store::model::NodeId { actor: self.actor, counter: self.counter };
        self.counter += 1;
        id
    }
}

/// Whose removal `App::take_out` carries out.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Removal {
    /// This replica's own: it edits the document.
    Local,
    /// Following another replica's delete, which the document already holds.
    Remote,
}

/// App state keyed by a node id that goes when the node does (`NodeScoped::forget`, from `App::forget_id`).
#[derive(Default)]
pub(crate) struct NodeScoped {
    /// Sleep requests awaiting the engine's `PageState` answer, by tab id.
    pub(crate) pending_sleeps: HashMap<NodeId, tabs::sleep::state::PendingSleep>,
    /// Scroll positions of woken tabs, restored once their first load finishes.
    pub(crate) saved_scroll: HashMap<NodeId, tabs::sleep::state::SavedScroll>,
    /// Each tab's trail (`nav_history`): its pages as a tree, kept across an engine replacement, a close and a restart.
    pub(crate) nav_history: HashMap<NodeId, trail::history::NavHistory>,
    /// The view each level was left at (`None` the top level), restored on the way back and written to personal
    /// state (`LocalState::levels`). The level in view lives in `camera` and `groups_collapsed` (`live_level_view`),
    /// so its entry here, if any, is stale until the next switch saves it.
    pub(crate) level_views: BTreeMap<Option<NodeId>, local_state::LevelView>,
    /// `(tab_id, draft_name)` while a tab's menu shows "New Jar..."'s name field in place of its items, the way
    /// `renaming_group` does for a group. Committing creates the jar and picks it for the tab (`NodeAction::NewJar`).
    pub(crate) naming_jar: Option<(NodeId, String)>,
    /// `(group_id, draft_name)` once renaming has started -- either the
    /// toolbar's group-name button (only shown while `current_group` is
    /// `Some`) or the top-level right-click context menu's own "Rename"
    /// (see `node_menu`) sets this the same way. A real
    /// `egui::TextEdit` replaces whichever plain button/menu row started
    /// it until Enter commits or Escape cancels. Transient UI state, not
    /// the group's name (`Mirror::name`).
    pub(crate) renaming_group: Option<(NodeId, String)>,
}

impl NodeScoped {
    /// Drops `id`, a node removed for good, from every table and draft naming it.
    pub(crate) fn forget(&mut self, id: NodeId) {
        let Self { pending_sleeps, saved_scroll, nav_history, level_views, naming_jar, renaming_group } = self;
        level_views.remove(&Some(id));
        pending_sleeps.remove(&id);
        saved_scroll.remove(&id);
        nav_history.remove(&id);
        if naming_jar.as_ref().is_some_and(|(t, _)| *t == id) {
            *naming_jar = None;
        }
        if renaming_group.as_ref().is_some_and(|(g, _)| *g == id) {
            *renaming_group = None;
        }
    }
}

/// Where a new node lands (spec `2026-09-30-place-node-design.md`).
pub(crate) struct Placement {
    pub(crate) group: Option<NodeId>,
    pub(crate) pos: egui::Vec2,
}

impl App {
    /// A fresh node id: the journal's counter (contract "Actors and ids"),
    /// or this launch's fallback actor without one.
    pub(crate) fn alloc_node_id(&mut self) -> NodeId {
        match &mut self.doc.journal {
            Some(journal) => journal.next_id(),
            None => self.doc.fallback_ids.next_id(),
        }
    }

    /// Opens `session.mnf` under a fresh actor. A file that isn't a session file this build reads (not one at all,
    /// or an older format) is set aside (`Journal::open_or_set_aside`) and a new one started, toasting where the
    /// old one went; that launch then counts as recovered (`store_recovered`), so nothing is pruned against the
    /// new, empty session. Any other failure is toasted and leaves the app running without a store.
    pub(crate) fn open_store(&mut self) {
        if let Some(path) = session::store_path() {
            self.open_store_at(&path);
        }
    }

    /// `open_store` for the store at `path`.
    pub(crate) fn open_store_at(&mut self, path: &std::path::Path) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match store::journal::Journal::open_or_set_aside(path, store::model::ActorId::random()) {
            Ok((journal, aside)) => {
                self.doc.journal = Some(journal);
                if let Some(aside) = aside {
                    // The new, empty session names none of the old one's fetched content or jars: pruning
                    // against it this launch would delete what the set-aside file still needs.
                    self.doc.store_recovered = true;
                    self.queue_toast(format!(
                        "The session file wasn't readable (damaged, or from an older version of manifold); it was kept \
                         (with its .prev/.local, if any) as {} and a new one started",
                        aside.display()
                    ));
                }
            }
            Err(err) => self.queue_toast(format!("Couldn't open the session file {}: {err}", path.display())),
        }
    }

    /// Where a new node `id` goes: `current_group` at `pos`, or at a
    /// jittered world-center spawn when `pos` is `None`. A JSON level is
    /// left first, so the node lands in the level the user then sees.
    pub(crate) fn placement(&mut self, id: NodeId, pos: Option<egui::Vec2>) -> Placement {
        self.leave_json_level();
        let pos = pos.unwrap_or_else(|| graph_ui::layout::GRAPH_WORLD_CENTER + graph_ui::layout::spawn_jitter(id));
        Placement { group: self.current_group, pos }
    }

    /// Inserts a node of type `kind` (colourless: colour is the mirror's, by id) showing `runtime` at `at` under `id`
    /// (allocated when `None`) and appends it to `node_order`. Its card is drawn at `at.pos` (`App::layout`).
    /// `at.group`, `at.pos`, a note's text, its type (`Mirror::ty`, from `kind`) and a file card's path seed the
    /// mirror only for an id with no entry yet (`Mirror::seed`; the type also lands on an entry that lacks one): a
    /// new node's document pos is its placement, which its `Create` carries, while a node shown again
    /// (a placeholder for a stored node, a reload) keeps its group, document pos, text and stored path.
    /// A content node's web source is seeded by its caller (`insert_content`, `insert_download_node`).
    /// Doesn't save: the caller does, as `apply_graph_action` does once per action.
    pub(crate) fn place_node(&mut self, id: Option<NodeId>, kind: Kind, runtime: Runtime, at: Placement) -> NodeId {
        let id = id.unwrap_or_else(|| self.alloc_node_id());
        // A tab's shared url starts as the one it is shown at. A stored tab's entry already holds
        // the document's (`materialize` runs `mirror_remote` first), so a live url it reopens at isn't taken for it.
        // A live tab's engine is the one it runs; an unloaded card has none of its own (`Mirror::engine`).
        let url = runtime.saved_tab().map(str::to_owned).unwrap_or_default();
        let view_kind = runtime.as_tab().map_or(0, |t| crate::session_sync::view_code(t.tab.kind));
        // A new note's text starts as what it was made with (its payload, the edit buffer).
        let text = runtime.as_note().map(|n| n.text.clone()).unwrap_or_default();
        let ty = kind.type_str().unwrap_or_default().to_owned();
        let path = runtime.file_path().unwrap_or_default().to_owned();
        self.nodes.insert(id, GraphNode { runtime });
        // The one way a node enters `nodes`: the next save creates it if the document lacks it.
        self.queue_create(id);
        self.snap_layout(id, at.pos);
        let carried = crate::mirror::Carried::default()
            .in_group(at.group)
            .with(field::POS, crate::session_sync::pos_value(at.pos))
            .with_str(field::URL, url)
            .with_str(field::TEXT, text)
            .with(field::VIEW_KIND, FieldValue::ViewCode(view_kind))
            .with_str(field::TYPE, ty)
            .with_str(field::PATH, path);
        self.doc.mirror.seed(id, &carried);
        self.node_order.push(id);
        id
    }

    /// `place_node` for a group named `name` (a group has no runtime). Like a note's text, the name seeds the mirror
    /// (`Mirror::name`) only for an id with no entry yet: a new group's `Create` carries it, and a stored group shown
    /// again keeps the document's (`materialize` runs `mirror_remote` first). Doesn't save.
    pub(crate) fn place_group(&mut self, id: Option<NodeId>, name: &str, at: Placement) -> NodeId {
        let id = id.unwrap_or_else(|| self.alloc_node_id());
        let fresh = !self.doc.mirror.has(id);
        self.place_node(Some(id), Kind::Group, Runtime::None, at);
        if fresh && !name.is_empty() {
            self.doc.mirror.set(id, crate::store::model::field::NAME, FieldValue::Str(name.to_owned()));
        }
        id
    }

    /// A download node for `path`, started from tab `tab_id`: placed next
    /// to that tab, in the tab's group, with an edge from it. Shared by an
    /// engine's `DownloadStarted` and the headless `ADDDOWNLOAD <path>`
    /// debug command (downloads otherwise only ever come from a real
    /// engine download, so headless runs had no way to get one). A no-op
    /// if `tab_id` is gone. The store keeps only completed downloads
    /// (`DownloadCompleted` saves), so no save.
    ///
    /// A download `tab_id` already has in flight for `path` restarts
    /// instead: the engine starts a download whose transfer broke again
    /// under the same path (two downloads in flight never share one, the
    /// host numbers the second's file), from 0 or from the bytes it kept,
    /// which a `DownloadProgress` right after says. The WebView2 host keeps
    /// the path either way (`retry_path` in its `download.rs`).
    pub(crate) fn insert_download_node(
        &mut self,
        tab_id: NodeId,
        path: String,
        total_bytes: Option<u64>,
        url: Option<String>,
    ) {
        let restarted = self.tab_download(tab_id, &path).and_then(|id| self.nodes.get_mut(&id));
        if let Some(dl) = restarted.and_then(|n| n.as_download_mut()) {
            dl.bytes_received = 0;
            dl.total_bytes = total_bytes;
            return;
        }
        let node_group = self.doc.mirror.group(tab_id);
        if !self.nodes.contains_key(&tab_id) {
            return;
        }
        let node_pos = self.layout_pos(tab_id);
        let id = self.alloc_node_id();
        let pos = node_pos + graph_ui::layout::spawn_jitter(id);
        let payload = Runtime::Download(DownloadPayload::started(path, total_bytes));
        self.place_node(Some(id), Kind::Download, payload, Placement { group: node_group, pos });
        self.seed_source_url(id, url);
        self.edit_parent(id, Some(tab_id)); // the edge from the tab it came from
    }

    /// The in-flight download node an engine's progress, failure or
    /// completion for `path` is about: the one `tab_id` started if there is
    /// one, else the newest. A finished node with the same path (a restored
    /// one, or an earlier download of the same file) is never matched.
    pub(crate) fn in_flight_download_id(&self, tab_id: NodeId, path: &str) -> Option<NodeId> {
        self.tab_download(tab_id, path)
            .or_else(|| self.node_order.iter().rev().copied().find(|&id| self.in_flight_download(id, path)))
    }

    /// The in-flight download node for `path` that `tab_id` started: at most one (`insert_download_node`).
    fn tab_download(&self, tab_id: NodeId, path: &str) -> Option<NodeId> {
        self.node_order
            .iter()
            .copied()
            .find(|&id| self.opened_from(id) == Some(tab_id) && self.in_flight_download(id, path))
    }

    fn in_flight_download(&self, id: NodeId, path: &str) -> bool {
        self.nodes.get(&id).and_then(|n| n.as_download()).is_some_and(|d| d.path == path && d.in_flight())
    }

    /// Inserts a node of type `kind` showing `runtime` at `pos`, top level, no colour of its own (colour is the
    /// mirror's, by id): `materialize`'s way in for kinds with no insert helper of their own,
    /// and for an image/JSON card whose file couldn't be loaded (see
    /// `ImagePayload::error`).
    pub(crate) fn insert_placeholder_node(&mut self, id: NodeId, pos: egui::Vec2, kind: Kind, runtime: Runtime) {
        self.place_node(Some(id), kind, runtime, Placement { group: None, pos });
    }

    /// `insert_placeholder_node` for an Image, Pdf or Json card whose file at `path` isn't loaded: not yet fetched
    /// (`error` `None`) or failed with `error`.
    pub(crate) fn insert_file_placeholder(
        &mut self,
        id: NodeId,
        pos: egui::Vec2,
        kind: Kind,
        path: String,
        error: Option<String>,
    ) {
        let runtime = match kind {
            Kind::Image => Runtime::Image(ImagePayload { path, width: 0, height: 0, error }),
            Kind::Pdf => Runtime::Pdf(PdfPayload { path, outcome: error.map(Err) }),
            Kind::Json => Runtime::Json(JsonPayload::file(path, Default::default(), error)),
            _ => unreachable!("a file placeholder is an Image, Pdf or Json card"),
        };
        self.insert_placeholder_node(id, pos, kind, runtime);
    }

    /// The local removal of any node (`take_out`, `Removal::Local`): a group, a tab, a download, note, image, PDF or
    /// Json node. Saves nothing; the caller does.
    pub(crate) fn remove_node(&mut self, id: NodeId) {
        self.take_out(id, Removal::Local);
    }

    /// The one removal of a node from the graph. `Removal::Local` edits the document (the re-parents, a group's
    /// members moving up, the queued delete); `Removal::Remote` follows another replica's delete, which the document
    /// already holds, so it only moves the mirror. In order:
    /// - what the node opened (a tab from Open as Tab) is opened from its own `parent`, so the forest stays
    ///   connected;
    /// - a group's members move to the group's own group, and the level leaves it if it was showing: no live node
    ///   names a removed group (`current_group` must never name a node that is gone, or `insertion_group` would
    ///   orphan new nodes). A remote delete also fixes a member change still waiting for a spawn;
    /// - `unshow` takes it out of the live graph (a tab's engine shuts down, an image's or PDF's texture is freed, a
    ///   PDF's render not yet started is cancelled; a download's file on disk is left alone);
    /// - local: the delete is queued, and a member whose `jar` edge now reaches nothing it did before loses it
    ///   (`recheck_edge_scope`; a remote device's own recheck arrives as ordinary tab edits, spec "Inheritance").
    pub(crate) fn take_out(&mut self, id: NodeId, how: Removal) {
        let local = how == Removal::Local;
        let present = self.nodes.contains_key(&id);
        let (parent, group) = (self.doc.mirror.parent(id), self.doc.mirror.group(id));
        let up = group.filter(|_| present);
        let children: Vec<NodeId> = self.children_of(id).collect();
        for child in children {
            if local {
                self.edit_parent(child, parent);
            } else {
                self.follow_remote_parent(child, parent);
            }
        }
        let members: Vec<NodeId> = if self.is_container(id) { self.members_of(id).collect() } else { Vec::new() };
        for &member in &members {
            if local {
                self.edit_group(member, up);
            } else {
                self.follow_remote_group(member, up);
            }
        }
        if !local && self.is_container(id) {
            for snap in self.spawns.remote_in_flight.values_mut().filter_map(crate::session_sync::Remote::shown_mut) {
                if snap.group == Some(id) {
                    snap.group = up;
                }
            }
        }
        let removed = self.unshow(id).is_some();
        if self.current_group == Some(id) {
            self.switch_group_level(up.filter(|_| removed));
        }
        if local {
            self.queue_delete(id);
            for &member in &members {
                self.recheck_edge_scope(member);
            }
        }
    }

    /// Takes `id` out of the live graph without touching the document: its `nodes` entry (a tab's `ViewTab` drops,
    /// shutting its view host down), its image decode or PDF render (`Workers::cancel_node`), its `node_order` slot,
    /// the view state naming it (`focused`, `split_tab`, `viewed_image`, an open find bar), then `forget_id`. Shared
    /// by the delete paths (`take_out`) and the hide path (`hide_node`), which add what each needs. Returns the
    /// removed node.
    pub(crate) fn unshow(&mut self, id: NodeId) -> Option<crate::GraphNode> {
        let removed = self.nodes.remove(&id);
        self.workers.cancel_node(id);
        self.node_order.retain(|d| *d != id);
        // A split only makes sense paired with its focused tab (`focus_tab`): losing either half clears it.
        if self.split_tab == Some(id) || self.focused == Some(id) {
            self.split_tab = None;
        }
        if self.focused == Some(id) {
            self.focused = None;
            // Only a showing Tab view drops to Graph: while an Image is maximized `focused` is the hidden last tab.
            if self.showing_tab_view() {
                self.view_mode = ViewMode::Graph;
            }
            // The tab is gone: nothing to send `FindStop` to, so only the UI side of the close.
            self.find.close();
        }
        if self.viewed_image == Some(id) {
            self.viewed_image = None;
            if self.view_mode == ViewMode::Item {
                self.view_mode = ViewMode::Graph;
            }
        }
        // The texture of an image or PDF (`GpuState::images`) goes with `forget_id`.
        self.forget_id(id);
        removed
    }

    /// Everything a permanently removed node leaves behind that carries no order: per-id tables, the gestures and
    /// latches naming it, its GPU textures. Called by every permanent removal: `take_out` (through `unshow`,
    /// node_store.rs), `teardown_running_session` and `forget_deleted`. `hide_node` runs it too (through `unshow`) and
    /// then re-inserts its `hidden_local` entry. Each group that owns id-keyed state forgets it in a `forget` that
    /// destructures `Self` exhaustively (`SpawnTracker`, `CookieState`, `GraphInteraction`, `NodeMenu`, `PageTabGate`,
    /// `LinkGesture`, `Doc`), so a new field fails to compile until it is handled or excused there;
    /// `every_id_field_of_app_is_forgotten_or_excused` (`main/tests.rs`) covers `App`'s own fields. Deliberately not
    /// here:
    /// - `undo` (`UndoHistory`): its entries keep their ids so Reopen works;
    /// - `layout`, `node_order`, `focused`, `split_tab`, `viewed_image`, `current_group`: see `EXCUSED` in that test.
    ///
    /// Never call it for a crash respawn or an engine swap: those are temporary, and keep `tab_crashes`,
    /// `nav_history` and `saved_scroll`.
    pub(crate) fn forget_id(&mut self, id: NodeId) {
        self.spawns.forget(id);
        self.doc.forget(id);
        self.scoped.forget(id);
        // `scoped` goes first: its `pending_sleeps` is gone, so the cookie hold dropped here awaits nothing.
        self.cookies.forget(id);
        self.page_tab_gate.forget(id);
        self.forget_jar_memory(id);
        self.menus.node_menu.forget(id);
        self.interaction.forget(CardId::from(id));
        self.link.forget(id);
        if let Some(gpu) = &mut self.gpu {
            gpu.forget(id);
        }
    }
}
