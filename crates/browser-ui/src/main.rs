mod debug_control;
mod gpu;
mod graph_layout;
mod process_manager;
mod settings;
mod suggestions;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use suggestions::{resolve_navigation_url, Suggestion, SuggestionFetcher};

use debug_control::DebugCommand;
use gpu::GpuState;
use ipc_protocol::{
    frame_path, modifiers, EngineToHost, HostToEngine, InputEvent, KeyCode, MouseButton,
    SharedFrameBuffer,
};
use process_manager::{EngineKind, EngineTab};
use settings::{SearchEngine, Settings};
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{DeviceId, ElementState, MouseButton as WinitMouseButton, MouseScrollDelta, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode as WinitKeyCode, ModifiersState, PhysicalKey};
use winit::platform::windows::WindowAttributesExtWindows;
use winit::window::{Window, WindowId};

/// Chrome (tab strip + toolbar) is two stacked top `Panel`s of these
/// exact heights, in egui points — kept as constants rather than read back
/// from egui's layout so the compositor's content viewport (see `gpu::
/// GpuState::render`'s `content_top_px`) always agrees with where egui
/// actually painted, without a frame of lag.
const TAB_STRIP_HEIGHT_POINTS: f32 = 26.0;
const TOOLBAR_HEIGHT_POINTS: f32 = 32.0;
const CHROME_HEIGHT_POINTS: f32 = TAB_STRIP_HEIGHT_POINTS + TOOLBAR_HEIGHT_POINTS;
/// Graph view's chrome is just the "+ New Tab" toolbar — no address bar,
/// no per-tab back/forward, since there's no single focused tab there.
const GRAPH_TOOLBAR_HEIGHT_POINTS: f32 = 26.0;

const GRAPH_NODE_SIZE: egui::Vec2 = egui::vec2(160.0, 110.0);
const GRAPH_THUMB_MARGIN: f32 = 6.0;
const GRAPH_PEEK_SIZE: egui::Vec2 = egui::vec2(360.0, 240.0);
const FAVICON_SIZE: egui::Vec2 = egui::vec2(16.0, 16.0);
/// The graph's world-space center that the physics centering force pulls
/// toward — independent of screen/window coordinates now that the graph is
/// pannable/zoomable (`egui::Scene`), unlike pre-pan/zoom where node
/// positions doubled as screen coordinates directly.
const GRAPH_WORLD_CENTER: egui::Vec2 = egui::Vec2::ZERO;
/// Initial visible world-space rect for the graph's `egui::Scene`, before
/// any user panning/zooming — generous enough to show a handful of settled
/// nodes around `GRAPH_WORLD_CENTER` at 1:1 zoom.
const GRAPH_SCENE_DEFAULT_SIZE: egui::Vec2 = egui::vec2(900.0, 620.0);
const GRAPH_ZOOM_MIN: f32 = 0.15;
const GRAPH_ZOOM_MAX: f32 = 3.0;

/// Distinct fills for the two stacked chrome bars (see `CHROME_HEIGHT_POINTS`'
/// doc comment) so they read as two separate strips rather than one flat
/// slab — the tab strip a shade darker (background-like), the toolbar a
/// shade lighter (surface-like), both distinct from the graph/content area
/// below.
const TAB_STRIP_FILL: egui::Color32 = egui::Color32::from_rgb(20, 21, 25);
const TOOLBAR_FILL: egui::Color32 = egui::Color32::from_rgb(30, 31, 36);

fn default_graph_scene_rect() -> egui::Rect {
    egui::Rect::from_center_size(egui::Pos2::new(0.0, 0.0), GRAPH_SCENE_DEFAULT_SIZE)
}

/// A single accent color used throughout — selection highlights, hyperlinks,
/// the focused-tab-strip fill, the focused graph node's border — so the
/// whole chrome reads as one coherent theme rather than egui's flat default
/// dark gray-on-gray.
const ACCENT: egui::Color32 = egui::Color32::from_rgb(88, 166, 255);

/// Applied once at startup (see `Default for App`). Bumps corner rounding
/// and spacing up from egui's fairly tight defaults (which read as "plain
/// tool window" rather than "app chrome") and threads `ACCENT` through
/// selection/hyperlink/focus colors.
fn apply_style(ctx: &egui::Context) {
    // `all_styles_mut` (not `style`/`set_style`, egui 0.36 keeps separate
    // dark/light styles internally) applies the same tweaks regardless of
    // which theme the OS/window ends up reporting — this app's chrome is
    // dark-only in practice, but there's no "just style whatever's active"
    // entry point, so set both identically.
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
        style.spacing.button_padding = egui::vec2(8.0, 4.0);

        let corner_radius: egui::CornerRadius = 6u8.into();
        for widgets in [
            &mut style.visuals.widgets.noninteractive,
            &mut style.visuals.widgets.inactive,
            &mut style.visuals.widgets.hovered,
            &mut style.visuals.widgets.active,
            &mut style.visuals.widgets.open,
        ] {
            widgets.corner_radius = corner_radius;
        }
        style.visuals.window_corner_radius = 8u8.into();
        style.visuals.menu_corner_radius = 8u8.into();

        style.visuals.selection.bg_fill = ACCENT;
        style.visuals.selection.stroke.color = egui::Color32::WHITE;
        style.visuals.hyperlink_color = ACCENT;
    });
}

fn engine_label(kind: EngineKind) -> &'static str {
    match kind {
        EngineKind::Servo => "Servo",
        EngineKind::Cef => "Blink (CEF)",
        EngineKind::Webkit => "WebKit",
    }
}

/// A distinct color per engine (loosely nodding to each project's own
/// branding — Servo orange, Chromium/Blink blue, Safari/WebKit teal) used
/// as a small identifying dot/badge wherever a tab's engine matters at a
/// glance: the tab strip, the graph node's engine badge. Deliberately
/// separate from `ACCENT` (which means "focused"/"selected", a different
/// axis of meaning) so the two never get confused for one another.
fn engine_color(kind: EngineKind) -> egui::Color32 {
    match kind {
        EngineKind::Servo => egui::Color32::from_rgb(255, 138, 80),
        EngineKind::Cef => egui::Color32::from_rgb(66, 133, 244),
        EngineKind::Webkit => egui::Color32::from_rgb(26, 188, 156),
    }
}

/// A small filled circle identifying a tab's engine at a glance — used
/// inline in a `ui.horizontal` layout (the tab strip, the engine picker's
/// own dropdown rows), so it allocates its own space rather than being a
/// raw `painter()` call positioned by hand.
fn engine_dot(ui: &mut egui::Ui, kind: EngineKind, diameter: f32) {
    let (rect, _response) = ui.allocate_exact_size(egui::vec2(diameter, diameter), egui::Sense::hover());
    ui.painter()
        .circle_filled(rect.center(), diameter / 2.0, engine_color(kind));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    Graph,
    Tab,
}

/// Which way a shrink/expand transition (see `App::transition`) is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransitionKind {
    /// Tab view's full-screen content quad shrinks down into `tab_id`'s
    /// graph-node thumbnail spot.
    ToGraph,
    /// `tab_id`'s graph-node thumbnail spot grows to fill the screen as
    /// Tab view's own content quad.
    ToTab,
}

/// An in-progress shrink/expand animation between Tab view and Graph view
/// for one specific tab. While this is `Some`, `build_ui` renders Graph
/// view's node layout as the backdrop (for *either* direction — it's the
/// one screen both endpoints have a coherent position in) with `tab_id`'s
/// live content quad drawn on top at an interpolated rect, rather than
/// switching `view_mode` instantly. See `App::content_rect_for_frame`.
struct Transition {
    tab_id: String,
    kind: TransitionKind,
    start: std::time::Instant,
}

const TRANSITION_DURATION: std::time::Duration = std::time::Duration::from_millis(220);

/// One tab: its engine-host process/IPC handle, the "opened from" edge
/// (`parent`) that makes the tab collection a forest for the M5 graph view,
/// per-tab navigation/UI state, and force-directed layout state (`pos`/
/// `vel`) — see `graph_layout`.
struct TabNode {
    tab: EngineTab,
    parent: Option<String>,
    url: String,
    title: String,
    can_go_back: bool,
    can_go_forward: bool,
    loading: bool,
    /// Set while a swap is in flight for *this* tab: the engine to swap
    /// *to*, once this tab's `GetState` reply (its `StateSnapshot`) comes
    /// back. Per-tab (not a flat `App` field) so a swap on one tab can't
    /// block or interfere with a concurrent swap/crash on another.
    swap_target: Option<EngineKind>,
    pos: egui::Vec2,
    vel: egui::Vec2,
}

struct App {
    window: Option<Arc<Window>>,
    gpu: Option<GpuState>,
    tabs: HashMap<String, TabNode>,
    /// Stable iteration/z-order for graph rendering and hit-testing — a
    /// `HashMap`'s own iteration order is unspecified and can change
    /// frame-to-frame, which would make node click/drag targeting flaky.
    tab_order: Vec<String>,
    /// The tab shown in Tab view, and — even while in Graph view — the
    /// default parent for a newly created tab (last thing the user
    /// selected). `None` only when there are no tabs left at all.
    focused: Option<String>,
    view_mode: ViewMode,
    next_tab_id: u64,
    cursor_pos: (f64, f64),
    /// Bitflags (see `ipc_protocol::modifiers`), tracked from winit's
    /// `WindowEvent::ModifiersChanged` so every forwarded `KeyDown`/`KeyUp`
    /// carries the modifier state at the time of that key, not just the
    /// currently-pressed key itself.
    key_modifiers: u8,

    egui_ctx: egui::Context,
    egui_state: Option<egui_winit::State>,
    address_bar_text: String,
    address_bar_focused: bool,
    /// Address-bar autocomplete: open-tab matches are recomputed
    /// synchronously (cheap, from data already in memory) whenever the text
    /// changes; search-engine suggestions arrive later, asynchronously, via
    /// `suggestion_fetcher`'s debounced background fetch. Only shown while
    /// `address_bar_focused`.
    open_tab_suggestions: Vec<Suggestion>,
    search_suggestions: Vec<String>,
    suggestion_fetcher: SuggestionFetcher,
    /// Index into `combined_suggestions()` the user has moved to via
    /// Up/Down — `None` means no row is highlighted (Enter falls through
    /// to a plain navigate of the typed text). Reset whenever the
    /// suggestion list itself changes or the dropdown closes, so a stale
    /// index can never point at an unrelated row — see `build_tab_ui`'s
    /// dropdown-handling comments for the full set of reset sites.
    dropdown_highlighted: Option<usize>,

    /// Graph view: the node currently being dragged (pinned in the physics
    /// sim — see `graph_layout::step`'s `pinned` parameter) and the node
    /// currently hovered (drives the peek-preview popup).
    dragging: Option<String>,
    hovered: Option<String>,
    /// The graph view's current pan/zoom state (visible world-space rect),
    /// persisted across frames per `egui::Scene::show`'s own contract —
    /// mutated by the user dragging (pan) or scrolling (zoom).
    graph_scene_rect: egui::Rect,

    /// Headless mode (`--headless`): the window is created real but
    /// positioned far off-screen (NOT invisible — a genuinely invisible
    /// window's swapchain never successfully acquires a frame on Windows,
    /// see `resumed`'s doc comment), so it never steals focus/foreground or
    /// visually contends with the user's own screen (project memory's
    /// `SetForegroundWindow`/foreground-lock gotchas), while still
    /// rendering normally. Driven entirely by `DebugCommand`s read from
    /// stdin instead of real OS input. See `debug_control`'s doc comment
    /// for the full rationale and the text-command protocol.
    headless: bool,
    /// Set by a headless `ESCAPE`/`ENTER`/`ARROWDOWN`/`ARROWUP` command,
    /// consumed (and reset) the next time `build_tab_ui` checks the
    /// corresponding real-keypress condition — see the comments at each
    /// check site.
    pending_debug_escape: bool,
    pending_debug_enter: bool,
    pending_debug_arrow_down: bool,
    pending_debug_arrow_up: bool,
    /// Set by a headless `ADDRESSBAR` command, cleared by `ENTER`/`ESCAPE`/
    /// accepting a suggestion. A real click gives the `TextEdit` real,
    /// *persistent* egui focus, so `response.has_focus()` keeps reporting
    /// `true` frame after frame on its own — headless mode has no way to
    /// synthesize that (see `debug_control`'s doc comment on why keyboard/
    /// text commands are direct calls, not synthetic events), so
    /// `ADDRESSBAR` sets `address_bar_focused` for only the one frame it's
    /// processed on; the very next frame, the real (never actually
    /// clicked) widget's honest `response.has_focus() == false` overwrites
    /// it right back. Without this pin, `editing_address` (see
    /// `build_tab_ui`) goes false again within ~1 frame for any URL-like
    /// input (nothing populates suggestions to keep it true the way a
    /// search query's does), and the very next frame's resync-to-tab-URL
    /// clobbers whatever `ADDRESSBAR` just set — confirmed live: a
    /// `SCREENSHOT` taken 1 second after `ADDRESSBAR https://github.com`
    /// showed the address bar already back to the tab's real URL, for
    /// every engine tried (not engine-specific). This pin makes
    /// `address_bar_focused` stay true across frames the same way real
    /// focus would, until something that should actually end editing
    /// clears it.
    debug_address_bar_pin: bool,
    /// Set by a headless `SCREENSHOT` command; consumed on the next
    /// `RedrawRequested` and passed into `gpu::GpuState::render`.
    pending_screenshot: Option<String>,

    /// Set when `spawn_root_tab`/`spawn_new_tab` fails (e.g. a stale
    /// engine-host from a previous crashed run still holding the tab's
    /// named pipe). Without this, a failed spawn left `tabs` empty and the
    /// user saw the ordinary "No tabs" Graph view with no indication
    /// anything had gone wrong — see `build_graph_ui`. Cleared at the start
    /// of the next spawn attempt so a retry doesn't leave a stale message
    /// on screen.
    tab_spawn_error: Option<String>,

    /// User-configurable defaults (search engine, starting engine, zoom,
    /// home page), loaded once at startup and persisted to
    /// `%APPDATA%\multiengine-browser\settings.json` on every change — see
    /// `settings.rs`. `settings_open` is the gear-icon popup's own
    /// visibility, app-wide rather than per-toolbar since Tab and Graph
    /// view never show chrome simultaneously.
    settings: Settings,
    settings_open: bool,

    /// The in-progress Tab↔Graph shrink/expand animation, if any — see
    /// `Transition`'s own doc comment.
    transition: Option<Transition>,
}

impl Default for App {
    fn default() -> Self {
        let egui_ctx = egui::Context::default();
        apply_style(&egui_ctx);
        let settings = Settings::load();
        Self {
            window: None,
            gpu: None,
            tabs: HashMap::new(),
            tab_order: Vec::new(),
            focused: None,
            view_mode: ViewMode::Tab,
            next_tab_id: 0,
            cursor_pos: (0.0, 0.0),
            key_modifiers: 0,
            egui_ctx,
            egui_state: None,
            address_bar_text: settings.home_page.clone(),
            address_bar_focused: false,
            open_tab_suggestions: Vec::new(),
            search_suggestions: Vec::new(),
            suggestion_fetcher: SuggestionFetcher::new(),
            dropdown_highlighted: None,
            dragging: None,
            hovered: None,
            graph_scene_rect: default_graph_scene_rect(),
            headless: false,
            pending_debug_escape: false,
            pending_debug_enter: false,
            pending_debug_arrow_down: false,
            pending_debug_arrow_up: false,
            debug_address_bar_pin: false,
            pending_screenshot: None,
            tab_spawn_error: None,
            settings,
            settings_open: false,
            transition: None,
        }
    }
}

impl App {
    fn chrome_height_points(&self) -> f32 {
        match self.view_mode {
            ViewMode::Tab => CHROME_HEIGHT_POINTS,
            ViewMode::Graph => GRAPH_TOOLBAR_HEIGHT_POINTS,
        }
    }

    fn chrome_height_px(&self) -> u32 {
        let ppp = self.window.as_ref().map_or(1.0, |w| w.scale_factor()) as f32;
        (self.chrome_height_points() * ppp).round() as u32
    }

    /// The engine-host's rendered surface only needs to cover the area
    /// below chrome — telling it to render at the full window size (when
    /// the compositor only ever displays it in the smaller content
    /// viewport, see `gpu::GpuState::render`) would waste work and,
    /// because the viewport stretches whatever the engine sends to fill
    /// it, visibly squash the page.
    fn content_size_px(&self) -> (u32, u32) {
        let full = self
            .window
            .as_ref()
            .map(|w| w.inner_size())
            .unwrap_or(winit::dpi::PhysicalSize::new(1024, 768));
        (full.width, full.height.saturating_sub(self.chrome_height_px()))
    }

    fn send_resize_to(&mut self, tab_id: &str) {
        let (width, height) = self.content_size_px();
        let dpi_scale = self.window.as_ref().map_or(1.0, |w| w.scale_factor());
        if let Some(node) = self.tabs.get_mut(tab_id) {
            node.tab.send(&HostToEngine::Resize {
                width,
                height,
                dpi_scale,
            });
        }
    }

    /// Applies `self.settings.zoom_factor` to one tab — called right after
    /// spawning it (new tabs pick up whatever the default is *at spawn
    /// time*; this never retroactively revisits already-open tabs, matching
    /// every other "default" setting here) and again by the Settings
    /// popup's zoom control for the currently focused tab specifically
    /// (`build_settings_popup`), since seeing the zoom control move with no
    /// visible effect on the tab you're looking at would be confusing.
    fn send_zoom_to(&mut self, tab_id: &str) {
        let factor = self.settings.zoom_factor;
        if let Some(node) = self.tabs.get_mut(tab_id) {
            node.tab.send(&HostToEngine::SetZoom { factor });
        }
    }

    /// Persists the current zoom setting and immediately applies it to
    /// whichever tab is focused right now — otherwise moving the zoom
    /// control would have no visible effect until the *next* tab spawn,
    /// which reads as broken rather than "this is a default for new tabs."
    fn apply_zoom_change(&mut self) {
        self.settings.save();
        if let Some(id) = self.focused.clone() {
            self.send_zoom_to(&id);
        }
    }

    /// The gear-icon popup: default search engine, default starting engine,
    /// zoom, and home page — see `settings.rs` for what each actually
    /// controls and `App::settings`'s own doc comment for why this is one
    /// shared popup rather than per-view state. Every control saves to disk
    /// immediately on change (no separate "Save" step, consistent with
    /// every other instant-apply control in this chrome, e.g. the engine
    /// picker).
    fn build_settings_popup(&mut self, ui: &mut egui::Ui, anchor_rect: egui::Rect) {
        let mut close = false;
        egui::Area::new(egui::Id::new("settings_popup"))
            .fixed_pos(anchor_rect.left_bottom())
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::default()
                    .fill(TOOLBAR_FILL)
                    .stroke(egui::Stroke::new(1.0, ui.visuals().weak_text_color()))
                    .corner_radius(6u8)
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        ui.set_min_width(260.0);
                        ui.label(egui::RichText::new("Settings").strong());
                        ui.separator();

                        ui.label("Default search engine");
                        egui::ComboBox::from_id_salt("settings_search_engine")
                            .selected_text(self.settings.default_search_engine.label())
                            .show_ui(ui, |ui| {
                                for engine in SearchEngine::ALL {
                                    let selected = self.settings.default_search_engine == engine;
                                    if ui.selectable_label(selected, engine.label()).clicked() && !selected {
                                        self.settings.default_search_engine = engine;
                                        self.settings.save();
                                    }
                                }
                            });
                        ui.label(
                            egui::RichText::new(
                                "Suggestions always come from DuckDuckGo; this only picks where a search actually goes.",
                            )
                            .small()
                            .weak(),
                        );

                        ui.add_space(8.0);
                        ui.label("Default starting engine (new tabs)");
                        egui::ComboBox::from_id_salt("settings_starting_engine")
                            .selected_text(engine_label(self.settings.default_starting_engine))
                            .show_ui(ui, |ui| {
                                for kind in [EngineKind::Servo, EngineKind::Cef, EngineKind::Webkit] {
                                    let selected = self.settings.default_starting_engine == kind;
                                    if ui.selectable_label(selected, engine_label(kind)).clicked() && !selected {
                                        self.settings.default_starting_engine = kind;
                                        self.settings.save();
                                    }
                                }
                            });

                        ui.add_space(8.0);
                        ui.label("Zoom (new tabs; also applies to the current tab now)");
                        ui.horizontal(|ui| {
                            if ui.button("\u{2212}").clicked() {
                                self.settings.zoom_factor = (self.settings.zoom_factor - 0.1).max(0.5);
                                self.apply_zoom_change();
                            }
                            ui.label(format!("{:.0}%", self.settings.zoom_factor * 100.0));
                            if ui.button("+").clicked() {
                                self.settings.zoom_factor = (self.settings.zoom_factor + 0.1).min(2.0);
                                self.apply_zoom_change();
                            }
                            if ui.button("Reset").clicked() {
                                self.settings.zoom_factor = 1.0;
                                self.apply_zoom_change();
                            }
                        });

                        ui.add_space(8.0);
                        ui.label("Home page (new tabs)");
                        if ui
                            .add(egui::TextEdit::singleline(&mut self.settings.home_page).desired_width(f32::INFINITY))
                            .lost_focus()
                        {
                            self.settings.save();
                        }

                        ui.add_space(10.0);
                        if ui.button("Close").clicked() {
                            close = true;
                        }
                    });
            });
        if close {
            self.settings_open = false;
        }
    }

    fn navigate_tab(&mut self, tab_id: &str, url: String) {
        if let Some(node) = self.tabs.get_mut(tab_id) {
            node.url = url.clone();
            node.tab
                .send(&HostToEngine::Navigate { url, referrer: None });
        }
    }

    /// Address-bar autocomplete's open-tab source: every *other* tab (an
    /// exact-match-only self isn't a useful "switch to" suggestion) whose
    /// URL or title contains `query`, case-insensitively. Capped and
    /// ordered by `tab_order` for determinism — this is a small in-memory
    /// list, not worth a fuzzy-match/relevance-ranking pass.
    fn compute_open_tab_suggestions(&self, query: &str, exclude: &str) -> Vec<Suggestion> {
        if query.is_empty() {
            return Vec::new();
        }
        let query = query.to_lowercase();
        self.tab_order
            .iter()
            .filter(|id| id.as_str() != exclude)
            .filter_map(|id| self.tabs.get(id).map(|node| (id, node)))
            .filter(|(_, node)| {
                node.url.to_lowercase().contains(&query) || node.title.to_lowercase().contains(&query)
            })
            .take(5)
            .map(|(id, node)| Suggestion::OpenTab {
                tab_id: id.clone(),
                url: node.url.clone(),
                title: node.title.clone(),
            })
            .collect()
    }

    /// The combined, ordered list the dropdown actually renders: open tabs
    /// first (jumping to something already loaded beats a fresh
    /// navigation/search), then search-engine suggestions.
    fn combined_suggestions(&self) -> Vec<Suggestion> {
        let mut all = self.open_tab_suggestions.clone();
        all.extend(
            self.search_suggestions
                .iter()
                .cloned()
                .map(|query| Suggestion::Search { query }),
        );
        all
    }

    /// Accepting a suggestion: jump straight to an already-open tab (no
    /// navigation at all), or navigate the focused tab to a search.
    /// Either way the dropdown itself is cleared and the address bar drops
    /// out of its "actively editing" state so it re-syncs to whichever
    /// tab ends up focused, same as any other navigation.
    fn accept_suggestion(&mut self, suggestion: Suggestion, focused_tab_id: &str) {
        match suggestion {
            Suggestion::OpenTab { tab_id, .. } => self.focus_tab(&tab_id),
            Suggestion::Search { query } => {
                let url = self.settings.default_search_engine.search_url(&query);
                self.navigate_tab(focused_tab_id, url);
            }
        }
        self.open_tab_suggestions.clear();
        self.search_suggestions.clear();
        self.address_bar_focused = false;
        self.debug_address_bar_pin = false;
        self.dropdown_highlighted = None;
    }

    /// Spawns the very first tab at startup: no parent, becomes focused,
    /// starts in Tab view (preserves "opens straight into a usable
    /// browser" from before M5 — Graph view is reached explicitly).
    fn spawn_root_tab(&mut self) {
        let id = self.alloc_tab_id();
        let home_page = self.settings.home_page.clone();
        self.tab_spawn_error = None;
        match process_manager::spawn_tab(self.settings.default_starting_engine, &id, &home_page) {
            Ok(mut tab) => {
                tab.set_visible(true);
                self.tabs.insert(
                    id.clone(),
                    TabNode {
                        tab,
                        parent: None,
                        url: home_page.clone(),
                        title: String::new(),
                        can_go_back: false,
                        can_go_forward: false,
                        loading: false,
                        swap_target: None,
                        pos: egui::Vec2::ZERO,
                        vel: egui::Vec2::ZERO,
                    },
                );
                self.tab_order.push(id.clone());
                self.focused = Some(id.clone());
                self.view_mode = ViewMode::Tab;
                self.address_bar_text = home_page;
                self.send_resize_to(&id);
                self.send_zoom_to(&id);
            }
            Err(err) => {
                eprintln!("spawn root tab failed: {err}");
                self.tab_spawn_error = Some(format!("Couldn't start a browser tab: {err}"));
            }
        }
    }

    fn alloc_tab_id(&mut self) -> String {
        let id = format!("tab-{}", self.next_tab_id);
        self.next_tab_id += 1;
        id
    }

    /// Graph view's "+ New Tab" action: a child of `parent` (or a new root
    /// if `parent` is `None`), spawned invisible — it stays in Graph view
    /// rather than auto-focusing (see plan's M5 scope decision 5), and
    /// background tabs render nothing until focused anyway. Also used for
    /// a page-triggered new tab (`EngineToHost::OpenTabRequested`), with
    /// that request's URL instead of the configured home page.
    fn spawn_new_tab(&mut self, parent: Option<String>, url: &str) {
        let id = self.alloc_tab_id();
        let parent_pos = parent
            .as_ref()
            .and_then(|p| self.tabs.get(p))
            .map(|n| n.pos)
            .unwrap_or(egui::Vec2::ZERO);
        let pos = parent_pos + graph_layout::spawn_jitter(&id);
        self.tab_spawn_error = None;
        match process_manager::spawn_tab(self.settings.default_starting_engine, &id, url) {
            Ok(mut tab) => {
                tab.set_visible(false);
                self.tabs.insert(
                    id.clone(),
                    TabNode {
                        tab,
                        parent,
                        url: url.to_string(),
                        title: String::new(),
                        can_go_back: false,
                        can_go_forward: false,
                        loading: false,
                        swap_target: None,
                        pos,
                        vel: egui::Vec2::ZERO,
                    },
                );
                self.tab_order.push(id.clone());
                self.send_zoom_to(&id);
            }
            Err(err) => {
                eprintln!("spawn new tab failed: {err}");
                self.tab_spawn_error = Some(format!("Couldn't start a new tab: {err}"));
            }
        }
    }

    /// Peek-click commits focus: the tab losing focus (if any, and if
    /// different) is told to stop rendering, the newly focused one is told
    /// to start, and the view switches to Tab. At most one tab is ever
    /// visible at a time.
    fn focus_tab(&mut self, id: &str) {
        if self.focused.as_deref() != Some(id) {
            if let Some(old_id) = self.focused.clone() {
                if let Some(old) = self.tabs.get_mut(&old_id) {
                    old.tab.set_visible(false);
                }
            }
        }
        if let Some(node) = self.tabs.get_mut(id) {
            node.tab.set_visible(true);
            if !self.address_bar_focused && !self.debug_address_bar_pin {
                self.address_bar_text = node.url.clone();
            }
        }
        self.focused = Some(id.to_string());
        self.view_mode = ViewMode::Tab;
        self.send_resize_to(id);
    }

    /// Leaves Tab view for Graph view: starts a shrink animation (see
    /// `Transition`) rather than switching instantly. The tab keeps
    /// rendering (`set_visible(true)`, unchanged) for the animation's
    /// whole duration, so the thumbnail it shrinks into shows real,
    /// current content instead of whatever frame happened to be on screen
    /// when the animation started — only actually suspended once the
    /// animation finishes (`finish_transition_if_done`).
    fn leave_to_graph(&mut self) {
        match self.focused.clone() {
            Some(id) => {
                self.transition = Some(Transition {
                    tab_id: id,
                    kind: TransitionKind::ToGraph,
                    start: std::time::Instant::now(),
                });
            }
            // No focused tab at all (shouldn't normally happen — Tab view
            // always has one — but nothing to animate either way).
            None => self.view_mode = ViewMode::Graph,
        }
        self.debug_address_bar_pin = false;
        self.dropdown_highlighted = None;
    }

    /// Graph view's node-click equivalent of `focus_tab` — starts an
    /// expand animation instead of switching instantly. Deliberately a
    /// separate method rather than folding this into `focus_tab` itself:
    /// `focus_tab` is also used for a same-view tab switch (accepting an
    /// open-tab autocomplete suggestion, already in Tab view), where an
    /// expand-from-a-graph-node animation wouldn't make sense.
    fn start_transition_to_tab(&mut self, id: &str) {
        self.focus_tab(id);
        // `focus_tab` already set `view_mode = Tab`, but `build_ui` renders
        // Graph view as the backdrop for as long as `self.transition` is
        // `Some` regardless of `view_mode` (see its own doc comment) —
        // `finish_transition_if_done` is what actually matters once the
        // animation completes.
        self.transition = Some(Transition {
            tab_id: id.to_string(),
            kind: TransitionKind::ToTab,
            start: std::time::Instant::now(),
        });
    }

    /// Checked once per frame, before rendering: once a `Transition`'s
    /// duration has elapsed, applies whatever `focus_tab`/`leave_to_graph`
    /// would have done instantly (the view-mode switch, and — for
    /// `ToGraph` specifically — finally suspending the tab now that its
    /// last real frame has been captured into the thumbnail it just
    /// shrank into).
    fn finish_transition_if_done(&mut self) {
        let Some(transition) = &self.transition else {
            return;
        };
        if transition.start.elapsed() < TRANSITION_DURATION {
            return;
        }
        let tab_id = transition.tab_id.clone();
        let kind = transition.kind;
        self.transition = None;
        match kind {
            TransitionKind::ToGraph => {
                self.view_mode = ViewMode::Graph;
                if let Some(node) = self.tabs.get_mut(&tab_id) {
                    node.tab.set_visible(false);
                }
            }
            TransitionKind::ToTab => {
                self.view_mode = ViewMode::Tab;
            }
        }
    }

    /// Replicates `egui::Scene`'s own internal `fit_to_rect_in_scene`
    /// (private to egui, not part of its public API) — needed to compute
    /// where a graph node actually lands on screen *outside* the `Scene`
    /// widget's own closure, for the shrink/expand animation's target
    /// rect. `(scale, translation)` such that
    /// `screen = translation + scale * world`, matching `Scene`'s own
    /// "fit the visible scene rect into the available screen area,
    /// preserving aspect ratio, clamped to the configured zoom range"
    /// behavior exactly (using the *same* `self.graph_scene_rect` `Scene`
    /// itself reads/writes) — verified live by screenshotting a
    /// mid-animation frame and confirming the content quad visually lines
    /// up with the target node's real on-screen position.
    fn graph_world_to_screen_transform(&self, scene_outer_rect: egui::Rect) -> (f32, egui::Vec2) {
        let scale = (scene_outer_rect.size() / self.graph_scene_rect.size())
            .min_elem()
            .clamp(GRAPH_ZOOM_MIN, GRAPH_ZOOM_MAX);
        let translation =
            scene_outer_rect.center().to_vec2() - scale * self.graph_scene_rect.center().to_vec2();
        (scale, translation)
    }

    /// The rect (in egui points, screen space) a `Transition` should draw
    /// its tab's live content quad at *this frame* — eased interpolation
    /// between Tab view's full content area and the tab's graph-node
    /// thumbnail spot, direction depending on `transition.kind`.
    fn transition_content_rect_points(&self, transition: &Transition) -> egui::Rect {
        let screen = self
            .window
            .as_ref()
            .map(|w| w.inner_size())
            .unwrap_or(winit::dpi::PhysicalSize::new(1024, 768));
        let ppp = self.window.as_ref().map_or(1.0, |w| w.scale_factor()) as f32;
        let screen_points = egui::vec2(screen.width as f32 / ppp, screen.height as f32 / ppp);

        let full_content_rect = egui::Rect::from_min_size(
            egui::pos2(0.0, CHROME_HEIGHT_POINTS),
            egui::vec2(screen_points.x, (screen_points.y - CHROME_HEIGHT_POINTS).max(0.0)),
        );

        let node_rect = self
            .tabs
            .get(&transition.tab_id)
            .map(|node| {
                let scene_outer_rect = egui::Rect::from_min_size(
                    egui::pos2(0.0, GRAPH_TOOLBAR_HEIGHT_POINTS),
                    egui::vec2(screen_points.x, (screen_points.y - GRAPH_TOOLBAR_HEIGHT_POINTS).max(0.0)),
                );
                let (scale, translation) = self.graph_world_to_screen_transform(scene_outer_rect);
                let world_rect =
                    egui::Rect::from_center_size(egui::pos2(node.pos.x, node.pos.y), GRAPH_NODE_SIZE)
                        .shrink(GRAPH_THUMB_MARGIN);
                egui::Rect::from_min_max(
                    egui::pos2(
                        world_rect.min.x * scale + translation.x,
                        world_rect.min.y * scale + translation.y,
                    ),
                    egui::pos2(
                        world_rect.max.x * scale + translation.x,
                        world_rect.max.y * scale + translation.y,
                    ),
                )
            })
            .unwrap_or(full_content_rect);

        let (from, to) = match transition.kind {
            TransitionKind::ToGraph => (full_content_rect, node_rect),
            TransitionKind::ToTab => (node_rect, full_content_rect),
        };
        let t = (transition.start.elapsed().as_secs_f32() / TRANSITION_DURATION.as_secs_f32()).clamp(0.0, 1.0);
        // Smoothstep: eases in and out, rather than the constant-velocity
        // (and slightly mechanical-looking) feel of plain linear `lerp`.
        let eased = t * t * (3.0 - 2.0 * t);
        egui::Rect::from_min_max(
            from.min + (to.min - from.min) * eased,
            from.max + (to.max - from.max) * eased,
        )
    }

    /// Closes a tab: tears down its process (via `EngineTab`'s `Drop` —
    /// `Shutdown` + kill), frees its graph-view thumbnail, and reparents
    /// its children to its own parent so the forest stays connected rather
    /// than orphaning that whole subtree. Falls back to Graph view if the
    /// closed tab was focused.
    fn close_tab(&mut self, id: &str) {
        let Some(parent) = self.tabs.get(id).map(|n| n.parent.clone()) else {
            return;
        };
        for node in self.tabs.values_mut() {
            if node.parent.as_deref() == Some(id) {
                node.parent = parent.clone();
            }
        }
        self.tabs.remove(id);
        self.tab_order.retain(|t| t != id);
        if let Some(gpu) = &mut self.gpu {
            gpu.free_tab_thumbnail(id);
        }
        if self.dragging.as_deref() == Some(id) {
            self.dragging = None;
        }
        if self.hovered.as_deref() == Some(id) {
            self.hovered = None;
        }
        if self.focused.as_deref() == Some(id) {
            self.focused = None;
            self.view_mode = ViewMode::Graph;
        }
    }

    /// M2's swap sequence, now per-tab and driven by the engine-picker
    /// dropdown: `GetState` -> spawn new engine at the same tab_id -> new
    /// engine's first `FrameReady` becomes the displayed texture (once
    /// focused) -> old engine-host dropped when its `TabNode` is replaced.
    fn start_swap_to(&mut self, tab_id: &str, target: EngineKind) {
        let Some(node) = self.tabs.get_mut(tab_id) else {
            return;
        };
        if node.tab.kind == target || node.swap_target.is_some() {
            return; // no-op swap, or one swap at a time for this tab
        }
        node.swap_target = Some(target);
        node.tab.send(&HostToEngine::GetState);
    }

    fn finish_swap(&mut self, tab_id: &str, target: EngineKind, url: String) {
        let Some((parent, pos, vel)) = self
            .tabs
            .get(tab_id)
            .map(|n| (n.parent.clone(), n.pos, n.vel))
        else {
            return;
        };
        // Must drop (and thus Shutdown + kill) the old tab before spawning
        // the new one at the same tab_id — the new engine-host's named-pipe
        // listeners would collide with the old ones otherwise.
        self.tabs.remove(tab_id);
        self.respawn_tab(tab_id, target, url, parent, pos, vel);
    }

    /// Shared by swap-finish and crash recovery: (re)spawn `kind` at `url`
    /// for `tab_id`, preserving its place in the graph (`parent`/`pos`/
    /// `vel`) and visibility (only the focused tab is ever visible).
    fn respawn_tab(
        &mut self,
        tab_id: &str,
        kind: EngineKind,
        url: String,
        parent: Option<String>,
        pos: egui::Vec2,
        vel: egui::Vec2,
    ) {
        match process_manager::spawn_tab(kind, tab_id, &url) {
            Ok(mut tab) => {
                let is_focused = self.focused.as_deref() == Some(tab_id);
                tab.set_visible(is_focused);
                self.tabs.insert(
                    tab_id.to_string(),
                    TabNode {
                        tab,
                        parent,
                        url: url.clone(),
                        title: String::new(),
                        can_go_back: false,
                        can_go_forward: false,
                        loading: false,
                        swap_target: None,
                        pos,
                        vel,
                    },
                );
                if !self.tab_order.iter().any(|t| t == tab_id) {
                    self.tab_order.push(tab_id.to_string());
                }
                if is_focused {
                    if !self.address_bar_focused && !self.debug_address_bar_pin {
                        self.address_bar_text = url;
                    }
                    self.send_resize_to(tab_id);
                }
            }
            Err(err) => {
                eprintln!("respawn {kind:?} for {tab_id} failed: {err}");
            }
        }
    }

    fn build_ui(&mut self, ui: &mut egui::Ui) {
        // A `Transition` (either direction) always renders Graph view's
        // node layout as the backdrop — it's the one screen both a
        // shrinking-away Tab view and an expanding-toward Tab view have a
        // coherent shared position in (the target/source node). The actual
        // animated content quad itself is drawn separately, in
        // `RedrawRequested` — see `transition_content_rect_points`.
        if self.transition.is_some() {
            self.build_graph_ui(ui);
            return;
        }
        match self.view_mode {
            ViewMode::Tab => self.build_tab_ui(ui),
            ViewMode::Graph => self.build_graph_ui(ui),
        }
    }

    fn build_tab_ui(&mut self, ui: &mut egui::Ui) {
        let Some(focused_id) = self.focused.clone() else {
            self.view_mode = ViewMode::Graph;
            return;
        };
        if !self.tabs.contains_key(&focused_id) {
            self.view_mode = ViewMode::Graph;
            return;
        }
        // Not just `!self.address_bar_focused`: found live that the moment
        // the suggestion dropdown's buttons first appear, egui's own
        // keyboard-focus tracking transiently moves off the `TextEdit` for
        // a frame (a real, reproducible bug, not a one-off) — `has_focus()`
        // then reports false for that one frame, which (without this) blew
        // away whatever the user had just typed by resyncing to the tab's
        // real URL mid-keystroke. Suggestions being non-empty is a good
        // proxy for "still actively editing" that survives that blip.
        let editing_address = self.address_bar_focused
            || !self.open_tab_suggestions.is_empty()
            || !self.search_suggestions.is_empty()
            || self.debug_address_bar_pin;
        if !editing_address {
            self.address_bar_text = self.tabs[&focused_id].url.clone();
        }
        let current_kind = self.tabs.get(&focused_id).map(|n| n.tab.kind);

        let mut go_back = false;
        let mut go_forward = false;
        let mut swap_to = None;
        let mut navigate = None;
        let mut back_to_graph = false;
        let mut address_bar_rect: Option<egui::Rect> = None;
        let mut settings_button_rect: Option<egui::Rect> = None;
        // Whether Enter fired this frame (real keypress or headless `ENTER`)
        // — deferred here rather than acted on immediately inside the
        // toolbar closure below, since deciding what Enter *does* (accept
        // the highlighted dropdown row vs. plain-navigate) needs `combined`
        // (the suggestion list), which isn't computed until after this
        // closure returns.
        let mut enter_pressed = false;

        egui::Panel::top("tab_strip")
            .exact_size(TAB_STRIP_HEIGHT_POINTS)
            .frame(
                egui::Frame::default()
                    .fill(TAB_STRIP_FILL)
                    .inner_margin(egui::Margin::symmetric(10, 0)),
            )
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    if ui.button("Graph").clicked() {
                        back_to_graph = true;
                    }
                    let node = &self.tabs[&focused_id];
                    let label = if node.title.is_empty() {
                        node.url.as_str()
                    } else {
                        node.title.as_str()
                    };
                    let kind = node.tab.kind;
                    engine_dot(ui, kind, 8.0);
                    if node.loading {
                        ui.add(egui::Spinner::new().size(12.0));
                    } else if let Some(tex_id) =
                        self.gpu.as_ref().and_then(|g| g.favicon_id(&focused_id))
                    {
                        ui.add(egui::Image::new(egui::load::SizedTexture::new(
                            tex_id,
                            FAVICON_SIZE,
                        )));
                    }
                    ui.add(egui::Label::new(label).truncate());
                });
            });

        egui::Panel::top("toolbar")
            .exact_size(TOOLBAR_HEIGHT_POINTS)
            .frame(
                egui::Frame::default()
                    .fill(TOOLBAR_FILL)
                    .inner_margin(egui::Margin::symmetric(8, 3)),
            )
            .show(ui, |ui| {
                // Not `horizontal_centered` (`Align::Center`): measured live
                // that `egui::ComboBox` paints itself ~4.5pt lower than a
                // plain `Button`/`TextEdit` even when all three report the
                // exact same `response.rect.height()` (confirmed via
                // temporary debug prints: 27.0 for all three, but the combo
                // box's `rect.min.y` alone sat 4.5pt below the back button's)
                // — an internal quirk of `ComboBox::show_ui` wrapping its
                // content in its own nested `ui.horizontal(...)`, whose
                // layout-pass size estimate apparently doesn't quite match
                // what it actually paints. Since every item in this row is
                // now confirmed the same real height (the address-bar-margin
                // fix above made sure of that), top-aligning instead of
                // center-aligning sidesteps the mismatch entirely rather
                // than compensating for a specific pixel offset that could
                // silently drift if padding/fonts ever change.
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                    let node = &self.tabs[&focused_id];
                    if ui
                        .add_enabled(node.can_go_back, egui::Button::new("\u{25c0}"))
                        .clicked()
                    {
                        go_back = true;
                    }
                    if ui
                        .add_enabled(node.can_go_forward, egui::Button::new("\u{25b6}"))
                        .clicked()
                    {
                        go_forward = true;
                    }

                    let combo_label = current_kind.map(engine_label).unwrap_or("?");
                    let combo_text = match current_kind {
                        Some(kind) => egui::RichText::new(combo_label).color(engine_color(kind)),
                        None => egui::RichText::new(combo_label),
                    };
                    egui::ComboBox::from_id_salt("engine_picker")
                        .selected_text(combo_text)
                        .show_ui(ui, |ui| {
                            for kind in [EngineKind::Servo, EngineKind::Cef, EngineKind::Webkit] {
                                let selected = current_kind == Some(kind);
                                ui.horizontal(|ui| {
                                    engine_dot(ui, kind, 8.0);
                                    if ui
                                        .selectable_label(selected, engine_label(kind))
                                        .clicked()
                                        && !selected
                                    {
                                        swap_to = Some(kind);
                                    }
                                });
                            }
                        });

                    // Reserve room for the gear button after the address
                    // bar — `desired_width(f32::INFINITY)` would otherwise
                    // claim the whole remaining row, leaving nothing for a
                    // widget added after it in the same horizontal layout.
                    let gear_reserve = 32.0;
                    let text_width = (ui.available_width() - gear_reserve).max(40.0);
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut self.address_bar_text)
                            .desired_width(text_width)
                            // Match `style.spacing.button_padding` (set in
                            // `apply_style`) so the address bar renders the
                            // same height as the back/forward buttons and
                            // engine picker next to it — `TextEdit`'s own
                            // default margin (`Margin::symmetric(4, 2)`)
                            // doesn't share that style value, so left unset
                            // the row's tallest item (the text field) stood
                            // out with visibly more height than its
                            // neighbors instead of a level toolbar.
                            .margin(egui::Margin::symmetric(8, 4)),
                    );
                    self.address_bar_focused = response.has_focus();
                    address_bar_rect = Some(response.rect);
                    let gear_response = ui.button("\u{2699}").on_hover_text("Settings");
                    if gear_response.clicked() {
                        self.settings_open = !self.settings_open;
                    }
                    settings_button_rect = Some(gear_response.rect);
                    if response.changed() {
                        self.open_tab_suggestions =
                            self.compute_open_tab_suggestions(&self.address_bar_text, &focused_id);
                        self.suggestion_fetcher.note_text_changed(&self.address_bar_text);
                        // A changed query invalidates whatever row was
                        // highlighted for the *previous* suggestion list —
                        // keeping a stale index could silently highlight
                        // (and Enter-accept) an unrelated row.
                        self.dropdown_highlighted = None;
                    }
                    // `pending_debug_enter` (headless mode's `ENTER` command,
                    // see `debug_control`) doesn't gate on `lost_focus()`:
                    // headless "focus" is set directly by the `ADDRESSBAR`
                    // command, not via a real click-driven egui focus
                    // transfer, so `response.lost_focus()` would never fire
                    // for it.
                    if std::mem::take(&mut self.pending_debug_enter)
                        || (response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                    {
                        enter_pressed = true;
                    }
                });
            });

        if self.settings_open {
            if let Some(rect) = settings_button_rect {
                self.build_settings_popup(ui, rect);
            }
        }

        if let Some(results) = self.suggestion_fetcher.tick() {
            self.search_suggestions = results;
        }

        // Autocomplete dropdown: a separate `egui::Area` anchored right
        // below the address bar (screen space, not the toolbar `Panel`'s
        // own layout — the dropdown needs to float over the content area
        // below chrome, which a plain widget inside the toolbar couldn't
        // do without stretching the toolbar itself). Clicking a row accepts
        // it directly; Up/Down move `dropdown_highlighted` (wrapping at
        // both ends) and Enter accepts whichever row is highlighted (see
        // `enter_pressed`'s resolution below) — reading arrow keys via
        // `ui.input(|i| i.key_pressed(...))` here, after the `TextEdit`
        // above already had its turn, is safe: `key_pressed` is a plain
        // per-frame boolean read, not a consumed event, and a singleline
        // `TextEdit` has no vertical text to move a cursor through, so
        // there's no competing behavior to fight over (Enter was the one
        // that actually needed care, via `enter_pressed`'s deferred
        // resolution — a singleline `TextEdit` losing focus on Enter is a
        // real, meaningful interaction to not double-handle).
        // Gated on `editing_address`, not raw `self.address_bar_focused`:
        // the same one-frame focus blip documented above (egui's focus
        // tracking transiently leaving the `TextEdit` when the dropdown's
        // buttons appear) was also hiding this whole Area, including the
        // buttons, on the exact frame a click landed on one — so the click
        // never reached anything and just silently dismissed the dropdown.
        let combined = self.combined_suggestions();
        if editing_address && !combined.is_empty() {
            let arrow_down = ui.input(|i| i.key_pressed(egui::Key::ArrowDown))
                || std::mem::take(&mut self.pending_debug_arrow_down);
            let arrow_up = ui.input(|i| i.key_pressed(egui::Key::ArrowUp))
                || std::mem::take(&mut self.pending_debug_arrow_up);
            if arrow_down {
                self.dropdown_highlighted = Some(match self.dropdown_highlighted {
                    Some(i) if i + 1 < combined.len() => i + 1,
                    _ => 0,
                });
            }
            if arrow_up {
                self.dropdown_highlighted = Some(match self.dropdown_highlighted {
                    Some(i) if i > 0 => i - 1,
                    _ => combined.len() - 1,
                });
            }

            if let Some(rect) = address_bar_rect {
                let mut accepted = None;
                egui::Area::new(egui::Id::new("address_suggestions"))
                    .fixed_pos(rect.left_bottom())
                    .order(egui::Order::Foreground)
                    .show(ui.ctx(), |ui| {
                        egui::Frame::default()
                            .fill(TOOLBAR_FILL)
                            .stroke(egui::Stroke::new(1.0, ui.visuals().weak_text_color()))
                            .corner_radius(6u8)
                            .inner_margin(egui::Margin::same(4))
                            .show(ui, |ui| {
                                ui.set_width(rect.width());
                                // `ui.set_width` alone only sets the *container's*
                                // layout width (affects wrapping/available_width),
                                // not an individual `Button`'s own size — left
                                // unset, each row was only as wide as its own
                                // text, producing a ragged right edge instead of
                                // a uniform list. `min_size`'s width stretches
                                // each row to fill the container; height stays
                                // `0.0` (no minimum) so natural row height/padding
                                // is untouched.
                                let full_width = ui.available_width();
                                for (idx, suggestion) in combined.iter().enumerate() {
                                    let highlighted = self.dropdown_highlighted == Some(idx);
                                    let mut button = egui::Button::new(suggestion.label())
                                        .truncate()
                                        .min_size(egui::vec2(full_width, 0.0));
                                    if highlighted {
                                        button = button.fill(ACCENT);
                                    }
                                    if ui.add(button).clicked() {
                                        accepted = Some(suggestion.clone());
                                    }
                                }
                            });
                    });
                if let Some(suggestion) = accepted {
                    self.accept_suggestion(suggestion, &focused_id);
                }
            }
        }

        if enter_pressed {
            match self.dropdown_highlighted.filter(|&i| i < combined.len()) {
                Some(idx) => self.accept_suggestion(combined[idx].clone(), &focused_id),
                None => navigate = Some(self.address_bar_text.clone()),
            }
        }

        let has_suggestions =
            !self.open_tab_suggestions.is_empty() || !self.search_suggestions.is_empty();
        // `pending_debug_escape` (headless mode's `ESCAPE` command) folds
        // into the same real-Escape check rather than getting its own
        // branch, so headless testing exercises the exact same dismiss-vs-
        // leave-to-graph logic a real keypress would.
        let escape_pressed =
            ui.input(|i| i.key_pressed(egui::Key::Escape)) || std::mem::take(&mut self.pending_debug_escape);
        if back_to_graph || (!has_suggestions && escape_pressed) {
            self.leave_to_graph();
            return;
        }
        if has_suggestions && escape_pressed {
            self.open_tab_suggestions.clear();
            self.search_suggestions.clear();
            self.dropdown_highlighted = None;
        }
        if go_back {
            if let Some(node) = self.tabs.get_mut(&focused_id) {
                node.tab.send(&HostToEngine::GoBack);
            }
        }
        if go_forward {
            if let Some(node) = self.tabs.get_mut(&focused_id) {
                node.tab.send(&HostToEngine::GoForward);
            }
        }
        if let Some(kind) = swap_to {
            self.start_swap_to(&focused_id, kind);
        }
        if let Some(text) = navigate {
            let url = resolve_navigation_url(&text, self.settings.default_search_engine);
            self.navigate_tab(&focused_id, url);
            self.open_tab_suggestions.clear();
            self.search_suggestions.clear();
            self.debug_address_bar_pin = false;
            self.dropdown_highlighted = None;
        }
    }

    fn build_graph_ui(&mut self, ui: &mut egui::Ui) {
        let mut new_tab_parent: Option<Option<String>> = None;
        let mut clicked_focus: Option<String> = None;
        let mut close_requested: Option<String> = None;

        let mut reset_view = false;
        let mut settings_button_rect: Option<egui::Rect> = None;
        egui::Panel::top("graph_toolbar")
            .exact_size(GRAPH_TOOLBAR_HEIGHT_POINTS)
            .frame(
                egui::Frame::default()
                    .fill(TOOLBAR_FILL)
                    .inner_margin(egui::Margin::symmetric(8, 3)),
            )
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    if ui.button("+ New Tab").clicked() {
                        new_tab_parent = Some(self.focused.clone());
                    }
                    if ui.button("Reset View").clicked() {
                        reset_view = true;
                    }
                    if let Some(err) = &self.tab_spawn_error {
                        ui.colored_label(egui::Color32::from_rgb(220, 60, 60), err);
                    }
                    let gear_response = ui.button("\u{2699}").on_hover_text("Settings");
                    if gear_response.clicked() {
                        self.settings_open = !self.settings_open;
                    }
                    settings_button_rect = Some(gear_response.rect);
                });
            });
        if self.settings_open {
            if let Some(rect) = settings_button_rect {
                self.build_settings_popup(ui, rect);
            }
        }
        if reset_view {
            self.graph_scene_rect = default_graph_scene_rect();
        }

        if self.tabs.is_empty() {
            ui.centered_and_justified(|ui| {
                match &self.tab_spawn_error {
                    Some(err) => ui.colored_label(
                        egui::Color32::from_rgb(220, 60, 60),
                        format!("{err}\nClick \"+ New Tab\" to try again."),
                    ),
                    None => ui.label("No tabs. Click \"+ New Tab\" to start browsing."),
                };
            });
            if let Some(parent) = new_tab_parent {
                let home_page = self.settings.home_page.clone();
                self.spawn_new_tab(parent, &home_page);
            }
            return;
        }

        // One force-directed layout tick. Real measured dt (clamped so a
        // hitch — e.g. spawning a new engine-host takes real wall time —
        // can't destabilize the spring math with a huge single step). Pure
        // world-space math, independent of the pan/zoom view below.
        let dt = ui.input(|i| i.stable_dt).min(0.1);
        {
            let mut positions: HashMap<String, egui::Vec2> =
                self.tabs.iter().map(|(id, n)| (id.clone(), n.pos)).collect();
            let mut velocities: HashMap<String, egui::Vec2> =
                self.tabs.iter().map(|(id, n)| (id.clone(), n.vel)).collect();
            let edges: Vec<(String, String)> = self
                .tabs
                .iter()
                .filter_map(|(id, n)| n.parent.clone().map(|p| (p, id.clone())))
                .collect();
            let mut pinned = HashSet::new();
            if let Some(d) = &self.dragging {
                pinned.insert(d.clone());
            }
            graph_layout::step(
                &mut positions,
                &mut velocities,
                &edges,
                &pinned,
                GRAPH_WORLD_CENTER,
                dt,
            );
            for (id, node) in self.tabs.iter_mut() {
                if let Some(p) = positions.get(id) {
                    node.pos = *p;
                }
                if let Some(v) = velocities.get(id) {
                    node.vel = *v;
                }
            }
        }

        // Pan (drag empty background) + zoom (scroll wheel, centered on the
        // cursor) via egui's built-in `Scene` container — everything drawn
        // inside `add_contents` below is in the same world-space coordinates
        // `node.pos` already uses; egui's transform layer handles converting
        // to/from screen space (including for nested widgets' own
        // hover/click/drag responses, e.g. a node's own drag below), so none
        // of that logic needs to know pan/zoom exists at all. `scene_rect`
        // must live outside `self` while the closure below also borrows
        // `self` mutably, so it's round-tripped through a local instead of
        // borrowed from `self.graph_scene_rect` directly.
        let mut scene_rect = self.graph_scene_rect;
        egui::Scene::new()
            .zoom_range(GRAPH_ZOOM_MIN..=GRAPH_ZOOM_MAX)
            .show(ui, &mut scene_rect, |ui| {
                // Edges first so node cards paint on top of the lines feeding them.
                for (id, node) in &self.tabs {
                    let Some(parent_id) = &node.parent else {
                        continue;
                    };
                    let Some(parent) = self.tabs.get(parent_id) else {
                        continue;
                    };
                    let _ = id;
                    ui.painter().line_segment(
                        [
                            egui::Pos2::new(parent.pos.x, parent.pos.y),
                            egui::Pos2::new(node.pos.x, node.pos.y),
                        ],
                        egui::Stroke::new(2.0, ui.visuals().weak_text_color()),
                    );
                }

                for id in self.tab_order.clone() {
                    let Some(node_pos) = self.tabs.get(&id).map(|n| n.pos) else {
                        continue;
                    };
                    let center_pos = egui::Pos2::new(node_pos.x, node_pos.y);
                    let rect = egui::Rect::from_center_size(center_pos, GRAPH_NODE_SIZE);
                    let response = ui.interact(
                        rect,
                        egui::Id::new(("graph_node", id.as_str())),
                        egui::Sense::click_and_drag(),
                    );

                    let is_focused = self.focused.as_deref() == Some(id.as_str());
                    let is_hovered = self.hovered.as_deref() == Some(id.as_str()) || response.hovered();

                    let card_fill = if is_hovered {
                        ui.visuals().extreme_bg_color.gamma_multiply(1.35)
                    } else {
                        ui.visuals().extreme_bg_color
                    };
                    ui.painter().rect_filled(rect, 6.0, card_fill);
                    let (border_width, border_color) = if is_focused {
                        (2.5, ACCENT)
                    } else if is_hovered {
                        (1.5, ui.visuals().text_color())
                    } else {
                        (1.0, ui.visuals().weak_text_color())
                    };
                    ui.painter().rect_stroke(
                        rect,
                        6.0,
                        egui::Stroke::new(border_width, border_color),
                        egui::StrokeKind::Inside,
                    );

                    let thumb_rect = rect.shrink(GRAPH_THUMB_MARGIN);
                    let thumb_id = self.gpu.as_ref().and_then(|g| g.thumbnail_id(&id));
                    if let Some(tex_id) = thumb_id {
                        ui.painter().image(
                            tex_id,
                            thumb_rect,
                            egui::Rect::from_min_max(egui::Pos2::new(0.0, 0.0), egui::Pos2::new(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );
                    } else {
                        ui.put(
                            egui::Rect::from_center_size(thumb_rect.center(), egui::vec2(24.0, 24.0)),
                            egui::Spinner::new().size(24.0),
                        );
                    }

                    if let Some(node) = self.tabs.get(&id) {
                        let label = if node.title.is_empty() {
                            node.url.as_str()
                        } else {
                            node.title.as_str()
                        };
                        // A flat scrim behind the title strip so it stays
                        // legible over a busy/light thumbnail — most real
                        // pages are light-background, and plain text on top
                        // of one was unreadable without this.
                        let title_strip = egui::Rect::from_min_max(
                            egui::Pos2::new(thumb_rect.left(), thumb_rect.bottom() - 18.0),
                            thumb_rect.right_bottom(),
                        );
                        ui.painter().rect_filled(
                            title_strip,
                            0.0,
                            egui::Color32::from_black_alpha(160),
                        );
                        // Engine identity as a small colored dot rather than
                        // repeating the "[Servo]"-style bracket text on every
                        // single node — the dot alone is enough once you've
                        // learned the color coding (also used in the tab
                        // strip and engine picker), and reads faster at this
                        // node's small size than another word would.
                        ui.painter().circle_filled(
                            egui::Pos2::new(title_strip.left() + 8.0, title_strip.center().y),
                            4.0,
                            engine_color(node.tab.kind),
                        );
                        ui.painter().text(
                            egui::Pos2::new(title_strip.left() + 16.0, title_strip.center().y),
                            egui::Align2::LEFT_CENTER,
                            label,
                            egui::FontId::default(),
                            egui::Color32::WHITE,
                        );
                    }
                    if let Some(tex_id) = self.gpu.as_ref().and_then(|g| g.favicon_id(&id)) {
                        let favicon_rect = egui::Rect::from_min_size(
                            rect.left_top() + egui::vec2(4.0, 4.0),
                            FAVICON_SIZE,
                        );
                        ui.painter().image(
                            tex_id,
                            favicon_rect,
                            egui::Rect::from_min_max(egui::Pos2::new(0.0, 0.0), egui::Pos2::new(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );
                    }

                    // Close button: a small square just outside the node's own
                    // top-right corner, given its own interact() id so it doesn't
                    // compete with the node's own click/drag sense over the same
                    // pixels. Only shown while the node is hovered — decluttering
                    // the graph at a glance was worth the tiny risk of "hover,
                    // then move a few px to reach the button" (same tradeoff
                    // real browser tab strips make for their own close buttons).
                    if is_hovered {
                        let close_rect =
                            egui::Rect::from_center_size(rect.right_top(), egui::vec2(18.0, 18.0));
                        let close_response = ui.interact(
                            close_rect,
                            egui::Id::new(("graph_node_close", id.as_str())),
                            egui::Sense::click(),
                        );
                        let close_fill = if close_response.hovered() {
                            egui::Color32::from_rgb(220, 60, 60)
                        } else {
                            ui.visuals().extreme_bg_color
                        };
                        ui.painter().rect_filled(close_rect, 4.0, close_fill);
                        ui.painter().rect_stroke(
                            close_rect,
                            4.0,
                            egui::Stroke::new(1.0, ui.visuals().weak_text_color()),
                            egui::StrokeKind::Inside,
                        );
                        ui.painter().text(
                            close_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "x",
                            egui::FontId::default(),
                            ui.visuals().text_color(),
                        );
                        if close_response.clicked() {
                            close_requested = Some(id.clone());
                        }
                    }

                    if response.dragged() {
                        self.dragging = Some(id.clone());
                        if let Some(node) = self.tabs.get_mut(&id) {
                            node.pos += response.drag_delta();
                        }
                    } else if self.dragging.as_deref() == Some(id.as_str()) {
                        self.dragging = None;
                    }
                    if response.hovered() {
                        self.hovered = Some(id.clone());
                    } else if self.hovered.as_deref() == Some(id.as_str()) {
                        self.hovered = None;
                    }
                    if response.clicked() {
                        clicked_focus = Some(id.clone());
                    }
                }

                // Peek preview: an enlarged view of the hovered node's cached
                // thumbnail, floating near it, without changing focus. Painted
                // directly in this same world-space layer (not a separate
                // `egui::Area`, which would paint in un-transformed screen
                // space) so it pans/zooms together with its node and simply
                // ends up on top by paint order, drawn after every node above.
                if let Some(hovered_id) = self.hovered.clone() {
                    let node_pos = self.tabs.get(&hovered_id).map(|n| n.pos);
                    let thumb_id = self.gpu.as_ref().and_then(|g| g.thumbnail_id(&hovered_id));
                    if let (Some(node_pos), Some(tex_id)) = (node_pos, thumb_id) {
                        let rect = egui::Rect::from_min_size(
                            egui::Pos2::new(
                                node_pos.x + GRAPH_NODE_SIZE.x / 2.0 + 12.0,
                                node_pos.y - GRAPH_PEEK_SIZE.y / 2.0,
                            ),
                            GRAPH_PEEK_SIZE,
                        );
                        ui.painter()
                            .rect_filled(rect, 6.0, ui.visuals().extreme_bg_color);
                        ui.painter().image(
                            tex_id,
                            rect,
                            egui::Rect::from_min_max(
                                egui::Pos2::new(0.0, 0.0),
                                egui::Pos2::new(1.0, 1.0),
                            ),
                            egui::Color32::WHITE,
                        );
                        ui.painter().rect_stroke(
                            rect,
                            6.0,
                            egui::Stroke::new(1.5, ACCENT),
                            egui::StrokeKind::Inside,
                        );
                    }
                }
            });
        self.graph_scene_rect = scene_rect;

        if let Some(parent) = new_tab_parent {
            let home_page = self.settings.home_page.clone();
            self.spawn_new_tab(parent, &home_page);
        }
        if let Some(id) = close_requested {
            self.close_tab(&id);
        }
        if let Some(id) = clicked_focus {
            if self.tabs.contains_key(&id) {
                self.start_transition_to_tab(&id);
            }
        }
    }
}

impl ApplicationHandler<DebugCommand> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let mut attrs = Window::default_attributes().with_title("multiengine-browser");
        if self.headless {
            // NOT `.with_visible(false)`: found live that a genuinely
            // invisible window's swapchain never successfully acquires a
            // frame on Windows (DWM permanently reports it occluded, so
            // `Surface::get_current_texture()` never returns `Success` —
            // confirmed by instrumenting `gpu::GpuState::render` and seeing
            // it bail out at that exact point on *every* frame, forever).
            // Positioning a real, visible-but-off-screen window instead
            // keeps DWM compositing it normally (frames actually render,
            // screenshots actually work) while it never overlaps the user's
            // actual screen — solving the real problem (focus/visual
            // contention with the user's own desktop use) without hitting
            // this platform limitation. `with_skip_taskbar` keeps it out of
            // the taskbar/alt-tab list since there's nothing to switch to.
            attrs = attrs
                .with_position(winit::dpi::PhysicalPosition::new(-32000, -32000))
                .with_decorations(false)
                .with_skip_taskbar(true);
        }
        let window = Arc::new(event_loop.create_window(attrs).expect("create window"));
        self.gpu = Some(GpuState::new(window.clone()));
        self.egui_state = Some(egui_winit::State::new(
            self.egui_ctx.clone(),
            egui::ViewportId::ROOT,
            window.as_ref(),
            Some(window.scale_factor() as f32),
            None,
            None,
        ));
        self.window = Some(window);
        self.spawn_root_tab();
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: DebugCommand) {
        self.handle_debug_command(event_loop, event);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let consumed_by_egui = match (&mut self.egui_state, &self.window) {
            (Some(state), Some(window)) => state.on_window_event(window.as_ref(), &event).consumed,
            _ => false,
        };
        let in_tab_view = matches!(self.view_mode, ViewMode::Tab);

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize(size.width, size.height);
                }
                if let Some(id) = self.focused.clone() {
                    self.send_resize_to(&id);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_pos = (position.x, position.y);
                let chrome_height = self.chrome_height_px() as f64;
                if in_tab_view && !consumed_by_egui && position.y >= chrome_height {
                    if let Some(id) = self.focused.clone() {
                        if let Some(node) = self.tabs.get_mut(&id) {
                            node.tab.send(&HostToEngine::Input(InputEvent::MouseMove {
                                x: position.x,
                                y: position.y - chrome_height,
                            }));
                        }
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let chrome_height = self.chrome_height_px() as f64;
                let (x, y) = self.cursor_pos;
                if in_tab_view && !consumed_by_egui && y >= chrome_height {
                    if let Some(button) = translate_mouse_button(button) {
                        if let Some(id) = self.focused.clone() {
                            if let Some(node) = self.tabs.get_mut(&id) {
                                let event = match state {
                                    ElementState::Pressed => InputEvent::MouseDown {
                                        x,
                                        y: y - chrome_height,
                                        button,
                                    },
                                    ElementState::Released => InputEvent::MouseUp {
                                        x,
                                        y: y - chrome_height,
                                        button,
                                    },
                                };
                                node.tab.send(&HostToEngine::Input(event));
                            }
                        }
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let chrome_height = self.chrome_height_px() as f64;
                let (x, y) = self.cursor_pos;
                if in_tab_view && !consumed_by_egui && y >= chrome_height {
                    if let Some(id) = self.focused.clone() {
                        if let Some(node) = self.tabs.get_mut(&id) {
                            let (dx, dy) = match delta {
                                winit::event::MouseScrollDelta::LineDelta(x, y) => {
                                    (x as f64 * 20.0, y as f64 * 20.0)
                                }
                                winit::event::MouseScrollDelta::PixelDelta(p) => (p.x, p.y),
                            };
                            node.tab.send(&HostToEngine::Input(InputEvent::Scroll {
                                x,
                                y: y - chrome_height,
                                delta_x: dx,
                                delta_y: dy,
                            }));
                        }
                    }
                }
            }
            WindowEvent::ModifiersChanged(new_modifiers) => {
                self.key_modifiers = winit_modifiers_to_bits(new_modifiers.state());
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if in_tab_view && !consumed_by_egui {
                    if let Some(id) = self.focused.clone() {
                        if let Some(node) = self.tabs.get_mut(&id) {
                            if let Some(key) = translate_key_code(event.physical_key) {
                                let input = match event.state {
                                    ElementState::Pressed => InputEvent::KeyDown {
                                        key,
                                        modifiers: self.key_modifiers,
                                    },
                                    ElementState::Released => InputEvent::KeyUp {
                                        key,
                                        modifiers: self.key_modifiers,
                                    },
                                };
                                node.tab.send(&HostToEngine::Input(input));
                            }
                            // The produced text (after layout/shift/dead-key
                            // processing) is separate from the physical key
                            // above — a page's text field wants this, a
                            // shortcut wants the key. winit only reports
                            // `text` on the press, not the release.
                            if event.state == ElementState::Pressed {
                                if let Some(text) = &event.text {
                                    for ch in text.chars() {
                                        node.tab.send(&HostToEngine::Input(InputEvent::Char {
                                            codepoint: ch as u32,
                                        }));
                                    }
                                }
                            }
                        }
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                self.drain_tab_events();
                // Checked before building this frame's UI so that, the
                // instant a `Transition` completes, this same frame already
                // renders the real destination view instead of an extra
                // stale transitioning frame.
                self.finish_transition_if_done();

                let egui_output = match (self.egui_state.take(), self.window.clone()) {
                    (Some(mut state), Some(window)) => {
                        let raw_input = state.take_egui_input(window.as_ref());
                        let ctx = self.egui_ctx.clone();
                        let output = ctx.run_ui(raw_input, |ui| self.build_ui(ui));
                        state.handle_platform_output(window.as_ref(), output.platform_output.clone());
                        self.egui_state = Some(state);
                        Some(output)
                    }
                    (state, _) => {
                        self.egui_state = state;
                        None
                    }
                };

                // The content quad's rect (points, screen space) and which
                // tab's texture to draw there — a `Transition` overrides
                // both to the animated in-between rect and that tab's id,
                // regardless of `view_mode`; otherwise the normal
                // full-content-area-in-Tab-view/nothing-in-Graph-view rule.
                let ppp = self.window.as_ref().map_or(1.0, |w| w.scale_factor()) as f32;
                let (content_rect_pts, focused_for_content) = if let Some(transition) = &self.transition {
                    (
                        self.transition_content_rect_points(transition),
                        Some(transition.tab_id.clone()),
                    )
                } else if self.view_mode == ViewMode::Tab {
                    let screen = self
                        .window
                        .as_ref()
                        .map(|w| w.inner_size())
                        .unwrap_or(winit::dpi::PhysicalSize::new(1024, 768));
                    let top = self.chrome_height_points();
                    let rect = egui::Rect::from_min_size(
                        egui::pos2(0.0, top),
                        egui::vec2(screen.width as f32 / ppp, (screen.height as f32 / ppp - top).max(0.0)),
                    );
                    (rect, self.focused.clone())
                } else {
                    (egui::Rect::ZERO, None)
                };
                let content_rect_px = (
                    content_rect_pts.min.x * ppp,
                    content_rect_pts.min.y * ppp,
                    content_rect_pts.width() * ppp,
                    content_rect_pts.height() * ppp,
                );

                let screenshot_path = self.pending_screenshot.take();
                if let Some(gpu) = &mut self.gpu {
                    match egui_output {
                        Some(output) => gpu.render(
                            content_rect_px,
                            focused_for_content.as_deref(),
                            Some((&self.egui_ctx, output)),
                            screenshot_path.as_deref(),
                        ),
                        None => gpu.render(
                            content_rect_px,
                            focused_for_content.as_deref(),
                            None,
                            screenshot_path.as_deref(),
                        ),
                    }
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            _ => {}
        }
    }
}

impl App {
    /// Feeds a synthetic `WindowEvent` through the exact same handling real
    /// OS input goes through (`Self::window_event`) — used by
    /// `handle_debug_command` for mouse commands, so headless mouse-driven
    /// testing exercises real egui hit-testing/rendering-timing rather than
    /// bypassing it (see `debug_control`'s doc comment for why this matters:
    /// it's the only way headless testing could have caught the dropdown
    /// click-swallowing bug fixed in the autocomplete feature). A no-op if
    /// the window doesn't exist yet (shouldn't happen in practice — commands
    /// only arrive after `resumed` has created it).
    fn dispatch_window_event(&mut self, event_loop: &ActiveEventLoop, event: WindowEvent) {
        let Some(window) = &self.window else { return };
        let id = window.id();
        self.window_event(event_loop, id, event);
    }

    /// Applies one headless `DebugCommand` (see `debug_control`'s doc
    /// comment for the overall design: mouse commands are real synthetic
    /// `WindowEvent`s, keyboard/text commands call the equivalent effect
    /// directly since `winit::event::KeyEvent` has no public constructor).
    fn handle_debug_command(&mut self, event_loop: &ActiveEventLoop, cmd: DebugCommand) {
        match cmd {
            DebugCommand::MouseMove { x, y } => {
                self.dispatch_window_event(
                    event_loop,
                    WindowEvent::CursorMoved {
                        device_id: DeviceId::dummy(),
                        position: PhysicalPosition::new(x, y),
                    },
                );
            }
            DebugCommand::MouseDown { x, y, button } => {
                self.dispatch_window_event(
                    event_loop,
                    WindowEvent::CursorMoved {
                        device_id: DeviceId::dummy(),
                        position: PhysicalPosition::new(x, y),
                    },
                );
                self.dispatch_window_event(
                    event_loop,
                    WindowEvent::MouseInput {
                        device_id: DeviceId::dummy(),
                        state: ElementState::Pressed,
                        button,
                    },
                );
            }
            DebugCommand::MouseUp { x, y, button } => {
                self.dispatch_window_event(
                    event_loop,
                    WindowEvent::CursorMoved {
                        device_id: DeviceId::dummy(),
                        position: PhysicalPosition::new(x, y),
                    },
                );
                self.dispatch_window_event(
                    event_loop,
                    WindowEvent::MouseInput {
                        device_id: DeviceId::dummy(),
                        state: ElementState::Released,
                        button,
                    },
                );
            }
            DebugCommand::Click { x, y, button } => {
                self.dispatch_window_event(
                    event_loop,
                    WindowEvent::CursorMoved {
                        device_id: DeviceId::dummy(),
                        position: PhysicalPosition::new(x, y),
                    },
                );
                self.dispatch_window_event(
                    event_loop,
                    WindowEvent::MouseInput {
                        device_id: DeviceId::dummy(),
                        state: ElementState::Pressed,
                        button,
                    },
                );
                self.dispatch_window_event(
                    event_loop,
                    WindowEvent::MouseInput {
                        device_id: DeviceId::dummy(),
                        state: ElementState::Released,
                        button,
                    },
                );
            }
            DebugCommand::Scroll { x, y, delta_x, delta_y } => {
                self.dispatch_window_event(
                    event_loop,
                    WindowEvent::CursorMoved {
                        device_id: DeviceId::dummy(),
                        position: PhysicalPosition::new(x, y),
                    },
                );
                self.dispatch_window_event(
                    event_loop,
                    WindowEvent::MouseWheel {
                        device_id: DeviceId::dummy(),
                        delta: MouseScrollDelta::PixelDelta(PhysicalPosition::new(delta_x, delta_y)),
                        phase: TouchPhase::Moved,
                    },
                );
            }
            DebugCommand::AddressBar(text) => {
                // Mirrors the real `TextEdit`'s `response.changed()` arm
                // (see `build_tab_ui`) — setting `address_bar_focused`
                // directly stands in for a real click-driven egui focus
                // transfer, which headless mode has no way to synthesize.
                // `debug_address_bar_pin` (see its own doc comment) is what
                // actually makes this stick across the frames between now
                // and whenever `ENTER`/`ESCAPE`/a suggestion click ends it —
                // `address_bar_focused` alone only lasts the one frame this
                // command is processed on.
                self.address_bar_focused = true;
                self.debug_address_bar_pin = true;
                let exclude = self.focused.clone().unwrap_or_default();
                self.open_tab_suggestions = self.compute_open_tab_suggestions(&text, &exclude);
                self.suggestion_fetcher.note_text_changed(&text);
                self.address_bar_text = text;
            }
            DebugCommand::Enter => self.pending_debug_enter = true,
            DebugCommand::Escape => self.pending_debug_escape = true,
            DebugCommand::ArrowDown => self.pending_debug_arrow_down = true,
            DebugCommand::ArrowUp => self.pending_debug_arrow_up = true,
            DebugCommand::TypeIntoPage(text) => {
                if let Some(id) = self.focused.clone() {
                    if let Some(node) = self.tabs.get_mut(&id) {
                        for ch in text.chars() {
                            node.tab
                                .send(&HostToEngine::Input(InputEvent::Char { codepoint: ch as u32 }));
                        }
                    }
                }
            }
            DebugCommand::KeyPage(key) => {
                if let Some(id) = self.focused.clone() {
                    if let Some(node) = self.tabs.get_mut(&id) {
                        node.tab.send(&HostToEngine::Input(InputEvent::KeyDown {
                            key,
                            modifiers: self.key_modifiers,
                        }));
                        node.tab.send(&HostToEngine::Input(InputEvent::KeyUp {
                            key,
                            modifiers: self.key_modifiers,
                        }));
                    }
                }
            }
            DebugCommand::SetVisible(visible) => {
                if let Some(id) = self.focused.clone() {
                    if let Some(node) = self.tabs.get_mut(&id) {
                        node.tab.set_visible(visible);
                    }
                }
            }
            DebugCommand::Screenshot(path) => {
                self.pending_screenshot = Some(path);
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            DebugCommand::Resize(width, height) => {
                if let Some(window) = &self.window {
                    let _ = window.request_inner_size(PhysicalSize::new(width, height));
                }
            }
            DebugCommand::Quit => event_loop.exit(),
        }
    }

    /// Drains every tab's `events_rx` (not just the focused one — a
    /// background tab can still crash, or finish an in-flight swap, while
    /// suspended) and applies the results. Frame uploads are collected and
    /// applied after crash/swap handling, since either of those can remove
    /// the very `TabNode` a queued `FrameReady` refers to — applying to a
    /// since-removed tab_id is harmless (checked at upload time via `gpu`
    /// only, no `self.tabs` lookup needed there) but there's no reason to
    /// upload a stale frame for a tab that's mid-replacement.
    fn drain_tab_events(&mut self) {
        struct FrameUpdate {
            tab_id: String,
            /// Socket/shared-memory id this tab's engine-host actually used
            /// (see `EngineTab::socket_id`) — usually equal to `tab_id`,
            /// but can diverge after the stale-pipe fallback in
            /// `process_manager::spawn_tab`. Only used to look up the
            /// shared frame buffer below; GPU texture bookkeeping stays
            /// keyed by `tab_id`.
            socket_id: String,
            kind: EngineKind,
            width: u32,
            height: u32,
            pixels: Option<Vec<u8>>,
        }

        let tab_ids: Vec<String> = self.tab_order.clone();
        let mut frame_updates: Vec<FrameUpdate> = Vec::new();
        let mut swap_finishes: Vec<(String, EngineKind, String)> = Vec::new();
        let mut crashes: Vec<(String, String)> = Vec::new();
        let mut open_tab_requests: Vec<(String, String)> = Vec::new();
        let mut favicon_updates: Vec<(String, Vec<u8>, u32, u32)> = Vec::new();

        for tab_id in &tab_ids {
            let Some(node) = self.tabs.get_mut(tab_id) else {
                continue;
            };
            while let Ok(msg) = node.tab.events_rx.try_recv() {
                match msg {
                    EngineToHost::FrameReady {
                        width,
                        height,
                        pixels,
                        ..
                    } => {
                        frame_updates.push(FrameUpdate {
                            tab_id: tab_id.clone(),
                            socket_id: node.tab.socket_id.clone(),
                            kind: node.tab.kind,
                            width,
                            height,
                            pixels,
                        });
                    }
                    EngineToHost::StateSnapshot(state) => {
                        if let Some(target) = node.swap_target.take() {
                            swap_finishes.push((tab_id.clone(), target, state.url));
                        }
                    }
                    EngineToHost::NavigationStateChanged {
                        url,
                        title,
                        can_go_back,
                        can_go_forward,
                        loading,
                    } => {
                        node.url = url;
                        node.title = title;
                        node.can_go_back = can_go_back;
                        node.can_go_forward = can_go_forward;
                        node.loading = loading;
                    }
                    EngineToHost::Crashed { reason } => {
                        crashes.push((tab_id.clone(), reason));
                    }
                    EngineToHost::OpenTabRequested { url } => {
                        open_tab_requests.push((tab_id.clone(), url));
                    }
                    EngineToHost::FaviconChanged { rgba, width, height } => {
                        favicon_updates.push((tab_id.clone(), rgba, width, height));
                    }
                }
            }
        }

        for (tab_id, reason) in crashes {
            let Some((kind, url, parent, pos, vel)) = self.tabs.get(&tab_id).map(|n| {
                (
                    n.tab.kind,
                    n.url.clone(),
                    n.parent.clone(),
                    n.pos,
                    n.vel,
                )
            }) else {
                continue;
            };
            eprintln!("engine-host crashed ({reason}) for {tab_id}, restarting at {url}");
            self.tabs.remove(&tab_id);
            self.respawn_tab(&tab_id, kind, url, parent, pos, vel);
        }

        for (tab_id, target, url) in swap_finishes {
            self.finish_swap(&tab_id, target, url);
        }

        // Page-triggered new tab (target=_blank/window.open()/ctrl+click,
        // intercepted by CEF/WebKit - see EngineToHost::OpenTabRequested's
        // doc comment): a child of whichever tab requested it, staying in
        // the current view rather than auto-focusing, same as an explicit
        // "+ New Tab" click.
        for (requesting_tab_id, url) in open_tab_requests {
            self.spawn_new_tab(Some(requesting_tab_id), &url);
        }

        for (tab_id, rgba, width, height) in favicon_updates {
            if let Some(gpu) = &mut self.gpu {
                gpu.upload_favicon(&tab_id, &rgba, width, height);
            }
        }

        for update in frame_updates {
            let format = match update.kind {
                EngineKind::Servo | EngineKind::Webkit => wgpu::TextureFormat::Rgba8UnormSrgb,
                EngineKind::Cef => wgpu::TextureFormat::Bgra8UnormSrgb,
            };
            if let Some(pixels) = update.pixels {
                // RemoteTransport engine-host (WebKit over WSL2): no shared
                // memory with this process, so the pixels travel inline in
                // the message itself.
                if let Some(gpu) = &mut self.gpu {
                    gpu.upload_frame(&update.tab_id, &pixels, update.width, update.height, format);
                }
            } else if let Ok(buf) = SharedFrameBuffer::open_existing(
                frame_path(&update.socket_id),
                update.width,
                update.height,
            ) {
                // The engine-host truncates-then-grows this file when
                // (re)creating it (see SharedFrameBuffer::create), which
                // briefly exposes a 0-byte or otherwise wrong-sized file to
                // a concurrent reader. Skip this frame rather than panic on
                // a mismatched slice — the next redraw tries again.
                let expected_len = (update.width as usize) * (update.height as usize) * 4;
                if buf.as_slice().len() == expected_len {
                    if let Some(gpu) = &mut self.gpu {
                        gpu.upload_frame(&update.tab_id, buf.as_slice(), update.width, update.height, format);
                    }
                }
            }
        }
    }
}

fn translate_mouse_button(button: WinitMouseButton) -> Option<MouseButton> {
    match button {
        WinitMouseButton::Left => Some(MouseButton::Left),
        WinitMouseButton::Right => Some(MouseButton::Right),
        WinitMouseButton::Middle => Some(MouseButton::Middle),
        _ => None,
    }
}

fn winit_modifiers_to_bits(state: ModifiersState) -> u8 {
    let mut bits = 0u8;
    if state.shift_key() {
        bits |= modifiers::SHIFT;
    }
    if state.control_key() {
        bits |= modifiers::CONTROL;
    }
    if state.alt_key() {
        bits |= modifiers::ALT;
    }
    if state.super_key() {
        bits |= modifiers::SUPER;
    }
    bits
}

/// Translates winit's cross-platform physical key into our own
/// engine-agnostic `ipc_protocol::KeyCode` — deliberately not forwarding
/// winit's `KeyCode` directly over IPC (not meant to be a stable wire
/// format across winit versions); see that type's own doc comment. Common
/// keys only, matching `KeyCode`'s own scope.
fn translate_key_code(key: PhysicalKey) -> Option<KeyCode> {
    let PhysicalKey::Code(code) = key else {
        return None;
    };
    Some(match code {
        WinitKeyCode::KeyA => KeyCode::KeyA,
        WinitKeyCode::KeyB => KeyCode::KeyB,
        WinitKeyCode::KeyC => KeyCode::KeyC,
        WinitKeyCode::KeyD => KeyCode::KeyD,
        WinitKeyCode::KeyE => KeyCode::KeyE,
        WinitKeyCode::KeyF => KeyCode::KeyF,
        WinitKeyCode::KeyG => KeyCode::KeyG,
        WinitKeyCode::KeyH => KeyCode::KeyH,
        WinitKeyCode::KeyI => KeyCode::KeyI,
        WinitKeyCode::KeyJ => KeyCode::KeyJ,
        WinitKeyCode::KeyK => KeyCode::KeyK,
        WinitKeyCode::KeyL => KeyCode::KeyL,
        WinitKeyCode::KeyM => KeyCode::KeyM,
        WinitKeyCode::KeyN => KeyCode::KeyN,
        WinitKeyCode::KeyO => KeyCode::KeyO,
        WinitKeyCode::KeyP => KeyCode::KeyP,
        WinitKeyCode::KeyQ => KeyCode::KeyQ,
        WinitKeyCode::KeyR => KeyCode::KeyR,
        WinitKeyCode::KeyS => KeyCode::KeyS,
        WinitKeyCode::KeyT => KeyCode::KeyT,
        WinitKeyCode::KeyU => KeyCode::KeyU,
        WinitKeyCode::KeyV => KeyCode::KeyV,
        WinitKeyCode::KeyW => KeyCode::KeyW,
        WinitKeyCode::KeyX => KeyCode::KeyX,
        WinitKeyCode::KeyY => KeyCode::KeyY,
        WinitKeyCode::KeyZ => KeyCode::KeyZ,
        WinitKeyCode::Digit0 => KeyCode::Digit0,
        WinitKeyCode::Digit1 => KeyCode::Digit1,
        WinitKeyCode::Digit2 => KeyCode::Digit2,
        WinitKeyCode::Digit3 => KeyCode::Digit3,
        WinitKeyCode::Digit4 => KeyCode::Digit4,
        WinitKeyCode::Digit5 => KeyCode::Digit5,
        WinitKeyCode::Digit6 => KeyCode::Digit6,
        WinitKeyCode::Digit7 => KeyCode::Digit7,
        WinitKeyCode::Digit8 => KeyCode::Digit8,
        WinitKeyCode::Digit9 => KeyCode::Digit9,
        WinitKeyCode::Enter => KeyCode::Enter,
        WinitKeyCode::Backspace => KeyCode::Backspace,
        WinitKeyCode::Tab => KeyCode::Tab,
        WinitKeyCode::Space => KeyCode::Space,
        WinitKeyCode::Escape => KeyCode::Escape,
        WinitKeyCode::Delete => KeyCode::Delete,
        WinitKeyCode::ArrowLeft => KeyCode::ArrowLeft,
        WinitKeyCode::ArrowRight => KeyCode::ArrowRight,
        WinitKeyCode::ArrowUp => KeyCode::ArrowUp,
        WinitKeyCode::ArrowDown => KeyCode::ArrowDown,
        WinitKeyCode::Home => KeyCode::Home,
        WinitKeyCode::End => KeyCode::End,
        WinitKeyCode::PageUp => KeyCode::PageUp,
        WinitKeyCode::PageDown => KeyCode::PageDown,
        WinitKeyCode::ShiftLeft => KeyCode::ShiftLeft,
        WinitKeyCode::ShiftRight => KeyCode::ShiftRight,
        WinitKeyCode::ControlLeft => KeyCode::ControlLeft,
        WinitKeyCode::ControlRight => KeyCode::ControlRight,
        WinitKeyCode::AltLeft => KeyCode::AltLeft,
        WinitKeyCode::AltRight => KeyCode::AltRight,
        _ => return None,
    })
}

fn main() {
    let headless = std::env::args().any(|arg| arg == "--headless");

    let event_loop = EventLoop::<DebugCommand>::with_user_event()
        .build()
        .expect("create event loop");
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = App::default();
    app.headless = headless;
    if headless {
        debug_control::spawn_stdin_reader(event_loop.create_proxy());
    }

    event_loop.run_app(&mut app).expect("run event loop");
}
