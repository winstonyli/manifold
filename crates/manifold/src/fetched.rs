//! Fetched and file-backed nodes: content keep/prune, refresh, inserting content nodes, file drops.

use super::*;
use crate::store::model::Kind;

/// Prunes `dir` of files neither `keep` nor any named session in
/// `sessions` names. `None`, deleting nothing, when a session can't be
/// read whole (`session::content_names`).
pub(crate) fn prune_unnamed(
    dir: &std::path::Path,
    snapshot: std::time::SystemTime,
    mut keep: HashSet<String>,
    sessions: Option<&std::path::Path>,
) -> Option<usize> {
    if let Some(sessions) = sessions {
        keep.extend(session::content_names(sessions)?);
    }
    Some(content::prune(dir, snapshot, |file| content::kept(&keep, file)))
}

/// Whether a Refresh of node `id` that saved `new` may delete the file it
/// replaced, `old`: only its own `<id>.<ext>` in `dir`, and only when
/// nothing else names it (`others`: `App::content_keep` without `id`).
pub(crate) fn replaced_file_unused(
    dir: &std::path::Path,
    id: &str,
    old: &std::path::Path,
    new: &std::path::Path,
    others: &HashSet<String>,
) -> bool {
    old != new
        && content::owned_content_file(dir, id, old)
        && old.file_name().is_some_and(|name| !content::kept(others, &name.to_string_lossy()))
}

/// A tab's `App::skip_convert` entry after a navigation report: settled
/// once a report says `loading == false`, and dropped by a url change
/// after that. A url change before it (an HTTP redirect) keeps it.
pub(crate) fn skip_convert_after(entry: Option<bool>, url_changed: bool, loading: bool) -> Option<bool> {
    let settled = entry?;
    if url_changed && settled {
        return None;
    }
    Some(settled || !loading)
}

/// Where a new node lands (spec `2026-09-30-place-node-design.md`).
pub(crate) struct Placement {
    pub(crate) group: Option<String>,
    pub(crate) pos: egui::Vec2,
}

impl App {
    /// Where a new node `id` goes: `current_group` at `pos`, or at a
    /// jittered world-center spawn when `pos` is `None`. A JSON level is
    /// left first, so the node lands in the level the user then sees.
    pub(crate) fn placement(&mut self, id: &str, pos: Option<egui::Vec2>) -> Placement {
        self.leave_json_level();
        let pos = pos.unwrap_or_else(|| GRAPH_WORLD_CENTER + graph_layout::spawn_jitter(id));
        Placement { group: self.current_group.clone(), pos }
    }

    /// Inserts a node of type `kind` (colourless: colour is the mirror's, by id) showing `runtime` at `at` under `id`
    /// (allocated when `None`) and appends it to `node_order`. Its card is drawn at `at.pos` (`App::layout`).
    /// `at.group`, `at.pos`, a note's text, its type (`Mirror::ty`, from `kind`) and a file card's path seed the
    /// mirror only for an id with no entry yet (`Mirror::seed`; the type also lands on an entry that lacks one): a
    /// new node's document pos is its placement, which its `Create` carries (2b-4 Ruling 3), while a node shown again
    /// (a placeholder for a stored node, a reload) keeps its group, document pos, text and stored path (2b-5 Ruling
    /// 5). A content node's web source is seeded by its caller (`insert_content`, `insert_download_node`).
    /// Doesn't save: the caller does, as `apply_graph_action` does once per action.
    pub(crate) fn place_node(&mut self, id: Option<String>, kind: Kind, runtime: Runtime, at: Placement) -> String {
        let id = id.unwrap_or_else(|| self.alloc_node_id());
        // A tab's shared url starts as the one it is shown at (2b-3 Ruling 10). A stored tab's entry already holds
        // the document's (`materialize` runs `mirror_remote` first), so a live url it reopens at isn't taken for it.
        // A live tab's engine is the one it runs; an unloaded card has none of its own (`Mirror::engine`).
        let url = runtime.saved_tab().map(str::to_owned).unwrap_or_default();
        let engine = runtime.as_tab().map_or(0, |t| crate::session_sync::engine_code(t.tab.kind));
        // A new note's text starts as what it was made with (its payload, the edit buffer, 2b-4 Ruling 6).
        let text = runtime.as_note().map(|n| n.text.clone()).unwrap_or_default();
        let ty = kind.type_str().unwrap_or_default().to_owned();
        let path = runtime.file_path().unwrap_or_default().to_owned();
        self.nodes.insert(id.clone(), GraphNode { runtime });
        // The one way a node enters `nodes`: the next save creates it if the document lacks it (Ruling 3).
        self.queue_create(&id);
        self.snap_layout(&id, at.pos);
        let (pos, group) = (Some(at.pos), at.group);
        let carried = crate::mirror::Carried { pos, group, url, text, engine, ty, path, ..Default::default() };
        self.mirror.seed(&id, &carried);
        self.node_order.push(id.clone());
        id
    }

    /// `place_node` for a group named `name` (a group has no runtime). Like a note's text, the name seeds the mirror
    /// (`Mirror::name`) only for an id with no entry yet: a new group's `Create` carries it, and a stored group shown
    /// again keeps the document's (`materialize` runs `mirror_remote` first). Doesn't save.
    pub(crate) fn place_group(&mut self, id: Option<String>, name: &str, at: Placement) -> String {
        let id = id.unwrap_or_else(|| self.alloc_node_id());
        let fresh = !self.mirror.has(&id);
        self.place_node(Some(id.clone()), Kind::Group, Runtime::None, at);
        if fresh && !name.is_empty() {
            self.mirror.set(&id, crate::store::model::field::NAME, FieldValue::Str(name.to_owned()));
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
    pub(crate) fn insert_download_node(
        &mut self,
        tab_id: String,
        path: String,
        total_bytes: Option<u64>,
        url: Option<String>,
    ) {
        let node_group = self.mirror.group(&tab_id).map(str::to_owned);
        if !self.nodes.contains_key(&tab_id) {
            return;
        }
        let node_pos = self.layout_pos(&tab_id);
        let id = self.alloc_node_id();
        let pos = node_pos + graph_layout::spawn_jitter(&id);
        let payload = Runtime::Download(DownloadPayload {
            path,
            bytes_received: 0,
            total_bytes,
            completed: false,
            failed: false,
        });
        self.place_node(Some(id.clone()), Kind::Download, payload, Placement { group: node_group, pos });
        self.seed_source_url(&id, url);
        self.edit_parent(&id, Some(tab_id)); // the edge from the tab it came from
    }

    /// The in-flight download node an engine's progress, failure or
    /// completion for `path` is about: one `tab_id` started if there is one,
    /// else the newest. A finished node with the same path (a restored one,
    /// or an earlier download of the same file) is never matched.
    pub(crate) fn in_flight_download_id(&self, tab_id: &str, path: &str) -> Option<String> {
        let in_flight = |id: &String| {
            self.nodes
                .get(id)
                .and_then(|n| n.as_download())
                .is_some_and(|d| d.path == path && !d.completed && !d.failed)
        };
        let from_tab = |id: &&String| self.mirror.parent(id) == Some(tab_id);
        let mut newest_first = self.node_order.iter().rev().filter(|id| in_flight(id));
        let first = newest_first.clone().next();
        newest_first.find(from_tab).or(first).cloned()
    }

    /// Inserts a node of type `kind` showing `runtime` at `pos`, top level, no colour of its own (colour is the
    /// mirror's, by id): `materialize`'s way in for kinds with no insert helper of their own,
    /// and for an image/JSON card whose file couldn't be loaded (see
    /// `ImagePayload::error`).
    pub(crate) fn insert_placeholder_node(&mut self, id: String, pos: egui::Vec2, kind: Kind, runtime: Runtime) {
        self.place_node(Some(id), kind, runtime, Placement { group: None, pos });
    }

    /// Content-dir names this session still uses, for `content::kept`:
    /// live nodes' ids (a replica's own copy is `<id>.<ext>`) and `path`
    /// file names, and Recently Closed entries' `path` file names. `except`
    /// leaves out one live node.
    pub(crate) fn content_keep(&self, except: Option<&str>) -> HashSet<String> {
        let mut keep: HashSet<String> = HashSet::new();
        for (id, node) in self.nodes.iter().filter(|(id, _)| Some(id.as_str()) != except) {
            keep.extend(node.runtime.file_path().and_then(content::file_name));
            keep.insert(id.clone());
        }
        for entry in &self.closed {
            for node in &entry.doc.nodes {
                keep.extend(node.fields.get("path").and_then(|v| v.as_str()).and_then(content::file_name));
            }
        }
        keep
    }

    /// Whether this launch knows enough to prune: with no journal, or one
    /// that was rebuilt (`store_recovered`), nothing or not everything was
    /// loaded, so an unnamed file may still belong to the real session.
    pub(crate) fn may_prune_content(&self) -> bool {
        self.journal.is_some() && !self.store_recovered
    }

    /// Deletes content-dir files nothing uses (spec "Where fetched files
    /// live"), on a thread. Called once at launch, after restore. The
    /// thread also reads the named sessions, which keep their files too.
    pub(crate) fn prune_content_dir(&self) {
        if !self.may_prune_content() {
            return;
        }
        let Some(dir) = profile::content_dir() else { return };
        let keep = self.content_keep(None);
        let sessions = session::named_sessions_dir();
        let snapshot = std::time::SystemTime::now();
        std::thread::spawn(move || match prune_unnamed(&dir, snapshot, keep, sessions.as_deref()) {
            Some(0) => {}
            Some(n) => eprintln!("content: pruned {n} unused file(s)"),
            None => eprintln!("content: a named session couldn't be read whole; not pruning"),
        });
    }

    /// Tries a placeholder's file again (a click on the card). A node with
    /// a known source URL (spec "Where fetched files live") refetches
    /// through `refresh_node` instead of retrying the same local path --
    /// covers a missing file synced from another replica, and a Refresh
    /// that failed, either way with no local copy to fall back to. Without
    /// a URL, the real card takes the placeholder's place: same id,
    /// position, group, color and draw order. Still failing, it stays a
    /// placeholder with the new error, which the insert has already
    /// toasted. A failed PDF card just renders again (back to its
    /// spinner).
    pub(crate) fn retry_missing_file(&mut self, id: &str) {
        if self.nodes.get(id).and_then(|n| n.as_unloaded()).is_some() {
            let kind = self.mirror.engine(id);
            // A tab that failed for want of its engine's runtime can't succeed by retrying: ask to
            // install the engine instead (the card's Download).
            match pick_outcome(kind, &self.engine_state(kind)) {
                PickOutcome::AskInstall(kind) => {
                    self.offer_engine_confirm(EngineConfirm::Download(kind));
                }
                PickOutcome::AlreadyInstalling(kind) => {
                    self.queue_toast(format!("{} is already installing", engine_label(kind)))
                }
                PickOutcome::Unavailable(kind, reason) => {
                    self.queue_toast(PickOutcome::unavailable_toast(kind, &reason))
                }
                PickOutcome::Switch(kind) => self.wake_tab(id, kind),
            }
            return;
        }
        if self.source_url(id).is_some() {
            self.refresh_node(id);
            return;
        }
        if let Some(pdf) = self.nodes.get_mut(id).and_then(|n| n.as_pdf_mut()) {
            if matches!(pdf.outcome, Some(Err(_))) {
                pdf.outcome = None;
                self.pdf_renderer.request(id.to_string(), pdf.path.clone());
            }
            return;
        }
        let Some(node) = self.nodes.get(id) else { return };
        let (path, is_image) = match &node.runtime {
            Runtime::Image(image) if image.error.is_some() => (image.path.clone(), true),
            Runtime::Json(json) if json.error.is_some() => (json.path.clone(), false),
            _ => return,
        };
        let (group, parent) = (self.mirror.group(id).map(str::to_owned), self.mirror.parent(id).map(str::to_owned));
        let pos = self.layout_pos(id);
        let in_order = self.node_order.iter().any(|n| n == id);
        let result = if is_image {
            self.insert_image_node_with_id(id.to_string(), path, pos, group)
        } else {
            self.insert_json_node_with_id(id.to_string(), path, pos, group)
        };
        match result {
            Ok(()) => {
                // The insert replaced the node and pushed its id again;
                // keep it where it was.
                if in_order {
                    self.node_order.pop();
                }
                self.edit_parent(id, parent);
                self.save_session();
            }
            Err(error) => match self.nodes.get_mut(id).map(|n| &mut n.runtime) {
                Some(Runtime::Image(image)) => image.error = Some(error),
                Some(Runtime::Json(json)) => json.error = Some(error),
                _ => {}
            },
        }
    }

    /// Fetches a node's source URL again; `reload_fetched_node` swaps it
    /// in once it passes its check. Refresh, and a missing file.
    pub(crate) fn refresh_node(&mut self, id: &str) {
        use content::ContentKind as K;
        let Some(node) = self.nodes.get(id) else { return };
        let kind = match &node.runtime {
            Runtime::Image(_) => K::Image,
            Runtime::Pdf(_) => K::Pdf,
            Runtime::Json(j) if !j.leaf => K::Json,
            _ => return,
        };
        let Some(url) = self.source_url(id).map(str::to_string) else { return };
        let job = FetchJob { target: FetchTarget::Node(id.to_string()), node_id: id.to_string(), url, kind };
        self.redirect_fetcher.request(job);
    }

    /// Reloads node `id` from freshly fetched content: same id, position,
    /// group, color and draw order. If the new content fails to load, the
    /// node isn't lost -- a placeholder of the same kind (mirroring
    /// `retry_missing_file`'s own failure handling, and `session_sync`'s
    /// restore-time placeholder) takes its place, carrying the error and
    /// the url.
    pub(crate) fn reload_fetched_node(&mut self, id: &str, body: WebBody) {
        use content::ContentKind as K;
        let Some(node) = self.nodes.get(id) else { return };
        let WebBody::File { kind, path } = &body else { return };
        // Checked before removing anything: only these kinds have a placeholder.
        if !matches!(kind, K::Image | K::Pdf | K::Json) {
            return;
        }
        let (kind, path) = (*kind, path.clone());
        let new_path = path.clone();
        let (group, parent) = (self.mirror.group(id).map(str::to_owned), self.mirror.parent(id).map(str::to_owned));
        let pos = self.layout_pos(id);
        // A PNG turned JPEG keeps its id stem, so `prune` never frees it.
        let old_path = node.runtime.file_path().map(str::to_owned);
        let Some(url) = self.source_url(id).map(str::to_string) else { return };
        let at = self.node_order.iter().position(|n| n == id);
        if let Some(gpu) = &mut self.gpu {
            gpu.free_image(id);
        }
        self.nodes.remove(id);
        self.node_order.retain(|n| n != id);
        let source = ContentSource::Web { url: url.clone(), body };
        let result = self.insert_content(source, id.to_string(), pos, group.clone());
        if let Err(error) = &result {
            let (ty, runtime) = match kind {
                K::Image => (
                    Kind::Image,
                    Runtime::Image(ImagePayload { path, width: 0, height: 0, error: Some(error.clone()) }),
                ),
                K::Pdf => (Kind::Pdf, Runtime::Pdf(PdfPayload { path, outcome: Some(Err(error.clone())) })),
                K::Json => (
                    Kind::Json,
                    Runtime::Json(JsonPayload {
                        path,
                        value: Default::default(),
                        leaf: false,
                        error: Some(error.clone()),
                    }),
                ),
                // Guarded above: `kind` is one of the three arms this
                // matches.
                _ => unreachable!("reload_fetched_node only reaches here for Image/Pdf/Json"),
            };
            self.insert_placeholder_node(id.to_string(), pos, ty, runtime);
        }
        self.edit_parent(id, parent);
        self.edit_group(id, group);
        // The url and stored path are the mirror's, which the remove and reinsert leave alone: the payload holds the
        // new file (2b-5 Ruling 5). The view is the mirror's too (2b-4 Ruling 8). Overrides the new
        // value can't resolve are dropped; an error card's view is left unpruned, since pruning against its `Null`
        // would wipe, and sync away, every override a Retry should get back.
        let value = self.nodes.get(id).and_then(|n| n.as_json()).filter(|_| result.is_ok()).map(|j| j.value.clone());
        if let (Some(value), Some(view)) = (value, self.mirror.view(id)) {
            let mut pruned = view.clone();
            pruned.retain_resolvable(&value);
            if pruned != *view {
                self.edit_view(id, &pruned.to_field());
            }
        }
        if let (Some(at), Some(last)) = (at, self.node_order.pop()) {
            self.node_order.insert(at.min(self.node_order.len()), last);
        }
        // The shown level holds the old value; the next sync rebuilds it
        // from the new one.
        if self.json_level.as_ref().is_some_and(|l| l.node_id == id) {
            self.json_level = None;
        }
        if result.is_ok() {
            if let (Some(old_path), Some(dir)) = (&old_path, crate::profile::content_dir()) {
                let (old, new) = (std::path::Path::new(old_path), std::path::Path::new(&new_path));
                if replaced_file_unused(&dir, id, old, new, &self.content_keep(Some(id))) {
                    let _ = std::fs::remove_file(old);
                }
            }
            self.save_session();
        }
    }

    /// A refetch whose fetch itself failed (network error, or bytes that
    /// didn't pass their kind's check). With the file still there (a
    /// Refresh), the node is untouched and a toast says so. With it missing
    /// (restore, sync or Retry), the node becomes the missing-file card,
    /// whose Retry is the feedback.
    pub(crate) fn refresh_fetch_failed(&mut self, id: &str) {
        let error = format!("Couldn't fetch {}", self.source_url(id).unwrap_or_default());
        let Some(node) = self.nodes.get_mut(id) else { return };
        let missing = |path: &str| !std::path::Path::new(path).exists();
        let name = match &mut node.runtime {
            Runtime::Image(p) if missing(&p.path) => return p.error = Some(error),
            Runtime::Json(p) if missing(&p.path) => return p.error = Some(error),
            Runtime::Pdf(p) if missing(&p.path) => return p.outcome = Some(Err(error)),
            Runtime::Image(ImagePayload { path, .. })
            | Runtime::Json(JsonPayload { path, .. })
            | Runtime::Pdf(PdfPayload { path, .. }) => crate::json_file::file_name(std::path::Path::new(path)),
            _ => return,
        };
        self.queue_toast(format!("Couldn't refresh {name}"));
    }

    /// A tab at the node's source URL beside it, which stays a tab on its
    /// first load (`skip_convert`), e.g. to read a PDF past page 1. The tab
    /// is opened from the node (`parent`), so the two stay linked.
    pub(crate) fn open_as_tab(&mut self, id: &str) {
        let Some(url) = self.source_url(id).map(str::to_string) else { return };
        let (pos, group) = (self.layout_pos(id) + egui::vec2(40.0, 40.0), self.mirror.group(id).map(str::to_owned));
        let tab = self.spawn_tab_at(&url, pos, group, Some(id.to_string()));
        self.skip_convert.insert(tab, false);
    }

    /// The one way content becomes a node (see `content`): a picked,
    /// dropped or `ADDFILE` file, or a page a tab converted. `id`, `pos`
    /// and `group` place it. A failure is toasted here.
    pub(crate) fn insert_content(
        &mut self,
        source: ContentSource,
        id: String,
        pos: egui::Vec2,
        group: Option<String>,
    ) -> Result<(), String> {
        use content::ContentKind as K;
        let (url, kind, path) = match source {
            ContentSource::Web { url, body: WebBody::Text(text) } => {
                self.insert_note_with_text(id, text, Some(url), pos, group);
                return Ok(());
            }
            ContentSource::Web { url, body: WebBody::File { kind, path } } => (Some(url), kind, path),
            ContentSource::Local(path) => {
                let name = crate::json_file::file_name(&path);
                let head = content::read_head(&path).map_err(|err| {
                    let err = format!("Couldn't open {name}: {err}");
                    self.queue_toast(err.clone());
                    err
                })?;
                let kind = content::kind_of(content::sniff(&head, &name));
                let path_str = path.to_string_lossy().into_owned();
                match kind {
                    K::Note if head.len() > content::NOTE_MAX_BYTES => {
                        let err = format!("Couldn't open {name}: too long for a note (over 1 MB)");
                        self.queue_toast(err.clone());
                        return Err(err);
                    }
                    K::Note => {
                        // `sniff` said text and the head is the whole file.
                        let text = String::from_utf8(head).map_err(|e| e.to_string())?;
                        self.insert_note_with_text(id, text, None, pos, group);
                        return Ok(());
                    }
                    K::Page => {
                        self.spawn_tab_at(&content::path_file_url(&path), pos, group, None);
                        return Ok(());
                    }
                    K::Other => {
                        let bytes = std::fs::metadata(&path).map_or(0, |m| m.len());
                        let payload = DownloadPayload {
                            path: path_str,
                            bytes_received: bytes,
                            total_bytes: Some(bytes),
                            completed: true,
                            failed: false,
                        };
                        self.insert_placeholder_node(id.clone(), pos, Kind::Download, Runtime::Download(payload));
                        if self.nodes.contains_key(&id) {
                            self.edit_group(&id, group);
                        }
                        return Ok(());
                    }
                    _ => (None, kind, path_str),
                }
            }
        };
        match kind {
            K::Image => self.insert_image_node_with_id(id.clone(), path, pos, group)?,
            K::Pdf => self.insert_pdf_node_with_id(id.clone(), path, pos, group),
            K::Json => self.insert_json_node_with_id(id.clone(), path, pos, group)?,
            // A web page never reaches here as a file (see `redirect_fetch`).
            K::Note | K::Page | K::Other => return Err(format!("{kind:?} is not a file kind")),
        }
        self.seed_source_url(&id, url);
        Ok(())
    }

    /// A new content node's web source (`Mirror::url`, `App::source_url`), seeded once it is placed: a stored node
    /// shown again (a reload, a restore) keeps the one the mirror holds. `None`, a local file, seeds nothing.
    fn seed_source_url(&mut self, id: &str, url: Option<String>) {
        if let Some(url) = url.filter(|url| !url.is_empty() && self.nodes.contains_key(id)) {
            self.mirror.init_field(id, crate::store::model::field::URL, FieldValue::Str(url));
        }
    }

    pub(crate) fn insert_note_with_text(
        &mut self,
        id: String,
        text: String,
        url: Option<String>,
        pos: egui::Vec2,
        group: Option<String>,
    ) {
        let payload = NotePayload { text, editing: false };
        self.place_node(Some(id.clone()), Kind::Note, Runtime::Note(payload), Placement { group, pos });
        self.seed_source_url(&id, url);
    }

    /// The insert every `image` card goes through: the caller picks the id,
    /// position and group -- `insert_content` a fresh id at the current
    /// level for a picked/dropped/`ADDFILE` image, session restore the saved
    /// id and position with no group, "Recently Closed" the old position and
    /// group (`reopen_closed`), among others.
    pub(crate) fn insert_image_node_with_id(
        &mut self,
        id: String,
        path: String,
        pos: egui::Vec2,
        group: Option<String>,
    ) -> Result<(), String> {
        // Toasted here, not by callers: session restore, add-file and
        // reopen all used to drop a missing or undecodable file silently.
        // The error comes back too, for restore's placeholder card. Decoded
        // by content (`image::load_from_memory` guesses the format from the
        // bytes), not by `path`'s own extension: `insert_content` already
        // sniffed this file as an image, which a lying extension (a `.pdf`
        // that's really a PNG) would otherwise defeat.
        let img = match std::fs::read(&path)
            .map_err(image::ImageError::IoError)
            .and_then(|bytes| image::load_from_memory(&bytes))
        {
            Ok(img) => img,
            Err(err) => {
                eprintln!("image: couldn't open {path}: {err}");
                let name = crate::json_file::file_name(std::path::Path::new(&path));
                let err = format!("Couldn't open image {name}: {err}");
                self.queue_toast(err.clone());
                return Err(err);
            }
        };
        // `into_rgba8` moves when the image is already RGBA8; `to_rgba8` copied it.
        let rgba = crate::rgba::fit_image(img.into_rgba8(), crate::rgba::MAX_TEXTURE_SIDE);
        let (width, height) = rgba.dimensions();
        if let Some(gpu) = &mut self.gpu {
            gpu.upload_image(&id, rgba.as_raw(), width, height);
        }
        self.place_node(
            Some(id),
            Kind::Image,
            Runtime::Image(ImagePayload { path, width, height, error: None }),
            Placement { group, pos },
        );
        Ok(())
    }

    /// Converts a tab whose page is content the table maps to a node (see
    /// `ipc_protocol::EngineToHost::RedirectableContent`): `body` is what
    /// `redirect_fetch` saved, inserted through `insert_content` at the
    /// tab's place, opened from where the tab was (its `parent`). Only once
    /// that succeeds does the tab go, through `remove_tab` (a close that
    /// isn't remembered, clearing every reference to it). A failed insert
    /// is toasted and the tab stays.
    pub(crate) fn redirect_tab_to_content(&mut self, tab_id: &str, node_id: String, url: String, body: WebBody) {
        let group = self.mirror.group(tab_id).map(str::to_owned);
        if !self.nodes.contains_key(tab_id) {
            return;
        }
        let pos = self.layout_pos(tab_id);
        let parent = self.mirror.parent(tab_id).map(str::to_owned);
        if self.insert_content(ContentSource::Web { url, body }, node_id.clone(), pos, group).is_ok() {
            self.edit_parent(&node_id, parent);
            // The tab's links go with it (`queue_delete`); the new card takes over each, keeping its direction.
            let old = tab_id.parse::<crate::store::model::NodeId>().ok();
            for link in self.links_naming(tab_id) {
                let (from, to) = self.mirror.link_ends(&link);
                let end = |e: Option<crate::store::model::NodeId>| match e {
                    e if e == old => node_id.clone(),
                    Some(other) => other.to_string(),
                    None => String::new(),
                };
                self.create_link(&end(from), &end(to));
            }
            self.remove_tab(tab_id);
            self.save_session();
        }
    }

    /// The insert every `pdf` card goes through: the caller picks the id,
    /// position and group -- `insert_content` a fresh id at the current
    /// level for a picked/dropped/`ADDFILE` PDF, session restore the saved
    /// id and position with no group, "Recently Closed" the old position and
    /// group (`reopen_closed`), among others. Unlike `insert_image_node_
    /// with_id`, this always inserts the node immediately (`outcome: None`,
    /// "still loading") rather than checking the file synchronously first --
    /// decoding a PDF is genuinely slow (see `PdfPayload`'s own doc comment),
    /// so there's nothing worth checking synchronously here; a bad/missing
    /// file surfaces later as a `Some(Err(_))` outcome instead of the node
    /// never having existed the way a bad image file does.
    pub(crate) fn insert_pdf_node_with_id(&mut self, id: String, path: String, pos: egui::Vec2, group: Option<String>) {
        self.pdf_renderer.request(id.clone(), path.clone());
        let pdf = Runtime::Pdf(PdfPayload { path, outcome: None });
        self.place_node(Some(id), Kind::Pdf, pdf, Placement { group, pos });
    }

    /// The insert every `json` card goes through: the caller picks the id,
    /// position and group -- `insert_content` a fresh id at the current
    /// level for a picked/dropped/`ADDFILE` JSON file, session restore the
    /// saved id and position with no group, "Recently Closed" the old
    /// position and group (`reopen_closed`), among others. Returns `Err`
    /// (doing nothing) if the file doesn't exist or isn't valid JSON, same
    /// fail-fast posture `insert_image_node_with_id` already has (a JSON
    /// parse is just as fast as decoding an image, so there's nothing here
    /// that needs `Pdf`'s own background-render treatment).
    pub(crate) fn insert_json_node_with_id(
        &mut self,
        id: String,
        path: String,
        pos: egui::Vec2,
        group: Option<String>,
    ) -> Result<(), String> {
        // Toasted here, not by callers -- see `insert_image_node_with_id`.
        let value = match crate::json_file::load::<serde_json::Value>(std::path::Path::new(&path), "JSON") {
            Ok(value) => value,
            Err(err) => {
                self.queue_toast(err.clone());
                return Err(err);
            }
        };
        let payload = Runtime::Json(JsonPayload { path, value: value.into(), leaf: false, error: None });
        self.place_node(Some(id), Kind::Json, payload, Placement { group, pos });
        Ok(())
    }

    /// The Graph toolbar's single "+ File" button -- one file picker
    /// covering every locally-addable node kind, dispatching whatever it
    /// picks through `insert_content` (see that method, and `content`'s own
    /// doc comment for the MIME table every way content arrives shares). A
    /// real file picker blocks on OS/human input neither exists in headless
    /// mode, so this is never called there; `ADDFILE <path>` is the headless
    /// equivalent, going through `add_file_at` the same way.
    /// `pos`: `None` for the toolbar button (jittered around world center,
    /// same as always); `Some(pos)` for the empty-canvas "New Tab/Note/
    /// Group/File" card (`pending_new_node`), which already knows exactly
    /// where it should land.
    pub(crate) fn add_file_node(&mut self, pos: Option<egui::Vec2>) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter(
                "Image, PDF, JSON or text",
                &["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "pdf", "json", "txt", "md", "html", "htm"],
            )
            .add_filter("All files", &["*"])
            .pick_file()
        else {
            return;
        };
        if self.add_file_at(path, pos).is_ok() {
            self.save_session();
        }
    }

    /// `path` as a new node at `pos`, or jittered around world center, at
    /// the current level (a JSON level is left first). The picker and
    /// `ADDFILE`.
    pub(crate) fn add_file_at(&mut self, path: std::path::PathBuf, pos: Option<egui::Vec2>) -> Result<(), String> {
        let id = self.alloc_node_id();
        let Placement { group, pos } = self.placement(&id, pos);
        self.insert_content(ContentSource::Local(path), id, pos, group)
    }

    /// A file dropped on the window (spec "Drag and drop"): `point` is the
    /// drop location in window points. In Tab view, beside the focused
    /// tab, in its group -- `insert_content` directly, since that group may
    /// not be `current_group`. Otherwise (Graph view, or Tab view with no
    /// focused tab, which lands near world center at the current level the
    /// same as any other point there would) the world position under
    /// `point`, via `add_file_at` (`current_group`). Several drops in one
    /// frame fan out by `drop_batch`, reset each frame in `render_frame`.
    pub(crate) fn drop_file(&mut self, path: std::path::PathBuf, point: egui::Pos2) {
        let fan_out = egui::vec2(24.0, 24.0) * self.drop_batch as f32;
        self.drop_batch += 1;
        let beside_focused = self
            .showing_tab_view()
            .then(|| self.focused.as_deref().filter(|id| self.nodes.contains_key(*id)))
            .flatten()
            .map(|id| (self.layout_pos(id) + egui::vec2(240.0, 0.0), self.group_of(&self.focused)));
        match beside_focused {
            Some((pos, group)) => {
                let id = self.alloc_node_id();
                let _ = self.insert_content(ContentSource::Local(path), id, pos + fan_out, group);
            }
            None => {
                // Leaving first: the drop point then maps into the level
                // the node lands in, with no outline beside it.
                self.leave_json_level();
                let pos = self.graph_screen_point_to_world(point);
                let _ = self.add_file_at(path, Some(pos + fan_out));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_sync::color_value;

    #[test]
    fn insert_content_dispatches_local_files_by_content() {
        let dir = crate::store::testutil::temp_dir("insert_content");
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbaImage::new(3, 2).write_to(&mut png, image::ImageFormat::Png).unwrap();
        let files = [
            ("pic.pdf", png.into_inner()), // lying extension: still an image
            ("doc", b"%PDF-1.4\n".to_vec()),
            ("data.txt", br#"{"a": 1}"#.to_vec()),
            ("bom.json", b"\xef\xbb\xbf[2]".to_vec()),
            ("notes.md", b"# hi".to_vec()),
            ("blob.bin", vec![0, 1, 2, 255]),
        ];
        let mut app = App::default();
        let mut kinds = Vec::new();
        for (name, bytes) in files {
            let path = dir.join(name);
            std::fs::write(&path, bytes).unwrap();
            let id = app.alloc_node_id();
            app.insert_content(ContentSource::Local(path), id.clone(), egui::Vec2::ZERO, None).unwrap();
            kinds.push(match &app.nodes[&id].runtime {
                Runtime::Image(p) => format!("image {}x{}", p.width, p.height),
                Runtime::Pdf(_) => "pdf".into(),
                Runtime::Json(p) => format!("json {}", p.value),
                Runtime::Note(p) => format!("note {}", p.text),
                Runtime::Download(p) => format!("download {} done={}", p.bytes_received, p.completed),
                _ => "other".into(),
            });
            assert_eq!(app.source_url(&id), None);
        }
        assert_eq!(kinds, ["image 3x2", "pdf", r#"json {"a":1}"#, "json [2]", "note # hi", "download 4 done=true"]);
    }

    /// A node whose mirror entry exists without a type (an op named it before its `Create` arrived) is typed when
    /// placed, so its card never reads as unknown.
    #[test]
    fn place_node_types_a_node_whose_entry_lacks_a_type() {
        let mut app = App::default();
        app.mirror.set("n", field::URL, FieldValue::Str("https://a/".into()));
        assert_eq!(app.mirror.ty("n"), "");
        let note = Runtime::Note(NotePayload { text: "t".into(), editing: false });
        app.insert_placeholder_node("n".into(), egui::Vec2::ZERO, Kind::Note, note);
        assert_eq!(app.mirror.ty("n"), crate::store::model::TYPE_NOTE);
        assert_eq!(app.card_kind("n").unwrap().ty, crate::store::model::TYPE_NOTE);
        assert_eq!(app.mirror.url("n"), "https://a/", "the entry's other fields are kept");
    }

    #[test]
    fn insert_content_refuses_a_failing_local_file() {
        let dir = crate::store::testutil::temp_dir("insert_content_refuse");
        let big = dir.join("big.txt");
        std::fs::write(&big, vec![b'a'; crate::content::NOTE_MAX_BYTES + 1]).unwrap();
        let broken = dir.join("broken.png");
        std::fs::write(&broken, b"\x89PNG\r\n\x1a\n not really").unwrap();
        let mut app = App::default();
        for path in [big, broken, dir.join("missing.txt")] {
            let id = app.alloc_node_id();
            assert!(app.insert_content(ContentSource::Local(path), id.clone(), egui::Vec2::ZERO, None).is_err());
            assert!(!app.nodes.contains_key(&id));
        }
        assert_eq!(app.save_toast_queue.len() + usize::from(app.save_toast.is_some()), 3, "each refusal toasts");
    }

    #[test]
    fn several_drops_in_a_frame_fan_out() {
        let dir = crate::store::testutil::temp_dir("drop_fan");
        let mut app = App::default();
        let mut ids = Vec::new();
        for name in ["a.txt", "b.txt"] {
            std::fs::write(dir.join(name), b"x").unwrap();
            let before: HashSet<String> = app.nodes.keys().cloned().collect();
            app.drop_file(dir.join(name), egui::pos2(300.0, 300.0));
            ids.extend(app.nodes.keys().filter(|k| !before.contains(*k)).cloned());
        }
        let (a, b) = (app.layout_pos(&ids[0]), app.layout_pos(&ids[1]));
        assert_eq!(b - a, egui::vec2(24.0, 24.0));
    }

    #[test]
    fn drop_file_lands_beside_the_focused_tab_in_its_own_group() {
        let dir = crate::store::testutil::temp_dir("drop_beside_focused");
        let mut app = App::default();
        // A sleeping (unloaded) tab is a real Tab-kind node with no live
        // engine process -- the same stand-in `session_sync::tests::
        // sleeping_tab` already uses elsewhere for "the focused tab" in a
        // unit test. `drop_file`'s "beside focused" branch only reads
        // `pos`/`group`, so this exercises exactly the same code a live
        // tab would. `App::default()` is already in Tab view
        // (`showing_tab_view()`, see `several_drops_in_a_frame_fan_out`'s
        // own comment).
        use crate::session_sync::tests::{add, sleeping_tab};
        let focused = add(&mut app, Kind::Tab, sleeping_tab("https://x.test/", "Focused"));
        app.snap_layout(&focused, egui::vec2(100.0, 50.0));
        app.edit_group(&focused, Some("g".into()));
        app.focused = Some(focused.clone());
        // Deliberately not "g": proves the drop uses the focused tab's own
        // group, not `current_group`, the way `insert_content` directly
        // (not `add_file_at`) is needed for.
        app.current_group = None;

        let mut ids = Vec::new();
        for name in ["a.txt", "b.txt"] {
            std::fs::write(dir.join(name), b"x").unwrap();
            let before: HashSet<String> = app.nodes.keys().cloned().collect();
            app.drop_file(dir.join(name), egui::pos2(300.0, 300.0));
            ids.extend(app.nodes.keys().filter(|k| !before.contains(*k)).cloned());
        }

        let beside = egui::vec2(100.0, 50.0) + egui::vec2(240.0, 0.0);
        assert_eq!(app.layout_pos(&ids[0]), beside);
        assert_eq!(app.layout_pos(&ids[1]), beside + egui::vec2(24.0, 24.0));
        assert_eq!(app.mirror.group(&ids[0]), Some("g"));
        assert_eq!(app.mirror.group(&ids[1]), Some("g"));
    }

    #[test]
    fn insert_content_dispatches_web_content_and_keeps_its_url() {
        let dir = crate::store::testutil::temp_dir("insert_content_web");
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbaImage::new(1, 1).write_to(&mut png, image::ImageFormat::Png).unwrap();
        let path = dir.join("fetched.png");
        std::fs::write(&path, png.into_inner()).unwrap();

        let mut app = App::default();
        let id = app.alloc_node_id();
        let source = ContentSource::Web {
            url: "https://x.test/a.png".into(),
            body: WebBody::File { kind: content::ContentKind::Image, path: path.to_string_lossy().into_owned() },
        };
        app.insert_content(source, id.clone(), egui::Vec2::ZERO, None).unwrap();
        assert_eq!(app.source_url(&id), Some("https://x.test/a.png"));

        let note_id = app.alloc_node_id();
        let source = ContentSource::Web { url: "https://x.test/note.txt".into(), body: WebBody::Text("hi".into()) };
        app.insert_content(source, note_id.clone(), egui::Vec2::ZERO, None).unwrap();
        assert_eq!(app.source_url(&note_id), Some("https://x.test/note.txt"));
        // 2b-5 Task 1: the url is the mirror's, seeded at creation, so the next save creates the node with it.
        assert_eq!(
            (app.mirror.url(&id), app.mirror.url(&note_id)),
            ("https://x.test/a.png", "https://x.test/note.txt")
        );
        let snapshot = app.snapshot(id.parse().unwrap()).unwrap();
        assert_eq!(snapshot.fields[&field::URL], FieldValue::Str("https://x.test/a.png".into()));
    }

    /// 2b-5 Task 1: a download's address is the mirror's from the moment its card is placed, and its type is seeded.
    #[test]
    fn a_new_download_has_its_url_and_type_in_the_mirror() {
        use crate::session_sync::tests::{add, sleeping_tab};
        let mut app = App::default();
        let tab = add(&mut app, Kind::Tab, sleeping_tab("https://x.test/", "T"));
        app.insert_download_node(tab, "f.bin".into(), Some(3), Some("https://x.test/f.bin".into()));
        let id = app.nodes.iter().find(|(_, n)| n.as_download().is_some()).map(|(id, _)| id.clone()).unwrap();
        assert_eq!(app.mirror.url(&id), "https://x.test/f.bin");
        assert_eq!(app.copyable_url(&id), Some("https://x.test/f.bin"));
        assert_eq!(app.mirror.ty(&id), crate::store::model::Kind::Download.type_str().unwrap());
        assert_eq!(app.mirror.path(&id), "f.bin");
    }

    /// 2b-5 Ruling 5: a reload that lands in a new file keeps the stored (create-time) path in the mirror, which the
    /// save writes, while the payload reads the new one, and Recently Closed records that resolved one.
    #[test]
    fn a_reload_keeps_the_stored_path_and_closing_records_the_resolved_one() {
        let dir = crate::store::testutil::temp_dir("reload_path");
        let mut app = App::default();
        let id = app.alloc_node_id();
        let first = crate::redirect_fetch::save(&dir, &id, crate::content::ContentKind::Json, b"[1]".to_vec()).unwrap();
        let WebBody::File { path: stored, .. } = &first else { panic!("a file") };
        let stored = stored.clone();
        app.insert_content(
            ContentSource::Web { url: "https://x.test/j".into(), body: first },
            id.clone(),
            egui::Vec2::ZERO,
            None,
        )
        .unwrap();
        assert_eq!(app.mirror.path(&id), stored);
        let moved = dir.join("moved.json");
        std::fs::write(&moved, b"[2]").unwrap();
        let moved = moved.to_string_lossy().into_owned();
        app.reload_fetched_node(&id, WebBody::File { kind: crate::content::ContentKind::Json, path: moved.clone() });
        assert_eq!(app.nodes[&id].as_json().unwrap().path, moved, "the payload reads the new file");
        assert_eq!(app.mirror.path(&id), stored, "the mirror keeps the stored path");
        let snapshot = app.snapshot(id.parse().unwrap()).unwrap();
        assert_eq!(snapshot.fields[&field::PATH], FieldValue::Str(stored.clone()), "a save writes the stored path");
        let entry = crate::closed::closed_entry(&app, &id).unwrap();
        assert_eq!(entry.doc.nodes[0].fields.get("path").and_then(|v| v.as_str()), Some(moved.as_str()));
    }

    #[test]
    fn a_content_file_resolves_to_this_machines_newest_copy_first() {
        use crate::content::tests::write_aged;
        let dir = crate::store::testutil::temp_dir("resolve_content");
        write_aged(&dir.join("5-9.png"), std::time::Duration::from_secs(60));
        let elsewhere = r"Z:\nowhere\5-9.png";
        let index = crate::ContentIndex::scan(Some(&dir));
        assert_eq!(index.resolve("5-9", elsewhere), dir.join("5-9.png").to_string_lossy());
        assert_eq!(index.resolve("5-8", elsewhere), elsewhere);
        assert_eq!(crate::ContentIndex::scan(None).resolve("5-9", elsewhere), elsewhere);
        // A stored file that exists (a re-id'd node's old copy) still loses
        // to the node's own fetch, and a Refresh's newer extension wins.
        let old = dir.join("5-1.png");
        write_aged(&old, std::time::Duration::from_secs(60));
        let old = old.to_string_lossy().into_owned();
        write_aged(&dir.join("5-9.jpg"), std::time::Duration::ZERO);
        let index = crate::ContentIndex::scan(Some(&dir));
        assert_eq!(index.resolve("5-9", &old), dir.join("5-9.jpg").to_string_lossy());
        assert_eq!(index.resolve("5-1", &old), old);
        // An unfinished save is never a node's file.
        write_aged(&dir.join("5-2.png.1234.tmp"), std::time::Duration::ZERO);
        assert_eq!(crate::ContentIndex::scan(Some(&dir)).resolve("5-2", elsewhere), elsewhere);
    }

    #[test]
    fn one_index_answers_every_node_of_a_pass() {
        use crate::content::tests::write_aged;
        let dir = crate::store::testutil::temp_dir("content_index");
        write_aged(&dir.join("5-9.png"), std::time::Duration::from_secs(60));
        write_aged(&dir.join("5-9.jpg"), std::time::Duration::ZERO);
        write_aged(&dir.join("5-10.json"), std::time::Duration::ZERO);
        let index = crate::ContentIndex::scan(Some(&dir));
        assert_eq!(index.resolve("5-9", "x"), dir.join("5-9.jpg").to_string_lossy(), "the newest wins");
        assert_eq!(index.resolve("5-10", "x"), dir.join("5-10.json").to_string_lossy());
        assert_eq!(index.resolve("5-1", "x"), "x", "an id that is a prefix of another's isn't matched");
    }

    #[test]
    fn pruning_keeps_files_live_or_recently_closed_nodes_name() {
        let dir = crate::store::testutil::temp_dir("prune_content");
        let mut app = App::default();
        // A live node by path, a live node by id (a refetched copy), a
        // closed entry by its path field, and an orphan.
        for name in ["a.png", "1-7.json", "c.pdf", "orphan.png"] {
            crate::content::tests::write_aged(&dir.join(name), crate::content::tests::OLD);
        }
        app.insert_placeholder_node(
            "1".into(),
            egui::Vec2::ZERO,
            Kind::Image,
            Runtime::Image(ImagePayload {
                path: dir.join("a.png").to_string_lossy().into_owned(),
                width: 0,
                height: 0,
                error: None,
            }),
        );
        app.insert_placeholder_node(
            "1-7".into(),
            egui::Vec2::ZERO,
            Kind::Json,
            Runtime::Json(JsonPayload {
                path: r"Z:\x.json".into(),
                value: Default::default(),
                leaf: false,
                error: None,
            }),
        );
        app.closed.push(ClosedEntry {
            doc: store::json::JsonDoc {
                nodes: vec![store::json::JsonNode {
                    id: "99".into(),
                    fields: serde_json::Map::from_iter([
                        ("type".to_string(), serde_json::Value::String(store::model::TYPE_PDF.into())),
                        (
                            "path".to_string(),
                            serde_json::Value::String(dir.join("c.pdf").to_string_lossy().into_owned()),
                        ),
                    ]),
                }],
                local: None,
            },
        });
        let keep = app.content_keep(None);
        let pruned = crate::content::prune(&dir, std::time::SystemTime::now(), |f| crate::content::kept(&keep, f));
        assert_eq!(pruned, 1);
        assert!(!dir.join("orphan.png").exists());
        assert!(dir.join("a.png").exists());
        assert!(dir.join("1-7.json").exists());
        assert!(dir.join("c.pdf").exists());
    }

    #[test]
    fn pruning_keeps_files_a_named_session_names() {
        use crate::content::tests::{write_aged, OLD};
        use crate::session_sync::tests::{add, app_on};
        let root = crate::store::testutil::temp_dir("prune_named");
        let (content_dir, sessions) = (root.join("content"), root.join("sessions"));
        std::fs::create_dir_all(&content_dir).unwrap();
        std::fs::create_dir_all(&sessions).unwrap();
        let mut app = app_on(&root.join("session.mnf"), 1);
        let image = |path: &std::path::Path| {
            let path = path.to_string_lossy().into_owned();
            Runtime::Image(ImagePayload { path, width: 1, height: 1, error: None })
        };
        // A live node by path, one by id (its stored path elsewhere), and a
        // closed one by path.
        add(&mut app, Kind::Image, image(&content_dir.join("by-path.png")));
        let by_id = add(&mut app, Kind::Image, image(std::path::Path::new(r"Z:\elsewhere.png")));
        let closed = add(&mut app, Kind::Image, image(&content_dir.join("closed.png")));
        assert!(app.save_session());
        app.close_node(&closed);
        assert!(app.save_session());
        assert_eq!(app.closed.len(), 1);
        let journal = app.journal.as_ref().unwrap();
        journal.save_copy(&sessions.join("saved.mnf"), Some(&app.local_state().to_bytes())).unwrap();

        for name in ["by-path.png", &format!("{by_id}.json"), "closed.png", "orphan.png"] {
            write_aged(&content_dir.join(name), OLD);
        }
        // The running session names none of them.
        let pruned = prune_unnamed(&content_dir, std::time::SystemTime::now(), HashSet::new(), Some(&sessions));
        assert_eq!(pruned, Some(1));
        assert!(!content_dir.join("orphan.png").exists());
        assert!(content_dir.join("by-path.png").exists());
        assert!(content_dir.join(format!("{by_id}.json")).exists());
        assert!(content_dir.join("closed.png").exists());
    }

    #[test]
    fn a_named_session_that_cant_be_read_whole_stops_the_prune() {
        use crate::content::tests::{write_aged, OLD};
        use crate::session_sync::tests::{add, app_on};
        let root = crate::store::testutil::temp_dir("prune_named_damaged");
        let (content_dir, sessions) = (root.join("content"), root.join("sessions"));
        std::fs::create_dir_all(&content_dir).unwrap();
        let orphan = content_dir.join("orphan.png");
        let prune = || {
            write_aged(&orphan, OLD);
            prune_unnamed(&content_dir, std::time::SystemTime::now(), HashSet::new(), Some(&sessions))
        };
        // No sessions dir yet names nothing.
        assert_eq!(prune(), Some(1));
        std::fs::create_dir_all(&sessions).unwrap();
        let saved = sessions.join("saved.mnf");
        let mut app = app_on(&root.join("session.mnf"), 1);
        add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "x".into(), editing: false }));
        assert!(app.save_session());
        app.journal.as_ref().unwrap().save_copy(&saved, None).unwrap();
        assert_eq!(prune(), Some(1));

        let local = crate::store::local::path_for(&saved);
        std::fs::write(&local, b"garbage").unwrap();
        assert_eq!(prune(), None, "a corrupt .mnf.local");
        std::fs::remove_file(&local).unwrap();

        let good = std::fs::read(&saved).unwrap();
        let mut damaged = good.clone();
        *damaged.last_mut().unwrap() ^= 1;
        std::fs::write(&saved, damaged).unwrap();
        assert_eq!(prune(), None, "a save that loads only by repair");
        std::fs::write(&saved, b"garbage").unwrap();
        assert_eq!(prune(), None, "a save that isn't a journal");
        assert!(orphan.exists());
    }

    #[test]
    fn a_repaired_store_is_never_pruned_against() {
        use crate::session_sync::tests::{add, app_on};
        let path = crate::store::testutil::temp_dir("prune_recovered").join("session.mnf");
        let mut app = app_on(&path, 1);
        add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "x".into(), editing: false }));
        assert!(app.save_session());
        assert!(app.may_prune_content());
        drop(app);
        // Damage the last byte: the repair skips that part and says so.
        let mut bytes = std::fs::read(&path).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        std::fs::write(&path, bytes).unwrap();
        let mut app = app_on(&path, 2);
        app.restore_session();
        assert!(app.store_recovered);
        assert!(!app.may_prune_content());
    }

    /// Review fix: a store that wasn't a journal is set aside and a new one started; that launch must not prune
    /// the old session's content against the new, empty one.
    #[test]
    fn a_store_set_aside_is_never_pruned_against() {
        let path = crate::store::testutil::temp_dir("prune_aside").join("session.mnf");
        std::fs::write(&path, b"not a session").unwrap();
        let mut app = App::default();
        app.open_store_at(&path);
        assert!(app.journal.is_some());
        assert!(app.store_recovered);
        assert!(!app.may_prune_content());
    }

    #[test]
    fn a_refresh_keeps_its_old_file_while_another_node_names_it() {
        let dir = crate::store::testutil::temp_dir("refresh_shared_file");
        let (old, new) = (dir.join("r.png"), dir.join("r.jpg"));
        let mut app = App::default();
        for id in ["r", "s"] {
            let path = old.to_string_lossy().into_owned();
            let payload = Runtime::Image(ImagePayload { path, width: 1, height: 1, error: None });
            app.insert_placeholder_node(id.into(), egui::Vec2::ZERO, Kind::Image, payload);
        }
        assert!(!replaced_file_unused(&dir, "r", &old, &new, &app.content_keep(Some("r"))));
        app.nodes.remove("s");
        assert!(replaced_file_unused(&dir, "r", &old, &new, &app.content_keep(Some("r"))));
        assert!(!replaced_file_unused(&dir, "r", &old, &old, &app.content_keep(Some("r"))));
    }

    #[test]
    fn closing_a_node_touches_its_own_content_file_only() {
        use crate::content::tests::{write_aged, OLD};
        let dir = crate::store::testutil::temp_dir("touch_closed");
        let (own, picked) = (dir.join("7-1.png"), dir.join("picked.png"));
        write_aged(&own, OLD);
        write_aged(&picked, OLD);
        let node = |path: &std::path::Path| store::json::JsonNode {
            id: "7-1".into(),
            fields: serde_json::Map::from_iter([
                ("type".into(), store::model::TYPE_IMAGE.into()),
                ("path".into(), path.to_string_lossy().into_owned().into()),
            ]),
        };
        let entry = ClosedEntry { doc: store::json::JsonDoc { nodes: vec![node(&own), node(&picked)], local: None } };
        touch_owned_content(&dir, &entry);
        let age = |p: &std::path::Path| std::fs::metadata(p).unwrap().modified().unwrap().elapsed().unwrap_or_default();
        assert!(age(&own) < std::time::Duration::from_secs(60), "touched");
        assert!(age(&picked) > crate::content::PRUNE_GRACE, "not its own <id>.<ext>: untouched");
    }

    /// A Refresh whose fetch fails, with the file still there, only toasts.
    #[test]
    fn a_failed_refresh_fetch_toasts() {
        let dir = crate::store::testutil::temp_dir("refresh_fetch_failed_toast");
        let mut app = App::default();
        let body = crate::redirect_fetch::save(&dir, "r", crate::content::ContentKind::Json, b"[1]".to_vec()).unwrap();
        app.insert_content(
            ContentSource::Web { url: "https://x.test/j".into(), body },
            "r".into(),
            egui::vec2(0.0, 0.0),
            None,
        )
        .unwrap();
        app.refresh_fetch_failed("r");
        assert_eq!(app.save_toast_queue.len(), 1);
        assert!(app.save_toast_queue[0].contains("Couldn't refresh"));
        // The node itself is untouched by a fetch-level failure.
        assert!(app.nodes.contains_key("r"));
    }

    #[test]
    fn a_refetched_file_reloads_the_node_in_place() {
        let dir = crate::store::testutil::temp_dir("refresh_reload");
        let mut app = App::default();
        let first = crate::redirect_fetch::save(&dir, "r", crate::content::ContentKind::Json, b"[1]".to_vec()).unwrap();
        app.insert_content(
            ContentSource::Web { url: "https://x.test/j".into(), body: first },
            "r".into(),
            egui::vec2(1.0, 2.0),
            None,
        )
        .unwrap();
        app.edit_field("r", field::COLOR, color_value(Some(egui::Color32::RED)));
        let order = app.node_order.clone();
        let second =
            crate::redirect_fetch::save(&dir, "r", crate::content::ContentKind::Json, b"[2]".to_vec()).unwrap();
        app.reload_fetched_node("r", second);
        let node = &app.nodes["r"];
        assert_eq!(*node.as_json().unwrap().value, serde_json::json!([2]));
        assert_eq!(
            (app.layout_pos("r"), app.mirror.pos("r"), app.mirror.color("r"), app.source_url("r")),
            (egui::vec2(1.0, 2.0), Some(egui::vec2(1.0, 2.0)), Some(egui::Color32::RED), Some("https://x.test/j"))
        );
        assert_eq!(app.node_order, order);
    }

    /// A refetched file that fails to load leaves a placeholder of the same
    /// kind in the node's place, with the error and the url.
    #[test]
    fn a_failed_refetch_keeps_a_placeholder_with_group_and_error() {
        let dir = crate::store::testutil::temp_dir("refresh_reload_fail");
        let mut app = App::default();
        let first = crate::redirect_fetch::save(&dir, "r", crate::content::ContentKind::Json, b"[1]".to_vec()).unwrap();
        app.insert_content(
            ContentSource::Web { url: "https://x.test/j".into(), body: first },
            "r".into(),
            egui::vec2(1.0, 2.0),
            Some("g".into()),
        )
        .unwrap();
        app.edit_field("r", field::COLOR, color_value(Some(egui::Color32::RED)));
        let order = app.node_order.clone();
        let bad = WebBody::File {
            kind: crate::content::ContentKind::Json,
            path: dir.join("missing.json").to_string_lossy().into_owned(),
        };
        app.reload_fetched_node("r", bad);
        let node = &app.nodes["r"];
        assert!(node.as_json().unwrap().error.is_some());
        assert_eq!(
            (app.layout_pos("r"), app.mirror.color("r"), app.mirror.group("r"), app.source_url("r")),
            (egui::vec2(1.0, 2.0), Some(egui::Color32::RED), Some("g"), Some("https://x.test/j"))
        );
        assert_eq!(app.node_order, order);
    }

    /// The spawn is parked in `test_spawns`; its id is already marked.
    #[test]
    fn open_as_tab_marks_the_new_tab_to_skip_its_first_conversion() {
        let dir = crate::store::testutil::temp_dir("open_as_tab");
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbaImage::new(1, 1).write_to(&mut png, image::ImageFormat::Png).unwrap();
        let path = dir.join("a.png");
        std::fs::write(&path, png.into_inner()).unwrap();

        let mut app = App::default();
        let source = ContentSource::Web {
            url: "https://x.test/a.png".into(),
            body: WebBody::File { kind: content::ContentKind::Image, path: path.to_string_lossy().into_owned() },
        };
        app.insert_content(source, "n".into(), egui::Vec2::ZERO, None).unwrap();
        app.open_as_tab("n");
        assert_eq!(app.test_spawns.len(), 1);
        let tab_id = app.test_spawns[0].0.id.clone();
        assert_eq!(app.test_spawns[0].0.url, "https://x.test/a.png");
        assert_eq!(app.test_spawns[0].0.parent.as_deref(), Some("n"), "opened from the node");
        assert_eq!(app.skip_convert.get(&tab_id), Some(&false));
    }

    /// Closing a node another node was opened from links that one to the closed node's own parent, as closing a
    /// tab does for its children.
    #[test]
    fn closing_a_content_node_moves_what_it_opened_up() {
        use crate::session_sync::tests::{add, sleeping_tab};
        let mut app = App::default();
        let opener = add(&mut app, Kind::Tab, sleeping_tab("https://x.test/", "Opener"));
        let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false }));
        app.edit_parent(&note, Some(opener.clone()));
        let tab = add(&mut app, Kind::Tab, sleeping_tab("https://x.test/from-note", "T"));
        app.edit_parent(&tab, Some(note.clone()));
        app.close_node(&note);
        assert_eq!(app.mirror.parent(&tab), Some(opener.as_str()));
    }

    /// The engine message arms need a live engine, so the entry's
    /// transitions are checked through `skip_convert_after` directly.
    #[test]
    fn skip_convert_survives_a_redirect_but_not_a_later_navigation() {
        // (url changed, loading) reports for one Open-as-Tab tab.
        let mut entry = Some(false);
        entry = skip_convert_after(entry, false, true);
        assert_eq!(entry, Some(false), "first load under way");
        entry = skip_convert_after(entry, true, true);
        assert_eq!(entry, Some(false), "an HTTP redirect keeps it");
        entry = skip_convert_after(entry, false, false);
        assert_eq!(entry, Some(true), "first load settled");
        entry = skip_convert_after(entry, false, true);
        assert_eq!(entry, Some(true), "F5 at the same url keeps it");
        entry = skip_convert_after(entry, true, true);
        assert_eq!(entry, None, "a navigation after it settled drops it");
        assert_eq!(skip_convert_after(None, true, false), None, "other tabs stay out");
    }

    /// Closing an Open-as-Tab tab drops its entry too.
    #[test]
    fn removing_a_tab_clears_its_skip_convert_flag() {
        use crate::session_sync::tests::{add, sleeping_tab};
        let mut app = App::default();
        let tab = add(&mut app, Kind::Tab, sleeping_tab("https://x.test/a.json", "A"));
        app.skip_convert.insert(tab.clone(), true);
        app.remove_tab(&tab);
        assert!(!app.skip_convert.contains_key(&tab));
    }

    /// A sleeping tab stands in for a live one: only its place is read.
    #[test]
    fn a_converted_tab_keeps_its_place_and_url() {
        use crate::session_sync::tests::{add, sleeping_tab};

        let dir = crate::store::testutil::temp_dir("redirect_convert");
        let mut app = App::default();
        let opener = add(&mut app, Kind::Tab, sleeping_tab("https://x.test/", "Opener"));
        let tab = add(&mut app, Kind::Tab, sleeping_tab("https://x.test/a.json", "A"));
        app.snap_layout(&tab, egui::vec2(5.0, 6.0));
        app.edit_group(&tab, Some("g".into()));
        app.edit_parent(&tab, Some(opener.clone()));
        let child = add(&mut app, Kind::Tab, sleeping_tab("https://x.test/child", "Child"));
        app.edit_parent(&child, Some(tab.clone()));

        let body = crate::redirect_fetch::save(&dir, "n", crate::content::ContentKind::Json, b"[1]".to_vec()).unwrap();
        app.redirect_tab_to_content(&tab, "n".into(), "https://x.test/a.json".into(), body);

        assert!(!app.nodes.contains_key(&tab));
        assert!(app.nodes["n"].as_json().is_some());
        assert_eq!((app.layout_pos("n"), app.mirror.group("n")), (egui::vec2(5.0, 6.0), Some("g")));
        assert_eq!(app.source_url("n"), Some("https://x.test/a.json"));
        assert_eq!(app.mirror.parent("n"), Some(opener.as_str()), "still opened from where the tab was");
        assert!(app.children_of(&tab).next().is_none(), "children moved up");
    }

    /// A converted tab leaves no reference behind: focus, split, selection
    /// and a pending respawn focus all clear, and the view drops to Graph.
    #[test]
    fn a_converted_tab_clears_every_stale_reference() {
        use crate::session_sync::tests::{add, sleeping_tab};

        let dir = crate::store::testutil::temp_dir("redirect_convert_stale");
        let mut app = App::default();
        let tab = add(&mut app, Kind::Tab, sleeping_tab("https://x.test/a.json", "A"));
        app.view_mode = ViewMode::Item;
        app.focused = Some(tab.clone());
        app.split_tab = Some(tab.clone());
        app.selected_tabs.insert(tab.clone());
        app.pending_respawn_focus = Some(tab.clone());

        let body = crate::redirect_fetch::save(&dir, "n", crate::content::ContentKind::Json, b"[1]".to_vec()).unwrap();
        app.redirect_tab_to_content(&tab, "n".into(), "https://x.test/a.json".into(), body);

        assert!(!app.nodes.contains_key(&tab));
        assert_eq!(app.focused, None, "focused cleared");
        assert_eq!(app.split_tab, None, "split cleared");
        assert!(!app.selected_tabs.contains(&tab), "selected_tabs cleared");
        assert_eq!(app.pending_respawn_focus, None, "pending respawn focus cleared");
        assert_eq!(app.view_mode, ViewMode::Graph, "falls back to Graph like any other close of the focused tab");
    }

    /// A web page stays a tab: a conversion whose insert fails keeps it.
    #[test]
    fn a_failed_conversion_keeps_the_tab() {
        use crate::session_sync::tests::{add, sleeping_tab};
        let dir = crate::store::testutil::temp_dir("redirect_convert_fail");
        let mut app = App::default();
        let tab = add(&mut app, Kind::Tab, sleeping_tab("https://x.test/a.png", "A"));
        let child = add(&mut app, Kind::Tab, sleeping_tab("https://x.test/child", "Child"));
        app.edit_parent(&child, Some(tab.clone()));
        app.focused = Some(tab.clone());
        let path = dir.join("n.png");
        std::fs::write(&path, b"not a picture").unwrap();
        let body = WebBody::File { kind: content::ContentKind::Image, path: path.to_string_lossy().into_owned() };
        app.redirect_tab_to_content(&tab, "n".into(), "https://x.test/a.png".into(), body);
        assert!(app.nodes.contains_key(&tab), "the tab stays");
        assert!(!app.nodes.contains_key("n"));
        assert_eq!(app.focused.as_deref(), Some(tab.as_str()));
        assert_eq!(app.mirror.parent(&child), Some(tab.as_str()));
        assert_eq!(app.save_toast_queue.len() + usize::from(app.save_toast.is_some()), 1, "toasted");
    }
}
