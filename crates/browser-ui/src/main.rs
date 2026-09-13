mod gpu;
mod process_manager;

use std::sync::Arc;

use gpu::GpuState;
use ipc_protocol::{frame_path, EngineToHost, HostToEngine, InputEvent, MouseButton, SharedFrameBuffer};
use process_manager::{EngineKind, EngineTab};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton as WinitMouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

const TAB_ID: &str = "tab-0";
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

fn engine_label(kind: EngineKind) -> &'static str {
    match kind {
        EngineKind::Servo => "Servo",
        EngineKind::Cef => "Blink (CEF)",
        EngineKind::Webkit => "WebKit",
    }
}

struct App {
    window: Option<Arc<Window>>,
    gpu: Option<GpuState>,
    tab: Option<EngineTab>,
    cursor_pos: (f64, f64),
    /// Set while a swap is in flight: the engine to swap *to*, once the
    /// current tab's `GetState` reply (its `StateSnapshot`) comes back.
    swap_target: Option<EngineKind>,
    /// Last known good URL for the current tab, kept up to date from
    /// `NavigationStateChanged` (and optimistically on user navigation) so
    /// a crash can be recovered from without needing to ask the (now dead)
    /// engine-host for its own state.
    current_url: String,
    current_title: String,
    can_go_back: bool,
    can_go_forward: bool,
    loading: bool,
    /// True once a crash has been observed and a same-engine respawn is
    /// in flight, so a second crash report (or an unrelated stray message)
    /// during that window doesn't trigger a second concurrent respawn.
    recovering: bool,

    egui_ctx: egui::Context,
    egui_state: Option<egui_winit::State>,
    address_bar_text: String,
    address_bar_focused: bool,
}

impl Default for App {
    fn default() -> Self {
        Self {
            window: None,
            gpu: None,
            tab: None,
            cursor_pos: (0.0, 0.0),
            swap_target: None,
            current_url: INITIAL_URL.to_string(),
            current_title: String::new(),
            can_go_back: false,
            can_go_forward: false,
            loading: false,
            recovering: false,
            egui_ctx: egui::Context::default(),
            egui_state: None,
            address_bar_text: INITIAL_URL.to_string(),
            address_bar_focused: false,
        }
    }
}

impl App {
    fn chrome_height_px(&self) -> u32 {
        let ppp = self.window.as_ref().map_or(1.0, |w| w.scale_factor()) as f32;
        (CHROME_HEIGHT_POINTS * ppp).round() as u32
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

    fn send_resize(&mut self) {
        let (width, height) = self.content_size_px();
        let dpi_scale = self.window.as_ref().map_or(1.0, |w| w.scale_factor());
        if let Some(tab) = &mut self.tab {
            tab.send(&HostToEngine::Resize {
                width,
                height,
                dpi_scale,
            });
        }
    }

    fn navigate_to(&mut self, url: String) {
        self.current_url = url.clone();
        if let Some(tab) = &mut self.tab {
            tab.send(&HostToEngine::Navigate { url, referrer: None });
        }
    }

    /// M2's swap sequence, now driven by the engine-picker dropdown instead
    /// of a fixed cycle: `GetState` -> spawn new engine -> replay (url, via
    /// the new engine's initial_url) -> new engine's first `FrameReady`
    /// swaps the displayed texture automatically (`RedrawRequested` always
    /// renders whatever `self.tab` currently is) -> old engine-host is
    /// killed by `EngineTab`'s `Drop` when `self.tab` is replaced.
    fn start_swap_to(&mut self, target: EngineKind) {
        let Some(tab) = &mut self.tab else { return };
        if tab.kind == target || self.swap_target.is_some() {
            return; // no-op swap, or one swap at a time
        }
        self.swap_target = Some(target);
        tab.send(&HostToEngine::GetState);
    }

    fn finish_swap(&mut self, target: EngineKind, url: String) {
        // Must drop (and thus Shutdown + kill) the old tab before spawning
        // the new one: both use the same tab_id, so the new engine-host's
        // named-pipe listeners would collide with the old ones otherwise.
        drop(self.tab.take());
        self.respawn(target, url);
    }

    /// Shared by both a user-triggered swap (`finish_swap`) and crash
    /// recovery: spawn `kind` at `url` sized to the current content area,
    /// and reset the per-navigation UI state — a fresh engine-host has no
    /// history yet, so stale can-go-back/forward/title from the dead one
    /// would be misleading.
    fn respawn(&mut self, kind: EngineKind, url: String) {
        self.current_url = url.clone();
        self.current_title.clear();
        self.can_go_back = false;
        self.can_go_forward = false;
        self.loading = false;

        match process_manager::spawn_tab(kind, TAB_ID, &url) {
            Ok(new_tab) => {
                self.tab = Some(new_tab);
                self.send_resize();
            }
            Err(err) => {
                eprintln!("spawn {kind:?} failed: {err}");
            }
        }
    }

    /// Actions clicked inside the egui closure (built with only `&mut Ui`,
    /// not `&mut self`) are collected here and applied afterward, once the
    /// closure — and its borrow of `self` fields it merely reads — has
    /// ended.
    fn build_ui(&mut self, ui: &mut egui::Ui) {
        if !self.address_bar_focused {
            self.address_bar_text = self.current_url.clone();
        }
        let current_kind = self.tab.as_ref().map(|t| t.kind);

        let mut go_back = false;
        let mut go_forward = false;
        let mut swap_to = None;
        let mut navigate = None;

        egui::Panel::top("tab_strip")
            .exact_size(TAB_STRIP_HEIGHT_POINTS)
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    let label = if self.current_title.is_empty() {
                        self.current_url.as_str()
                    } else {
                        self.current_title.as_str()
                    };
                    // Only one tab exists until M5 — this is scaffolding
                    // for the real tab strip, not a functional multi-tab
                    // control yet.
                    let text = if self.loading {
                        format!("\u{27f3} {label}")
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
                    if ui
                        .add_enabled(self.can_go_back, egui::Button::new("\u{25c0}"))
                        .clicked()
                    {
                        go_back = true;
                    }
                    if ui
                        .add_enabled(self.can_go_forward, egui::Button::new("\u{25b6}"))
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

        if go_back {
            if let Some(tab) = &mut self.tab {
                tab.send(&HostToEngine::GoBack);
            }
        }
        if go_forward {
            if let Some(tab) = &mut self.tab {
                tab.send(&HostToEngine::GoForward);
            }
        }
        if let Some(kind) = swap_to {
            self.start_swap_to(kind);
        }
        if let Some(url) = navigate {
            self.navigate_to(url);
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
        self.tab = Some(
            process_manager::spawn_tab(STARTING_ENGINE, TAB_ID, INITIAL_URL)
                .expect("spawn engine-host"),
        );
        self.window = Some(window);
        self.send_resize();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let consumed_by_egui = match (&mut self.egui_state, &self.window) {
            (Some(state), Some(window)) => state.on_window_event(window.as_ref(), &event).consumed,
            _ => false,
        };

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize(size.width, size.height);
                }
                self.send_resize();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_pos = (position.x, position.y);
                let chrome_height = self.chrome_height_px() as f64;
                if !consumed_by_egui && position.y >= chrome_height {
                    if let Some(tab) = &mut self.tab {
                        tab.send(&HostToEngine::Input(InputEvent::MouseMove {
                            x: position.x,
                            y: position.y - chrome_height,
                        }));
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let chrome_height = self.chrome_height_px() as f64;
                let (x, y) = self.cursor_pos;
                if !consumed_by_egui && y >= chrome_height {
                    if let Some(button) = translate_mouse_button(button) {
                        if let Some(tab) = &mut self.tab {
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
                            tab.send(&HostToEngine::Input(event));
                        }
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let chrome_height = self.chrome_height_px() as f64;
                let (x, y) = self.cursor_pos;
                if !consumed_by_egui && y >= chrome_height {
                    if let Some(tab) = &mut self.tab {
                        let (dx, dy) = match delta {
                            winit::event::MouseScrollDelta::LineDelta(x, y) => {
                                (x as f64 * 20.0, y as f64 * 20.0)
                            }
                            winit::event::MouseScrollDelta::PixelDelta(p) => (p.x, p.y),
                        };
                        tab.send(&HostToEngine::Input(InputEvent::Scroll {
                            x,
                            y: y - chrome_height,
                            delta_x: dx,
                            delta_y: dy,
                        }));
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                // Drain to the most recent FrameReady (older ones are stale
                // by the time we get to render), watch for the
                // StateSnapshot reply to an in-flight swap, fold in
                // navigation-state updates, and notice a crash. Scoped so
                // the borrow of `self.tab` ends before any `&mut self` call
                // below (finish_swap/respawn need one).
                let mut latest = None;
                let mut swap_state = None;
                let mut crashed = None;
                let mut tab_id_for_frame = None;
                let mut tab_kind_for_frame = None;
                let mut nav_update = None;
                if let Some(tab) = &self.tab {
                    while let Ok(msg) = tab.events_rx.try_recv() {
                        match msg {
                            EngineToHost::FrameReady {
                                width,
                                height,
                                pixels,
                                ..
                            } => {
                                latest = Some((width, height, pixels));
                                tab_id_for_frame = Some(tab.tab_id.clone());
                                tab_kind_for_frame = Some(tab.kind);
                            }
                            EngineToHost::StateSnapshot(state) => {
                                swap_state = Some(state);
                            }
                            EngineToHost::NavigationStateChanged {
                                url,
                                title,
                                can_go_back,
                                can_go_forward,
                                loading,
                            } => {
                                nav_update = Some((url, title, can_go_back, can_go_forward, loading));
                            }
                            EngineToHost::Crashed { reason } => {
                                crashed = Some(reason);
                            }
                            _ => {}
                        }
                    }
                }

                if let Some((url, title, can_go_back, can_go_forward, loading)) = nav_update {
                    self.current_url = url;
                    self.current_title = title;
                    self.can_go_back = can_go_back;
                    self.can_go_forward = can_go_forward;
                    self.loading = loading;
                }

                if let Some(reason) = crashed {
                    // A crash always wins over an in-flight swap: there is
                    // no StateSnapshot coming from a dead engine-host, so
                    // any pending swap_target would otherwise wait forever.
                    self.swap_target = None;
                    if !self.recovering {
                        self.recovering = true;
                        eprintln!("engine-host crashed ({reason}), restarting at {}", self.current_url);
                        let kind = self.tab.as_ref().map(|t| t.kind).unwrap_or(STARTING_ENGINE);
                        let url = self.current_url.clone();
                        drop(self.tab.take());
                        self.respawn(kind, url);
                        self.recovering = false;
                    }
                } else if swap_state.is_some() && self.swap_target.is_some() {
                    // .take() only now that we know both are Some — doing
                    // it unconditionally inside the tuple pattern below
                    // would clear swap_target as a side effect even on a
                    // frame where no StateSnapshot arrived yet, discarding
                    // the in-flight swap before its reply ever showed up.
                    let target = self.swap_target.take().unwrap();
                    let state = swap_state.unwrap();
                    self.finish_swap(target, state.url);
                } else if let (Some((width, height, pixels)), Some(tab_id), Some(kind)) =
                    (latest, tab_id_for_frame, tab_kind_for_frame)
                {
                    let format = match kind {
                        EngineKind::Servo | EngineKind::Webkit => wgpu::TextureFormat::Rgba8UnormSrgb,
                        EngineKind::Cef => wgpu::TextureFormat::Bgra8UnormSrgb,
                    };
                    if let Some(pixels) = pixels {
                        // RemoteTransport engine-host (WebKit over WSL2): no
                        // shared memory with this process, so the pixels
                        // travel inline in the message itself.
                        if let Some(gpu) = &mut self.gpu {
                            gpu.upload_frame(&pixels, width, height, format);
                        }
                    } else if let Ok(buf) =
                        SharedFrameBuffer::open_existing(frame_path(&tab_id), width, height)
                    {
                        // The engine-host truncates-then-grows this file
                        // when (re)creating it (see SharedFrameBuffer::
                        // create), which briefly exposes a 0-byte or
                        // otherwise wrong-sized file to a concurrent
                        // reader. Skip this frame rather than panic on a
                        // mismatched slice — the next redraw tries again.
                        let expected_len = (width as usize) * (height as usize) * 4;
                        if buf.as_slice().len() == expected_len {
                            if let Some(gpu) = &mut self.gpu {
                                gpu.upload_frame(buf.as_slice(), width, height, format);
                            }
                        }
                    }
                }

                // Take egui_state/window out (rather than borrow) so the
                // closure below is free to reborrow `self` mutably for
                // `build_ui` — a borrow of `self.egui_state`/`self.window`
                // held across that call would conflict with it, since a
                // method call on `self` is a borrow of the whole struct as
                // far as the borrow checker is concerned.
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
                if let Some(gpu) = &mut self.gpu {
                    match egui_output {
                        Some(output) => gpu.render(chrome_height, Some((&self.egui_ctx, output))),
                        None => gpu.render(chrome_height, None),
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

fn translate_mouse_button(button: WinitMouseButton) -> Option<MouseButton> {
    match button {
        WinitMouseButton::Left => Some(MouseButton::Left),
        WinitMouseButton::Right => Some(MouseButton::Right),
        WinitMouseButton::Middle => Some(MouseButton::Middle),
        _ => None,
    }
}

fn main() {
    let event_loop = EventLoop::new().expect("create event loop");
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::default();
    event_loop.run_app(&mut app).expect("run event loop");
}
