//! Node storage: id allocation, opening the session store, and removing or forgetting nodes.

use super::*;

impl App {
    /// A fresh node id: the journal's counter (contract "Actors and ids"),
    /// or this launch's fallback actor without one.
    pub(crate) fn alloc_node_id(&mut self) -> String {
        match &mut self.journal {
            Some(journal) => journal.next_id().to_string(),
            None => {
                let (actor, counter) = &mut self.fallback_ids;
                let id = store::model::NodeId { actor: *actor, counter: *counter };
                *counter += 1;
                id.to_string()
            }
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
                self.journal = Some(journal);
                if let Some(aside) = aside {
                    // The new, empty session names none of the old one's fetched content or jars: pruning
                    // against it this launch would delete what the set-aside file still needs.
                    self.store_recovered = true;
                    self.queue_toast(format!(
                        "The session file wasn't readable (damaged, or from an older version of manifold); it was kept \
                         (with its .prev/.local) as {} and a new one started",
                        aside.display()
                    ));
                }
            }
            Err(err) => self.queue_toast(format!("Couldn't open the session file {}: {err}", path.display())),
        }
    }

    /// The local removal of a download, note, image, PDF or Json node: `drop_node` + `queue_delete`.
    pub(crate) fn remove_node(&mut self, id: &str) {
        self.drop_node(id);
        self.queue_delete(id);
    }

    /// Removes a download, note, image, PDF or Json node from the graph without queueing a delete (`remove_node`
    /// adds that; a node another replica deleted, `remove_remotely_deleted`, needs none). A download's file on disk
    /// is left untouched (this only forgets the node); an image's or PDF's texture is freed, and a PDF's render not
    /// yet started is cancelled. Neither kind has an engine process or a membership to tear down, so one plain
    /// `self.nodes`/`node_order` removal (plus the same stale-`dragging`/`hovered` guard `destroy_tab`/`finish_swap`
    /// also apply), then `forget_id`, covers them all.
    ///
    /// Removing the Json node whose level is being viewed (another replica
    /// deleted it, or its group was closed) leaves to that node's group,
    /// as `dissolve_group` does for a group: `current_group` must never
    /// name a node that's gone, or `insertion_group` would orphan new nodes.
    ///
    /// What it opened (a tab from Open as Tab) is then opened from its own
    /// `parent`, as `drop_tab` does for a tab's children.
    pub(crate) fn drop_node(&mut self, id: &str) {
        let parent = self.mirror.parent(id).map(str::to_owned);
        let children: Vec<String> = self.children_of(id).map(str::to_owned).collect();
        for child in children {
            self.edit_parent(&child, parent.clone());
        }
        let removed = self.nodes.remove(id);
        if removed.as_ref().is_some_and(|n| n.as_pdf().is_some()) {
            self.pdf_renderer.cancel(id);
        }
        if self.current_group.as_deref() == Some(id) {
            self.switch_group_level(self.mirror.group(id).filter(|_| removed.is_some()).map(str::to_owned));
        }
        self.node_order.retain(|d| d != id);
        // The texture of an image or PDF (`GpuState::images`) goes here too.
        self.forget_id(id);
    }

    /// Everything a permanently removed node leaves behind that carries no order: per-id tables, the gestures and
    /// latches naming it, its GPU textures. Called by every permanent removal (`drop_tab`, `drop_node`,
    /// `dissolve_members`, `merge_groups`, `teardown_running_session`); `main.rs`'s
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
        self.skip_convert.remove(id);
        self.pending_sleeps.remove(id);
        self.clear_cookie_state(id);
        self.saved_scroll.remove(id);
        self.nav_history.remove(id);
        self.navigated_from.remove(id);
        self.popup_gate.forget(id);
        self.tab_crashes.remove(id);
        if self.naming_jar.as_ref().is_some_and(|(t, _)| t == id) {
            self.naming_jar = None;
        }
        if self.renaming_group.as_ref().is_some_and(|(g, _)| g == id) {
            self.renaming_group = None;
        }
        if self.node_menu.as_ref().is_some_and(|(n, _)| n == id) {
            self.node_menu = None;
        }
        if self.pending_respawn_focus.as_deref() == Some(id) {
            self.pending_respawn_focus = None;
        }
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
    /// engine-host. Used by `node_file_path`.
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
