//! Node storage: id allocation, opening the session store, and removing or forgetting nodes.

use super::*;

/// The ids of nodes made while there is no journal (unit tests, a failed open): this launch's own actor and a counter,
/// so they parse as store ids.
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
    fn next_id(&mut self) -> String {
        let id = store::model::NodeId { actor: self.actor, counter: self.counter };
        self.counter += 1;
        id.to_string()
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

impl App {
    /// A fresh node id: the journal's counter (contract "Actors and ids"),
    /// or this launch's fallback actor without one.
    pub(crate) fn alloc_node_id(&mut self) -> String {
        match &mut self.doc.journal {
            Some(journal) => journal.next_id().to_string(),
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

    /// The local removal of any node (`take_out`, `Removal::Local`): a group, a tab, a download, note, image, PDF or
    /// Json node. Saves nothing; the caller does.
    pub(crate) fn remove_node(&mut self, id: &str) {
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
    pub(crate) fn take_out(&mut self, id: &str, how: Removal) {
        let local = how == Removal::Local;
        let present = self.nodes.contains_key(id);
        let (parent, group) =
            (self.doc.mirror.parent(id).map(str::to_owned), self.doc.mirror.group(id).map(str::to_owned));
        let up = group.filter(|_| present);
        let children: Vec<String> = self.children_of(id).map(str::to_owned).collect();
        for child in children {
            if local {
                self.edit_parent(&child, parent.clone());
            } else {
                self.follow_remote_parent(&child, parent.clone());
            }
        }
        let members: Vec<String> =
            if self.is_container(id) { self.members_of(id).map(str::to_owned).collect() } else { Vec::new() };
        for member in &members {
            if local {
                self.edit_group(member, up.clone());
            } else {
                self.follow_remote_group(member, up.clone());
            }
        }
        if !local && self.is_container(id) {
            for (_, _, group) in
                self.spawns.remote_in_flight.values_mut().filter_map(crate::session_sync::Remote::shown_mut)
            {
                if group.as_deref() == Some(id) {
                    *group = up.clone();
                }
            }
        }
        let removed = self.unshow(id).is_some();
        if self.current_group.as_deref() == Some(id) {
            self.switch_group_level(up.filter(|_| removed));
        }
        if local {
            self.queue_delete(id);
            for member in &members {
                self.recheck_edge_scope(member);
            }
        }
    }

    /// Takes `id` out of the live graph without touching the document: its `nodes` entry (a tab's `ViewTab` drops,
    /// shutting its engine host down), a PDF's queued render, its `node_order` slot, the view state naming it
    /// (`focused`, `split_tab`, `viewed_image`, an open find bar), then `forget_id`. Shared by the delete paths
    /// (`take_out`) and the hide path (`hide_node`), which add what each needs. Returns the removed node.
    pub(crate) fn unshow(&mut self, id: &str) -> Option<crate::GraphNode> {
        let removed = self.nodes.remove(id);
        if removed.as_ref().is_some_and(|n| n.as_pdf().is_some()) {
            self.pdf_renderer.cancel(id);
        }
        self.node_order.retain(|d| d != id);
        // A split only makes sense paired with its focused tab (`focus_tab`): losing either half clears it.
        if self.split_tab.as_deref() == Some(id) || self.focused.as_deref() == Some(id) {
            self.split_tab = None;
        }
        if self.focused.as_deref() == Some(id) {
            self.focused = None;
            // Only a showing Tab view drops to Graph: while an Image is maximized `focused` is the hidden last tab.
            if self.showing_tab_view() {
                self.view_mode = ViewMode::Graph;
            }
            // The tab is gone: nothing to send `FindStop` to, so only the UI side of the close.
            self.find.close();
        }
        if self.viewed_image.as_deref() == Some(id) {
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
    /// latches naming it, its GPU textures. Called by every permanent removal (`take_out`,
    /// `merge_groups`, `teardown_running_session`); `main.rs`'s
    /// `every_id_keyed_field_is_forgotten_or_excused` fails if an id-holding App field is in neither this nor its
    /// `NOT_FORGOTTEN` list. Deliberately not here:
    /// - `wakes_in_flight`: a tab closed while waking needs it, so the landing commits its delete;
    /// - `spawning`, `remote_in_flight`, `host_loads`: the spawn lifecycle owns them;
    /// - the `pending*` save queues: the save's `editable` filter drops a deleted node's ops;
    /// - `layout`, `held_text`: pruned to live nodes at every save.
    ///
    /// Never call it for a crash respawn or an engine swap: those are temporary, and keep `tab_crashes`,
    /// `nav_history` and `saved_scroll`.
    pub(crate) fn forget_id(&mut self, id: &str) {
        self.spawns.forget(id);
        // A hide re-inserts its entry after this (plan D5); every permanent removal drops it.
        self.doc.hidden_local.remove(id);
        self.pending_sleeps.remove(id);
        self.clear_cookie_state(id);
        self.saved_scroll.remove(id);
        self.nav_history.remove(id);
        self.page_tab_gate.forget(id);
        if let Ok(jar) = id.parse::<crate::store::model::NodeId>() {
            self.forget_jar_memory(jar);
        }
        if self.naming_jar.as_ref().is_some_and(|(t, _)| t == id) {
            self.naming_jar = None;
        }
        if self.renaming_group.as_ref().is_some_and(|(g, _)| g == id) {
            self.renaming_group = None;
        }
        self.menus.node_menu.forget(id);
        self.interaction.forget(id);
        self.forget_connect_source(id);
        if let Some(gpu) = &mut self.gpu {
            gpu.free_tab_thumbnail(id);
            gpu.free_tab_texture(id);
            gpu.free_image(id);
        }
    }

    /// A WebKit-sourced download path is a WSL2-side path (WebKit runs
    /// inside WSL2 — see project docs), not reachable from this Windows
    /// process as a plain path, so it's translated to the
    /// `\\wsl.localhost\<distro>\...` UNC form both Explorer and a pasted
    /// clipboard path understand, using the same "Ubuntu" distro name
    /// `process_manager` already hardcodes for spawning the WebKit
    /// view-host. Used by `node_file_path`.
    pub(crate) fn windows_path_for_download(&self, id: &str) -> Option<String> {
        let download = self.nodes.get(id)?.as_download()?;
        // Off Windows a download's path is already native (WebKit is not run through WSL there).
        if cfg!(not(windows)) {
            return Some(download.path.clone());
        }
        Some(if let Some(posix) = download.path.strip_prefix('/') {
            format!("\\\\wsl.localhost\\Ubuntu\\{}", posix.replace('/', "\\"))
        } else {
            download.path.clone()
        })
    }
}
