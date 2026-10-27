#[cfg(test)]
mod bench;
mod bookmarks;
mod caps;
mod card;
mod card_id;
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
mod debug_export;
#[cfg(test)]
mod deleted_names;
mod deselect;
mod disambiguate;
mod doc;
mod duplicate;
mod embedded_host;
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
mod host_pin;
mod identity;
mod idle_sleep;
mod image_decode;
mod image_view;
mod input;
mod jar;
mod jar_defs;
mod json_edit;
mod json_file;
mod json_level;
mod json_level_ui;
mod json_view;
mod labels;
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
mod omnibar;
mod page_tab_gate;
mod pdf;
mod pdf_render;
mod popups;
mod process_manager;
mod profile;
mod redirect_fetch;
mod reveal;
mod rgba;
mod search;
mod session;
mod session_files;
mod session_sync;
mod settings;
mod settings_page;
mod settings_rows;
mod shortcuts;
mod sleep_state;
mod store;
mod suggestions;
mod tab_view;
mod tabs;
mod telemetry;
mod text;
mod theme;
mod trail_nav;
mod trail_strip;
mod tree_layout;
mod undo;
mod url_parts;
mod view_info;
mod view_install;
mod view_modes;
mod viewport;
mod virtual_source;
mod walk;
mod zoom_chip;

use crate::store::model::NodeId;
use event_loop::UserEvent;
use export::*;
use fetched::*;
use node_menu::*;
use session::sanitize_session_name;
use tabs::{SpawnPurpose, SpawnTracker, TabCarry, TabSpawn, TabSpawnOutcome};
use text::*;
use undo::{Moved, UndoKind};
use view_modes::*;
use zoom_chip::*;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use favicon_fetch::FaviconFetcher;
use image_decode::ImageDecoder;
use omnibar::{Host, OmniBar};
use pdf_render::PdfRenderer;
use redirect_fetch::{FetchJob, FetchResult, FetchTarget, RedirectFetcher};
use suggestions::{looks_like_url, resolve_navigation_url, SuggestionFetcher};

use card::*;
use card_id::CardId;
use card_kind::*;
use debug_control::DebugJob;
use gpu::GpuState;
use graph_nav::GraphInteraction;
use graph_ui::*;
use identity::*;
use image_view::*;
use input::*;
use json_level::{JsonEdit, JsonLevel};
use level::{CardEntry, EdgeCurve, EdgeKind, LevelEdge, LevelScan, Origin};
use links::LinkGesture;
use manifold_view_protocol::{modifiers, HostToView, InputEvent, KeyCode, MouseButton, ViewToHost};
use mirror::Carried;
use node::*;
use popups::*;
use process_manager::{ViewKind, ViewTab};
use settings::{SearchEngine, Settings, ThemeChoice};
use store::model::{field, FieldValue};
use tab_view::*;
use theme::*;
use url_parts::UrlParts;
use view_info::*;
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
    /// The theme `sync_theme` last applied to the title bar and pages (`None` before the first frame).
    last_theme: Option<egui::Theme>,
    gpu: Option<GpuState>,
    /// Every graph node -- tabs, downloads, and groups alike (see
    /// `GraphNode`'s own doc comment). `node_order` is a stable
    /// iteration/z-order for graph rendering and hit-testing, separate from
    /// this `HashMap`'s own unspecified iteration order (which would make
    /// node click/drag targeting flaky if used directly).
    nodes: NodeMap,
    node_order: Vec<NodeId>,
    /// Where each node's card is drawn, by id (`Layout`): runtime and personal, never the document's
    /// pos (`Mirror::pos`). Keyed by id and left alone by a respawn's remove-and-reinsert, so a respawning tab keeps
    /// its place; pruned with the mirror at a save. Read through `layout_pos`: a node without an entry (a hand-built
    /// test node) is at the origin.
    layout: HashMap<NodeId, Layout>,
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
    /// `App::place_group`), and the jar new tabs in it start with
    /// (`Mirror::default_jar`) are the mirror's, as are its document pos and
    /// `color`; the drawn position is `App::layout`.
    ///
    /// **Groups nest.** A group's own group (`Mirror::group`) can itself be
    /// `Some(parent_group_id)`: walking group links upward from any node
    /// forms the whole ancestor chain (`App::group_breadcrumb`), with no
    /// separate path/stack type. This field names whichever group's
    /// subgraph is currently showing (`None` at the true top level); leaving a group (the toolbar's "◀" button) goes to that
    /// group's own parent, not straight to the top, so leaving a deeply-nested
    /// group takes as many "◀" clicks as it took to get there (or one
    /// breadcrumb click to jump straight to any ancestor). Dissolving a nested
    /// group reparents its members to *its own* parent (`App::take_out`),
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
    current_group: Option<NodeId>,
    /// The level of the Json node `current_group` names (spec "Level
    /// sources"); `None` on every other level.
    json_level: Option<JsonLevel>,
    /// Sources of virtual cards (`virtual_source.rs`), whose entries `level_items` appends after the stored ones.
    virtual_sources: Vec<std::rc::Rc<dyn virtual_source::VirtualSource>>,
    /// `(group_id, draft_name)` once renaming has started -- either the
    /// toolbar's group-name button (only shown while `current_group` is
    /// `Some`) or the top-level right-click context menu's own "Rename"
    /// (see `node_menu`) sets this the same way. A real
    /// `egui::TextEdit` replaces whichever plain button/menu row started
    /// it until Enter commits or Escape cancels. Transient UI state, not
    /// the group's name (`Mirror::name`).
    renaming_group: Option<(NodeId, String)>,
    /// `(tab_id, draft_name)` while a tab's menu shows "New Jar..."'s name field in place of its items, the way
    /// `renaming_group` does for a group. Committing creates the jar and picks it for the tab (`NodeAction::NewJar`).
    naming_jar: Option<(NodeId, String)>,
    /// The confirm dialog that is up, if any (`build_confirm_popup`).
    confirm: Option<popups::Confirm>,
    /// The settings popup's "Save Session As..." name field -- kept across
    /// popup open/close within the running session (not persisted), same
    /// convenience the group-rename draft gets.
    session_name_draft: String,
    /// The tab shown in Tab view, and — even while in Graph view — the
    /// default parent for a newly created tab (last thing the user
    /// selected). `None` only when there are no tabs left at all.
    focused: Option<NodeId>,
    /// The Image node `ViewMode::Item` is showing, if any. `None` means
    /// Item view (if active) shows `focused`, a Tab. Kept separate from
    /// `focused` because `focused` is tab-only by contract -- see the
    /// Image Item View spec's "Plan-time corrections".
    viewed_image: Option<NodeId>,
    /// Zoom/pan for `viewed_image` -- see `ImageViewState`.
    image_view: ImageViewState,
    /// The secondary pane's tab id while a split (engine-vs-engine compare
    /// view) is showing, alongside `focused` in the primary pane -- `None`
    /// for ordinary single-pane Tab view. Set once the tab `start_split`
    /// spawns actually finishes (`SpawnPurpose::Split`). Unlike `focused`,
    /// this tab never receives keyboard/mouse input in this first cut --
    /// it's a live, real graph node (its own view-host process, closable
    /// and reachable from Graph view like any other tab), just view-only
    /// while shown here. Cleared (not the tab itself closed) whenever the
    /// split closes, `focused` changes to a different tab, or this tab
    /// closes out from under the split -- see `close_split`.
    split_tab: Option<NodeId>,
    /// When each live tab was last on screen, for idle auto-sleep.
    idle_clock: idle_sleep::IdleClock,
    /// Sleep requests awaiting the engine's `PageState` answer, by tab id.
    pending_sleeps: HashMap<NodeId, sleep_state::PendingSleep>,
    /// Page text: load-end timers and awaited answers (`tabs::page_text`).
    page_text: tabs::PageTextTracker,
    /// The page text log (`search`).
    search: search::Search,
    /// The cookie jars WebView2 and CEF tabs share, by jar card id (see `jar`). A jar loads the first time a tab
    /// spawns with it (`ensure_jar_loaded`). An ephemeral jar lives only here.
    jars: BTreeMap<store::model::NodeId, jar::Jar>,
    /// Per-host cookie bookkeeping: pull rounds, holds, dirty jars (see `jar::CookieState`).
    cookies: jar::CookieState,
    /// Scroll positions of woken tabs, restored once their first load finishes.
    saved_scroll: HashMap<NodeId, sleep_state::SavedScroll>,
    /// Each tab's trail (`nav_history`): its pages as a tree, kept across an engine replacement, a close and a restart.
    nav_history: HashMap<NodeId, nav_history::NavHistory>,
    /// Files dropped on the window since the last frame -- each one's
    /// placement (`drop_file`) is offset by `egui::vec2(24.0, 24.0) *
    /// drop_batch`, so several dropped in the same frame fan out instead of
    /// stacking exactly on top of each other. Reset to 0 at the top of
    /// `render_frame`.
    drop_batch: u32,
    view_mode: ViewMode,
    /// The document and its sync state (see `doc::Doc`).
    doc: doc::Doc,
    /// Shared across every note's own rendered (non-`editing`) view --
    /// `egui_commonmark`'s own recommended usage (glyph-layout/heading-id
    /// state, not per-note content), not a fresh cache per card per frame.
    commonmark_cache: egui_commonmark::CommonMarkCache,
    /// Pointer, modifier and key state forwarded to pages (see `event_loop::PageInput`).
    page_input: event_loop::PageInput,

    egui_ctx: egui::Context,
    egui_state: Option<egui_winit::State>,
    /// The omnibar of whichever view is showing (the Item view's address bar, the Graph's and Settings' search). See
    /// `OmniBar`.
    omni: OmniBar,
    /// The Ctrl+K popup's omnibar.
    omni_popup: OmniBar,
    /// The background fetchers and decoders (see `event_loop::Workers`).
    workers: event_loop::Workers,

    /// Pick-target mode and the connect-drag/retarget gestures. See `LinkGesture`.
    link: LinkGesture,
    /// Graph view drag, hover, selection and pending-card state. See `GraphInteraction`.
    interaction: GraphInteraction,
    /// Tab spawns in flight, crash history and navigation bookkeeping. See `SpawnTracker`.
    spawns: SpawnTracker,
    /// Graph view's pan/zoom and its animations.
    camera: GraphCamera,
    /// Per-frame Graph view state: level of detail, key hints, text and settle caches (see `graph_ui::GraphUi`).
    graph: graph_ui::GraphUi,

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
    /// `--debug-commands`: a real window driven by debug commands. It tells egui it has focus, as headless does,
    /// so scripted keys reach egui without the OS giving the window the foreground.
    debug_commands: bool,
    /// JSON level view settings (see `json_level_ui::JsonLevelUi`).
    json_ui: json_level_ui::JsonLevelUi,
    /// The right panel's width (`App::build_right_panel`), shared by the JSON outline and the card pane.
    right_panel_width: f32,
    /// Bookmarks and history popup state (see `history::ListPopupsUi`).
    list_popups: history::ListPopupsUi,
    /// Engine install jobs, failures and the Settings drafts for them (see `view_install::ViewInstallUi`).
    view_ui: view_install::ViewInstallUi,
    /// Open menus and popups (see `popups::MenuPopups`).
    menus: popups::MenuPopups,
    /// Toast state (see `failure_toasts::Toasts`).
    toasts: failure_toasts::Toasts,
    /// Undo and redo history of drops and folds (see `undo::UndoHistory`).
    undo: undo::UndoHistory,
    /// Headless debug-control state (see `debug_control::HeadlessDebug`).
    debug: debug_control::HeadlessDebug,
    /// Frame pacing: minimize and idle state (see `event_loop::FramePacing`).
    pacing: event_loop::FramePacing,
    /// A focus request waiting for its `TextEdit` to be built (see
    /// `FocusTarget`): set by `Ctrl+L`/`Ctrl+F`/`Ctrl+K`, starting a
    /// rename, and headless `FOCUSADDRESS`/`ADDRESSBAR`/`OPENFINDBAR`.
    /// Unlike the headless key events above, this drives a real
    /// `request_focus()` on the real widget.
    pending_focus: Option<FocusTarget>,

    /// Admits or drops page-opened tabs (`ViewToHost::OpenTabRequested`)
    /// per source tab: a user gesture and a rate budget.
    page_tab_gate: page_tab_gate::PageTabGate,

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

    /// The Home page field's text while it's being edited; Escape drops it (`build_settings_page`).
    home_page_draft: Option<String>,
    /// The Settings row to scroll to and outline, for one frame (`settings_page::draw_setting_rows`).
    settings_focus: Option<&'static str>,

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
    /// Every distinct `(title, url)` this session has actually navigated
    /// to, most-recent-last, capped at `history::MAX_HISTORY_ENTRIES` (oldest
    /// dropped first) -- recorded once per real URL change in
    /// `ViewToHost::NavigationStateChanged`'s own handling, so it covers
    /// in-page navigation (clicked links) the same as address-bar entries,
    /// not just the latter. Persisted to `history.json` whenever saved.
    history: Vec<history::HistoryEntry>,

    /// The in-progress Tab↔Graph shrink/expand animation, if any — see
    /// `Transition`'s own doc comment.
    transition: Option<Transition>,

    /// Set right after `App::default()` in `main`, before `run_app` — needed
    /// by `spawn_tab_async` (called from `resumed`'s `spawn_root_tab`, which
    /// runs before any other opportunity to set it) to hand a background
    /// spawn thread a way to report back. `None` only in the brief window
    /// before `main` sets it; every real spawn happens after that.
    event_proxy: Option<EventLoopProxy<UserEvent>>,

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
        apply_theme_choice(&egui_ctx, settings.theme);
        manifold_pages::set_dark(egui_ctx.theme() == egui::Theme::Dark);
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
        let jars: BTreeMap<store::model::NodeId, jar::Jar> = BTreeMap::new();
        let jar_snapshots = Arc::new(std::sync::Mutex::new(jar::Snapshots::new(jars.clone())));
        let mut app = Self {
            window: None,
            last_window_title: "manifold".to_string(),
            last_theme: None,
            gpu: None,
            nodes: NodeMap::default(),
            node_order: Vec::new(),
            layout: HashMap::new(),
            groups_collapsed: false,
            current_group: None,
            json_level: None,
            virtual_sources: vec![std::rc::Rc::new(trail_strip::TrailSource)],
            renaming_group: None,
            naming_jar: None,
            confirm: None,
            session_name_draft: String::new(),
            focused: None,
            viewed_image: None,
            image_view: ImageViewState::default(),
            split_tab: None,
            idle_clock: idle_sleep::IdleClock::default(),
            pending_sleeps: HashMap::new(),
            page_text: Default::default(),
            search: search::Search::open(),
            jars,
            cookies: jar::CookieState { jar_snapshots, ..Default::default() },
            saved_scroll: HashMap::new(),
            nav_history: nav_history::load(),
            drop_batch: 0,
            view_mode: ViewMode::Item,
            doc: doc::Doc::default(),
            commonmark_cache: egui_commonmark::CommonMarkCache::default(),
            page_input: Default::default(),
            egui_ctx,
            egui_state: None,
            omni: OmniBar::new(settings.home_page.clone()),
            omni_popup: OmniBar::new(String::new()),
            workers: Default::default(),
            link: LinkGesture::default(),
            interaction: GraphInteraction::default(),
            spawns: SpawnTracker::default(),
            camera: GraphCamera::new(),
            graph: Default::default(),
            headless: false,
            debug_commands: false,
            json_ui: json_level_ui::JsonLevelUi::default(),
            right_panel_width: 260.0,
            list_popups: history::ListPopupsUi::default(),
            view_ui: view_install::ViewInstallUi::default(),
            menus: popups::MenuPopups::default(),
            toasts: failure_toasts::Toasts::default(),
            undo: undo::UndoHistory::default(),
            debug: debug_control::HeadlessDebug::default(),
            pacing: event_loop::FramePacing::default(),
            pending_focus: None,
            page_tab_gate: page_tab_gate::PageTabGate::default(),
            settings_fingerprint,
            settings_save_due: None,
            content_index: None,
            find: FindState::default(),
            home_page_draft: None,
            settings_focus: None,
            settings,
            settings_return_view: ViewMode::Item,
            bookmarks,
            history,
            transition: None,
            event_proxy: None,
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
    telemetry::init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = cli::run(&args) {
        std::process::exit(code);
    }

    // Must happen before the event loop/window are created. Confirmed live
    // that this process was *not* actually DPI-aware despite the assumption
    // that `winit` sets this up automatically: `window.scale_factor()`
    // (`self.ppp()`) reported `1.0` on a real 125%-scaled display -- the
    // exact same DPI-virtualization symptom `manifold-view-webview2`'s own
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
    std::thread::spawn(process_manager::sweep_view_logs);

    let event_loop = EventLoop::<UserEvent>::with_user_event().build().expect("create event loop");
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = App {
        headless,
        debug_commands,
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
