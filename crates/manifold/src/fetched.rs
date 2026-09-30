//! Fetched and file-backed nodes: content keep/prune, refresh, inserting content nodes, file drops.

use super::*;

/// One "Recently Closed" entry: a store-shaped JSON snapshot of the
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

impl App {
    /// A download node for `path`, started from tab `tab_id`: placed next
    /// to that tab, in the tab's group, with an edge from it. Shared by an
    /// engine's `DownloadStarted` and the headless `ADDDOWNLOAD <path>`
    /// debug command (downloads otherwise only ever come from a real
    /// engine download, so headless runs had no way to get one). A no-op
    /// if `tab_id` is gone. The store keeps only completed downloads
    /// (`DownloadCompleted` saves), so no save.
    pub(crate) fn insert_download_node(&mut self, tab_id: String, path: String, total_bytes: Option<u64>) {
        let Some((node_pos, node_group)) = self.nodes.get(&tab_id).map(|n| (n.pos, n.group.clone())) else {
            return;
        };
        let id = self.alloc_node_id();
        let pos = node_pos + graph_layout::spawn_jitter(&id);
        self.nodes.insert(
            id.clone(),
            GraphNode {
                parent: Some(tab_id),
                group: node_group,
                pos,
                vel: egui::Vec2::ZERO,
                color: None,
                payload: NodePayload::Download(DownloadPayload {
                    path,
                    bytes_received: 0,
                    total_bytes,
                    completed: false,
                }),
            },
        );
        self.node_order.push(id);
    }

    /// Inserts a node showing `payload` at `pos`, top level and uncolored:
    /// `materialize`'s way in for kinds with no insert helper of their own,
    /// and for an image/JSON card whose file couldn't be loaded (see
    /// `ImagePayload::error`).
    pub(crate) fn insert_placeholder_node(&mut self, id: String, pos: egui::Vec2, payload: NodePayload) {
        let node = GraphNode { parent: None, group: None, pos, vel: egui::Vec2::ZERO, color: None, payload };
        self.nodes.insert(id.clone(), node);
        self.node_order.push(id);
    }

    /// Content-dir names this session still uses, for `content::kept`:
    /// live nodes' ids (a replica's own copy is `<id>.<ext>`) and `path`
    /// file names, and Recently Closed entries' `path` file names. `except`
    /// leaves out one live node.
    pub(crate) fn content_keep(&self, except: Option<&str>) -> HashSet<String> {
        let mut keep: HashSet<String> = HashSet::new();
        for (id, node) in self.nodes.iter().filter(|(id, _)| Some(id.as_str()) != except) {
            let path = match &node.payload {
                NodePayload::Image(p) => Some(&p.path),
                NodePayload::Pdf(p) => Some(&p.path),
                NodePayload::Json(p) => Some(&p.path),
                NodePayload::Download(p) => Some(&p.path),
                _ => None,
            };
            keep.extend(path.and_then(|p| content::file_name(p)));
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
        if let Some(kind) = self.nodes.get(id).and_then(|n| n.as_unloaded()).map(|t| t.kind) {
            self.wake_tab(id, kind);
            return;
        }
        if self.nodes.get(id).is_some_and(|n| n.source_url().is_some()) {
            self.refresh_node(id);
            return;
        }
        if let Some(pdf) = self.nodes.get_mut(id).and_then(|n| n.as_pdf_mut()) {
            if matches!(pdf.outcome, Some(Err(_))) {
                pdf.outcome = None;
                self.pdf_fetcher.request(id.to_string(), pdf.path.clone());
            }
            return;
        }
        let Some(node) = self.nodes.get(id) else { return };
        let (path, is_image) = match &node.payload {
            NodePayload::Image(image) if image.error.is_some() => (image.path.clone(), true),
            NodePayload::Json(json) if json.error.is_some() => (json.path.clone(), false),
            _ => return,
        };
        let (pos, group, parent, color) = (node.pos, node.group.clone(), node.parent.clone(), node.color);
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
                if let Some(node) = self.nodes.get_mut(id) {
                    node.parent = parent;
                    node.color = color;
                }
                self.save_session();
            }
            Err(error) => match self.nodes.get_mut(id).map(|n| &mut n.payload) {
                Some(NodePayload::Image(image)) => image.error = Some(error),
                Some(NodePayload::Json(json)) => json.error = Some(error),
                _ => {}
            },
        }
    }

    /// Fetches a node's source URL again; `reload_fetched_node` swaps it
    /// in once it passes its check. Refresh, and a missing file.
    pub(crate) fn refresh_node(&mut self, id: &str) {
        use content::ContentKind as K;
        let Some(node) = self.nodes.get(id) else { return };
        let kind = match &node.payload {
            NodePayload::Image(_) => K::Image,
            NodePayload::Pdf(_) => K::Pdf,
            NodePayload::Json(j) if j.key.is_none() => K::Json,
            _ => return,
        };
        let Some(url) = node.source_url().map(str::to_string) else { return };
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
        let (pos, group, parent, color) = (node.pos, node.group.clone(), node.parent.clone(), node.color);
        let Some(url) = node.source_url().map(str::to_string) else { return };
        // A PNG turned JPEG keeps its id stem, so `prune` never frees it.
        let old_path = match &node.payload {
            NodePayload::Image(p) => Some(p.path.clone()),
            NodePayload::Pdf(p) => Some(p.path.clone()),
            NodePayload::Json(p) => Some(p.path.clone()),
            _ => None,
        };
        let view = node.as_json().map(|j| j.view.clone()).unwrap_or_default();
        let at = self.node_order.iter().position(|n| n == id);
        if let Some(gpu) = &mut self.gpu {
            gpu.free_image(id);
        }
        self.nodes.remove(id);
        self.node_order.retain(|n| n != id);
        let source = ContentSource::Web { url: url.clone(), body };
        let result = self.insert_content(source, id.to_string(), pos, group.clone());
        if let Err(error) = &result {
            let payload = match kind {
                K::Image => NodePayload::Image(ImagePayload {
                    path,
                    width: 0,
                    height: 0,
                    error: Some(error.clone()),
                    url: Some(url),
                }),
                K::Pdf => NodePayload::Pdf(PdfPayload { path, outcome: Some(Err(error.clone())), url: Some(url) }),
                K::Json => NodePayload::Json(JsonPayload {
                    path,
                    value: serde_json::Value::Null,
                    key: None,
                    error: Some(error.clone()),
                    url: Some(url),
                    view: json_view::JsonView::default(),
                }),
                // Guarded above: `kind` is one of the three arms this
                // matches.
                _ => unreachable!("reload_fetched_node only reaches here for Image/Pdf/Json"),
            };
            self.insert_placeholder_node(id.to_string(), pos, payload);
        }
        if let Some(node) = self.nodes.get_mut(id) {
            (node.parent, node.color, node.group) = (parent, color, group);
            // The view goes back unpruned onto an error card: pruning
            // against its `Null` would wipe, and sync away, every override
            // a Retry should get back.
            if let NodePayload::Json(json) = &mut node.payload {
                json.view = view;
                if result.is_ok() {
                    json.view.retain_resolvable(&json.value);
                }
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
        let Some(node) = self.nodes.get_mut(id) else { return };
        let error = format!("Couldn't fetch {}", node.source_url().unwrap_or_default());
        let missing = |path: &str| !std::path::Path::new(path).exists();
        let name = match &mut node.payload {
            NodePayload::Image(p) if missing(&p.path) => return p.error = Some(error),
            NodePayload::Json(p) if missing(&p.path) => return p.error = Some(error),
            NodePayload::Pdf(p) if missing(&p.path) => return p.outcome = Some(Err(error)),
            NodePayload::Image(ImagePayload { path, .. })
            | NodePayload::Json(JsonPayload { path, .. })
            | NodePayload::Pdf(PdfPayload { path, .. }) => crate::json_file::file_name(std::path::Path::new(path)),
            _ => return,
        };
        self.queue_toast(format!("Couldn't refresh {name}"));
    }

    /// A tab at the node's source URL beside it, which stays a tab on its
    /// first load (`skip_convert`), e.g. to read a PDF past page 1.
    pub(crate) fn open_as_tab(&mut self, id: &str) {
        let Some(node) = self.nodes.get(id) else { return };
        let Some(url) = node.source_url().map(str::to_string) else { return };
        let (pos, group) = (node.pos + egui::vec2(40.0, 40.0), node.group.clone());
        let tab = self.spawn_tab_at(&url, pos, group);
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
                        self.spawn_tab_at(&content::path_file_url(&path), pos, group);
                        return Ok(());
                    }
                    K::Other => {
                        let bytes = std::fs::metadata(&path).map_or(0, |m| m.len());
                        let payload = DownloadPayload {
                            path: path_str,
                            bytes_received: bytes,
                            total_bytes: Some(bytes),
                            completed: true,
                        };
                        self.insert_placeholder_node(id.clone(), pos, NodePayload::Download(payload));
                        if let Some(node) = self.nodes.get_mut(&id) {
                            node.group = group;
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
        if let Some(slot) = self.nodes.get_mut(&id).and_then(|n| n.source_url_mut()) {
            *slot = url;
        }
        Ok(())
    }

    pub(crate) fn insert_note_with_text(
        &mut self,
        id: String,
        text: String,
        url: Option<String>,
        pos: egui::Vec2,
        group: Option<String>,
    ) {
        let payload = NotePayload { text, editing: false, url };
        self.insert_placeholder_node(id.clone(), pos, NodePayload::Note(payload));
        if let Some(node) = self.nodes.get_mut(&id) {
            node.group = group;
        }
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
        let rgba = img.to_rgba8();
        let (width, height) = rgba.dimensions();
        if let Some(gpu) = &mut self.gpu {
            gpu.upload_image(&id, rgba.as_raw(), width, height);
        }
        self.nodes.insert(
            id.clone(),
            GraphNode {
                parent: None,
                group,
                pos,
                vel: egui::Vec2::ZERO,
                color: None,
                payload: NodePayload::Image(ImagePayload { path, width, height, error: None, url: None }),
            },
        );
        self.node_order.push(id);
        Ok(())
    }

    /// Converts a tab whose page is content the table maps to a node (see
    /// `ipc_protocol::EngineToHost::RedirectableContent`): `body` is what
    /// `redirect_fetch` saved, inserted through `insert_content` at the
    /// tab's place. Only once that succeeds does the tab go, through
    /// `remove_tab` (a close that isn't remembered, clearing every
    /// reference to it). A failed insert is toasted and the tab stays.
    pub(crate) fn redirect_tab_to_content(&mut self, tab_id: &str, node_id: String, url: String, body: WebBody) {
        let Some((group, pos)) = self.nodes.get(tab_id).map(|n| (n.group.clone(), n.pos)) else {
            return;
        };
        if self.insert_content(ContentSource::Web { url, body }, node_id, pos, group).is_ok() {
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
        self.pdf_fetcher.request(id.clone(), path.clone());
        self.nodes.insert(
            id.clone(),
            GraphNode {
                parent: None,
                group,
                pos,
                vel: egui::Vec2::ZERO,
                color: None,
                payload: NodePayload::Pdf(PdfPayload { path, outcome: None, url: None }),
            },
        );
        self.node_order.push(id);
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
        self.nodes.insert(
            id.clone(),
            GraphNode {
                parent: None,
                group,
                pos,
                vel: egui::Vec2::ZERO,
                color: None,
                payload: NodePayload::Json(JsonPayload {
                    path,
                    value,
                    key: None,
                    error: None,
                    url: None,
                    view: json_view::JsonView::default(),
                }),
            },
        );
        self.node_order.push(id);
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
        self.leave_json_level();
        let id = self.alloc_node_id();
        let pos = pos.unwrap_or_else(|| GRAPH_WORLD_CENTER + graph_layout::spawn_jitter(&id));
        let group = self.insertion_group();
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
            .then(|| self.focused.as_ref().and_then(|id| self.nodes.get(id)))
            .flatten()
            .map(|node| (node.pos + egui::vec2(240.0, 0.0), node.group.clone()));
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
