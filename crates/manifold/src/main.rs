#[cfg(test)]
mod bench;
mod bookmarks;
mod caps;
mod card;
mod card_kind;
mod cef_pin;
mod cef_platform;
mod chrome_ui;
mod cli;
mod click_count;
mod closed;
mod content;
mod crash_report;
mod debug_control;
#[cfg(test)]
mod deleted_names;
mod deselect;
mod engine_info;
mod engine_install;
mod event_loop;
mod export;
mod failure_toasts;
mod favicon_fetch;
mod fetched;
mod gpu;
mod graph_layout;
mod graph_nav;
mod graph_ui;
mod groups;
mod history;
mod idle_sleep;
mod image_view;
mod input;
mod jar;
mod jar_defs;
mod json_file;
mod json_level;
mod json_level_ui;
mod json_view;
mod level;
mod link_gestures;
mod links;
mod local_state;
mod mirror;
mod names;
mod nav_history;
mod net;
mod node;
mod node_menu;
mod node_ops;
mod node_store;
mod pdf;
mod pdf_render;
mod popup_gate;
mod popups;
mod process_manager;
mod profile;
mod redirect_fetch;
mod reveal;
mod rgba;
mod session;
mod session_files;
mod session_sync;
mod settings;
mod settings_page;
mod shortcuts;
mod sleep_state;
mod store;
mod suggestions;
mod tab_view;
mod tabs;
mod text;
mod theme;
mod view_modes;
mod viewport;
mod zoom_chip;

use closed::*;
use event_loop::UserEvent;
use export::*;
use fetched::*;
use node_menu::*;
use session::sanitize_session_name;
use tabs::{SpawnPurpose, TabCarry, TabSpawn, TabSpawnOutcome};
use text::*;
use view_modes::*;
use zoom_chip::*;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use favicon_fetch::FaviconFetcher;
use pdf_render::PdfRenderer;
use redirect_fetch::{FetchJob, FetchResult, FetchTarget, RedirectFetcher};
use suggestions::{resolve_navigation_url, Suggestion, SuggestionFetcher};

use card::*;
use card_kind::*;
use debug_control::DebugCommand;
use engine_info::*;
use gpu::GpuState;
use graph_ui::*;
use image_view::*;
use input::*;
use ipc_protocol::{
    frame_path, modifiers, EngineToHost, HostToEngine, InputEvent, KeyCode, MouseButton, SharedFrameBuffer,
};
use json_level::{JsonEdit, JsonLevel};
use level::{EdgeCurve, EdgeKind, LevelEdge, LevelItem, LevelItemKind, LevelScan};
use links::LinkGesture;
use mirror::Carried;
use node::*;
use popups::*;
use process_manager::{EngineKind, EngineTab};
use settings::{SearchEngine, Settings};
use store::model::{field, FieldValue};
use tab_view::*;
use theme::*;
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{
    DeviceId, ElementState, MouseButton as WinitMouseButton, MouseScrollDelta, TouchPhase, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{KeyCode as WinitKeyCode, ModifiersState, PhysicalKey};
#[cfg(windows)]
use winit::platform::windows::WindowAttributesExtWindows;
use winit::window::{Window, WindowId};

/// How long settings wait after the last change before they are written:
/// a window drag sends dozens of moves a second.
const SETTINGS_SAVE_DELAY: std::time::Duration = std::time::Duration::from_millis(500);

/// How long a "Saved <path>" toast stays on screen — see `App::save_toast`'s
/// doc comment.
const SAVE_TOAST_DURATION: std::time::Duration = std::time::Duration::from_secs(4);

/// Content on its way to becoming a node (`App::insert_content`).
pub(crate) enum ContentSource {
    /// A file on this machine. Sniffed; the node points at it where it is.
    Local(std::path::PathBuf),
    /// Checked content from `url` (see `redirect_fetch`).
    Web { url: String, body: WebBody },
}

pub(crate) enum WebBody {
    /// Saved in the content dir as `<id>.<ext>`; `id` is the node's id.
    File { kind: content::ContentKind, path: String },
    /// A note's text.
    Text(String),
}

struct App {
    window: Option<Arc<Window>>,
    /// The OS window title `update_window_title` last actually set --
    /// compared against each frame's freshly computed title so `Window::
    /// set_title` (a real OS call) is only made on an actual change, not
    /// once a frame regardless (this app's winit loop runs `ControlFlow::
    /// Poll`, redrawing continuously).
    last_window_title: String,
    gpu: Option<GpuState>,
    /// Every graph node -- tabs, downloads, and groups alike (see
    /// `GraphNode`'s own doc comment). `node_order` is a stable
    /// iteration/z-order for graph rendering and hit-testing, separate from
    /// this `HashMap`'s own unspecified iteration order (which would make
    /// node click/drag targeting flaky if used directly).
    nodes: NodeMap,
    node_order: Vec<String>,
    /// Where each node's card is drawn, by id (`Layout`, 2b-4 Ruling 1): runtime and personal, never the document's
    /// pos (`Mirror::pos`). Keyed by id and left alone by a respawn's remove-and-reinsert, so a respawning tab keeps
    /// its place; pruned with the mirror at a save. Read through `layout_pos`: a node without an entry (a hand-built
    /// test node) is at the origin.
    layout: HashMap<String, Layout>,
    /// Toggled by the Graph toolbar's "Collapse Groups"/"Expand Groups"
    /// button -- while `true`, every non-empty group card falls back to
    /// the plain centered name+count layout an empty group already uses,
    /// skipping the 2x2 member-thumbnail grid. A denser overview for a
    /// graph with many/large groups, purely a rendering toggle -- doesn't
    /// touch `groups`/membership at all. Session-only, resets on restart.
    groups_collapsed: bool,
    /// `None` while viewing the top level; `Some(group_id)` while a
    /// group's own subgraph has been drilled into — only meaningful when
    /// `view_mode == Graph`, but not reset on leaving Graph view, so
    /// returning to Graph resumes wherever the user last drilled into, same
    /// "graph view remembers its own state" treatment `camera.rect`'s
    /// pan/zoom already gets.
    ///
    /// A group is a container node (the target of the `contains` edge,
    /// `card_kind::is_container`) shown at whichever level of Graph view it
    /// itself belongs to that, double-clicked, drills into its own subgraph of
    /// members (tabs/downloads/notes/images/PDFs/JSON, and other groups) in
    /// place of that level's -- the same "swap which set of nodes this view
    /// renders" idea `ViewMode::Item`/`Graph` already is, just recursively, and
    /// reusing that same flat `graph_layout::step` simulation rather than a
    /// second nested one running inside the group's own node. It has no
    /// runtime state of its own (`Runtime::None`): its name (`Mirror::name`,
    /// written by `App::edit_name`; a new group's is seeded by
    /// `App::place_group`), whether an older build's "Expand as Graph" made it
    /// (`Mirror::from_json`) and the jar new tabs in it start with
    /// (`Mirror::default_jar`) are the mirror's, as are its document pos and
    /// `color`; the drawn position is `App::layout`.
    ///
    /// **Groups nest.** A group's own group (`Mirror::group`) can itself be
    /// `Some(parent_group_id)` -- the "no nesting, a node's group is a
    /// single `Option<String>` not a path" limitation this doc comment used to
    /// describe no longer applies: a single `Option<String>` *is* enough to
    /// nest, since walking group links upward from any node already forms
    /// the whole ancestor chain (`App::group_breadcrumb`), with no separate
    /// path/stack type needed. This field names whichever group's
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
    current_group: Option<String>,
    /// The level of the Json node `current_group` names (spec "Level
    /// sources"); `None` on every other level.
    json_level: Option<JsonLevel>,
    /// The JSON outline panel shows (local state).
    json_outline_visible: bool,
    /// Its width in points, as last drawn (local state).
    json_outline_width: f32,
    /// Copy path's form on a JSON level (local state).
    json_accessor_paths: bool,
    /// `(group_id, draft_name)` once renaming has started -- either the
    /// toolbar's group-name button (only shown while `current_group` is
    /// `Some`) or the top-level right-click context menu's own "Rename"
    /// (see `node_menu`) sets this the same way. A real
    /// `egui::TextEdit` replaces whichever plain button/menu row started
    /// it until Enter commits or Escape cancels. Transient UI state, not
    /// the group's name (`Mirror::name`).
    renaming_group: Option<(String, String)>,
    /// `(tab_id, draft_name)` while a tab's menu shows "New Jar..."'s name field in place of its items, the way
    /// `renaming_group` does for a group. Committing creates the jar and picks it for the tab (`NodeAction::NewJar`).
    naming_jar: Option<(String, String)>,
    /// A jar's "Clear Cookies" or "Delete" from Settings, waiting for its confirm dialog (`build_jar_confirm_popup`).
    pending_jar_confirm: Option<jar_defs::JarConfirm>,
    /// A named session's delete from Settings, waiting for its confirm dialog (`build_session_delete_popup`).
    pending_session_delete: Option<String>,
    /// `(node_id, screen_pos)` while a right-click context menu is showing
    /// for that node's card -- one field for every kind, so at most one
    /// menu is ever open by construction. Right-click is otherwise the
    /// rectangle-select gesture over the graph panel (see that gesture's
    /// own doc comment on `handle_rect_select`); a press landing on any card
    /// opens this instead of starting a marquee. Drawn by
    /// `App::build_node_menu`, which dispatches on the node's kind.
    node_menu: Option<(String, egui::Pos2)>,
    /// Last real value of `node_menu`, kept around so `build_node_menu`
    /// can keep drawing (fading out, not vanishing instantly) for a few
    /// frames after `node_menu` itself goes back to `None` -- see
    /// `fade_popup_state`.
    node_menu_cache: Option<(String, egui::Pos2, NodeMenuView)>,
    /// `Ctrl+K` quick-switcher, if currently open — see `QuickSwitcher`'s
    /// own doc comment and `App::build_quick_switcher`.
    quick_switcher: Option<QuickSwitcher>,
    /// Same fade-out role as `node_menu_cache`, for `quick_switcher`
    /// -- but rendered read-only (no live `TextEdit`, no clickable rows)
    /// once actually closing, since editing/clicking needs `&mut self.
    /// quick_switcher`, which no longer exists by then. See `build_quick_
    /// switcher`'s own `is_live` branch.
    quick_switcher_cache: Option<QuickSwitcher>,
    /// `F1` / headless `TOGGLESHORTCUTSHELP` -- whether the keyboard-
    /// shortcut reference popup is showing. See `App::build_shortcuts_help`.
    shortcuts_help_open: bool,
    /// A session picked via "Import Session..." (or headless `IMPORTSESSION`),
    /// staged here rather than applied immediately -- `build_import_confirm_
    /// popup` shows its tab count against the currently running session's
    /// own, so replacing a real working session with an accidentally-wrong
    /// file is a deliberate second click, not a silent one-shot dialog pick.
    pending_import: Option<store::json::JsonDoc>,
    /// The settings popup's "Save Session As..." name field -- kept across
    /// popup open/close within the running session (not persisted), same
    /// convenience the group-rename draft gets.
    session_name_draft: String,
    /// Graph view's own toolbar filter box -- dims (doesn't hide) any
    /// tab/download/group whose title/url/path/name doesn't contain this
    /// case-insensitively, keeping edges and spatial layout intact rather
    /// than making the graph reflow the way actually removing non-matches
    /// would. Empty means no filtering. See `App::graph_node_dimmed`.
    graph_filter: String,
    /// The tab shown in Tab view, and — even while in Graph view — the
    /// default parent for a newly created tab (last thing the user
    /// selected). `None` only when there are no tabs left at all.
    focused: Option<String>,
    /// The Image node `ViewMode::Item` is showing, if any. `None` means
    /// Item view (if active) shows `focused`, a Tab. Kept separate from
    /// `focused` because `focused` is tab-only by contract -- see the
    /// Image Item View spec's "Plan-time corrections".
    viewed_image: Option<String>,
    /// Zoom/pan for `viewed_image` -- see `ImageViewState`.
    image_view: ImageViewState,
    /// The secondary pane's tab id while a split (engine-vs-engine compare
    /// view) is showing, alongside `focused` in the primary pane -- `None`
    /// for ordinary single-pane Tab view. Set once the tab `start_split`
    /// spawns actually finishes (`SpawnPurpose::Split`). Unlike `focused`,
    /// this tab never receives keyboard/mouse input in this first cut --
    /// it's a live, real graph node (its own engine-host process, closable
    /// and reachable from Graph view like any other tab), just view-only
    /// while shown here. Cleared (not the tab itself closed) whenever the
    /// split closes, `focused` changes to a different tab, or this tab
    /// closes out from under the split -- see `close_split`.
    split_tab: Option<String>,
    /// When each live tab was last on screen, for idle auto-sleep.
    idle_clock: idle_sleep::IdleClock,
    /// Sleep requests awaiting the engine's `PageState` answer, by tab id.
    pending_sleeps: HashMap<String, sleep_state::PendingSleep>,
    /// The cookie jars WebView2 and CEF tabs share, by key (see `jar`). The default jar loads at startup; a named jar
    /// loads the first time a tab spawns with it (`ensure_jar_loaded`). An ephemeral jar lives only here.
    jars: BTreeMap<jar::JarKey, jar::Jar>,
    /// Per running host, the jar it was **spawned with** (spec "Live tabs when `jar` changes") and what its engine is
    /// believed to hold (`jar::TabCookies`): only a cookie the tab itself changed reaches that jar, and only a cookie
    /// the tab itself reported can be deleted by its absence.
    jar_tabs: HashMap<String, (jar::JarKey, jar::TabCookies)>,
    /// Hosts still holding cookies from a jar the user cleared (`clear_jar_cookies`): they switch (`will_switch`)
    /// although their key is unchanged, and their pulls are dropped so the old cookies cannot refill the jar. An id
    /// leaves when its host is replaced or closed (`clear_cookie_state`).
    jar_reseed: HashSet<String>,
    /// A copy of `jars` for spawn threads: the seed is read when the host connects, not when the spawn starts. Also
    /// counts each jar's clears, so a host seeded before a clear is caught when it lands (`note_spawned_jar`).
    jar_snapshots: Arc<std::sync::Mutex<jar::Snapshots>>,
    /// A sleep's `PageState` or a swap's `StateSnapshot` waiting for the tab's `Cookies` reply (or its deadline), so
    /// the host is not dropped before its last cookies arrive.
    cookie_holds: HashMap<String, (std::time::Instant, tabs::Held)>,
    /// When `jar_pull_all` last ran, for the 30 s pull in `about_to_wait`.
    jar_last_pull: Option<std::time::Instant>,
    /// Tabs with a `GetCookies` in flight, by tab id. A host may never answer, so an entry expires
    /// (`JAR_PULL_TIMEOUT`) instead of blocking the next pull.
    jar_pulls: HashMap<String, std::time::Instant>,
    /// Consecutive unanswered pulls per tab (`tabs::start_pull`).
    jar_silent: HashMap<String, u8>,
    /// Jars a pull changed and not saved yet, each with when it first changed: saved once the pull round ends
    /// (`flush_dirty_jars`), not once per tab's reply. Memory and `jar_snapshots` are current; only the file lags.
    jar_dirty: std::collections::BTreeMap<jar::JarKey, std::time::Instant>,
    /// Scroll positions of woken tabs, restored once their first load finishes.
    saved_scroll: HashMap<String, sleep_state::SavedScroll>,
    /// Each tab's back/forward list as the host saw it, by tab id: kept
    /// when the tab's engine is replaced (crash, wake, swap). See `nav_history`.
    nav_history: HashMap<String, nav_history::NavHistory>,
    /// Files dropped on the window since the last frame -- each one's
    /// placement (`drop_file`) is offset by `egui::vec2(24.0, 24.0) *
    /// drop_batch`, so several dropped in the same frame fan out instead of
    /// stacking exactly on top of each other. Reset to 0 at the top of
    /// `render_frame`.
    drop_batch: u32,
    view_mode: ViewMode,
    /// The session store (spec "Journal"): `None` before `open_store`, when
    /// `%APPDATA%` is unset, or when the file couldn't be opened (toasted).
    journal: Option<store::journal::Journal>,
    /// Ids for nodes made while there is no journal (unit tests, a failed
    /// open): this launch's own actor, so they parse as store ids.
    fallback_ids: (store::model::ActorId, u64),
    /// Tab spawns started by `spawn_tab_async` whose `TabSpawned` result
    /// hasn't been handled yet. Read by the headless `WAITTABS` command,
    /// whose waiting senders sit in `tabs_spawned_waiters` until this
    /// reaches zero.
    tab_spawns_in_flight: usize,
    /// Ids whose tab spawn is in flight (every `SpawnPurpose`), each with
    /// the tab it was started as (`session_sync::spawn_snapshot`). No live
    /// node yet (contract "In-flight spawns"); a save creates one the store
    /// doesn't hold yet (a new, imported or reopened tab) from this snapshot
    /// (`snapshot`), so a quit before it lands doesn't lose it. A respawn
    /// queues no delete, so the store keeps the tab meanwhile.
    spawning: HashMap<String, session_sync::Snapshot>,
    /// Other replicas' latest change to a node in `spawning`, applied when
    /// its spawn lands (`None`: deleted). The spawn itself carries only
    /// what it was started with.
    remote_in_flight: HashMap<String, Option<session_sync::RemoteNode>>,
    /// Tabs loading a url the host gave them, with when each was last seen
    /// loading; their url changes aren't shared meanwhile
    /// (`App::shares_url_change`).
    host_loads: HashMap<String, std::time::Instant>,
    /// Tabs `navigate_tab` just moved on, with the url each left, when, and
    /// whether the engine has reported loading since (`App::reported_url`).
    navigated_from: HashMap<String, (String, std::time::Instant, bool)>,
    /// Nodes placed here that the next save creates (2b-5 Ruling 3): every way a node enters the live graph
    /// (`place_node`, `spawn_tab_async`, a jar's `add_jar`) queues its id; the save creates each the document lacks,
    /// from the mirror or the spawn's snapshot (`snapshot`). One the document already holds (a restore, another
    /// replica's node) is dropped. Cleared of created ids by a successful commit.
    pending_creates: std::collections::BTreeSet<store::model::NodeId>,
    /// Nodes removed here that the next save deletes (Ruling 4: `queue_delete`). Only a local removal queues: a
    /// node another replica deleted is already gone from the document, and a respawn, swap or reload only takes
    /// the node out of `nodes` for a while. Cleared by a successful commit.
    pending_deletes: std::collections::BTreeSet<store::model::NodeId>,
    /// The decoded copy of the document's mirrored fields (`mirror::Mirror`).
    mirror: mirror::Mirror,
    /// Local edits to mirrored fields that the next save commits as `Set`s, by node and field. Cleared by a
    /// successful commit.
    pending: std::collections::BTreeMap<(store::model::NodeId, u32), Vec<u8>>,
    /// Local group moves the next save commits as `Move`s, by node: the target group, `None` for the top level.
    /// Cleared by a successful commit.
    pending_moves: std::collections::BTreeMap<store::model::NodeId, Option<store::model::NodeId>>,
    /// Another replica's text for a note being typed in, held back from its mirror and box until the blur
    /// (`end_note_edit`, 2b-4 Ruling 7), by node. Dropped when this replica commits its own text after it.
    held_text: HashMap<String, String>,
    /// Personal state (spec "Personal state") last written to
    /// `session.mnf.local`, or read at startup: `maybe_write_local` skips
    /// writing when the current state compares equal, so an idle session
    /// never rewrites the file.
    local_written: Option<local_state::LocalState>,
    /// Set while startup couldn't read `session.mnf.local` (another
    /// window held the lock, or the read failed): nothing writes it, so
    /// this profile's saved state isn't replaced by defaults, and
    /// `poll_store` retries the read.
    local_unread: Option<session_sync::LocalUnread>,
    /// When `local_written` was last written (or, at startup, `Instant::now`
    /// so the first `EVERY` window starts from launch, not from `UNIX_EPOCH`).
    local_written_at: std::time::Instant,
    /// Set when Recently Closed changes (Task 8); `maybe_write_local` writes
    /// `AFTER_CLOSED` after this, instead of waiting for the `EVERY` timer.
    closed_changed_at: Option<std::time::Instant>,
    /// Personal-state entries for nodes `apply_local` found still spawning
    /// (no live node yet): applied once each one's node is inserted
    /// (`handle_tab_spawn_result`), then removed. Only `nodes` is ever
    /// populated here -- the view fields are resolved once, in `apply_local`
    /// itself, since spawning is enough for those to take effect.
    pending_local: Option<local_state::LocalState>,
    tabs_spawned_waiters: Vec<std::sync::mpsc::Sender<()>>,
    /// Senders of the headless `WAITIDLE` commands waiting for `debug_idle`.
    idle_waiters: Vec<std::sync::mpsc::Sender<()>>,
    /// Bumped by `replace_running_session`. A tab spawn carries the value it
    /// started under, and `handle_tab_spawn_result` drops a result from an
    /// older one: it belongs to the replaced session, and inserting it would
    /// add a stray tab to the new scene, so a spawn from a replaced session
    /// never lands in it.
    session_generation: u64,
    /// Shared across every note's own rendered (non-`editing`) view --
    /// `egui_commonmark`'s own recommended usage (glyph-layout/heading-id
    /// state, not per-note content), not a fresh cache per card per frame.
    commonmark_cache: egui_commonmark::CommonMarkCache,
    cursor_pos: (f64, f64),
    /// The tab a left `InputEvent::MouseDown` was actually forwarded to and
    /// which hasn't been sent the matching `MouseUp` yet — i.e. "a native
    /// drag is in progress on this tab".
    ///
    /// `MouseDown` forwarding is gated on the cursor being below chrome and
    /// in the primary pane; applying that same gate to `MouseUp` means a
    /// press inside the page followed by a release over the app's own
    /// toolbar never delivers a button-up to the page at all. That was
    /// harmless while input was JS-driven, but mouse input is real native
    /// injection now (`SendMouseInput` on WebView2, `SendMouseClickEvent`
    /// on CEF), so Chromium itself never learns the button came up and
    /// keeps extending a text selection under every later hover. Tracking
    /// the in-progress drag here lets the release be forwarded to the same
    /// tab unconditionally, wherever it lands — the `manifold`-side mirror
    /// of the engine-side `DragState` each native-input engine-host keeps.
    native_drag_tab: Option<String>,
    /// Bitflags (see `ipc_protocol::modifiers`), tracked from winit's
    /// `WindowEvent::ModifiersChanged` so every forwarded `KeyDown`/`KeyUp`
    /// carries the modifier state at the time of that key, not just the
    /// currently-pressed key itself.
    key_modifiers: u8,
    /// Click counts for page presses (`InputEvent::MouseDown::click_count`).
    click_counter: click_count::ClickCounter,
    /// Physical keys of a shortcut chord pressed in Tab view, whose release (and typed text) must not
    /// reach the page either (`shortcuts::claims_chord`).
    swallowed_keys: std::collections::HashSet<WinitKeyCode>,

    egui_ctx: egui::Context,
    egui_state: Option<egui_winit::State>,
    /// Tab view's address bar and its autocomplete. See `AddressBar`.
    address_bar: AddressBar,
    /// Background fetcher for `favicon_fetch::fetch_favicon` -- see that
    /// module's own doc comment for why favicons are fetched independently
    /// here rather than aggregated from each engine's own (inconsistent)
    /// native favicon API.
    favicon_fetcher: FaviconFetcher,
    /// See `PdfPayload::outcome`'s own doc comment.
    pdf_renderer: PdfRenderer,
    /// See `redirect_fetch`'s own module doc comment.
    redirect_fetcher: RedirectFetcher,
    /// Origins already resolved (successfully or not) by `favicon_fetcher`
    /// -- checked before requesting again, so a failed origin (no
    /// `/favicon.ico`, a network error, undecodable bytes) isn't refetched
    /// on every navigation back to it. A *successful* fetch's actual pixel
    /// data lives in `GpuState::favicons` (keyed by the same origin
    /// string), not duplicated here -- this only needs to remember whether
    /// resolution was already attempted, not the result itself.
    favicon_resolved: HashSet<String>,

    /// Graph view: the node currently being dragged (pinned in the physics
    /// sim — see `graph_layout::step`'s `pinned` parameter) and the node
    /// currently hovered (drives the peek-preview popup).
    dragging: Option<String>,
    /// Every node the current drag is moving (a lone card, or the whole
    /// selection when the dragged card is selected — see `take_drag_unit`
    /// and the `dragged()` branch that fills this in). Read and cleared at
    /// release (`release_drag`), where each one's `pos` is written exactly
    /// once (`edit_pos`, 2b-4 Ruling 2): writing every frame of the drag
    /// instead would let an intermediate position reach the store, since a
    /// save can fire mid-drag from an unrelated event (`poll_store`, or a
    /// url change batched through `drain_tab_events`), independent of the
    /// gesture. Until then a remote pos for any of them doesn't snap its
    /// layout (`is_being_dragged`). Cleared without a write when the drag
    /// stops being live (`end_stale_drag`).
    drag_moved: Vec<String>,
    /// Pick-target mode and the connect-drag/retarget gestures. See `LinkGesture`.
    link: LinkGesture,
    hovered: Option<String>,
    /// Set to `self.hovered` by an Escape press while a hover-peek preview
    /// is showing, so that preview can be dismissed early without needing
    /// the pointer to actually move away -- the peek drawing code skips
    /// rendering while this matches the current `hovered` id. Cleared
    /// whenever `hovered` changes to anything else (including `None`), so
    /// hovering the same or a different node again always shows a fresh
    /// peek.
    peek_suppressed: Option<String>,
    /// Graph view rectangle-select: `Some` (holding the screen-space anchor
    /// point) for exactly the duration of a right-button drag over the
    /// graph panel -- the right button specifically, since the primary
    /// button is already Scene's own pan gesture (`drag_pan_buttons` is
    /// restricted to `PRIMARY` in `build_graph_ui` so the two never fight
    /// over the same drag). `selected_tabs` is the committed result, kept
    /// until the next selection (a fresh right-drag) or an explicit clear
    /// (a plain click on empty background, or Escape).
    rect_select_start: Option<egui::Pos2>,
    selected_tabs: HashSet<String>,
    /// World position of the small "New Tab/Note/Group/File" card shown
    /// after a plain right-click (no real drag) on empty Graph-view
    /// background -- see the marquee-release logic below for where this
    /// gets set (same gesture, disambiguated from a real rectangle-select
    /// drag by movement distance) and `build_graph_ui`'s own rendering of
    /// it. Deliberately *not* a `GraphNode`/`self.nodes` entry -- it's a
    /// pending choice, not real graph content, so it never needs excluding
    /// from `level_scan`/session persistence/drag-into-group/
    /// filters the way a real new node kind would. Cleared on any camera
    /// movement (pan/zoom/Reset View/entering or leaving a group/minimap
    /// click), Escape, clicking elsewhere, or picking one of its own
    /// actions -- deliberately short-lived rather than a droppable/
    /// draggable placeholder (a real node's card lingers regardless of
    /// camera movement; this one is a transient choice, not content, so
    /// letting it silently drift out of sync with a moved camera would be
    /// the wrong feel for it).
    pending_new_node: Option<egui::Vec2>,
    /// Graph view's pan/zoom and its animations.
    camera: GraphCamera,
    /// Level-of-detail: true once a frame's own count of viewport-visible
    /// nodes (post-cull, see `build_graph_ui`'s own pre-pass) crossed
    /// `LOD_SIMPLIFY_COUNT`, gating the per-kind render loops' thumbnail/
    /// title-text painting to keep dense views cheap. Flips back to
    /// `false` only once the count drops under the lower `LOD_RESTORE_
    /// COUNT` -- a deliberate hysteresis gap, not a single threshold, so
    /// a count sitting right at one boundary doesn't flicker every frame.
    graph_lod_simplified: bool,
    /// See `truncate_to_width`'s own doc comment.
    truncate_cache: HashMap<TruncateCacheKey, (String, String)>,
    /// See `App::cached_favicon_origin`'s own doc comment.
    favicon_origin_cache: HashMap<String, Option<String>>,
    /// `(level, node_count)` for the graph level `graph_settle_current_level`
    /// last ran a settle burst for -- `build_graph_ui` compares this
    /// against the current level/count every frame and only re-settles
    /// (and updates this) when they differ, instead of the physics sim
    /// running continuously. See `graph_settle_current_level`'s own doc
    /// comment for why continuous physics was replaced with this at all.
    graph_settled_key: Option<(Option<String>, usize)>,

    /// Headless mode (`--headless`): the window is created real but
    /// positioned far off-screen (NOT invisible — a genuinely invisible
    /// window's swapchain never successfully acquires a frame on Windows,
    /// see `resumed`'s doc comment), and shown without activation
    /// (`with_active(false)`; the WebView2 host window is `WS_EX_NOACTIVATE`),
    /// so it never steals focus/foreground (`scripts/foreground-watch.ps1`) or
    /// visually contends with the user's own screen (project memory's
    /// `SetForegroundWindow`/foreground-lock gotchas), while still
    /// rendering normally. Driven entirely by `DebugCommand`s read from
    /// stdin instead of real OS input. See `debug_control`'s doc comment
    /// for the full rationale and the text-command protocol.
    headless: bool,
    /// Whether the last `about_to_wait` found the window minimized: the
    /// loop then ticks at `MINIMIZED_TICK` instead of redrawing, and on the
    /// way back resumes redrawing.
    was_minimized: bool,
    /// The last input, engine message burst or egui repaint request: after
    /// `IDLE_AFTER` without one the loop slows to `IDLE_FRAME` (see
    /// `about_to_wait`).
    last_activity: std::time::Instant,
    /// Engine frames seen in the current second (a burst is activity).
    frame_burst: (std::time::Instant, u32),
    /// When an idle loop next draws.
    next_idle_frame: std::time::Instant,
    /// The wait between idle frames: `IDLE_FRAME`, or less when egui asked
    /// for a repaint sooner (`request_repaint_after`).
    idle_interval: std::time::Duration,
    /// Tabs `SetVisible(false)` was sent to on minimize, to be shown again
    /// on restore (`hide_tabs_for_minimize`).
    hidden_for_minimize: Vec<String>,
    /// Key events from headless `ENTER`/`ESCAPE`/`ARROWDOWN`/`ARROWUP`,
    /// added to the next frame's egui input exactly like real key presses
    /// (see `run_egui_frame`), so they take the same code paths and, like
    /// a real key, last one frame whether or not anything reads them.
    debug_input_events: Vec<egui::Event>,
    /// Text from a headless `ADDRESSBAR`, typed (select-all, then the text)
    /// once the address bar actually has egui focus: the command requests
    /// focus through `pending_focus`, which only takes hold in
    /// the frame after, so typing waits for a frame where
    /// `address_bar.focused` is true.
    debug_address_bar_typing: Option<String>,
    /// A focus request waiting for its `TextEdit` to be built (see
    /// `FocusTarget`): set by `Ctrl+L`/`Ctrl+F`/`Ctrl+K`, starting a
    /// rename, and headless `FOCUSADDRESS`/`ADDRESSBAR`/`OPENFINDBAR`.
    /// Unlike the headless key events above, this drives a real
    /// `request_focus()` on the real widget.
    pending_focus: Option<FocusTarget>,
    /// Set by a headless `QUIT` that arrived while debug input above (or
    /// `pending_screenshot`) was still waiting for a frame — e.g. `ENTER`+`QUIT`,
    /// or `SCREENSHOT`+`QUIT`, sent back-to-back with no delay. Exiting
    /// immediately in that case could drop the pending command/screenshot
    /// before any frame ever consumed it (`event_loop.exit()` gives no
    /// guarantee a `RedrawRequested` runs first). Holds how many more frames
    /// to wait: `headless_end_of_frame` exits once nothing is pending, or when
    /// this runs out (an `ADDRESSBAR` needs two frames, focus then typing;
    /// a screenshot on a stalled surface might never land).
    quit_after_redraws: Option<u8>,
    /// Set by a headless `SCREENSHOT` command; consumed on the next
    /// `RedrawRequested` and passed into `gpu::GpuState::render` (`render_frame`).
    pending_screenshot: Option<String>,
    /// The stdin reader waiting on `pending_screenshot` (see
    /// `DebugCommand::Screenshot`); sent `()` once it's been attempted.
    screenshot_waiter: Option<std::sync::mpsc::Sender<()>>,
    /// "Print to PDF": (temp PNG, PDF): the capture `start_print_to_pdf`
    /// asked for, and where its PDF goes. Paired so an unrelated screenshot
    /// can't take the PDF target. Set alongside `pending_screenshot` so the
    /// GPU capture needn't know PDF exists; once that capture completes
    /// (`handle_screenshot_result`), the temp PNG is converted via
    /// `pdf::write_image_as_pdf` and deleted. `None` the rest of the time,
    /// including for an ordinary "Screenshot" button click.
    pending_print_to_pdf: Option<(String, String)>,

    /// "Recently Closed", oldest first: one entry per close, capped at
    /// `MAX_CLOSED`. The toolbar dropdown reopens any entry;
    /// `Ctrl+Shift+T`/headless `REOPENTAB` reopens the newest. See
    /// `ClosedEntry`.
    closed: Vec<ClosedEntry>,

    /// The minimal right-click context menu, if currently showing — see
    /// `ContextMenuState`'s own doc comment.
    context_menu: Option<ContextMenuState>,
    /// Same fade-out role as `node_menu_cache`, for `context_menu`.
    context_menu_cache: Option<ContextMenuState>,

    /// A brief toast message (full text, not just a path -- see `build_
    /// save_toast`'s own doc comment) for the most recent completed
    /// download/save or crash/respawn notice, and when it was first drawn —
    /// cleared once `SAVE_TOAST_DURATION` has passed, at which point the
    /// next queued message (`save_toast_queue`) takes over, if any. Was
    /// "latest wins" with no queue at all until this session, when a
    /// second toast (e.g. a crash notice) arriving while a download's own
    /// "Saved <path>" toast was still showing was confirmed to silently
    /// replace it, unlike `FindState::result`'s own "latest wins" (correct
    /// there -- a stale search count would be actively wrong, whereas a
    /// dropped one-time notification like "tab crashed" is just gone).
    save_toast: Option<(String, std::time::Instant)>,
    /// See `save_toast`'s own doc comment -- messages waiting to be shown,
    /// one at a time in order, each after the previous one's
    /// `SAVE_TOAST_DURATION` elapses. Pushed to by `queue_toast` (and
    /// directly by `DownloadCompleted`, for borrow reasons).
    save_toast_queue: std::collections::VecDeque<String>,
    /// Spawn-failure toasts held to merge by cause (`failure_toasts`).
    failure_notices: failure_toasts::FailureNotices,
    /// Admits or drops page-opened tabs (`EngineToHost::OpenTabRequested`)
    /// per source tab: a user gesture and a rate budget.
    popup_gate: popup_gate::PopupGate,
    /// Recent engine crash times per tab id (`tabs::crashes_too_often`): a
    /// tab crashing too often is left as a failed card, not respawned.
    tab_crashes: HashMap<String, Vec<std::time::Instant>>,

    /// When `save_session` last committed to the store -- shown in the
    /// settings popup's Sessions section next to a "Save Now" button, so a
    /// save that's actually continuous (every mutation calls
    /// `save_session`, not a timer) has some visible confirmation instead
    /// of being purely invisible. `None` until the first save this run.
    last_session_save: Option<std::time::Instant>,
    /// When a save that didn't commit (another instance kept writing, or
    /// the write failed) is tried again by `poll_store`; `None` when
    /// nothing is owed. Without it an edit could sit unsaved until the next
    /// local action or remote write.
    save_retry_at: Option<std::time::Instant>,
    /// Store errors and repair notices toasted since the last commit
    /// (`store_error`, `take_store_recovery`), each shown once: a read-only
    /// file with a damaged tail reports both at every reload.
    store_notices: std::collections::BTreeSet<String>,
    /// Set once the store reports a repair (`take_store_recovery`): what
    /// was loaded may be missing nodes, so `prune_content_dir` doesn't run.
    store_recovered: bool,
    /// `settings::fingerprint()` as this process last saw it (set at load,
    /// updated after every successful save) -- see `save_settings`. `None`
    /// until the file has actually been loaded or saved once this run,
    /// which also means "skip the conflict check" the first time.
    settings_fingerprint: Option<u64>,
    /// When settings changed by a window move or resize are due to be
    /// written (`note_settings_changed`, `flush_settings_if_due`); `None`
    /// when nothing is waiting.
    settings_save_due: Option<std::time::Instant>,
    /// Set for the length of one restore or apply pass, so each node's file
    /// lookup doesn't read the content dir again.
    content_index: Option<ContentIndex>,

    /// Find-in-page, for the focused tab only. See `FindState`.
    find: FindState,

    /// Engine runtime installs, at most one per engine (see `engine_install::InstallJob`).
    engine_installs: HashMap<EngineKind, engine_install::InstallJob>,
    /// The last install's failure message per engine, until the next attempt starts.
    engine_failures: HashMap<EngineKind, String>,
    /// An engine download or removal waiting for its confirm dialog (`build_engine_confirm_popup`).
    pending_engine_confirm: Option<EngineConfirm>,
    /// `engine_state` per engine for the current frame: it stats the disk, and cards, pickers, menus
    /// and Settings all ask. Cleared at the start of each frame and whenever an install changes.
    engine_state_cache: std::cell::RefCell<HashMap<EngineKind, engine_install::EngineState>>,
    /// The bring-your-own CEF directory's last validation, with the path it was for (`custom_cef_dir`).
    cef_dir_check: std::cell::RefCell<Option<engine_install::DirCheck>>,
    /// Settings' text field for that directory while it is being edited; committed on Enter or focus loss.
    cef_dir_draft: Option<String>,
    /// The Home page field's text while it's being edited; Escape drops it (`build_settings_page`).
    home_page_draft: Option<String>,

    /// User-configurable defaults (search engine, starting engine, zoom,
    /// home page), loaded once at startup and persisted to
    /// `%APPDATA%\manifold\settings.json` on every change — see
    /// `settings.rs`.
    settings: Settings,
    /// Which view `ViewMode::Settings` should return to (its own Back
    /// button, `Escape`, or `Ctrl+G`) -- set once when the settings page is
    /// entered, from whichever of Tab/Graph was showing at that moment.
    settings_return_view: ViewMode,
    /// Flat `(title, url)` list, loaded once at startup and persisted to
    /// `%APPDATA%\manifold\bookmarks.json` on every change — see
    /// `bookmarks.rs`. Listed by `App::build_bookmarks_popup`.
    bookmarks: Vec<bookmarks::Bookmark>,
    /// The bookmarks popup's own filter box text -- cleared each time the
    /// popup is opened, same "fresh state each open" reasoning
    /// `FindState::match_case` resetting on a new find session already has.
    bookmarks_filter: String,
    /// `(bookmark_url, draft)` while a bookmark's folder label in the
    /// popup is being renamed -- keyed by url (a bookmark's own natural
    /// identity, same as `toggle_bookmark`'s own match) rather than index,
    /// so it can't end up pointing at the wrong row if the list reorders.
    /// Same "real `TextEdit` replaces the label, Enter/blur commits,
    /// Escape cancels" shape `renaming_group` already has.
    renaming_bookmark_folder: Option<(String, String)>,
    /// Every distinct `(title, url)` this session has actually navigated
    /// to, most-recent-last, capped at `history::MAX_HISTORY_ENTRIES` (oldest
    /// dropped first) -- recorded once per real URL change in
    /// `EngineToHost::NavigationStateChanged`'s own handling, so it covers
    /// in-page navigation (clicked links) the same as address-bar entries,
    /// not just the latter. Persisted to `history.json` whenever saved.
    history: Vec<history::HistoryEntry>,
    /// Set whenever an entry is pushed to `history` or when history is cleared.
    /// Saved by `save_session` or `close_store` when `true`.
    history_dirty: bool,
    /// The history popup's own filter box text -- same "cleared on each
    /// open" reasoning as `bookmarks_filter`.
    history_filter: String,
    /// The History popup's filter, the history length it was computed at, and its matches: recomputed only when
    /// either changes (or when `history_dirty` is set, which also covers an in-place title update).
    history_matches_cache: Option<(String, usize, Vec<usize>)>,
    /// Which popup anchored below a toolbar or Settings button is open, if
    /// any; one at a time. See `AnchoredPopup`, `toggle_popup` and
    /// `close_popup`.
    open_popup: Option<AnchoredPopup>,
    /// A graph card or one of its controls had keyboard focus in the last card pass: the cards
    /// show their Tab-order numbers (`CardPass::show_order`).
    card_nav_active: bool,
    /// Ctrl has been held on its own long enough to reveal key hints (`ctrl_reveal_step`).
    hints_revealed: bool,
    /// The card holding keyboard focus this frame: its kind and screen rect, for the key-hint strip
    /// (`build_card_hint`).
    card_hint: Option<(CardKind, egui::Rect)>,

    /// The in-progress Tab↔Graph shrink/expand animation, if any — see
    /// `Transition`'s own doc comment.
    transition: Option<Transition>,

    /// Set right after `App::default()` in `main`, before `run_app` — needed
    /// by `spawn_tab_async` (called from `resumed`'s `spawn_root_tab`, which
    /// runs before any other opportunity to set it) to hand a background
    /// spawn thread a way to report back. `None` only in the brief window
    /// before `main` sets it; every real spawn happens after that.
    event_proxy: Option<EventLoopProxy<UserEvent>>,

    /// The tab id `respawn_tab` was called for *while it was the focused
    /// tab* — i.e. an engine swap or crash recovery is currently respawning
    /// the tab the user is looking at. `build_tab_view_ui`'s existing "focused
    /// tab missing from `self.nodes`" guard (a pre-existing one-frame safety
    /// net, from before spawning was async) would otherwise bounce the user
    /// to Graph view for the tab's *entire* respawn duration — imperceptible
    /// when spawning was synchronous, but a visible multi-second flicker
    /// back to Graph and then back to Tab view now that a WebKit respawn can
    /// take many seconds. Set in `respawn_tab`, cleared in
    /// `handle_tab_spawn_result` once that same id's `Respawn` outcome
    /// (`Ok` or `Err`) arrives.
    pending_respawn_focus: Option<String>,

    /// Unloaded tabs' cards whose `wake_tab` spawn is in flight. Unlike a
    /// crash respawn, the card is still on the graph meanwhile, so it can
    /// be moved or closed: the outcome takes the card's placement as it is
    /// now, and is dropped if the card is gone. Set in `wake_tab`, cleared
    /// by that outcome in `handle_tab_spawn_result`.
    wakes_in_flight: HashSet<String>,

    /// Tabs that stay tabs whatever they load, each with whether that load
    /// has settled (reported `loading == false`): one opened by "Open as
    /// tab", and one loading another replica's url (`materialize`), which
    /// that replica converts itself. Every `RedirectableContent`/`JsonBody`
    /// report is skipped while a tab is in here. It stays until a real url
    /// change after the load settles (an HTTP redirect during it doesn't
    /// count; see `skip_convert_after`) or the tab closes (`remove_tab`).
    /// Not persisted (spec "Left open").
    skip_convert: HashMap<String, bool>,

    /// Windowless tests have no event loop to spawn on: `spawn_tab_async`
    /// parks each spawn here (with its `session_generation`) for the test
    /// to hand `handle_tab_spawn_result` an outcome itself.
    #[cfg(test)]
    test_spawns: Vec<(TabSpawn, u64)>,
    /// A test's jar directory, read only by `jars_dir`. `None`: no jar files at all (`profile::jars_dir()` is `None`
    /// under test).
    #[cfg(test)]
    jars_dir_override: Option<std::path::PathBuf>,
    /// Jar files written (`save_jar`), per app so parallel tests don't share it.
    #[cfg(test)]
    jar_saves: std::sync::atomic::AtomicUsize,
}

impl Default for App {
    fn default() -> Self {
        let egui_ctx = egui::Context::default();
        apply_style(&egui_ctx);
        apply_fonts(&egui_ctx);
        let (settings, settings_err) = Settings::load();
        // Re-reads the file `settings::fingerprint()` already just read for
        // `load()` -- a second read of a file this small, once at startup,
        // isn't worth threading a fingerprint out through `load()`'s own
        // return type for.
        let settings_fingerprint = settings::fingerprint();
        let (bookmarks, bookmarks_err) = bookmarks::load();
        let (history, history_err) = history::load();
        // Before constructing anything that might make its own first real
        // HTTPS request (search suggestions, favicons) -- see `suggestions::
        // warm_up_https_client`'s own doc comment for why this needs to run
        // exactly once, centrally, rather than each network consumer doing
        // its own redundant warm-up (or, worse, one racing a real request
        // against another's not-yet-finished warm-up).
        suggestions::warm_up_https_client();
        let jars: BTreeMap<jar::JarKey, jar::Jar> = profile::jars_dir()
            .map(|dir| (jar::JarKey::Default, jar::Jar::load(&dir, jar::JarKey::Default)))
            .into_iter()
            .collect();
        let jar_snapshots = Arc::new(std::sync::Mutex::new(jar::Snapshots::new(jars.clone())));
        let mut app = Self {
            window: None,
            last_window_title: "manifold".to_string(),
            gpu: None,
            nodes: NodeMap::default(),
            node_order: Vec::new(),
            layout: HashMap::new(),
            groups_collapsed: false,
            current_group: None,
            json_level: None,
            json_outline_visible: true,
            json_outline_width: 260.0,
            json_accessor_paths: false,
            renaming_group: None,
            naming_jar: None,
            pending_jar_confirm: None,
            pending_session_delete: None,
            node_menu: None,
            node_menu_cache: None,
            quick_switcher: None,
            quick_switcher_cache: None,
            shortcuts_help_open: false,
            pending_import: None,
            session_name_draft: String::new(),
            graph_filter: String::new(),
            focused: None,
            viewed_image: None,
            image_view: ImageViewState::default(),
            split_tab: None,
            idle_clock: idle_sleep::IdleClock::default(),
            pending_sleeps: HashMap::new(),
            jars,
            jar_tabs: HashMap::new(),
            jar_reseed: HashSet::new(),
            jar_snapshots,
            cookie_holds: HashMap::new(),
            jar_last_pull: None,
            jar_pulls: HashMap::new(),
            jar_silent: HashMap::new(),
            jar_dirty: std::collections::BTreeMap::new(),
            saved_scroll: HashMap::new(),
            nav_history: nav_history::load(),
            drop_batch: 0,
            view_mode: ViewMode::Item,
            journal: None,
            fallback_ids: (store::model::ActorId::random(), 0),
            tab_spawns_in_flight: 0,
            spawning: HashMap::new(),
            remote_in_flight: HashMap::new(),
            host_loads: HashMap::new(),
            navigated_from: HashMap::new(),
            pending_creates: std::collections::BTreeSet::new(),
            pending_deletes: std::collections::BTreeSet::new(),
            mirror: mirror::Mirror::default(),
            pending: std::collections::BTreeMap::new(),
            held_text: HashMap::new(),
            pending_moves: std::collections::BTreeMap::new(),
            local_written: None,
            local_unread: None,
            local_written_at: std::time::Instant::now(),
            closed_changed_at: None,
            pending_local: None,
            tabs_spawned_waiters: Vec::new(),
            idle_waiters: Vec::new(),
            session_generation: 0,
            commonmark_cache: egui_commonmark::CommonMarkCache::default(),
            cursor_pos: (0.0, 0.0),
            native_drag_tab: None,
            key_modifiers: 0,
            click_counter: Default::default(),
            swallowed_keys: Default::default(),
            egui_ctx,
            egui_state: None,
            address_bar: AddressBar::new(settings.home_page.clone()),
            favicon_fetcher: FaviconFetcher::new(profile::favicon_cache_dir()),
            pdf_renderer: PdfRenderer::new(),
            redirect_fetcher: RedirectFetcher::new(profile::content_dir()),
            favicon_resolved: HashSet::new(),
            dragging: None,
            drag_moved: Vec::new(),
            link: LinkGesture::default(),
            hovered: None,
            peek_suppressed: None,
            rect_select_start: None,
            selected_tabs: HashSet::new(),
            pending_new_node: None,
            camera: GraphCamera::new(),
            graph_lod_simplified: false,
            truncate_cache: HashMap::new(),
            favicon_origin_cache: HashMap::new(),
            graph_settled_key: None,
            headless: false,
            was_minimized: false,
            last_activity: std::time::Instant::now(),
            frame_burst: (std::time::Instant::now(), 0),
            next_idle_frame: std::time::Instant::now(),
            idle_interval: event_loop::IDLE_FRAME,
            hidden_for_minimize: Vec::new(),
            debug_input_events: Vec::new(),
            debug_address_bar_typing: None,
            pending_focus: None,
            quit_after_redraws: None,
            pending_screenshot: None,
            screenshot_waiter: None,
            pending_print_to_pdf: None,
            closed: Vec::new(),
            context_menu: None,
            context_menu_cache: None,
            save_toast: None,
            save_toast_queue: std::collections::VecDeque::new(),
            failure_notices: Default::default(),
            popup_gate: popup_gate::PopupGate::default(),
            tab_crashes: HashMap::new(),
            last_session_save: None,
            save_retry_at: None,
            store_notices: std::collections::BTreeSet::new(),
            store_recovered: false,
            settings_fingerprint,
            settings_save_due: None,
            content_index: None,
            find: FindState::default(),
            engine_installs: HashMap::new(),
            engine_failures: HashMap::new(),
            pending_engine_confirm: None,
            engine_state_cache: Default::default(),
            cef_dir_check: Default::default(),
            cef_dir_draft: None,
            home_page_draft: None,
            settings,
            settings_return_view: ViewMode::Item,
            bookmarks,
            bookmarks_filter: String::new(),
            renaming_bookmark_folder: None,
            history,
            history_dirty: false,
            history_filter: String::new(),
            history_matches_cache: None,
            open_popup: None,
            card_nav_active: false,
            hints_revealed: false,
            card_hint: None,
            transition: None,
            event_proxy: None,
            pending_respawn_focus: None,
            wakes_in_flight: HashSet::new(),
            skip_convert: HashMap::new(),
            #[cfg(test)]
            test_spawns: Vec::new(),
            #[cfg(test)]
            jars_dir_override: None,
            #[cfg(test)]
            jar_saves: std::sync::atomic::AtomicUsize::new(0),
        };
        for err in [settings_err, bookmarks_err, history_err].into_iter().flatten() {
            app.queue_toast(err);
        }
        app
    }
}

fn main() {
    crash_report::install();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = cli::run(&args) {
        std::process::exit(code);
    }

    // Must happen before the event loop/window are created. Confirmed live
    // that this process was *not* actually DPI-aware despite the assumption
    // that `winit` sets this up automatically: `window.scale_factor()`
    // (`self.ppp()`) reported `1.0` on a real 125%-scaled display -- the
    // exact same DPI-virtualization symptom `manifold-engine-webview2`'s own
    // `main()` had (see that module's doc comment on
    // `SetProcessDpiAwarenessContext` for the full mechanism). Without this,
    // the whole app runs on virtualized, bitmap-stretched coordinates:
    // `content_size_px()` (and so every engine's rendered surface) comes out
    // at the wrong real size, and the cursor positions winit reports don't
    // line up with where the content actually is on screen. Windows-only:
    // winit already reports the real scale factor on X11/Wayland.
    #[cfg(windows)]
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
    }
    let headless = std::env::args().any(|arg| arg == "--headless");
    // Independent of `--headless` -- accepts the exact same stdin-driven
    // `DebugCommand` protocol, but on a completely normal, visible,
    // on-screen window (real position, file dialogs available, window
    // geometry saved) rather than headless mode's off-screen one. Doesn't
    // touch `app.headless` itself, since none of that other headless-only
    // behavior is what this flag is for -- only `spawn_stdin_reader`'s own
    // gate below cares about it. Lets a real running window be driven by
    // debug commands (e.g. `SCREENSHOT`, which already just captures
    // whatever `GpuState::render` actually drew, headless or not) instead
    // of scripting an OS-level window capture around it.
    let debug_commands = std::env::args().any(|arg| arg == "--debug-commands");
    std::thread::spawn(process_manager::sweep_cef_cache);
    std::thread::spawn(process_manager::sweep_webview2_profiles);
    std::thread::spawn(process_manager::sweep_frame_files);

    let event_loop = EventLoop::<UserEvent>::with_user_event().build().expect("create event loop");
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = App {
        headless,
        // Must be set before `run_app` — `resumed()` calls `spawn_root_tab`,
        // which needs this to hand its background spawn thread a way to report
        // back (see `TabSpawnOutcome`'s doc comment).
        event_proxy: Some(event_loop.create_proxy()),
        ..App::default()
    };
    if headless || debug_commands {
        debug_control::spawn_stdin_reader(event_loop.create_proxy());
    }

    event_loop.run_app(&mut app).expect("run event loop");
    drop(app);
    process_manager::finish_cache_removals();
}

#[cfg(test)]
#[path = "main/tests.rs"]
mod tests;
