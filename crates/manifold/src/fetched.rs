//! Fetched and file-backed nodes: content keep/prune, refresh, inserting content nodes, file drops.

use super::*;
use crate::store::model::Kind;
use crate::store::model::NodeId;

/// Prunes `dir` of the files `only` picks that neither `keep` nor any named session in `sessions` names and that
/// were last modified before `cutoff`. The startup prune picks every file with `cutoff` a grace before its snapshot;
/// a release picks one node's files with no grace. `None`, deleting nothing, when a session can't be read whole
/// (`session::backing_names`).
pub(crate) fn prune_unnamed(
    dir: &std::path::Path,
    cutoff: std::time::SystemTime,
    mut keep: HashSet<String>,
    sessions: Option<&std::path::Path>,
    only: impl Fn(&str) -> bool,
) -> Option<usize> {
    if let Some(sessions) = sessions {
        keep.extend(session::backing_names(sessions)?);
    }
    Some(content::prune(dir, cutoff, |file| !only(file) || content::kept(&keep, file)))
}

/// The backing-file names `doc`'s live nodes hold (closed and hidden ones included, spec decision 11): each id
/// (a content copy is `<id>.<ext>`, a jar's cookies `<id>.json`) and each stored `path`'s file name. `except` leaves
/// one node out.
pub(crate) fn doc_backing_names(
    doc: &crate::store::model::Doc,
    except: Option<crate::store::model::NodeId>,
) -> HashSet<String> {
    let mut names = HashSet::new();
    for (id, node) in doc.live().filter(|&(id, _)| Some(id) != except) {
        names.insert(id.to_string());
        if let Some(path) = node.get(crate::store::model::field::PATH).as_str() {
            names.extend(content::file_name(path));
        }
    }
    names
}

/// Whether a Refresh of node `id` that saved `new` may delete the file it
/// replaced, `old`: only its own `<id>.<ext>` in `dir`, and only when
/// nothing else names it (`others`: `App::backing_keep` without `id`).
pub(crate) fn replaced_file_unused(
    dir: &std::path::Path,
    id: NodeId,
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
    /// The backing-file names this session still uses, for `content::kept` (spec "Backing files"): the shown
    /// nodes' ids (a replica's own content copy is `<id>.<ext>`, a jar's cookies `<id>.json`) and resolved file
    /// names, and every live node the document holds, closed and hidden ones included (`doc_backing_names`).
    /// `except` leaves out one node.
    pub(crate) fn backing_keep(&self, except: Option<NodeId>) -> HashSet<String> {
        let mut keep: HashSet<String> = HashSet::new();
        for (id, node) in self.nodes.iter().filter(|(id, _)| Some(**id) != except) {
            keep.extend(node.runtime.file_path().and_then(content::file_name));
            keep.insert(id.to_string());
        }
        if let Some(j) = &self.doc.journal {
            keep.extend(doc_backing_names(j.doc(), except));
        }
        keep
    }

    /// Whether this launch knows every node that may hold a backing file: not with no journal, a rebuilt one
    /// (`store_recovered`), or one a newer build wrote (read-only), any of which may not have loaded everything.
    pub(crate) fn may_prune(&self) -> bool {
        self.doc.journal.as_ref().is_some_and(|j| !j.read_only()) && !self.doc.store_recovered
    }

    /// Deletes the backing files nothing holds (spec "Backing files"), in the content and jars dirs, on a thread.
    /// Called once at launch, after restore. The thread also reads the named sessions, which hold their files too;
    /// a file touched within `content::PRUNE_GRACE` stays (a sibling instance may hold it, and
    /// one it wrote after this keep set was built is never a candidate).
    pub(crate) fn prune_backing_dirs(&self) {
        if !self.may_prune() {
            return;
        }
        let cutoff = std::time::SystemTime::now().checked_sub(content::PRUNE_GRACE).unwrap_or(std::time::UNIX_EPOCH);
        self.prune_backing_files(cutoff, None, "pruned");
    }

    /// A removed node's backing files (stem `id`, in the content and jars dirs), deleted at once unless something
    /// still holds them: the startup prune's keep rule with no grace. Called where a removal records no Recently
    /// Closed entry (`remove_remotely_deleted`), after the node has left `nodes`.
    pub(crate) fn release_backing(&self, id: NodeId) {
        if !self.may_prune() || self.backing_keep(None).contains(&id.to_string()) {
            return;
        }
        let cutoff = std::time::SystemTime::now() + std::time::Duration::from_secs(1);
        self.prune_backing_files(cutoff, Some(id.to_string()), "released");
    }

    /// The shared body: on a thread, prunes each backing dir of the files `only` picks (every file with `None`)
    /// that nothing holds and that were modified before `cutoff`.
    fn prune_backing_files(&self, cutoff: std::time::SystemTime, only: Option<String>, verb: &'static str) {
        let dirs: Vec<std::path::PathBuf> = [profile::content_dir(), self.jars_dir()].into_iter().flatten().collect();
        let keep = self.backing_keep(None);
        let sessions = session::named_sessions_dir();
        std::thread::spawn(move || {
            let pick = |file: &str| only.as_deref().is_none_or(|id| content::stem(file) == id);
            for dir in dirs {
                match prune_unnamed(&dir, cutoff, keep.clone(), sessions.as_deref(), pick) {
                    Some(0) => {}
                    Some(n) => eprintln!("backing files: {verb} {n} file(s) in {}", dir.display()),
                    None => eprintln!("backing files: a named session couldn't be read whole; not deleting"),
                }
            }
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
    pub(crate) fn retry_missing_file(&mut self, id: NodeId) {
        if self.nodes.get(&id).and_then(|n| n.as_unloaded()).is_some() {
            let kind = self.doc.mirror.view_kind(id);
            // A tab that failed for want of its engine can't succeed by retrying. A click does what the card and the
            // menu's first row say (`PickOutcome::retry_label`): offer the engine's download, say it is already
            // installing, say why it can't run here, or wake the tab when the engine is fine.
            match pick_outcome(kind, &self.view_status(kind)) {
                PickOutcome::AskInstall(kind) => {
                    self.offer_confirm(Confirm::View(ViewConfirm::Download(kind)));
                }
                PickOutcome::AlreadyInstalling(kind) => {
                    self.queue_toast(format!("{} is already installing", view_label(kind)))
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
        if let Some(pdf) = self.nodes.get_mut(&id).and_then(|n| n.as_pdf_mut()) {
            if matches!(pdf.outcome, Some(Err(_))) {
                pdf.outcome = None;
                self.workers.pdf_renderer.request(id, pdf.path.clone());
            }
            return;
        }
        if let Some(image) = self.nodes.get_mut(&id).and_then(|n| n.as_image_mut()) {
            if image.error.take().is_some() {
                self.workers.image_decoder.request(id, image.path.clone());
            }
            return;
        }
        let Some(node) = self.nodes.get(&id) else { return };
        let path = match &node.runtime {
            Runtime::Json(json) if json.error.is_some() => json.path().to_owned(),
            _ => return,
        };
        let (group, parent) = (self.doc.mirror.group(id), self.doc.mirror.parent(id));
        let pos = self.layout_pos(id);
        let in_order = self.node_order.contains(&id);
        match self.insert_json_node_with_id(id, path, pos, group) {
            Ok(()) => {
                // The insert replaced the node and pushed its id again;
                // keep it where it was.
                if in_order {
                    self.node_order.pop();
                }
                self.edit_parent(id, parent);
                self.save_session();
            }
            Err(error) => {
                if let Some(json) = self.nodes.get_mut(&id).and_then(|n| n.as_json_mut()) {
                    json.error = Some(error);
                }
            }
        }
    }

    /// Fetches a node's source URL again; `reload_fetched_node` swaps it
    /// in once it passes its check. Refresh, and a missing file.
    pub(crate) fn refresh_node(&mut self, id: NodeId) {
        use content::ContentKind as K;
        let Some(node) = self.nodes.get(&id) else { return };
        let kind = match &node.runtime {
            Runtime::Image(_) => K::Image,
            Runtime::Pdf(_) => K::Pdf,
            Runtime::Json(_) => K::Json,
            _ => return,
        };
        let Some(url) = self.source_url(id).map(str::to_string) else { return };
        let job = FetchJob { target: FetchTarget::Node(id), node_id: id, url, kind };
        self.workers.redirect_fetcher.request(job);
    }

    /// Reloads node `id` from freshly fetched content: same id, position,
    /// group, color and draw order. If the new content fails to load, the
    /// node isn't lost -- a placeholder of the same kind (mirroring
    /// `retry_missing_file`'s own failure handling, and `session_sync`'s
    /// restore-time placeholder) takes its place, carrying the error and
    /// the url.
    pub(crate) fn reload_fetched_node(&mut self, id: NodeId, body: WebBody) {
        use content::ContentKind as K;
        let Some(node) = self.nodes.get(&id) else { return };
        let WebBody::File { kind, path } = &body else { return };
        // Checked before removing anything: only these kinds have a placeholder.
        if !matches!(kind, K::Image | K::Pdf | K::Json) {
            return;
        }
        let (kind, path) = (*kind, path.clone());
        let new_path = path.clone();
        let (group, parent) = (self.doc.mirror.group(id), self.doc.mirror.parent(id));
        let pos = self.layout_pos(id);
        // A PNG turned JPEG keeps its id stem, so `prune` never frees it.
        let old_path = node.runtime.file_path().map(str::to_owned);
        let Some(url) = self.source_url(id).map(str::to_string) else { return };
        let at = self.node_order.iter().position(|n| *n == id);
        if let Some(gpu) = &mut self.gpu {
            gpu.free_image(id);
        }
        self.nodes.remove(&id);
        self.workers.cancel_node(id);
        self.node_order.retain(|n| *n != id);
        let source = ContentSource::Web { url: url.clone(), body };
        let result = self.insert_content(source, id, pos, group);
        if let Err(error) = &result {
            let ty = match kind {
                K::Image => Kind::Image,
                K::Pdf => Kind::Pdf,
                K::Json => Kind::Json,
                // Guarded above: `kind` is one of the three arms this
                // matches.
                _ => unreachable!("reload_fetched_node only reaches here for Image/Pdf/Json"),
            };
            self.insert_file_placeholder(id, pos, ty, path, Some(error.clone()));
        }
        self.edit_parent(id, parent);
        self.edit_group(id, group);
        // The url and stored path are the mirror's, which the remove and reinsert leave alone: the payload holds the
        // new file. The view is the mirror's too. Overrides the new
        // value can't resolve are dropped; an error card's view is left unpruned, since pruning against its `Null`
        // would wipe, and sync away, every override a Retry should get back.
        let value = self.nodes.get(&id).and_then(|n| n.as_json()).filter(|_| result.is_ok()).map(|j| j.value.clone());
        if let (Some(value), Some(view)) = (value, self.doc.mirror.view(id)) {
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
                if replaced_file_unused(&dir, id, old, new, &self.backing_keep(Some(id))) {
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
    pub(crate) fn refresh_fetch_failed(&mut self, id: NodeId) {
        let error = format!("Couldn't fetch {}", self.source_url(id).unwrap_or_default());
        let Some(node) = self.nodes.get_mut(&id) else { return };
        let missing = |path: &str| !std::path::Path::new(path).exists();
        let name = match &mut node.runtime {
            Runtime::Image(p) if missing(&p.path) => return p.error = Some(error),
            Runtime::Json(p) if missing(p.path()) => return p.error = Some(error),
            Runtime::Pdf(p) if missing(&p.path) => return p.outcome = Some(Err(error)),
            Runtime::Json(p) => crate::profile::file::file_name(std::path::Path::new(p.path())),
            Runtime::Image(ImagePayload { path, .. }) | Runtime::Pdf(PdfPayload { path, .. }) => {
                crate::profile::file::file_name(std::path::Path::new(path))
            }
            _ => return,
        };
        self.queue_toast(format!("Couldn't refresh {name}"));
    }

    /// A tab at the node's source URL beside it, which stays a tab on its
    /// first load (`skip_convert`), e.g. to read a PDF past page 1. The tab
    /// is opened from the node (`parent`), so the two stay linked.
    pub(crate) fn open_as_tab(&mut self, id: NodeId) {
        let Some(url) = self.source_url(id).map(str::to_string) else { return };
        let (pos, group) = (self.layout_pos(id) + egui::vec2(40.0, 40.0), self.doc.mirror.group(id));
        let tab = self.spawn_tab_at(&url, pos, group, Some(id));
        self.spawns.skip_convert.insert(tab, false);
    }

    /// The one way content becomes a node (see `content`): a picked,
    /// dropped or `ADDFILE` file, or a page a tab converted. `id`, `pos`
    /// and `group` place it. A failure is toasted here.
    pub(crate) fn insert_content(
        &mut self,
        source: ContentSource,
        id: NodeId,
        pos: egui::Vec2,
        group: Option<NodeId>,
    ) -> Result<(), String> {
        use content::ContentKind as K;
        let (url, kind, path) = match source {
            ContentSource::Web { url, body: WebBody::Text(text) } => {
                self.insert_note_with_text(id, text, Some(url), pos, group);
                return Ok(());
            }
            ContentSource::Web { url, body: WebBody::File { kind, path } } => (Some(url), kind, path),
            ContentSource::Local(path) => {
                let name = crate::profile::file::file_name(&path);
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
                        let payload = DownloadPayload::finished(path_str, bytes);
                        self.insert_placeholder_node(id, pos, Kind::Download, Runtime::Download(payload));
                        if self.nodes.contains_key(&id) {
                            self.edit_group(id, group);
                        }
                        return Ok(());
                    }
                    _ => (None, kind, path_str),
                }
            }
        };
        match kind {
            K::Image => self.insert_image_node_with_id(id, path, pos, group),
            K::Pdf => self.insert_pdf_node_with_id(id, path, pos, group),
            K::Json => self.insert_json_node_with_id(id, path, pos, group)?,
            // A web page never reaches here as a file (see `redirect_fetch`).
            K::Note | K::Page | K::Other => return Err(format!("{kind:?} is not a file kind")),
        }
        self.seed_source_url(id, url);
        Ok(())
    }

    /// A new content node's web source (`Mirror::url`, `App::source_url`), seeded once it is placed: a stored node
    /// shown again (a reload, a restore) keeps the one the mirror holds. `None`, a local file, seeds nothing.
    pub(crate) fn seed_source_url(&mut self, id: NodeId, url: Option<String>) {
        if let Some(url) = url.filter(|url| !url.is_empty() && self.nodes.contains_key(&id)) {
            self.doc.mirror.init_field(id, crate::store::model::field::URL, FieldValue::Str(url));
        }
    }

    pub(crate) fn insert_note_with_text(
        &mut self,
        id: NodeId,
        text: String,
        url: Option<String>,
        pos: egui::Vec2,
        group: Option<NodeId>,
    ) {
        let payload = NotePayload { text, editing: false };
        self.place_node(Some(id), Kind::Note, Runtime::Note(payload), Placement { group, pos });
        self.seed_source_url(id, url);
    }

    /// The insert every `image` card goes through: the caller picks the id,
    /// position and group -- `insert_content` a fresh id at the current
    /// level for a picked/dropped/`ADDFILE` image, session restore the saved
    /// id and position with no group, a reopen the stored position and
    /// group (`reopen_node`), among others. The node appears at once as a
    /// decoding placeholder (`ImagePayload::decoding`); `ImageDecoder`
    /// fills in the size, or the error, from a worker.
    pub(crate) fn insert_image_node_with_id(
        &mut self,
        id: NodeId,
        path: String,
        pos: egui::Vec2,
        group: Option<NodeId>,
    ) {
        // The card is placed at once with no size (`ImagePayload::decoding`); the file is read and decoded on a
        // worker (`ImageDecoder`), and `tick_fetchers` sizes the card, uploads the texture, or toasts and keeps
        // the error as the card's tooltip. Decoded by content, not by `path`'s extension: `insert_content` already
        // sniffed this file as an image, which a lying extension (a `.pdf` that's really a PNG) would defeat.
        self.workers.image_decoder.request(id, path.clone());
        let payload = ImagePayload { path, width: 0, height: 0, error: None };
        self.place_node(Some(id), Kind::Image, Runtime::Image(payload), Placement { group, pos });
    }

    /// Converts a tab whose page is content the table maps to a node (see
    /// `manifold_view_protocol::ViewToHost::RedirectableContent`): `body` is what
    /// `redirect_fetch` saved, inserted through `insert_content` at the
    /// tab's place, opened from where the tab was (its `parent`). Only once
    /// that succeeds does the tab go, through `remove_tab` (a close that
    /// isn't remembered, clearing every reference to it). A failed insert
    /// is toasted and the tab stays.
    pub(crate) fn redirect_tab_to_content(&mut self, tab_id: NodeId, node_id: NodeId, url: String, body: WebBody) {
        let group = self.doc.mirror.group(tab_id);
        if !self.nodes.contains_key(&tab_id) {
            return;
        }
        let pos = self.layout_pos(tab_id);
        let parent = self.doc.mirror.parent(tab_id);
        if self.insert_content(ContentSource::Web { url, body }, node_id, pos, group).is_ok() {
            self.edit_parent(node_id, parent);
            // The tab's links go with it (`remove_tab`); the new card takes over each.
            self.retarget_links(tab_id, node_id);
            self.remove_tab(tab_id);
            self.save_session();
        }
    }

    /// The insert every `pdf` card goes through: the caller picks the id,
    /// position and group -- `insert_content` a fresh id at the current
    /// level for a picked/dropped/`ADDFILE` PDF, session restore the saved
    /// id and position with no group, a reopen the stored position and
    /// group (`reopen_node`), among others. Like `insert_image_node_
    /// with_id`, this always inserts the node immediately (`outcome: None`,
    /// "still loading") rather than checking the file synchronously first --
    /// decoding a PDF is genuinely slow (see `PdfPayload`'s own doc comment);
    /// a bad/missing file surfaces later as a `Some(Err(_))` outcome.
    pub(crate) fn insert_pdf_node_with_id(&mut self, id: NodeId, path: String, pos: egui::Vec2, group: Option<NodeId>) {
        self.workers.pdf_renderer.request(id, path.clone());
        let pdf = Runtime::Pdf(PdfPayload { path, outcome: None });
        self.place_node(Some(id), Kind::Pdf, pdf, Placement { group, pos });
    }

    /// The insert every `json` card goes through: the caller picks the id,
    /// position and group -- `insert_content` a fresh id at the current
    /// level for a picked/dropped/`ADDFILE` JSON file, session restore the
    /// saved id and position with no group, a reopen the stored
    /// position and group (`reopen_node`), among others. Returns `Err`
    /// (doing nothing) if the file doesn't exist or isn't valid JSON, same
    /// fail-fast posture (a JSON parse on the UI thread is bounded by the
    /// body cap; moving it to a worker like `ImageDecoder` is parked).
    pub(crate) fn insert_json_node_with_id(
        &mut self,
        id: NodeId,
        path: String,
        pos: egui::Vec2,
        group: Option<NodeId>,
    ) -> Result<(), String> {
        // Toasted here, not by callers -- see `insert_image_node_with_id`.
        let value = match crate::profile::file::load::<serde_json::Value>(std::path::Path::new(&path), "JSON") {
            Ok(value) => value,
            Err(err) => {
                self.queue_toast(err.clone());
                return Err(err);
            }
        };
        let payload = Runtime::Json(JsonPayload::file(path, value.into(), None));
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
    /// `pos`: `None` for the view's centre; `Some(pos)` for the empty-canvas
    /// "New Tab/Note/Group/File" card (`pending_new_node`), which already
    /// knows exactly where it should land.
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

    /// `path` as a new node at `pos`, or the view's centre, at
    /// the current level (a JSON level is left first). The picker and
    /// `ADDFILE`.
    pub(crate) fn add_file_at(&mut self, path: std::path::PathBuf, pos: Option<egui::Vec2>) -> Result<(), String> {
        let id = self.alloc_node_id();
        let Placement { group, pos } = self.placement(id, pos);
        self.insert_content(ContentSource::Local(path), id, pos, group)
    }

    /// A file dropped on the window (spec "Drag and drop"): `point` is the
    /// drop location in window points. In Tab view, beside the focused
    /// tab, in its group -- `insert_content` directly, since that group may
    /// not be `current_group`. Otherwise (Graph view, or Tab view with no
    /// focused tab, which lands in the Graph's last view at the current level the
    /// same as any other point there would) the world position under
    /// `point`, via `add_file_at` (`current_group`). Several drops in one
    /// frame fan out by `drop_batch`, reset each frame in `render_frame`.
    pub(crate) fn drop_file(&mut self, path: std::path::PathBuf, point: egui::Pos2) {
        let fan_out = egui::vec2(24.0, 24.0) * self.drop_batch as f32;
        self.drop_batch += 1;
        let beside_focused = self
            .showing_tab_view()
            .then(|| self.focused.filter(|id| self.nodes.contains_key(id)))
            .flatten()
            .map(|id| (self.layout_pos(id) + egui::vec2(240.0, 0.0), self.group_of(self.focused)));
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

/// Where nodes' files are on this machine (spec "Where fetched files
/// live"): this machine's own fetch in the content dir (`<id>.<ext>`, the
/// newest if a Refresh changed the extension), which a Refresh replaces
/// without touching the shared `path`; else the stored path. A missing
/// stored path shows as missing, and `session_sync::materialize` refetches
/// it when the node has a URL. An unfinished `.tmp` save is never picked.
/// One directory read answers a whole restore or apply pass.
#[derive(Debug, Default)]
pub(crate) struct ContentIndex(HashMap<NodeId, (std::time::SystemTime, std::path::PathBuf)>);

impl ContentIndex {
    pub(crate) fn scan(dir: Option<&std::path::Path>) -> Self {
        let mut newest: HashMap<NodeId, (std::time::SystemTime, std::path::PathBuf)> = HashMap::new();
        let entries = dir.and_then(|dir| std::fs::read_dir(dir).ok()).into_iter().flatten().flatten();
        for e in entries {
            let name = e.file_name().to_string_lossy().into_owned();
            let Some((id, _)) = name.split_once('.') else { continue };
            let Ok(id) = id.parse::<NodeId>() else { continue };
            if name.ends_with(".tmp") {
                continue;
            }
            let modified = e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
            match newest.get(&id) {
                Some((at, _)) if *at >= modified => {}
                _ => {
                    newest.insert(id, (modified, e.path()));
                }
            }
        }
        Self(newest)
    }

    pub(crate) fn resolve(&self, id: NodeId, stored: &str) -> String {
        self.0.get(&id).map_or_else(|| stored.to_string(), |(_, path)| path.to_string_lossy().into_owned())
    }
}

#[cfg(test)]
mod tests;
