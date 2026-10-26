//! Settings and named-session persistence, and the first-launch demo, of `App`.

use super::*;
use std::collections::BTreeSet;

use crate::local_state::LocalState;
use crate::session_sync::{imported_fields, Snapshot};
use crate::store::json::{Imported, JsonDoc};
use crate::store::model::{Kind, NodeId};

impl App {
    /// Writes `settings.json`, unless another process (another
    /// `manifold.exe` on this profile) changed it since this one last read
    /// or wrote it: then the write goes to `settings::conflict_path()` and a
    /// toast says so, rather than clobbering theirs. The first save of a run
    /// (`settings_fingerprint` still `None`) always proceeds. Replaces every
    /// direct `self.settings.save()` call site. It writes the window
    /// geometry too, so a debounced save still waiting is done by it.
    pub(crate) fn save_settings(&mut self) {
        self.settings_save_due = None;
        let on_disk = settings::fingerprint();
        if self.settings_fingerprint.is_some() && on_disk != self.settings_fingerprint {
            if let Some(path) = settings::conflict_path() {
                self.settings.save_to(&path);
                eprintln!(
                    "settings: settings.json changed on disk since last load/save -- wrote {} instead",
                    path.display()
                );
                self.queue_toast(format!(
                    "Settings changed by another process -- your changes saved to {}",
                    path.display()
                ));
            }
            return;
        }
        self.settings.save();
        self.settings_fingerprint = settings::fingerprint();
    }

    /// Settings changed (window size or position): written once things
    /// are quiet for `SETTINGS_SAVE_DELAY`, or at exit.
    pub(crate) fn note_settings_changed(&mut self) {
        self.settings_save_due = Some(std::time::Instant::now() + SETTINGS_SAVE_DELAY);
    }

    /// Writes settings if a change is due by `now`. Returns whether it wrote.
    pub(crate) fn flush_settings_if_due(&mut self, now: std::time::Instant) -> bool {
        match self.settings_save_due {
            Some(due) if due <= now => {
                self.settings_save_due = None;
                self.save_settings();
                true
            }
            _ => false,
        }
    }

    /// At exit: writes a debounced settings change still waiting.
    pub(crate) fn flush_pending_settings(&mut self) {
        if self.settings_save_due.is_some() {
            self.save_settings();
        }
    }

    /// `session::named_session_path`, toasting why there is none: "Not a session name" for a name that can't be a
    /// file (`session_name_is_safe`; a device name such as `CON` survives `sanitize_session_name`), "No session
    /// file" without a profile.
    fn named_session_path_or_toast(&mut self, name: &str) -> Option<std::path::PathBuf> {
        let path = session::named_session_path(name);
        if path.is_none() {
            self.queue_toast(if session::session_name_is_safe(name) {
                "No session file".into()
            } else {
                let shown = crate::names::to_plain(&crate::names::display_name(name, crate::names::UNTITLED));
                format!("Not a session name: {shown}")
            });
        }
        path
    }

    /// "Save Session As..." (settings popup) / headless `SAVESESSIONAS` --
    /// a full compacted copy of the running store (spec "Named sessions"),
    /// with this profile's own personal state (`local_state_for_copy`), saved to `sessions/<name>
    /// .mnf`. Saves the running session first so the copy isn't missing
    /// recent edits; a failure there or in the copy itself is toasted, not
    /// fatal (the running session is untouched either way). `raw` is sanitised here (`sanitize_session_name`), for
    /// Settings and the headless command alike; nothing left of it saves nothing.
    pub(crate) fn save_named_session(&mut self, raw: &str) {
        let name = sanitize_session_name(raw);
        if name.is_empty() {
            return;
        }
        // The Settings page lists saves and sizes: read them again next frame, whatever happens below.
        crate::settings_page::forget_disk_cache(&self.egui_ctx);
        let Some(path) = self.named_session_path_or_toast(&name) else { return };
        if !self.save_session() {
            self.queue_toast("The saved copy may miss recent edits: the session couldn't be saved first".into());
        }
        let local = self.local_state_for_copy();
        let Some(journal) = self.doc.journal.as_ref() else { return };
        if let Err(err) = journal.save_copy(&path, local.as_deref()) {
            let shown = crate::names::to_plain(&crate::names::display_name(&name, crate::names::UNTITLED));
            self.queue_toast(format!("Couldn't save as {shown}: {err}"));
        }
    }

    /// The personal state Save As gives its copy: this profile's, or none while startup's is still unread
    /// (`App::local_persist`.`local_unread`). The live view then lacks what the file holds (its view state, say); a copy
    /// with none opens with defaults rather than a partial state passed off as this profile's.
    pub(crate) fn local_state_for_copy(&self) -> Option<Vec<u8>> {
        self.doc.local_persist.local_unread.is_none().then(|| self.local_state().to_bytes())
    }

    /// "Open Session" (settings popup) / headless `OPENSESSION` -- reads a
    /// named save's store file and personal state, then replaces the
    /// running session with it via `replace_running_session` (same path
    /// Import Session uses). A read error, including one from a file
    /// written by a newer manifold (`JournalError::ReadOnly` -- `Journal::
    /// read` still reads it, just says so in its `Display`), is toasted
    /// and leaves the running session untouched.
    pub(crate) fn load_named_session(&mut self, name: &str) {
        let shown = crate::names::to_plain(&crate::names::display_name(name, crate::names::UNTITLED));
        let Some(path) = self.named_session_path_or_toast(name) else { return };
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) => {
                self.queue_toast(format!("Couldn't open {shown}: {err}"));
                return;
            }
        };
        let doc = match store::journal::Journal::read(&bytes) {
            Ok(doc) => doc,
            Err(err) => {
                self.queue_toast(format!("Couldn't open {shown}: {err}"));
                return;
            }
        };
        let local = store::local::read(&path, doc.id)
            .ok()
            .flatten()
            .and_then(|bytes| local_state::LocalState::from_bytes(&bytes));
        let mut json = store::json::doc_to_json(&doc);
        json.local = local.map(|l| serde_json::to_value(l).expect("personal state serializes"));
        self.replace_running_session(&json, store::json::Trust::Own);
    }

    /// The one-time first-launch experience for a brand-new install -- see
    /// `Settings::has_shown_first_launch_demo`'s own doc comment for
    /// exactly when this fires instead of `spawn_root_tab`. A small,
    /// hand-arranged graph -- fixed positions, not `graph_layout::
    /// spawn_jitter`, unlike every other multi-node spawn in this file --
    /// since this is the one place a deliberately curated first impression
    /// matters more than avoiding a few lines of manual layout. Fully
    /// network-independent: every tab points at a `manifold://` page (see
    /// that scheme's own design doc), so it renders identically offline.
    /// `SpawnPurpose::Restore` for every tab, with `self.focused` set
    /// before any of them are issued, mirrors exactly how `apply_session`
    /// starts a real saved session -- landing in Graph view with nothing
    /// stealing focus from the intended main tab, not `Root`'s own "jump
    /// straight to Tab view" behavior, which would hide the very layout
    /// this exists to show off.
    ///
    /// Every engine serves `manifold://` (the `manifold-pages` crate), so the tabs use the default starting engine.
    pub(crate) fn spawn_first_launch_demo(&mut self) {
        let kind = self.settings.default_starting_view_kind;
        let zoom = self.settings.zoom_factor;

        let welcome_id = self.alloc_node_id();
        self.focused = Some(welcome_id);
        self.view_mode = ViewMode::Graph;
        let welcome = "manifold://welcome".to_string();
        self.spawn_tab_async(TabSpawn::new(welcome_id, welcome, kind, egui::Vec2::ZERO, SpawnPurpose::Restore, zoom));

        let note = Runtime::Note(NotePayload {
            text: "This is a Note -- double-click to edit it, drag it anywhere, or right-click to delete it \
                           (undo via \"Recently Closed\" if you change your mind)."
                .to_string(),
            editing: false,
        });
        self.place_node(None, store::model::Kind::Note, note, Placement { group: None, pos: egui::vec2(-300.0, 0.0) });

        let group_pos = egui::vec2(300.0, 0.0);
        let group_id = self.place_group(None, "Try grouping", Placement { group: None, pos: group_pos });
        // A small fixed offset, not `graph_layout::spawn_jitter` -- its
        // radius is calibrated to spread a new tab away from a busy live
        // layout (see its own doc comment), which is far too large for
        // "nestle just barely off-center inside a two-member group" and
        // would occasionally land a member tab embarrassingly far from the
        // group it belongs to. These positions are only ever visible if
        // this group is later entered/ungrouped, but they should still
        // look deliberate then, not scattered.
        for (url, offset) in
            [("manifold://welcome", egui::vec2(-40.0, 40.0)), ("manifold://tips", egui::vec2(40.0, 40.0))]
        {
            let id = self.alloc_node_id();
            let pos = group_pos + offset;
            let spawn = TabSpawn::new(id, url.to_string(), kind, pos, SpawnPurpose::Restore, zoom);
            self.spawn_tab_async(TabSpawn { group: Some(group_id), ..spawn });
        }

        // Deliberately left as a plain card, not entered -- the welcome
        // page's own "Try it" section already tells the user to
        // double-click this exact kind of node themselves, so entering its
        // level here would rob them of the discovery the copy is pointing
        // at. Synthetic, not file-backed (`path` is a display-only label, not a
        // real path).
        let json = Runtime::Json(JsonPayload::file(
            "(demo).json".to_string(),
            std::sync::Arc::new(serde_json::json!({
                "project": "Manifold",
                "tags": ["browser", "graph-ui"],
                "version": {"major": 0, "minor": 1}
            })),
            None,
        ));
        self.place_node(None, store::model::Kind::Json, json, Placement { group: None, pos: egui::vec2(0.0, -260.0) });
    }
}

/// Where `App::materialize_imported` puts an imported file's root (its
/// first node): the level it's brought into, and optionally a fresh
/// position there.
pub(crate) struct ImportRoot {
    pub group: Option<NodeId>,
    pub pos: Option<egui::Vec2>,
}

impl App {
    /// The session as a JSON snapshot (Export Session), saved first so it
    /// holds every edit; if that save didn't commit, it exports anyway and
    /// warns. Carries this profile's personal state as `local` (spec
    /// "Personal state"), same as a named session's own `.mnf.local`.
    /// `None` without a store.
    pub(crate) fn export_session_json(&mut self) -> Option<JsonDoc> {
        self.doc.journal.as_ref()?;
        if !self.save_session() {
            self.queue_toast("The export may miss recent edits: the session couldn't be saved first".into());
        }
        let mut json = crate::store::json::doc_to_json(self.doc.journal.as_ref()?.doc());
        json.local = Some(serde_json::to_value(self.local_state()).expect("personal state serializes"));
        Some(json)
    }

    /// A group's own subtree as a JSON snapshot (Export Group; spec "JSON
    /// snapshots"), root first (`store::json::subtree_to_json`'s own
    /// contract), saved first like `export_session_json`. Only *saved*
    /// nodes are exported. `None` without a store or for an
    /// unknown group; toasted when the store doesn't hold the group live
    /// (its save failed), rather than exporting an empty file.
    pub(crate) fn export_group_json(&mut self, root: NodeId) -> Option<JsonDoc> {
        self.doc.journal.as_ref()?;
        if !self.save_session() {
            self.queue_toast("The export may miss recent edits: the session couldn't be saved first".into());
        }
        let doc = self.doc.journal.as_ref()?.doc();
        if !doc.is_shown(root) {
            self.queue_toast("Couldn't export the group: it isn't shown in the session".into());
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
    /// lands with the root (`materialize_imported`). A label in the file whose name key matches a live label is not
    /// created: its cards carry the live one (`import_with`). Ends with
    /// `save_session`, same as `replace_running_session`.
    pub(crate) fn import_group_json(&mut self, json: &JsonDoc) {
        self.leave_json_level();
        let live = self.label_keys();
        let reuse = |node: &crate::store::json::JsonNode| {
            if node.kind() != Some(Kind::Label) {
                return None;
            }
            let key = crate::names::name_key(node.fields.get("name")?.as_str()?);
            live.get(&key).copied()
        };
        let next_id = || self.alloc_node_id();
        let imported = match crate::store::json::import_with(json, next_id, crate::store::json::Trust::Foreign, reuse) {
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
        let pos = crate::graph_layout::GRAPH_WORLD_CENTER + crate::graph_layout::spawn_jitter(root);
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
        // A node `import_with` matched to an existing one (a label with a live twin) is already in the document.
        imported.retain(|i| !i.reused);
        // A file's Default jar is not placed: this profile has its own, and every `jar` edge naming the file's is
        // re-pointed at it (spec "The Default jar").
        let file_defaults: Vec<NodeId> = imported
            .iter()
            .filter(|i| i.kind() == Kind::Jar && i.fields.get(&field::IS_DEFAULT) == Some(&FieldValue::Bool(true)))
            .map(|i| i.id)
            .collect();
        imported.retain(|i| !file_defaults.contains(&i.id));
        let running = self.default_jar();
        for i in &mut imported {
            if matches!(i.fields.get(&field::JAR), Some(FieldValue::Id(Some(j))) if file_defaults.contains(j)) {
                i.fields.insert(field::JAR, FieldValue::Id(running));
            }
        }
        let groups: BTreeSet<NodeId> = imported.iter().filter(|i| i.kind() == Kind::Group).map(|i| i.id).collect();
        let ids: BTreeSet<NodeId> = imported.iter().map(|i| i.id).collect();
        let root_id = root.as_ref().and(imported.first()).map(|i| i.id);
        let (dest, root_pos) = root.map_or((None, None), |r| (r.group, r.pos));
        imported.sort_by_key(|i| match i.kind() {
            Kind::Group => 1,
            Kind::Link | Kind::Label => 3, // after the cards they name or join
            _ => 2,
        });
        let toasts = self.toasts.save_toast_queue.len();
        let pass = self.begin_content_pass();
        for node in imported {
            // A link whose end did not resolve, or names no card in this batch or live, has nothing to join.
            if crate::store::json::is_half_made_link(&node, |n| ids.contains(&n) || self.nodes.contains_key(&n)) {
                continue;
            }
            let is_root = Some(node.id) == root_id;
            let group = match node.fields.get(&field::GROUP) {
                Some(FieldValue::Id(Some(g))) if !is_root && groups.contains(g) => Some(*g),
                _ => dest,
            };
            let mut fields = imported_fields(node.kind(), &node.fields);
            if let Some(FieldValue::Id(parent)) = fields.get_mut(&field::PARENT) {
                let live = |p: &NodeId| keep_live_parents && self.nodes.contains_key(p);
                *parent = parent.filter(|p| ids.contains(p) || live(p));
            }
            if let (true, Some(pos)) = (is_root, root_pos) {
                fields.insert(field::POS, FieldValue::Pos(pos.x, pos.y));
            }
            let snap = Snapshot { ty: node.ty.clone(), group, fields, trees: node.trees.clone() };
            self.materialize(node.id, &snap, None);
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

    /// After a session import with fresh ids: each jar whose old id is no longer a live node gets a copy of its cookie
    /// file under its new id, so an imported jar comes back signed in. A copy, not a move: a named session may still
    /// name the old id, and the startup prune removes the old file once nothing holds it. `imported[i]` is
    /// `doc.nodes[i]` (`store::json::import`).
    pub(crate) fn rebind_jar_files(&self, doc: &JsonDoc, imported: &[crate::store::json::Imported]) {
        let Some(dir) = self.jars_dir() else { return };
        for (old, new) in doc.nodes.iter().zip(imported) {
            if new.kind() != Kind::Jar || old.id.parse().is_ok_and(|id: NodeId| self.nodes.contains_key(&id)) {
                continue;
            }
            let (from, to) = (dir.join(format!("{}.json", old.id)), crate::jar::Jar::path(&dir, new.id));
            if from.exists() && !to.exists() {
                if let Err(e) = std::fs::copy(&from, &to) {
                    eprintln!("jar: could not carry jar {} over to {}: {e}", old.id, new.id);
                }
            }
        }
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
    ) -> HashMap<String, NodeId> {
        let next_id = || self.alloc_node_id();
        let imported = match crate::store::json::import(json, next_id, trust) {
            Ok(imported) => imported,
            Err(err) => {
                self.queue_toast(format!("Couldn't import the session: {err}"));
                return HashMap::new();
            }
        };
        if !imported.iter().any(|i| i.kind() == Kind::Tab) {
            self.queue_toast("The session has no tabs".into());
            return HashMap::new();
        }
        self.teardown_running_session();
        self.rebind_jar_files(json, &imported);
        let map: HashMap<String, NodeId> =
            json.nodes.iter().zip(&imported).map(|(node, i)| (node.id.clone(), i.id)).collect();
        let first_tab = imported.iter().find(|i| i.kind() == Kind::Tab).map(|i| i.id);
        // No root: a group or parent outside the file (import keeps real
        // node ids) names nothing once the old session is gone.
        self.materialize_imported(imported, None, false);
        self.ensure_default_jar();
        self.focused = first_tab;
        self.view_mode = ViewMode::Graph;
        self.camera.frame_restored = true;
        if let Some(state) =
            json.local.clone().and_then(|value| serde_json::from_value::<LocalState<String>>(value).ok())
        {
            self.apply_local(state.remap(&map));
        }
        // Personal state startup couldn't read is the old session's: its view state names nodes that are gone.
        if let Some(unread) = self.doc.local_persist.local_unread.as_mut() {
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
    pub(crate) fn teardown_running_session(&mut self) {
        for (id, _) in self.nodes.iter().filter(|(_, node)| node.as_pdf().is_some()) {
            self.pdf_renderer.cancel(*id);
        }
        // The Default jar is the profile's own: the replaced session's cards go, it stays (spec "The Default jar").
        let default = self.default_jar();
        let old: Vec<NodeId> = self
            .doc
            .journal
            .as_ref()
            .map(|j| j.doc().live().map(|(id, _)| id).filter(|&id| Some(id) != default).collect())
            .unwrap_or_default();
        for id in old {
            self.queue_delete(id);
        }
        // A link is not a card, so no endpoint's cascade is certain to reach it (a dangling one has none): the
        // replaced session's links all go, as its cards do.
        for link in self.doc.mirror.link_ids().collect::<Vec<_>>() {
            self.delete_link(link);
        }
        // The old session's nodes not created yet are not created at all.
        self.doc.writes.retain_creates(|id| Some(id) == default);
        // Each old node's per-id state and GPU textures (`forget_id`), including tabs whose spawn is still in flight
        // (no `nodes` entry yet, but cookie and popup state already). The session-wide clears below stay.
        let old_ids: Vec<NodeId> =
            self.nodes.keys().chain(self.spawns.spawning.keys()).filter(|id| Some(**id) != default).copied().collect();
        for id in old_ids {
            self.forget_id(id);
        }
        self.begin_view_shutdown(); // the tabs below drop together: one SHUTDOWN_FLUSH wait, not one each
        self.nodes.retain(|id, _| Some(*id) == default);
        self.node_order.retain(|id| Some(*id) == default);
        self.layout.retain(|id, _| Some(*id) == default);
        self.doc.writes.held_text.clear();
        self.doc.hidden_local.clear();
        // A closed tab's trail is stashed under an id not in `nodes`, so the loop above does not forget it.
        self.nav_history.clear();
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
        self.doc.local_persist.pending_local = None;
    }

    /// Collapses the toasts queued since `mark` into one, so a bulk restore
    /// or import with many missing files toasts once, not once per card.
    pub(crate) fn collapse_toasts_since(&mut self, mark: usize) {
        let extra = self.toasts.save_toast_queue.len().saturating_sub(mark + 1);
        if extra > 0 {
            let first = self.toasts.save_toast_queue.drain(mark..).next().expect("toasts past the mark");
            self.toasts.save_toast_queue.push_back(format!("{first} (and {extra} more)"));
        }
    }
}
