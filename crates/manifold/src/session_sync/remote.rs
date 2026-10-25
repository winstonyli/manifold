//! Showing the store in the live graph: `materialize` a node, `apply_remote` another replica's changes, and the
//! hide/remove/wake paths a change can take.

use super::*;
use crate::card_id::CardId;

impl App {
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
    /// A type mismatch between the live node and `ty` is ignored (the
    /// store never changes a node's type, so the mirror's first string stands). A file card whose file can't be
    /// loaded is kept, showing its error: the node stays in the store.
    ///
    /// `local`, personal state's entry for a new node, lays it out at its
    /// settled position instead of `fields`'s stored `pos` (which the mirror
    /// holds), and reopens a tab at its live url (`LocalNode::live_url`).
    /// Every caller but `restore_session` passes `None`.
    pub(crate) fn materialize(&mut self, id: NodeId, snap: &Snapshot, local: Option<&LocalNode>) {
        let (ty, fields, group) = (snap.ty.as_str(), &snap.fields, snap.group);
        let kind = Kind::from_type(ty);
        // A node or jar deleted here stays gone: a remote edit arriving before the save commits the queued delete
        // would otherwise re-show it, nameless from a partial change or whole (2b-5 Ruling 9, final review M1).
        if self.doc.writes.life(id) == Some(Life::Delete) {
            return;
        }
        // A link is stored and mirrored, and is never a graph node.
        if kind == Kind::Link {
            self.doc.mirror.add_link(id);
            self.queue_create(id);
            self.mirror_remote(id, fields, &Trees::new(), None);
            return;
        }
        if self.spawns.spawning.contains_key(&id) {
            self.spawns.remote_in_flight.insert(id, Remote::Shown(snap.clone()));
            return;
        }
        let (held, changed) = self.mirror_remote(id, fields, &snap.trees, group);
        // The store never changes a node's type, so the first one seen stands (2b-5 Ruling 10). Kept as written.
        if !ty.is_empty() {
            self.doc.mirror.init_field(id, field::TYPE, FieldValue::Str(ty.to_owned()));
        }
        let pos = local.map_or_else(|| pos_of(id, fields), |l| egui::vec2(l.pos[0], l.pos[1]));
        if self.update_live_node(id, fields, pos, &changed, &held) {
            return;
        }
        self.insert_materialized(id, kind, fields, local);
    }

    /// Applies `fields` to the live node `id` in the fields `mirror_remote` found changed (`changed`); the ones it
    /// held at a local edit (`held`) are left out: their live effects follow the mirror's verdict (2b-3 Ruling 3),
    /// so a tab whose url edit is pending is not navigated away from it, and a card whose pos edit is pending is not
    /// moved. A new pos snaps the layout to `pos`, at rest, except for a card being dragged: the drag keeps it under
    /// the pointer, and its release then wins (2b-4 Ruling 4). False when there is no such node.
    pub(super) fn update_live_node(
        &mut self,
        id: NodeId,
        fields: &Fields,
        pos: egui::Vec2,
        changed: &[u32],
        held: &[u32],
    ) -> bool {
        let in_flight = self.swap_in_flight(id);
        let changed = |no: u32| !held.contains(&no) && changed.contains(&no);
        if !self.nodes.contains_key(&id) {
            return false;
        }
        if changed(field::POS) && !self.is_being_dragged(CardId::from(id)) {
            self.snap_layout(id, pos);
        }
        // Only a jar has `ephemeral`. The mirror already holds the new flag (`mirror_remote`), so this applies its
        // effects without comparing (`after_jar_ephemeral_changed`).
        if changed(field::EPHEMERAL) {
            let on = matches!(fields.get(&field::EPHEMERAL), Some(FieldValue::Bool(true)));
            self.after_jar_ephemeral_changed(id, on);
        }
        let Some(node) = self.nodes.get_mut(&id) else {
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
            // A group's fields (it has no runtime), a jar's, an unknown-type card's (all generic), a content node's
            // url and a Json node's view are the mirror's alone (`mirror_remote`; the level shown is relaid out
            // below). Paths, keys and values never change after creation.
            Runtime::None
            | Runtime::Jar
            | Runtime::Unknown
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
            self.spawns.skip_convert.insert(id, false);
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
    pub(crate) fn begin_content_pass(&mut self) -> bool {
        if self.content_index.is_some() {
            return false;
        }
        self.content_index = Some(crate::ContentIndex::scan(crate::profile::content_dir().as_deref()));
        true
    }

    pub(crate) fn end_content_pass(&mut self, began: bool) {
        if began {
            self.content_index = None;
        }
    }

    /// Creates the node `id` of `kind` from `fields` (a file card whose file is gone gets a placeholder).
    pub(super) fn insert_materialized(&mut self, id: NodeId, kind: Kind, fields: &Fields, local: Option<&LocalNode>) {
        let pos = local.map_or_else(|| pos_of(id, fields), |l| egui::vec2(l.pos[0], l.pos[1]));
        // Resolved once up front for the arms below and the missing-file
        // refetch check: this machine's newest `<id>.*` in the content dir,
        // else `stored` (`ContentIndex`): the pass's own index when one is
        // running (`content_index`), so a restore reads the dir once.
        let content_path = matches!(kind, Kind::Image | Kind::Pdf | Kind::Json).then(|| {
            let stored = str_of(fields, field::PATH);
            match &self.content_index {
                Some(index) => index.resolve(id, &stored),
                None => crate::ContentIndex::scan(crate::profile::content_dir().as_deref()).resolve(id, &stored),
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
        // The URL test goes first: it is free, and `exists` can wait on a network share.
        let missing_with_url =
            url_of(fields).is_some() && content_path.as_deref().is_some_and(|p| !std::path::Path::new(p).exists());
        match kind {
            Kind::Tab => {
                let shared = str_of(fields, field::URL);
                let live = local.and_then(|l| l.live_url.as_ref()).filter(|l| l.from == shared);
                let url = live.map_or(shared, |l| l.url.clone());
                // Title, mute and zoom come from personal state (`apply_local`); the engine is the mirror's.
                let zoom_factor = self.settings.zoom_factor;
                let (title, muted, state) = (String::new(), false, UnloadedState::Sleeping);
                let payload = UnloadedTabPayload { url, title, muted, zoom_factor, state };
                self.insert_placeholder_node(id, pos, kind, Runtime::Unloaded(payload));
            }
            Kind::Group => {
                // Its name, `from_json` and default jar are already the mirror's (`mirror_remote`); it has no runtime.
                self.insert_placeholder_node(id, pos, kind, Runtime::None);
            }
            Kind::Note => {
                let payload = NotePayload { text: str_of(fields, field::TEXT), editing: false };
                self.insert_placeholder_node(id, pos, kind, Runtime::Note(payload));
            }
            Kind::Image => {
                let path = content_path.clone().unwrap_or_default();
                if missing_with_url {
                    let payload = ImagePayload { path, width: 0, height: 0, error: None };
                    self.insert_placeholder_node(id, pos, kind, Runtime::Image(payload));
                } else {
                    self.insert_image_node_with_id(id, path, pos, None);
                }
            }
            Kind::Pdf => {
                let path = content_path.clone().unwrap_or_default();
                if missing_with_url {
                    let payload = PdfPayload { path, outcome: None };
                    self.insert_placeholder_node(id, pos, kind, Runtime::Pdf(payload));
                } else {
                    self.insert_pdf_node_with_id(id, path, pos, None);
                }
            }
            Kind::Json => {
                let path = content_path.clone().unwrap_or_default();
                let error = if missing_with_url {
                    None
                } else {
                    self.insert_json_node_with_id(id, path.clone(), pos, None).err()
                };
                if missing_with_url || error.is_some() {
                    let payload = JsonPayload { path, value: Default::default(), error };
                    self.insert_placeholder_node(id, pos, kind, Runtime::Json(payload));
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
                self.insert_placeholder_node(id, pos, kind, Runtime::Download(payload));
            }
            Kind::Jar => {
                self.insert_placeholder_node(id, pos, kind, Runtime::Jar);
                // A jar first seen already ephemeral (made on another device, or restored) has no file of its own:
                // a leftover one goes, as a toggle on would delete it (`update_live_node` does that for a change).
                if let Some(FieldValue::Bool(true)) = fields.get(&field::EPHEMERAL) {
                    self.after_jar_ephemeral_changed(id, true);
                }
            }
            Kind::Unknown => self.insert_placeholder_node(id, pos, kind, Runtime::Unknown),
            Kind::Link => return,
        }
        // A content node's url, its stored path and a Json node's view are already the mirror's (`mirror_remote`).
        // The placeholder just inserted above (see `missing_with_url`)
        // shows until this fetch lands.
        if missing_with_url {
            self.refresh_node(id);
        }
    }

    pub(super) fn remote_changes(doc: &Doc, remote: &[NodeId]) -> Vec<RemoteChange> {
        remote
            .iter()
            .map(|&id| match doc.nodes.get(&id).filter(|n| n.is_live()) {
                None => (id, Remote::Deleted),
                Some(n) if doc.is_shown(id) => (id, Remote::Shown(Snapshot::of(doc, id, n))),
                Some(_) => (id, Remote::Hidden),
            })
            .collect()
    }

    /// Shows or hides `touched` and everything under them to match the document, for the nodes whose shown state
    /// differs from the graph's: the local half of what `apply_remote` does for other replicas' changes. A local close
    /// already hid its subtree (`close_node`), so this only finds what a reopen or a repair brought back.
    pub(super) fn apply_visibility(&mut self, touched: BTreeSet<NodeId>) {
        let Some(doc) = self.doc.journal.as_ref().map(|j| j.doc()) else { return };
        if touched.is_empty() {
            return;
        }
        let changes: Vec<RemoteChange> = doc
            .with_descendants(touched)
            .into_iter()
            .filter_map(|id| {
                let node = doc.nodes.get(&id).filter(|n| n.is_live() && n.kind().card())?;
                // `here` is the graph's: a tab still spawning or waking has no `nodes` entry (or a hidden one's was
                // dropped), and its landing reads the parked change (`remote_in_flight`), which a show replaces.
                let here = self.nodes.contains_key(&id);
                match (doc.is_shown(id), here) {
                    (true, false) => Some((id, Remote::Shown(Snapshot::of(doc, id, node)))),
                    (false, true) => Some((id, Remote::Hidden)),
                    _ => None,
                }
            })
            .collect();
        if !changes.is_empty() {
            self.apply_remote(changes);
        }
    }

    /// Shows other replicas' changes, before the next commit (contract
    /// "Missing means deleted"). A deleted node is removed without a
    /// document entry of its own and without queueing a delete of its own
    /// (`remove_remotely_deleted`: the document already lacks it; it does queue the deletes of the links naming
    /// the node), and a
    /// tab's engine host is shut down. Groups are materialized before the
    /// nodes inside them, so a new member never names a group that doesn't
    /// exist yet. A new tab arrives asleep (`materialize`, spec "Lazy
    /// tabs"): another replica opening a big session mustn't start an
    /// engine here for every tab in it. A hidden node (closed, or under a closed node) leaves the graph through
    /// `hide_node`; a cleared one comes back through `materialize` with its kept personal state.
    pub(crate) fn apply_remote(&mut self, mut changes: Vec<RemoteChange>) {
        changes.sort_by_key(|(_, c)| match c {
            Remote::Shown(s) if s.kind() == Kind::Group => 0,
            _ => 1,
        });
        let toasts = self.toasts.save_toast_queue.len();
        let had_default = self.default_jar().is_some();
        let pass = self.begin_content_pass();
        for (id, change) in changes {
            match change {
                Remote::Shown(s) => {
                    // A node coming back from hidden takes the personal state it left with (spec decision 8).
                    let local = self.doc.hidden_local.remove(&id);
                    self.materialize(id, &s, local.as_ref());
                    if let (Some(local), Some(live)) = (&local, self.nodes.get_mut(&id)) {
                        Self::apply_local_node(live, local);
                    }
                }
                Remote::Hidden => {
                    self.leave_hidden(id);
                    self.hide_node(id);
                }
                Remote::Deleted if self.spawns.spawning.contains_key(&id) => {
                    self.spawns.remote_in_flight.insert(id, Remote::Deleted);
                }
                Remote::Deleted => self.remove_remotely_deleted(id),
            }
        }
        self.end_content_pass(pass);
        self.collapse_toasts_since(toasts);
        // Two Defaults arriving from replicas' concurrent first launches converge, and a remote delete of the Default
        // is repaired. Only once this replica has had a Default, so a sync test that never made one gets no surprise card.
        if had_default && self.ensure_default_jar() {
            self.save_session();
        }
        // A remote jar edit follows the same table (spec "Live tabs when `jar` changes"); once the whole batch is
        // in, so a tab is judged by where the batch leaves it.
        self.sweep_jar_switches();
        // Closes past the cap arriving from other replicas are evicted here too (spec decision 6).
        self.evict_closed();
    }

    /// Removes a node another replica deleted, recording nothing and saving
    /// nothing (but see the links below). Its children move to its parent. A group's members move
    /// to the group's own group (`take_out`, which also leaves the
    /// level if it was showing): a member the store still has is in
    /// `remote` too (it lists descendants) and gets its effective group
    /// from there, and one only this replica has lands where the store
    /// would show it. Either way no live node names a removed group.
    pub(crate) fn remove_remotely_deleted(&mut self, id: NodeId) {
        // The links naming a node another replica deleted go with it. Unlike the node, a link may still be live in the
        // document (the deleting replica did not know it, or an older build left it), and a link forgotten here
        // while live would fail `check_graph_matches_document`: so these deletes are queued (`delete_link`).
        for link in self.links_naming(id) {
            self.delete_link(link);
        }
        if self.doc.mirror.is_link(id) {
            self.doc.mirror.forget_link(id);
            return;
        }
        if !self.nodes.contains_key(&id) {
            return self.forget_deleted(id);
        }
        self.take_out(id, crate::node_store::Removal::Remote);
        self.spawns.wakes_in_flight.remove(&id);
        self.release_backing(id);
    }

    /// Hides `id` (spec decision 3): out of `nodes`, `node_order`, the layout and the mirror, its engine unloaded
    /// (`unshow`), its personal state kept in `hidden_local` for a reopen, and its trail kept in `nav_history`. Nothing
    /// in the document changes: no re-parent, no link delete, no backing-file release. A tab whose spawn is in flight
    /// (in `spawning`: a respawn, or a wake, whose placeholder is dropped here) is parked as `Remote::Hidden` and
    /// hidden when it lands (plan D3). A wake stops being one (`wakes_in_flight`), so its landing is an ordinary spawn
    /// that applies the parked change: a reopen before then replaces the `Hidden` with a `Shown` (`materialize`), and
    /// the tab shows when it lands.
    pub(crate) fn hide_node(&mut self, id: NodeId) {
        if self.spawns.spawning.contains_key(&id) {
            self.spawns.remote_in_flight.insert(id.to_owned(), Remote::Hidden);
            self.spawns.wakes_in_flight.remove(&id);
        }
        let Some(node) = self.nodes.get(&id) else { return };
        // A note comes back idle: the box it was typed in is gone, and its text was committed.
        let local = LocalNode { editing: false, ..self.local_node(id, node) };
        // A closed tab keeps its trail (trail spec decision 10); `unshow` -> `forget_id` would drop it.
        let trail = self.nav_history.remove(&id);
        self.unshow(id);
        self.doc.mirror.forget_node(id);
        self.layout.remove(&id);
        self.doc.hidden_local.insert(id.to_owned(), local);
        if let Some(mut trail) = trail {
            trail.set_left_to(None);
            self.nav_history.insert(id.to_owned(), trail);
        }
    }

    /// The current level leaves a subtree that is being hidden: when it is `root` or inside it, the level falls to
    /// the nearest shown node above `root` (`Doc::shown_level`), as `take_out` leaves a removed group.
    /// Reads the raw chain, so it holds before the close is committed (a local close) and after (a remote one).
    pub(crate) fn leave_hidden(&mut self, root: NodeId) {
        let Some(doc) = self.doc.journal.as_ref().map(|j| j.doc()) else { return };
        let Some(level) = self.current_group else { return };
        if level != root && !doc.is_ancestor(root, level) {
            return;
        }
        let to = doc.nodes.get(&root).and_then(|n| n.group).and_then(|g| doc.shown_level(g));
        self.switch_group_level(to);
    }

    /// A node the document deleted that this replica no longer shows (an evicted hidden subtree, local or remote):
    /// its backing files go once nothing holds them, and so do its per-id tables and kept personal state.
    pub(crate) fn forget_deleted(&mut self, id: NodeId) {
        self.release_backing(id);
        self.forget_id(id);
    }

    /// Applies what other replicas changed while `id`'s spawn was in flight,
    /// now that its node exists (`handle_tab_spawn_result`).
    pub(crate) fn land_remote_in_flight(&mut self, id: NodeId, remote: Option<Remote<Snapshot>>) {
        match remote {
            Some(Remote::Shown(snap)) => self.materialize(id, &snap, None),
            Some(Remote::Hidden) => {
                self.leave_hidden(id);
                self.hide_node(id);
            }
            Some(Remote::Deleted) => self.remove_remotely_deleted(id),
            None => {}
        }
    }

    /// Tab `id` is loading a url the host gave it (another replica's `url`,
    /// a restore, or a typed url, which `navigate_tab` shares itself): its
    /// unlabelled url changes stay local until the load settles
    /// (`shares_url_change`).
    pub(crate) fn note_host_load(&mut self, id: NodeId) {
        self.spawns.host_loads.insert(id, std::time::Instant::now());
    }

    /// Starts the engines a restored session starts (spec "Lazy tabs"): the
    /// focused tab's, and the saved split tab's too when the session was
    /// saved split -- otherwise `apply_local` restores `split_tab` onto a
    /// card that never wakes, leaving the split pane empty forever.
    pub(crate) fn wake_focused(&mut self) {
        let asleep = |id: NodeId, this: &Self| {
            let card = this.nodes.get(&id).and_then(|n| n.as_unloaded());
            card.is_some_and(|t| t.state == UnloadedState::Sleeping).then(|| this.doc.mirror.view_kind(id))
        };
        if let Some(id) = self.focused {
            if let Some(kind) = asleep(id, self) {
                self.wake_tab(id, kind);
            }
        }
        if let Some(id) = self.split_tab {
            if let Some(kind) = asleep(id, self) {
                self.wake_tab(id, kind);
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
    pub(crate) fn shares_url_change(&mut self, id: NodeId, user_started: Option<bool>) -> bool {
        match user_started {
            Some(true) => {
                self.spawns.host_loads.remove(&id);
                return true;
            }
            Some(false) => return false,
            None => {}
        }
        let Some(since) = self.spawns.host_loads.get_mut(&id) else { return true };
        if self.nodes.get(&id).and_then(|n| n.as_tab()).is_some_and(|t| t.loading) {
            *since = std::time::Instant::now();
            return false;
        }
        if since.elapsed() < HOST_LOAD_GRACE {
            return false;
        }
        self.spawns.host_loads.remove(&id);
        true
    }

    /// Shows another replica's values of `fields` (the node's type's fields, Ruling 11) and its effective `group`,
    /// except where this replica has an edit waiting for a save: it commits next and wins (last writer, per field or
    /// link). Returns `(held, changed)`: the fields held back that way, whose live effects `update_live_node` then
    /// skips, and the fields whose value differs from what the mirror showed (Ruling 9), whose live effects it
    /// applies. A note's text is held too while the note is typed in, and remembered in `held_text` for the blur
    /// (2b-4 Ruling 7). The links (`parent`, `group`) are set but reported in neither list.
    pub(super) fn mirror_remote(
        &mut self,
        id: NodeId,
        fields: &Fields,
        trees: &Trees,
        group: Option<NodeId>,
    ) -> (Vec<u32>, Vec<u32>) {
        let (mut held, mut changed) = (Vec::new(), Vec::new());
        for (&no, value) in fields {
            if no == field::PARENT {
                continue;
            }
            if self.doc.writes.field_queued(id, no) {
                held.push(no);
                continue;
            }
            if no == field::TEXT && self.is_typed_in(id) {
                held.push(no);
                match value {
                    FieldValue::Str(theirs) if theirs != self.doc.mirror.text(id) => {
                        self.doc.writes.held_text.insert(id.to_owned(), theirs.clone());
                    }
                    // Back to what the mirror shows: nothing left to apply.
                    _ => {
                        self.doc.writes.held_text.remove(&id);
                    }
                }
                continue;
            }
            if self.doc.mirror.get(id, no) != *value {
                changed.push(no);
                self.doc.mirror.set(id, no, value.clone());
            }
        }
        // A tree is replaced by the document's, then the edits still waiting for a save go back on top: they commit
        // next and win, and everything else the other replica changed shows now.
        for &(no, _) in &TREE_FIELDS {
            let mut tree = trees.get(&no).cloned().unwrap_or_default();
            for (path, enc) in self.doc.writes.tree_edits(id, no) {
                tree::edit(&mut tree, path, Leaf::decode(enc).ok().flatten());
            }
            if self.doc.mirror.set_tree(id, no, tree) {
                changed.push(no);
            }
        }
        if !self.doc.writes.field_queued(id, field::PARENT) {
            self.doc.mirror.set_parent(id, id_of(fields, field::PARENT));
        }
        if !self.doc.writes.move_queued(id) {
            self.doc.mirror.set_group(id, group);
        }
        (held, changed)
    }
}
