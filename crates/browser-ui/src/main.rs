mod gpu;
mod graph_layout;
mod process_manager;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpu::GpuState;
use ipc_protocol::{
    frame_path, modifiers, EngineToHost, HostToEngine, InputEvent, KeyCode, MouseButton,
    SharedFrameBuffer,
};
use process_manager::{EngineKind, EngineTab};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton as WinitMouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode as WinitKeyCode, ModifiersState, PhysicalKey};
use winit::window::{Window, WindowId};

const INITIAL_URL: &str = "https://example.com";
const STARTING_ENGINE: EngineKind = EngineKind::Servo;

/// Chrome (tab strip + toolbar) is two stacked top `Panel`s of these
/// exact heights, in egui points — kept as constants rather than read back
/// from egui's layout so the compositor's content viewport (see `gpu::
/// GpuState::render`'s `content_top_px`) always agrees with where egui
/// actually painted, without a frame of lag.
const TAB_STRIP_HEIGHT_POINTS: f32 = 28.0;
const TOOLBAR_HEIGHT_POINTS: f32 = 40.0;
const CHROME_HEIGHT_POINTS: f32 = TAB_STRIP_HEIGHT_POINTS + TOOLBAR_HEIGHT_POINTS;
/// Graph view's chrome is just the "+ New Tab" toolbar — no address bar,
/// no per-tab back/forward, since there's no single focused tab there.
const GRAPH_TOOLBAR_HEIGHT_POINTS: f32 = 32.0;

const GRAPH_NODE_SIZE: egui::Vec2 = egui::vec2(160.0, 110.0);
const GRAPH_THUMB_MARGIN: f32 = 6.0;
const GRAPH_PEEK_SIZE: egui::Vec2 = egui::vec2(360.0, 240.0);

fn engine_label(kind: EngineKind) -> &'static str {
    match kind {
        EngineKind::Servo => "Servo",
        EngineKind::Cef => "Blink (CEF)",
        EngineKind::Webkit => "WebKit",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    Graph,
    Tab,
}

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

    /// Graph view: the node currently being dragged (pinned in the physics
    /// sim — see `graph_layout::step`'s `pinned` parameter) and the node
    /// currently hovered (drives the peek-preview popup).
    dragging: Option<String>,
    hovered: Option<String>,
}

impl Default for App {
    fn default() -> Self {
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
            egui_ctx: egui::Context::default(),
            egui_state: None,
            address_bar_text: INITIAL_URL.to_string(),
            address_bar_focused: false,
            dragging: None,
            hovered: None,
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

    fn navigate_tab(&mut self, tab_id: &str, url: String) {
        if let Some(node) = self.tabs.get_mut(tab_id) {
            node.url = url.clone();
            node.tab
                .send(&HostToEngine::Navigate { url, referrer: None });
        }
    }

    /// Spawns the very first tab at startup: no parent, becomes focused,
    /// starts in Tab view (preserves "opens straight into a usable
    /// browser" from before M5 — Graph view is reached explicitly).
    fn spawn_root_tab(&mut self) {
        let id = self.alloc_tab_id();
        match process_manager::spawn_tab(STARTING_ENGINE, &id, INITIAL_URL) {
            Ok(mut tab) => {
                tab.set_visible(true);
                self.tabs.insert(
                    id.clone(),
                    TabNode {
                        tab,
                        parent: None,
                        url: INITIAL_URL.to_string(),
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
                self.address_bar_text = INITIAL_URL.to_string();
                self.send_resize_to(&id);
            }
            Err(err) => {
                eprintln!("spawn root tab failed: {err}");
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
    /// that request's URL instead of `INITIAL_URL`.
    fn spawn_new_tab(&mut self, parent: Option<String>, url: &str) {
        let id = self.alloc_tab_id();
        let parent_pos = parent
            .as_ref()
            .and_then(|p| self.tabs.get(p))
            .map(|n| n.pos)
            .unwrap_or(egui::Vec2::ZERO);
        let pos = parent_pos + graph_layout::spawn_jitter(&id);
        match process_manager::spawn_tab(STARTING_ENGINE, &id, url) {
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
                self.tab_order.push(id);
            }
            Err(err) => {
                eprintln!("spawn new tab failed: {err}");
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
            if !self.address_bar_focused {
                self.address_bar_text = node.url.clone();
            }
        }
        self.focused = Some(id.to_string());
        self.view_mode = ViewMode::Tab;
        self.send_resize_to(id);
    }

    /// Leaves Tab view for Graph view: the focused tab stops rendering
    /// (nothing is visible while in Graph view — its node just shows its
    /// last frame as a thumbnail) but keeps running.
    fn leave_to_graph(&mut self) {
        if let Some(id) = &self.focused {
            if let Some(node) = self.tabs.get_mut(id) {
                node.tab.set_visible(false);
            }
        }
        self.view_mode = ViewMode::Graph;
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
                    if !self.address_bar_focused {
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
        if !self.address_bar_focused {
            self.address_bar_text = self.tabs[&focused_id].url.clone();
        }
        let current_kind = self.tabs.get(&focused_id).map(|n| n.tab.kind);

        let mut go_back = false;
        let mut go_forward = false;
        let mut swap_to = None;
        let mut navigate = None;
        let mut back_to_graph = false;

        egui::Panel::top("tab_strip")
            .exact_size(TAB_STRIP_HEIGHT_POINTS)
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
                    let text = if node.loading {
                        format!("(loading) {label}")
                    } else {
                        label.to_string()
                    };
                    ui.add(egui::Label::new(text).truncate());
                });
            });

        egui::Panel::top("toolbar")
            .exact_size(TOOLBAR_HEIGHT_POINTS)
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
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
                    egui::ComboBox::from_id_salt("engine_picker")
                        .selected_text(combo_label)
                        .show_ui(ui, |ui| {
                            for kind in [EngineKind::Servo, EngineKind::Cef, EngineKind::Webkit] {
                                let selected = current_kind == Some(kind);
                                if ui.selectable_label(selected, engine_label(kind)).clicked()
                                    && !selected
                                {
                                    swap_to = Some(kind);
                                }
                            }
                        });

                    let response = ui.add(
                        egui::TextEdit::singleline(&mut self.address_bar_text)
                            .desired_width(f32::INFINITY),
                    );
                    self.address_bar_focused = response.has_focus();
                    if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        navigate = Some(self.address_bar_text.clone());
                    }
                });
            });

        if back_to_graph || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.leave_to_graph();
            return;
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
        if let Some(url) = navigate {
            self.navigate_tab(&focused_id, url);
        }
    }

    fn build_graph_ui(&mut self, ui: &mut egui::Ui) {
        let mut new_tab_parent: Option<Option<String>> = None;
        let mut clicked_focus: Option<String> = None;
        let mut close_requested: Option<String> = None;

        egui::Panel::top("graph_toolbar")
            .exact_size(GRAPH_TOOLBAR_HEIGHT_POINTS)
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    if ui.button("+ New Tab").clicked() {
                        new_tab_parent = Some(self.focused.clone());
                    }
                });
            });

        if self.tabs.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label("No tabs. Click \"+ New Tab\" to start browsing.");
            });
            if let Some(parent) = new_tab_parent {
                self.spawn_new_tab(parent, INITIAL_URL);
            }
            return;
        }

        let canvas_rect = ui.available_rect_before_wrap();
        let center = egui::Vec2::new(canvas_rect.center().x, canvas_rect.center().y);

        // One force-directed layout tick. Real measured dt (clamped so a
        // hitch — e.g. spawning a new engine-host takes real wall time —
        // can't destabilize the spring math with a huge single step).
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
            graph_layout::step(&mut positions, &mut velocities, &edges, &pinned, center, dt);
            for (id, node) in self.tabs.iter_mut() {
                if let Some(p) = positions.get(id) {
                    node.pos = *p;
                }
                if let Some(v) = velocities.get(id) {
                    node.vel = *v;
                }
            }
        }

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
            ui.painter()
                .rect_filled(rect, 6.0, ui.visuals().extreme_bg_color);
            ui.painter().rect_stroke(
                rect,
                6.0,
                egui::Stroke::new(
                    if is_focused { 2.5 } else { 1.0 },
                    ui.visuals().text_color(),
                ),
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
                ui.painter().text(
                    thumb_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "loading...",
                    egui::FontId::default(),
                    ui.visuals().weak_text_color(),
                );
            }

            if let Some(node) = self.tabs.get(&id) {
                let label = if node.title.is_empty() {
                    node.url.as_str()
                } else {
                    node.title.as_str()
                };
                ui.painter().text(
                    egui::Pos2::new(rect.center().x, rect.bottom() - 4.0),
                    egui::Align2::CENTER_BOTTOM,
                    format!("{label} [{}]", engine_label(node.tab.kind)),
                    egui::FontId::default(),
                    ui.visuals().text_color(),
                );
            }

            // Close button: a small square just outside the node's own
            // top-right corner, given its own interact() id so it doesn't
            // compete with the node's own click/drag sense over the same
            // pixels.
            let close_rect = egui::Rect::from_center_size(rect.right_top(), egui::vec2(18.0, 18.0));
            let close_response = ui.interact(
                close_rect,
                egui::Id::new(("graph_node_close", id.as_str())),
                egui::Sense::click(),
            );
            ui.painter()
                .rect_filled(close_rect, 3.0, ui.visuals().extreme_bg_color);
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
        // thumbnail, floating near it, without changing focus.
        if let Some(hovered_id) = self.hovered.clone() {
            let node_pos = self.tabs.get(&hovered_id).map(|n| n.pos);
            let thumb_id = self.gpu.as_ref().and_then(|g| g.thumbnail_id(&hovered_id));
            if let (Some(node_pos), Some(tex_id)) = (node_pos, thumb_id) {
                let anchor = egui::Pos2::new(
                    node_pos.x + GRAPH_NODE_SIZE.x / 2.0 + 12.0,
                    node_pos.y - GRAPH_PEEK_SIZE.y / 2.0,
                );
                egui::Area::new(egui::Id::new("graph_peek_preview"))
                    .fixed_pos(anchor)
                    .show(ui.ctx(), |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            let (rect, _) =
                                ui.allocate_exact_size(GRAPH_PEEK_SIZE, egui::Sense::hover());
                            ui.painter().image(
                                tex_id,
                                rect,
                                egui::Rect::from_min_max(
                                    egui::Pos2::new(0.0, 0.0),
                                    egui::Pos2::new(1.0, 1.0),
                                ),
                                egui::Color32::WHITE,
                            );
                        });
                    });
            }
        }

        if let Some(parent) = new_tab_parent {
            self.spawn_new_tab(parent, INITIAL_URL);
        }
        if let Some(id) = close_requested {
            self.close_tab(&id);
        }
        if let Some(id) = clicked_focus {
            if self.tabs.contains_key(&id) {
                self.focus_tab(&id);
            }
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let window = Arc::new(
            event_loop
                .create_window(Window::default_attributes().with_title("multiengine-browser"))
                .expect("create window"),
        );
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

                let chrome_height = self.chrome_height_px();
                let focused_for_content = match self.view_mode {
                    ViewMode::Tab => self.focused.as_deref(),
                    ViewMode::Graph => None,
                };
                if let Some(gpu) = &mut self.gpu {
                    match egui_output {
                        Some(output) => {
                            gpu.render(chrome_height, focused_for_content, Some((&self.egui_ctx, output)))
                        }
                        None => gpu.render(chrome_height, focused_for_content, None),
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
                    _ => {}
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
            } else if let Ok(buf) =
                SharedFrameBuffer::open_existing(frame_path(&update.tab_id), update.width, update.height)
            {
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
    let event_loop = EventLoop::new().expect("create event loop");
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::default();
    event_loop.run_app(&mut app).expect("run event loop");
}
