//! The graph's node model: `GraphNode`, its `Runtime` and the payload
//! structs the runtime's variants hold.

use super::*;

/// What a node holds on this machine beyond its stored fields (which are the
/// `Mirror`'s): the live state its card shows and works from. A node's type
/// is `Mirror::ty`, not the variant; the variant is the runtime shape that
/// type's card needs, and `None` for a kind with none (a group: its name,
/// `from_json` and default jar are all the mirror's). Tab/Download were the
/// migration off separate hardcoded node types (the old
/// `TabNode`/`DownloadNode`/`GroupNode`); `Note`/`Image`/`Pdf`/`Json` are
/// genuinely new kinds added on top of that unified model -- see each one's
/// own `*Payload` doc comment.
pub(crate) enum Runtime {
    Tab(TabPayload),
    Unloaded(UnloadedTabPayload),
    Download(DownloadPayload),
    Note(NotePayload),
    Image(ImagePayload),
    Pdf(PdfPayload),
    Json(JsonPayload),
    None,
}

/// `as_x` (and `as_x_mut` where listed) on `Runtime`, and the same
/// pair on `GraphNode` forwarding to its runtime.
macro_rules! runtime_accessors {
    ($( $variant:ident($ty:ty) => $as:ident $(, $as_mut:ident)?; )*) => {
        impl Runtime {
            $(
                pub(crate) fn $as(&self) -> Option<&$ty> {
                    match self {
                        Runtime::$variant(p) => Some(p),
                        _ => None,
                    }
                }
                $(
                    pub(crate) fn $as_mut(&mut self) -> Option<&mut $ty> {
                        match self {
                            Runtime::$variant(p) => Some(p),
                            _ => None,
                        }
                    }
                )?
            )*
        }

        impl GraphNode {
            $(
                pub(crate) fn $as(&self) -> Option<&$ty> {
                    self.runtime.$as()
                }
                $(
                    pub(crate) fn $as_mut(&mut self) -> Option<&mut $ty> {
                        self.runtime.$as_mut()
                    }
                )?
            )*
        }
    };
}

runtime_accessors! {
    Tab(TabPayload) => as_tab, as_tab_mut;
    Unloaded(UnloadedTabPayload) => as_unloaded;
    Download(DownloadPayload) => as_download, as_download_mut;
    Note(NotePayload) => as_note, as_note_mut;
    Image(ImagePayload) => as_image, as_image_mut;
    Pdf(PdfPayload) => as_pdf, as_pdf_mut;
    Json(JsonPayload) => as_json, as_json_mut;
}

impl Runtime {
    /// A live or unloaded tab's live url (what it is copied and reopened
    /// at); `None` for any other node. Its engine is `Mirror::engine`.
    pub(crate) fn saved_tab(&self) -> Option<&str> {
        match self {
            Runtime::Tab(t) => Some(&t.url),
            Runtime::Unloaded(t) => Some(&t.url),
            Runtime::Download(_)
            | Runtime::Note(_)
            | Runtime::Image(_)
            | Runtime::Pdf(_)
            | Runtime::Json(_)
            | Runtime::None => None,
        }
    }

    /// The file a content card shows on this machine (an image, PDF, Json file, or download): its stored
    /// path resolved here (`ContentIndex::resolve`), or a reload's new file. `None` for other kinds. The stored path is
    /// the mirror's (`Mirror::path`, 2b-5 Ruling 5).
    pub(crate) fn file_path(&self) -> Option<&str> {
        match self {
            Runtime::Image(p) => Some(&p.path),
            Runtime::Pdf(p) => Some(&p.path),
            Runtime::Json(p) => Some(&p.path),
            Runtime::Download(p) => Some(&p.path),
            Runtime::Tab(_) | Runtime::Unloaded(_) | Runtime::Note(_) | Runtime::None => None,
        }
    }

    /// A live or unloaded tab's title and url: what tab lists (Quick
    /// Switcher, Ctrl+Tab, the address bar's open tabs) show and match.
    pub(crate) fn listed_tab(&self) -> Option<(&str, &str)> {
        match self {
            Runtime::Tab(t) => Some((&t.title, &t.url)),
            Runtime::Unloaded(t) => Some((&t.title, &t.url)),
            Runtime::Download(_)
            | Runtime::Note(_)
            | Runtime::Image(_)
            | Runtime::Pdf(_)
            | Runtime::Json(_)
            | Runtime::None => None,
        }
    }
}

/// A sticky note's own data -- free-form user-typed text, no engine process
/// or IPC of any kind behind it (unlike every other node kind so far). The
/// first new kind added on top of the unified `GraphNode`/`Runtime`
/// model, rather than another migration of a pre-existing type -- proof the
/// model actually supports "arrange anything" the way it was built to.
/// Persisted in the session store (`field::TEXT`) since a note is
/// user-authored content, not a display preference like `Mirror::pinned`
/// or a group's own membership -- losing it on restart would defeat the
/// point of writing one down.
pub(crate) struct NotePayload {
    /// The text box's edit buffer (2b-4 Ruling 6). The text is the mirror's (`Mirror::text`); this copy follows it
    /// (`update_live_node`, `end_note_edit`) except while the box is typed in, when it holds what the user typed until
    /// a save or the blur commits it (`App::edit_text`). Read through `App::note_text`.
    pub(crate) text: String,
    /// `true` shows a real, focused `TextEdit` (today's own behavior);
    /// `false` renders `text` as real Markdown (`egui_commonmark`) instead,
    /// read-only, with the whole card (not just its header strip) clickable
    /// to switch back to editing -- the same "click to edit, blur to
    /// render" split Obsidian/Notion use, since a rendered view has no
    /// natural place to put a text cursor. Session-only -- not persisted
    /// (the store's note fields have no `editing`), so a restored note
    /// always starts rendered regardless of
    /// what it was showing when the app last closed. A freshly created
    /// note starts `true` instead, so typing into it works immediately
    /// with no extra click.
    pub(crate) editing: bool,
    // Where it came from on the web, when it did, is the mirror's (`Mirror::url`, `App::source_url`).
}

/// A locally-added picture, embedded directly on the graph -- the second
/// new node kind built on the unified model (see `NotePayload`'s own doc
/// comment for the first). Unlike a tab's thumbnail or a favicon, this
/// isn't fetched or re-rendered continuously: `path` is decoded, on a worker (`ImageDecoder`), at
/// creation (`App::insert_content`, from the picker, a drop, `ADDFILE`
/// or a converted tab), at session restore, or after a Refresh into the
/// texture `GpuState::images` keeps under this node's own id; the card exists meanwhile with no size
/// (`decoding`). `width`/`height` are the *decoded* image's
/// real pixel dimensions, kept so the card can size itself to the image's
/// own aspect ratio instead of stretching/squashing it into a fixed box.
/// Persisted in the session store (`field::PATH`) as just the file
/// path -- same "re-derive, don't duplicate the bytes" posture a restored
/// tab's fresh navigation already has, rather than embedding the picture's
/// own bytes a second time. If the file has since moved or been deleted,
/// restore keeps a placeholder card for it (`error`) rather than failing
/// the whole session load or forgetting the card.
pub(crate) struct ImagePayload {
    pub(crate) path: String,
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// `Some` for a card restored from the session store whose file
    /// couldn't be opened: no texture, `width`/`height` 0, drawn like a failed PDF
    /// with this message as its tooltip. It stays in the session, so the
    /// next launch tries the file again.
    pub(crate) error: Option<String>,
}

impl ImagePayload {
    /// No pixels yet and no error: the decode is queued or running (`ImageDecoder`), or the card waits for its file
    /// to be fetched again. `width` and `height` are 0 until the texture is uploaded.
    pub(crate) fn decoding(&self) -> bool {
        self.width == 0 && self.error.is_none()
    }
}

/// A locally-added PDF, rendered as its own graph node -- the third new
/// kind built on the unified model. Unlike an image, rendering a PDF page
/// is genuinely slow the first time (`pdf_render::PdfRenderer` may need to
/// download and cache a real `pdfium.dll` before it can render anything at
/// all), so `outcome` starts `None` (rendering in the
/// background) and is filled in once `PdfRenderer::tick` reports a result
/// for this node's own id, the same "spawn a thread, `mpsc` channel, main
/// loop `tick()`s it" pattern `SuggestionFetcher`/`FaviconFetcher` already
/// use. Only the first page is ever rendered -- no page-navigation UI yet,
/// same "start with the simplest useful slice" scope Notes' own initial cut
/// had for its context menu/drag-into-group (both added in a later step).
/// Persisted in the session store (`field::PATH`) as just the file
/// path, same "re-derive, don't duplicate" posture `ImagePayload` already
/// has -- restoring re-renders the first page from scratch rather than
/// caching the rendered bitmap anywhere.
pub(crate) struct PdfPayload {
    pub(crate) path: String,
    /// `None` while the background render is still in flight. `Some(Ok(_))`
    /// once the first page's texture is ready in `GpuState::images` (keyed
    /// by this node's own id -- the exact same map/texture path `Image`
    /// nodes use, since a rendered PDF page is just another RGBA bitmap by
    /// the time it reaches the GPU). `Some(Err(_))` if the background
    /// render failed (bad file, corrupt PDF, `pdfium.dll` download/bind
    /// failure) -- shown in place of the page image, distinct from "still
    /// loading" so a real failure isn't mistaken for a hang.
    pub(crate) outcome: Option<Result<PdfOutcome, String>>,
}

pub(crate) struct PdfOutcome {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) page_count: usize,
}

/// A locally-added JSON file, embedded directly on the graph -- the fourth
/// new kind built on the unified model. Unlike `Image`/`Pdf`, there's no GPU
/// texture at all: the card is drawn with plain egui widgets (a compact
/// shape summary), and double-clicking it enters the node's own JSON level
/// (`App::json_level`, drawn by `draw_json_level`) rather than opening a
/// popup. `value` is parsed once, at creation (`App::add_json_node`, via a
/// real file picker, same headless-unavailable posture `ImagePayload`
/// already has) or at session restore -- like `ImagePayload`, a
/// bad/no-longer-JSON file just means that one node is skipped at restore
/// rather than failing the whole session load.
pub(crate) struct JsonPayload {
    pub(crate) path: String,
    /// Shared with an open `JsonLevel` (entering the node doesn't copy it).
    pub(crate) value: std::sync::Arc<serde_json::Value>,
    /// Same as `ImagePayload::error`: a restored card whose file couldn't
    /// be loaded (`value` is `Null`).
    pub(crate) error: Option<String>,
    // Where it came from on the web (`Mirror::url`) and what the user changed about its level (spec "Saved view
    // state", `Mirror::view`) are the mirror's.
}

impl JsonPayload {
    /// Whether this node has a level to enter and a view to edit: a file's
    /// node whose value parsed. Not an error card (plan ruling
    /// 10), and not a refetch placeholder (`Null`, no error, its file not
    /// here yet), whose view must outlive the fetch. A file that really
    /// holds `null` is still on disk, so the stat runs only for `Null`.
    pub(crate) fn enterable(&self) -> bool {
        self.error.is_none() && !(self.value.is_null() && !std::path::Path::new(&self.path).exists())
    }
}

/// A tab without an engine: asleep since a restore (only the focused tab
/// starts at launch), waking (a spawn in flight), or failed (its engine
/// couldn't start: a crash restart, a restore or a retry). The card keeps
/// the tab where it was, and in the saved session as a plain tab. Opening
/// it (`focus_tab`), or a click on a failed card, starts its engine
/// (`App::wake_tab`), and a success replaces the card with the live tab
/// under the same id. It can stay `App::focused`: Tab view shows only live
/// tabs, so it shows the respawning placeholder while waking and falls
/// back to Graph otherwise.
pub(crate) struct UnloadedTabPayload {
    pub(crate) url: String,
    /// From personal state (`LocalNode::title`); the live tab reports its
    /// own once it loads.
    pub(crate) title: String,
    /// From personal state, like `title`.
    pub(crate) muted: bool,
    pub(crate) zoom_factor: f64,
    pub(crate) state: UnloadedState,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum UnloadedState {
    /// A tab that hasn't been opened yet: every restored tab but the
    /// focused one, and every tab another replica, Recently Closed or an
    /// import creates (`materialize`).
    Sleeping,
    /// A spawn is in flight: the card shows a spinner, and further opens
    /// wait for it.
    Waking,
    /// The engine couldn't start; the error is the card's tooltip.
    Failed(String),
}

/// A tab's own data -- was `TabNode`'s non-shared fields (`parent`/`group`, `color`, `pinned` and the
/// document pos live in the `Mirror`; where its card is drawn is `App::layout`). See
/// `Runtime`'s own doc comment.
pub(crate) struct TabPayload {
    pub(crate) tab: ViewTab,
    pub(crate) url: String,
    pub(crate) title: String,
    pub(crate) can_go_back: bool,
    pub(crate) can_go_forward: bool,
    pub(crate) loading: bool,
    /// Shown as a speaker icon on this tab's graph node — see
    /// `ipc_protocol::ViewToHost::AudioPlayingChanged`'s doc comment for
    /// which engines actually send this (WebView2, WebKit; never CEF or
    /// Servo, so this just stays `false` for those tabs).
    pub(crate) audio_playing: bool,
    /// User-toggled via the toolbar's "Muted" button — host-tracked (not
    /// read back from the engine), see `ipc_protocol::HostToView::
    /// SetMuted`'s doc comment. Resets to `false` on a fresh spawn/respawn/
    /// engine swap, same as every other per-tab UI-only flag here (nothing
    /// about mute state survives a swap to a different engine process).
    pub(crate) muted: bool,
    /// This tab's own current zoom, as last sent via `SetZoom` -- was
    /// implicitly just "whatever `Settings::zoom_factor` happens to be
    /// right now" (a single global value with no per-tab tracking at
    /// all), confirmed live to cause a real discrepancy: zooming tab A
    /// then switching to tab B left the Settings page's zoom control
    /// showing tab A's value while tab B's engine was still at its own
    /// (different) real zoom, so pressing "-" from that stale display
    /// jumped tab B's actual zoom by an unexpected amount. Initialized
    /// from `Settings::zoom_factor` at spawn time (a fresh tab still
    /// picks up "whatever the default is right now", unchanged), then
    /// tracked per-tab from there -- carried through a respawn/engine
    /// swap/crash recovery the same way `Mirror::pinned`/`Mirror::color` are. Not
    /// persisted to the session store: a restored tab starts at
    /// `Settings::zoom_factor`.
    pub(crate) zoom_factor: f64,
    /// Set while a swap is in flight for *this* tab: the engine to swap
    /// *to*, once this tab's `GetState` reply (its `StateSnapshot`) comes
    /// back. Per-tab (not a flat `App` field) so a swap on one tab can't
    /// block or interfere with a concurrent swap/crash on another.
    pub(crate) swap_target: Option<ViewKind>,
}

/// A download's own data -- was `DownloadNode`'s non-shared fields (`path`,
/// byte progress). See `Runtime`'s own doc comment for why this is
/// split out from the position/graph-membership fields `GraphNode` now
/// holds for every kind.
pub(crate) struct DownloadPayload {
    pub(crate) path: String,
    // What the engine fetched, if it said, is the mirror's (`Mirror::url`): not a refetch source, a download has no
    // Refresh.
    /// Bytes received so far -- created (node included) at
    /// `ViewToHost::DownloadStarted`, not only once finished, so a
    /// download is visible with live progress for its whole lifetime
    /// instead of only appearing once done. `None` total (see below) still
    /// updates this field; it just can't be shown as a fraction/percentage.
    pub(crate) bytes_received: u64,
    /// The server's reported final size, if any (a `Content-Length`-less
    /// response -- chunked transfer -- means `None`). Shown as a byte count
    /// with no percentage/bar in that case, rather than a misleading guess.
    pub(crate) total_bytes: Option<u64>,
    /// Set once `DownloadCompleted` arrives for this node's own `path`.
    /// Distinguishes an in-progress node (still shows a progress bar) from
    /// a finished one (plain filename, same as before this feature) without
    /// needing a separate enum -- nothing about a completed download's
    /// other fields becomes invalid or needs clearing.
    pub(crate) completed: bool,
    /// Set by `DownloadFailed` (cancelled or interrupted). Never both this and `completed`. Not stored: the store
    /// keeps only completed downloads, so a failed one is gone after a restart.
    pub(crate) failed: bool,
}

/// `App::nodes`'s own map type -- `std::collections::HashMap` (same type,
/// same API, at every one of its call sites throughout this file) with
/// `foldhash` plugged in as the hasher instead of the standard library's
/// own SipHash default. SipHash's DoS-resistance guarantee is wasted here:
/// every key is an app-generated id (`"tab-0"`, `"note-5"`, ...), never
/// derived from untrusted input, and this map is looked up many times per
/// frame across the Graph-view render loops. `foldhash` is already fully
/// resolved in this workspace's own dependency tree (see `Cargo.toml`'s
/// own comment on it) -- this is a hasher swap, not a new dependency.
pub(crate) type NodeMap = HashMap<String, GraphNode, foldhash::fast::RandomState>;

/// One node in the graph view. Its document fields (`pos`, `color`, `pinned`, `parent`, the "opened from" edge,
/// and `group`, the level it is shown in) are not fields here: see `Mirror`; where its card is drawn is
/// `App::layout`. `runtime` holds whatever live state the node's kind needs here (`Runtime`).
pub(crate) struct GraphNode {
    pub(crate) runtime: Runtime,
}

/// Where a node's card is drawn and how fast physics is moving it (`App::layout`, 2b-4 Ruling 1): runtime state,
/// personal to this replica. The force-directed settle and drags move it; it starts at the node's placement, a
/// remote pos snaps it, and personal state (`LocalNode::pos`) restores it. The document never reads it: only a drag
/// release (`App::edit_pos`) writes the document pos.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Layout {
    pub(crate) pos: egui::Vec2,
    pub(crate) vel: egui::Vec2,
}

impl Layout {
    /// At rest at `pos`.
    pub(crate) fn at(pos: egui::Vec2) -> Self {
        Layout { pos, vel: egui::Vec2::ZERO }
    }
}

impl GraphNode {
    /// What a screen reader announces for the node's card: its kind, then what identifies it. A content card is named
    /// by its web `source` (`App::source_url`), or by its file name when it has none; a note by the first line of
    /// `text` and a group by `text`, the stored text its card shows (`App::own_text`), ignored for other kinds.
    pub(crate) fn accessible_name(&self, text: &str, source: Option<&str>) -> String {
        let first_nonempty = |parts: &[&str]| parts.iter().find(|p| !p.trim().is_empty()).map(|p| p.trim().to_string());
        // Either separator: a saved path may come from the other platform.
        let file_name =
            |path: &str| path.rsplit(['/', '\\']).next().filter(|name| !name.is_empty()).map(str::to_string);
        let source_or_file = |path: &str| source.map(str::to_string).or_else(|| file_name(path));
        let (kind, detail) = match &self.runtime {
            Runtime::Tab(t) => ("Tab", first_nonempty(&[&t.title, &t.url])),
            Runtime::Unloaded(t) => ("Sleeping tab", first_nonempty(&[&t.title, &t.url])),
            Runtime::Download(d) => ("Download", file_name(&d.path)),
            // A group is the one kind with no runtime (2b-6 Ruling 8: no unknown-type card yet); its name is `text`.
            Runtime::None => {
                ("Group", Some(crate::names::to_plain(&crate::names::display_name(text, crate::names::UNTITLED))))
            }
            Runtime::Note(_) => {
                // The note's text is never cleaned; its first non-blank line is shown by the display rule.
                let first = text.lines().map(str::trim).find(|l| !l.is_empty());
                ("Note", first.map(|l| crate::names::to_plain(&crate::names::display_name(l, crate::names::UNTITLED))))
            }
            Runtime::Image(p) => ("Image", source_or_file(&p.path)),
            Runtime::Pdf(p) => ("PDF", source_or_file(&p.path)),
            Runtime::Json(p) => ("JSON", source_or_file(&p.path)),
        };
        match detail {
            Some(detail) => format!("{kind}: {detail}"),
            None => kind.to_string(),
        }
    }
}

impl App {
    /// `GraphNode::accessible_name` of node `id` with a rename (`NodeAction::CommitRename`) in place of the derived
    /// detail; a group's own name is already its detail. `None` for an id that isn't a node.
    pub(crate) fn accessible_name(&self, id: &str) -> Option<String> {
        let base = self.nodes.get(id)?.accessible_name(self.own_text(id), self.source_url(id));
        let renamed = self.mirror.name(id); // name rule: drawn through display_name
        if renamed.is_empty() || self.is_container(id) {
            return Some(base);
        }
        let kind = base.split(": ").next().unwrap_or(&base);
        let shown = crate::names::to_plain(&crate::names::display_name(renamed, crate::names::UNTITLED));
        Some(format!("{kind}: {shown}"))
    }

    /// The web source of an Image, Pdf, Json or Note node (`Mirror::url`): Refresh and Open as tab use it,
    /// and a missing file is fetched again from it. `None` for a local file and for other kinds.
    pub(crate) fn source_url(&self, id: &str) -> Option<&str> {
        let content = matches!(
            &self.nodes.get(id)?.runtime,
            Runtime::Image(_) | Runtime::Pdf(_) | Runtime::Json(_) | Runtime::Note(_)
        );
        Some(self.mirror.url(id)).filter(|url| content && !url.is_empty())
    }

    /// The URL a card's "Copy URL" puts on the clipboard: its `source_url`, or the address a download came from.
    pub(crate) fn copyable_url(&self, id: &str) -> Option<&str> {
        let download = self.nodes.get(id).is_some_and(|n| n.as_download().is_some());
        self.source_url(id).or_else(|| Some(self.mirror.url(id)).filter(|url| download && !url.is_empty()))
    }
}

#[cfg(test)]
impl App {
    /// Places a node `id` of type `kind` (its `TYPE` in the mirror) showing `runtime`, at the origin, as
    /// `insert_placeholder_node` would. For tests that need a node of a given type.
    pub(crate) fn test_node(&mut self, id: &str, kind: crate::store::model::Kind, runtime: Runtime) {
        self.insert_placeholder_node(id.to_owned(), egui::Vec2::ZERO, kind, runtime);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A card's screen-reader name is its kind, then its title (or the first line of a note).
    #[test]
    fn accessible_name_is_kind_then_title() {
        let mut app = crate::App::default();
        let (g, n) = (app.alloc_node_id(), app.alloc_node_id());
        app.insert_group_at(g.clone(), None);
        app.insert_note_at(n.clone(), None);
        let name = |app: &crate::App, id: &str| app.nodes[id].accessible_name(app.own_text(id), None);
        assert_eq!(name(&app, &n), "Note");
        app.nodes.get_mut(&n).unwrap().runtime.as_note_mut().unwrap().text = "
  first line
second"
            .into();
        assert_eq!(name(&app, &n), "Note: first line");
        app.mirror.set(&g, crate::store::model::field::NAME, crate::store::model::FieldValue::Str("Reading".into()));
        assert_eq!(name(&app, &g), "Group: Reading");
        app.mirror.set(
            &g,
            crate::store::model::field::NAME,
            crate::store::model::FieldValue::Str("Re\u{200B}ading".into()),
        );
        assert_eq!(name(&app, &g), "Group: Re<U+200B>ading");
        app.mirror.set(&g, crate::store::model::field::NAME, crate::store::model::FieldValue::Str(" ".into()));
        assert_eq!(name(&app, &g), "Group: Untitled");
        let raw = "\n\tRe\u{200B}ading\u{00A0}\u{00A0}list \nbody";
        app.nodes.get_mut(&n).unwrap().runtime.as_note_mut().unwrap().text = raw.into();
        assert_eq!(name(&app, &n), "Note: Re<U+200B>ading list");
        assert_eq!(app.nodes[&n].as_note().unwrap().text, raw, "the note's text is not cleaned");
    }

    /// Review C-B12: a local file's card is named by its file name; a web source still wins.
    #[test]
    fn a_local_file_card_has_an_accessible_name() {
        let node = |runtime| GraphNode { runtime };
        let pdf = node(Runtime::Pdf(PdfPayload { path: r"C:\docs\report.pdf".into(), outcome: None }));
        assert_eq!(pdf.accessible_name("", None), "PDF: report.pdf");
        let source = Some("https://example.org/r.pdf");
        assert_eq!(pdf.accessible_name("", source), "PDF: https://example.org/r.pdf");
        let image =
            node(Runtime::Image(ImagePayload { path: "/home/u/cat.png".into(), width: 1, height: 1, error: None }));
        assert_eq!(image.accessible_name("", None), "Image: cat.png");
        let json =
            node(Runtime::Json(JsonPayload { path: r"D:\data\x.json".into(), value: Default::default(), error: None }));
        assert_eq!(json.accessible_name("", None), "JSON: x.json");
    }

    /// An unloaded tab's card is saved, exported and copied as its tab.
    #[test]
    fn saved_tab_keeps_a_failed_tab() {
        assert_eq!(
            (Runtime::Unloaded(UnloadedTabPayload {
                url: "https://example.org/".into(),
                title: String::new(),
                muted: false,
                zoom_factor: 1.5,
                state: UnloadedState::Failed("no binary".into()),
            }))
            .saved_tab(),
            Some("https://example.org/")
        );
        assert_eq!(Runtime::None.saved_tab(), None);
    }
}
