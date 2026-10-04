//! The graph's node model: `GraphNode`, its `NodePayload` kinds and their
//! payload structs.

use super::*;

/// Kind-specific data for one node in the unified graph-node model --
/// `GraphNode` holds the fields every kind shares (position, graph
/// membership); this holds only what varies per kind. Tab/Download/Group
/// were the migration off three separate hardcoded node types (the old
/// `TabNode`/`DownloadNode`/`GroupNode`); `Note`/`Image`/`Pdf`/`Json` are
/// genuinely new kinds added on top of that unified model -- see each one's
/// own `*Payload` doc comment.
pub(crate) enum NodePayload {
    Tab(TabPayload),
    Unloaded(UnloadedTabPayload),
    Download(DownloadPayload),
    Group(GroupPayload),
    Note(NotePayload),
    Image(ImagePayload),
    Pdf(PdfPayload),
    Json(JsonPayload),
}

impl NodePayload {
    pub(crate) fn as_tab(&self) -> Option<&TabPayload> {
        match self {
            NodePayload::Tab(t) => Some(t),
            _ => None,
        }
    }

    pub(crate) fn as_tab_mut(&mut self) -> Option<&mut TabPayload> {
        match self {
            NodePayload::Tab(t) => Some(t),
            _ => None,
        }
    }

    pub(crate) fn as_unloaded(&self) -> Option<&UnloadedTabPayload> {
        match self {
            NodePayload::Unloaded(t) => Some(t),
            _ => None,
        }
    }

    /// The url and engine a live or unloaded tab is saved (and exported,
    /// and copied) with.
    pub(crate) fn saved_tab(&self) -> Option<(&str, EngineKind)> {
        match self {
            NodePayload::Tab(t) => Some((&t.url, t.tab.kind)),
            NodePayload::Unloaded(t) => Some((&t.url, t.kind)),
            NodePayload::Download(_)
            | NodePayload::Group(_)
            | NodePayload::Note(_)
            | NodePayload::Image(_)
            | NodePayload::Pdf(_)
            | NodePayload::Json(_) => None,
        }
    }

    /// A live or unloaded tab's title and url: what tab lists (Quick
    /// Switcher, Ctrl+Tab, the address bar's open tabs) show and match.
    pub(crate) fn listed_tab(&self) -> Option<(&str, &str)> {
        match self {
            NodePayload::Tab(t) => Some((&t.title, &t.url)),
            NodePayload::Unloaded(t) => Some((&t.title, &t.url)),
            NodePayload::Download(_)
            | NodePayload::Group(_)
            | NodePayload::Note(_)
            | NodePayload::Image(_)
            | NodePayload::Pdf(_)
            | NodePayload::Json(_) => None,
        }
    }

    pub(crate) fn pinned_mut(&mut self) -> Option<&mut bool> {
        match self {
            NodePayload::Tab(t) => Some(&mut t.pinned),
            NodePayload::Unloaded(t) => Some(&mut t.pinned),
            NodePayload::Download(_)
            | NodePayload::Group(_)
            | NodePayload::Note(_)
            | NodePayload::Image(_)
            | NodePayload::Pdf(_)
            | NodePayload::Json(_) => None,
        }
    }

    /// Whether a live or unloaded tab is pinned in place on the graph.
    pub(crate) fn pinned(&self) -> bool {
        match self {
            NodePayload::Tab(t) => t.pinned,
            NodePayload::Unloaded(t) => t.pinned,
            NodePayload::Download(_)
            | NodePayload::Group(_)
            | NodePayload::Note(_)
            | NodePayload::Image(_)
            | NodePayload::Pdf(_)
            | NodePayload::Json(_) => false,
        }
    }

    pub(crate) fn as_download(&self) -> Option<&DownloadPayload> {
        match self {
            NodePayload::Download(d) => Some(d),
            _ => None,
        }
    }

    pub(crate) fn as_download_mut(&mut self) -> Option<&mut DownloadPayload> {
        match self {
            NodePayload::Download(d) => Some(d),
            _ => None,
        }
    }

    pub(crate) fn as_group(&self) -> Option<&GroupPayload> {
        match self {
            NodePayload::Group(g) => Some(g),
            _ => None,
        }
    }

    pub(crate) fn as_group_mut(&mut self) -> Option<&mut GroupPayload> {
        match self {
            NodePayload::Group(g) => Some(g),
            _ => None,
        }
    }

    pub(crate) fn as_note(&self) -> Option<&NotePayload> {
        match self {
            NodePayload::Note(n) => Some(n),
            _ => None,
        }
    }

    pub(crate) fn as_note_mut(&mut self) -> Option<&mut NotePayload> {
        match self {
            NodePayload::Note(n) => Some(n),
            _ => None,
        }
    }

    pub(crate) fn as_image(&self) -> Option<&ImagePayload> {
        match self {
            NodePayload::Image(i) => Some(i),
            _ => None,
        }
    }

    pub(crate) fn as_pdf(&self) -> Option<&PdfPayload> {
        match self {
            NodePayload::Pdf(p) => Some(p),
            _ => None,
        }
    }

    pub(crate) fn as_pdf_mut(&mut self) -> Option<&mut PdfPayload> {
        match self {
            NodePayload::Pdf(p) => Some(p),
            _ => None,
        }
    }

    pub(crate) fn as_json(&self) -> Option<&JsonPayload> {
        match self {
            NodePayload::Json(j) => Some(j),
            _ => None,
        }
    }
}

/// A cheap, owned, `Copy` classifier for `&NodePayload` -- computed once
/// up front so its immutable borrow of `self.nodes` ends immediately,
/// before a dispatcher needs `&mut self` to render that kind's toolbar
/// content. Mirrors `close_node`'s own match for exactly the same reason:
/// matching directly on `&NodePayload` would keep that borrow alive for as
/// long as the match arms run, which conflicts with the `&mut self` those
/// arms need.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ItemNodeKind {
    Tab,
    Unloaded,
    Download,
    Group,
    Note,
    Image,
    Pdf,
    Json,
}

/// Every kind, in the order Graph view draws their cards: a later kind
/// draws, and takes the pointer, on top of an earlier one.
pub(crate) const GRAPH_DRAW_ORDER: [ItemNodeKind; 8] = [
    ItemNodeKind::Tab,
    ItemNodeKind::Unloaded,
    ItemNodeKind::Download,
    ItemNodeKind::Note,
    ItemNodeKind::Image,
    ItemNodeKind::Pdf,
    ItemNodeKind::Json,
    ItemNodeKind::Group,
];

pub(crate) fn item_node_kind(payload: &NodePayload) -> ItemNodeKind {
    match payload {
        NodePayload::Tab(_) => ItemNodeKind::Tab,
        NodePayload::Unloaded(_) => ItemNodeKind::Unloaded,
        NodePayload::Download(_) => ItemNodeKind::Download,
        NodePayload::Group(_) => ItemNodeKind::Group,
        NodePayload::Note(_) => ItemNodeKind::Note,
        NodePayload::Image(_) => ItemNodeKind::Image,
        NodePayload::Pdf(_) => ItemNodeKind::Pdf,
        NodePayload::Json(_) => ItemNodeKind::Json,
    }
}

/// A sticky note's own data -- free-form user-typed text, no engine process
/// or IPC of any kind behind it (unlike every other node kind so far). The
/// first new kind added on top of the unified `GraphNode`/`NodePayload`
/// model, rather than another migration of a pre-existing type -- proof the
/// model actually supports "arrange anything" the way it was built to.
/// Persisted in the session store (`field::TEXT`) since a note is
/// user-authored content, not a display preference like `TabPayload::pinned`
/// or a group's own membership -- losing it on restart would defeat the
/// point of writing one down.
pub(crate) struct NotePayload {
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
    /// Where this content came from on the web, when it did (see
    /// `content`): Refresh and Open as tab use it, and a missing file is
    /// fetched again from it. `None` for a local file.
    pub(crate) url: Option<String>,
}

/// A locally-added picture, embedded directly on the graph -- the second
/// new node kind built on the unified model (see `NotePayload`'s own doc
/// comment for the first). Unlike a tab's thumbnail or a favicon, this
/// isn't fetched or re-rendered continuously: `path` is decoded at
/// creation (`App::insert_content`, from the picker, a drop, `ADDFILE`
/// or a converted tab), at session restore, or after a Refresh into the
/// texture `GpuState::images` keeps under this node's own id. `width`/`height` are the *decoded* image's
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
    /// Where this content came from on the web, when it did (see
    /// `content`): Refresh and Open as tab use it, and a missing file is
    /// fetched again from it. `None` for a local file.
    pub(crate) url: Option<String>,
}

/// A locally-added PDF, rendered as its own graph node -- the third new
/// kind built on the unified model. Unlike an image, rendering a PDF page
/// is genuinely slow the first time (`pdf_render::PdfRenderer` may need to
/// download and cache a real `pdfium.dll` before it can render anything at
/// all), so this can't decode synchronously at creation the way
/// `ImagePayload` does -- `outcome` starts `None` (rendering in the
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
    /// Where this content came from on the web, when it did (see
    /// `content`): Refresh and Open as tab use it, and a missing file is
    /// fetched again from it. `None` for a local file.
    pub(crate) url: Option<String>,
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
    /// The JSON key (or `[index]` for an array element) this value came
    /// from -- `None` for a genuine top-level file-backed JSON node (its
    /// card just shows the value's own shape), `Some(k)` for a leaf that an
    /// older build's "Expand as Graph" made, whose card shows `"{k}: {value}"`
    /// instead so it's clear which field this is. Such a leaf is stored as
    /// its own `JsonLeaf` node, not this field, but still loads from a
    /// session saved before that feature was removed -- see
    /// `GroupPayload::from_json`.
    pub(crate) key: Option<String>,
    /// Same as `ImagePayload::error`: a restored card whose file couldn't
    /// be loaded (`value` is `Null`).
    pub(crate) error: Option<String>,
    /// Where this content came from on the web, when it did (see
    /// `content`): Refresh and Open as tab use it, and a missing file is
    /// fetched again from it. `None` for a local file, and for a leaf
    /// (`key.is_some()`, never persisted).
    pub(crate) url: Option<String>,
    /// What the user changed about this node's level (spec "Saved view
    /// state"): the store's `view` field. Empty for a leaf.
    pub(crate) view: crate::json_view::JsonView,
}

impl JsonPayload {
    /// Whether this node has a level to enter and a view to edit: a file's
    /// node whose value parsed. Not a leaf, not an error card (plan ruling
    /// 10), and not a refetch placeholder (`Null`, no error, its file not
    /// here yet), whose view must outlive the fetch. A file that really
    /// holds `null` is still on disk, so the stat runs only for `Null`.
    pub(crate) fn enterable(&self) -> bool {
        self.key.is_none()
            && self.error.is_none()
            && !(self.value.is_null() && !std::path::Path::new(&self.path).exists())
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
    pub(crate) kind: EngineKind,
    pub(crate) pinned: bool,
    /// From personal state, like `title`.
    pub(crate) muted: bool,
    pub(crate) zoom_factor: f64,
    pub(crate) state: UnloadedState,
    /// See `jar::JarChoice`. Synced (`field::JAR`, `field::JAR_SET`).
    pub(crate) jar: crate::jar::JarChoice,
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

/// A tab's own data -- was `TabNode`'s non-shared fields (`parent`/`group`/
/// `pos`/`vel`/`color` moved to the now-shared `GraphNode`). See
/// `NodePayload`'s own doc comment.
pub(crate) struct TabPayload {
    pub(crate) tab: EngineTab,
    pub(crate) url: String,
    pub(crate) title: String,
    pub(crate) can_go_back: bool,
    pub(crate) can_go_forward: bool,
    pub(crate) loading: bool,
    /// Shown as a speaker icon on this tab's graph node — see
    /// `ipc_protocol::EngineToHost::AudioPlayingChanged`'s doc comment for
    /// which engines actually send this (WebView2, WebKit; never CEF or
    /// Servo, so this just stays `false` for those tabs).
    pub(crate) audio_playing: bool,
    /// User-toggled via the toolbar's "Muted" button — host-tracked (not
    /// read back from the engine), see `ipc_protocol::HostToEngine::
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
    /// swap/crash recovery the same way `pinned`/`color` already are. Not
    /// persisted to the session store: a restored tab starts at
    /// `Settings::zoom_factor`.
    pub(crate) zoom_factor: f64,
    /// Graph view's pin toggle — a pinned node is never moved by the
    /// force-directed layout (the same `pinned` handling `graph_layout::step`
    /// gives a node being dragged; it still pushes its neighbours), so it
    /// stays exactly where the user put it instead of drifting under
    /// repulsion/centering forces as siblings come and go. Saved as
    /// `field::PINNED`.
    pub(crate) pinned: bool,
    /// Set while a swap is in flight for *this* tab: the engine to swap
    /// *to*, once this tab's `GetState` reply (its `StateSnapshot`) comes
    /// back. Per-tab (not a flat `App` field) so a swap on one tab can't
    /// block or interfere with a concurrent swap/crash on another.
    pub(crate) swap_target: Option<EngineKind>,
    /// See `jar::JarChoice`. Synced (`field::JAR`, `field::JAR_SET`).
    pub(crate) jar: crate::jar::JarChoice,
}

/// A download's own data -- was `DownloadNode`'s non-shared fields (`path`,
/// byte progress). See `NodePayload`'s own doc comment for why this is
/// split out from the position/graph-membership fields `GraphNode` now
/// holds for every kind.
pub(crate) struct DownloadPayload {
    pub(crate) path: String,
    /// What the engine fetched, if it said (not a refetch source: a download has no Refresh).
    pub(crate) url: Option<String>,
    /// Bytes received so far -- created (node included) at
    /// `EngineToHost::DownloadStarted`, not only once finished, so a
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

/// One node in the graph view -- fields every kind shares (position, graph
/// membership) live here; `payload` holds whatever's specific to the
/// node's actual kind. See `NodePayload`'s own doc comment for the
/// migration this is a step of.
pub(crate) struct GraphNode {
    /// The "opened from" edge for a tab (which tab spawned it, making the
    /// tab collection a forest for Graph view), or, for a download, the tab
    /// it was downloaded from. `None` for a root node, and always `None`
    /// for a group (never "opened from").
    pub(crate) parent: Option<String>,
    /// Which group's subgraph this node is shown in -- `None` at the top
    /// level. Groups nest, so a group node has one too; see
    /// `GroupPayload`'s own doc comment.
    pub(crate) group: Option<String>,
    pub(crate) pos: egui::Vec2,
    pub(crate) vel: egui::Vec2,
    /// User-chosen tag color -- `None` for any kind that doesn't use one
    /// (`Download` doesn't; `Tab`/`Group` do).
    pub(crate) color: Option<egui::Color32>,
    pub(crate) payload: NodePayload,
}

impl GraphNode {
    pub(crate) fn as_tab(&self) -> Option<&TabPayload> {
        self.payload.as_tab()
    }

    pub(crate) fn as_tab_mut(&mut self) -> Option<&mut TabPayload> {
        self.payload.as_tab_mut()
    }

    pub(crate) fn as_unloaded(&self) -> Option<&UnloadedTabPayload> {
        self.payload.as_unloaded()
    }

    pub(crate) fn as_download(&self) -> Option<&DownloadPayload> {
        self.payload.as_download()
    }

    pub(crate) fn as_download_mut(&mut self) -> Option<&mut DownloadPayload> {
        self.payload.as_download_mut()
    }

    pub(crate) fn as_group(&self) -> Option<&GroupPayload> {
        self.payload.as_group()
    }

    pub(crate) fn as_group_mut(&mut self) -> Option<&mut GroupPayload> {
        self.payload.as_group_mut()
    }

    pub(crate) fn as_note(&self) -> Option<&NotePayload> {
        self.payload.as_note()
    }

    pub(crate) fn as_note_mut(&mut self) -> Option<&mut NotePayload> {
        self.payload.as_note_mut()
    }

    pub(crate) fn as_image(&self) -> Option<&ImagePayload> {
        self.payload.as_image()
    }

    pub(crate) fn as_pdf(&self) -> Option<&PdfPayload> {
        self.payload.as_pdf()
    }

    pub(crate) fn as_pdf_mut(&mut self) -> Option<&mut PdfPayload> {
        self.payload.as_pdf_mut()
    }

    pub(crate) fn as_json(&self) -> Option<&JsonPayload> {
        self.payload.as_json()
    }

    /// What a screen reader announces for the node's card: its kind, then what identifies it. A local file (no
    /// web source) is named by its file name.
    pub(crate) fn accessible_name(&self) -> String {
        let first_nonempty = |parts: &[&str]| parts.iter().find(|p| !p.trim().is_empty()).map(|p| p.trim().to_string());
        // Either separator: a saved path may come from the other platform.
        let file_name =
            |path: &str| path.rsplit(['/', '\\']).next().filter(|name| !name.is_empty()).map(str::to_string);
        let source_or_file = |path: &str| self.source_url().map(str::to_string).or_else(|| file_name(path));
        let (kind, detail) = match &self.payload {
            NodePayload::Tab(t) => ("Tab", first_nonempty(&[&t.title, &t.url])),
            NodePayload::Unloaded(t) => ("Sleeping tab", first_nonempty(&[&t.title, &t.url])),
            NodePayload::Download(d) => ("Download", file_name(&d.path)),
            NodePayload::Group(g) => {
                ("Group", Some(crate::names::to_plain(&crate::names::display_name(&g.name, crate::names::UNTITLED))))
            }
            NodePayload::Note(n) => {
                // The note's text is never cleaned; its first non-blank line is shown by the display rule.
                let first = n.text.lines().map(str::trim).find(|l| !l.is_empty());
                ("Note", first.map(|l| crate::names::to_plain(&crate::names::display_name(l, crate::names::UNTITLED))))
            }
            NodePayload::Image(p) => ("Image", source_or_file(&p.path)),
            NodePayload::Pdf(p) => ("PDF", source_or_file(&p.path)),
            NodePayload::Json(p) => ("JSON", source_or_file(&p.path)),
        };
        match detail {
            Some(detail) => format!("{kind}: {detail}"),
            None => kind.to_string(),
        }
    }

    /// The web source of an Image, Pdf, Json (not a leaf) or Note node.
    pub(crate) fn source_url(&self) -> Option<&str> {
        match &self.payload {
            NodePayload::Image(p) => p.url.as_deref(),
            NodePayload::Pdf(p) => p.url.as_deref(),
            NodePayload::Json(p) if p.key.is_none() => p.url.as_deref(),
            NodePayload::Note(p) => p.url.as_deref(),
            _ => None,
        }
    }

    /// The URL a card's "Copy URL" puts on the clipboard: its `source_url`, or the address a download came from.
    pub(crate) fn copyable_url(&self) -> Option<&str> {
        self.source_url().or_else(|| self.as_download().and_then(|d| d.url.as_deref()))
    }

    pub(crate) fn source_url_mut(&mut self) -> Option<&mut Option<String>> {
        match &mut self.payload {
            NodePayload::Image(p) => Some(&mut p.url),
            NodePayload::Pdf(p) => Some(&mut p.url),
            NodePayload::Json(p) if p.key.is_none() => Some(&mut p.url),
            NodePayload::Note(p) => Some(&mut p.url),
            _ => None,
        }
    }
}

/// A tab group's own data -- was `GroupNode`'s non-shared field (`name`;
/// `pos`/`vel`/`color` moved to `GraphNode`, shared with every kind). A
/// group is a container node shown at whichever level of Graph view it
/// itself belongs to that, double-clicked, drills into its own subgraph of
/// members (tabs/downloads/notes/images/PDFs/JSON, and other groups) in
/// place of that level's -- the same "swap which set of nodes this view
/// renders" idea `ViewMode::Item`/`Graph` already is, just recursively, and
/// reusing that same flat `graph_layout::step` simulation rather than a
/// second nested one running inside the group's own node.
///
/// **Groups nest.** A group's own `GraphNode::group` can itself be
/// `Some(parent_group_id)` -- the "no nesting, `GraphNode::group` is a
/// single `Option<String>` not a path" limitation this doc comment used to
/// describe no longer applies: a single `Option<String>` *is* enough to
/// nest, since walking `.group` links upward from any node already forms
/// the whole ancestor chain (`App::group_breadcrumb`), with no separate
/// path/stack type needed. `App::current_group` names whichever group's
/// subgraph is currently showing (`None` at the true top level, same as
/// before); leaving a group (the toolbar's "◀" button) goes to that
/// group's own parent, not straight to the top, so leaving a deeply-nested
/// group takes as many "◀" clicks as it took to get there (or one
/// breadcrumb click to jump straight to any ancestor). Dissolving a nested
/// group reparents its members to *its own* parent (`App::dissolve_group`),
/// not unconditionally to the top level.
///
/// Dragging one group's card onto another's is deliberately left as
/// **merge** (dissolve the source, flatten its members into the target),
/// unchanged from before nesting existed -- it does not nest the source
/// under the target. Nesting is created only through "+ New Group"/Import
/// Group/"Group Selected" while already viewing a group's own subgraph, a
/// deliberate choice (confirmed with the user) to avoid changing what an
/// already-shipped drag gesture does.
///
/// Entering/leaving eases the camera via `GraphCameraTween` rather than
/// `Transition` — see that type's own doc comment for why. Groups, their
/// members and nesting are saved in the session store (`session_sync`).
pub(crate) struct GroupPayload {
    pub(crate) name: String,
    /// Made by an older build's "Expand as Graph", now removed. Saved like
    /// any group, as are the JSON leaves inside it, so a session saved by
    /// that build still loads.
    pub(crate) from_json: bool,
    /// The jar new tabs in this group start with (`field::DEFAULT_JAR`); `None` inherits from the parent group.
    pub(crate) default_jar: Option<crate::store::model::NodeId>,
}

impl GroupPayload {
    /// A plain group: not from JSON, no default jar.
    pub(crate) fn named(name: impl Into<String>) -> GroupPayload {
        GroupPayload { name: name.into(), from_json: false, default_jar: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json_view::JsonView;

    // `Tab` is left out: `TabPayload` owns a live `EngineTab`, which a unit
    // test can't construct. Its arm is covered by live Tab-view runs instead.
    #[test]
    fn maps_each_constructible_payload_variant_to_its_own_kind() {
        assert_eq!(
            item_node_kind(&NodePayload::Download(DownloadPayload {
                path: "file.bin".into(),
                url: None,
                bytes_received: 0,
                total_bytes: Some(1024),
                completed: false,
                failed: false
            })),
            ItemNodeKind::Download
        );
        assert_eq!(item_node_kind(&NodePayload::Group(GroupPayload::named("g"))), ItemNodeKind::Group);
        assert_eq!(
            item_node_kind(&NodePayload::Note(NotePayload { text: "n".into(), editing: false, url: None })),
            ItemNodeKind::Note
        );
        assert_eq!(
            item_node_kind(&NodePayload::Image(ImagePayload {
                path: "img.png".into(),
                width: 100,
                height: 100,
                error: None,
                url: None
            })),
            ItemNodeKind::Image
        );
        assert_eq!(
            item_node_kind(&NodePayload::Pdf(PdfPayload { path: "doc.pdf".into(), outcome: None, url: None })),
            ItemNodeKind::Pdf
        );
        assert_eq!(
            item_node_kind(&NodePayload::Json(JsonPayload {
                path: "data.json".into(),
                value: Default::default(),
                key: None,
                error: None,
                url: None,
                view: JsonView::default()
            })),
            ItemNodeKind::Json
        );
    }

    /// A card's screen-reader name is its kind, then its title (or the first line of a note).
    #[test]
    fn accessible_name_is_kind_then_title() {
        let mut app = crate::App::default();
        let (g, n) = (app.alloc_node_id(), app.alloc_node_id());
        app.insert_group_at(g.clone(), None);
        app.insert_note_at(n.clone(), None);
        let name = |app: &crate::App, id: &str| app.nodes[id].accessible_name();
        assert_eq!(name(&app, &n), "Note");
        app.nodes.get_mut(&n).unwrap().payload.as_note_mut().unwrap().text = "
  first line
second"
            .into();
        assert_eq!(name(&app, &n), "Note: first line");
        app.nodes.get_mut(&g).unwrap().as_group_mut().unwrap().name = "Reading".into();
        assert_eq!(name(&app, &g), "Group: Reading");
        app.nodes.get_mut(&g).unwrap().as_group_mut().unwrap().name = "Re\u{200B}ading".into();
        assert_eq!(name(&app, &g), "Group: Re<U+200B>ading");
        app.nodes.get_mut(&g).unwrap().as_group_mut().unwrap().name = " ".into();
        assert_eq!(name(&app, &g), "Group: Untitled");
        let raw = "\n\tRe\u{200B}ading\u{00A0}\u{00A0}list \nbody";
        app.nodes.get_mut(&n).unwrap().payload.as_note_mut().unwrap().text = raw.into();
        assert_eq!(name(&app, &n), "Note: Re<U+200B>ading list");
        assert_eq!(app.nodes[&n].as_note().unwrap().text, raw, "the note's text is not cleaned");
    }

    /// Review C-B12: a local file's card is named by its file name; a web source still wins.
    #[test]
    fn a_local_file_card_has_an_accessible_name() {
        let node = |payload| GraphNode {
            parent: None,
            group: None,
            pos: egui::Vec2::ZERO,
            vel: egui::Vec2::ZERO,
            color: None,
            payload,
        };
        let pdf = |url: Option<&str>| {
            node(NodePayload::Pdf(PdfPayload {
                path: r"C:\docs\report.pdf".into(),
                outcome: None,
                url: url.map(str::to_string),
            }))
        };
        assert_eq!(pdf(None).accessible_name(), "PDF: report.pdf");
        assert_eq!(pdf(Some("https://example.org/r.pdf")).accessible_name(), "PDF: https://example.org/r.pdf");
        let image = node(NodePayload::Image(ImagePayload {
            path: "/home/u/cat.png".into(),
            width: 1,
            height: 1,
            error: None,
            url: None,
        }));
        assert_eq!(image.accessible_name(), "Image: cat.png");
        let json = node(NodePayload::Json(JsonPayload {
            path: r"D:\data\x.json".into(),
            value: Default::default(),
            key: None,
            error: None,
            url: None,
            view: JsonView::default(),
        }));
        assert_eq!(json.accessible_name(), "JSON: x.json");
    }

    /// An unloaded tab's card is saved, exported and copied as its tab.
    #[test]
    fn saved_tab_keeps_a_failed_tab() {
        assert_eq!(
            (NodePayload::Unloaded(UnloadedTabPayload {
                url: "https://example.org/".into(),
                title: String::new(),
                kind: EngineKind::Cef,
                pinned: true,
                muted: false,
                zoom_factor: 1.5,
                state: UnloadedState::Failed("no binary".into()),
                jar: Default::default(),
            }))
            .saved_tab(),
            Some(("https://example.org/", EngineKind::Cef))
        );
        assert_eq!(NodePayload::Group(GroupPayload::named("g")).saved_tab(), None);
    }
}
