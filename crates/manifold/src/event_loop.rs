//! The winit event loop: the ApplicationHandler impl, each frame's egui
//! pass and render, window-event dispatch, the headless debug commands and
//! the engine processes' messages.

use super::*;
use crate::debug_control::debug_key_event;
use crate::tabs::{AfterCrash, Held, LeftClosed};
use winit::window::Icon;

/// The event loop's real user-event type: headless debug commands, plus
/// `TabSpawned` — see its own doc comment for why view-host spawning needs
/// to be async in the first place.
pub(crate) enum UserEvent {
    Debug(DebugCommand),
    TabSpawned(Box<TabSpawnOutcome>),
    /// A screen reader connected, acted on a node, or left (`egui_winit::accesskit_winit`).
    Accessibility(egui_winit::accesskit_winit::Event),
    /// An engine reader handed over a message (`view_wake`): apply it now. Posted at most once until handled.
    ViewMessage,
}

impl From<egui_winit::accesskit_winit::Event> for UserEvent {
    fn from(event: egui_winit::accesskit_winit::Event) -> Self {
        UserEvent::Accessibility(event)
    }
}

/// The event loop's frame pacing (`App::pacing`): minimize, idle detection and the wait between idle frames.
pub(crate) struct FramePacing {
    /// Whether the last `about_to_wait` found the window minimized: the
    /// loop then ticks at `MINIMIZED_TICK` instead of redrawing, and on the
    /// way back resumes redrawing.
    pub(crate) was_minimized: bool,
    /// The last input, engine message burst or egui repaint request: after
    /// `IDLE_AFTER` without one the loop slows to `IDLE_FRAME` (see
    /// `about_to_wait`).
    pub(crate) last_activity: std::time::Instant,
    /// Engine frames seen in the current second (a burst is activity).
    pub(crate) frame_burst: (std::time::Instant, u32),
    /// When the last frame finished; with a present mode that does not block, the active loop draws at most once per
    /// `frame_interval` after it.
    pub(crate) last_present: std::time::Instant,
    /// The window's monitor refresh interval, read at most once a second.
    pub(crate) frame_interval: (std::time::Instant, std::time::Duration),
    /// When an idle loop next draws.
    pub(crate) next_idle_frame: std::time::Instant,
    /// The wait between idle frames: `IDLE_FRAME`, or less when egui asked
    /// for a repaint sooner (`request_repaint_after`).
    pub(crate) idle_interval: std::time::Duration,
    /// Tabs `SetVisible(false)` was sent to on minimize, to be shown again
    /// on restore (`hide_tabs_for_minimize`).
    pub(crate) hidden_for_minimize: Vec<String>,
    /// Set by a reader when it posts `UserEvent::ViewMessage`, cleared when the loop handles it, so a burst of
    /// messages posts one event (`view_wake`).
    pub(crate) view_wake: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Default for FramePacing {
    fn default() -> Self {
        Self {
            was_minimized: false,
            last_activity: std::time::Instant::now(),
            frame_burst: (std::time::Instant::now(), 0),
            last_present: std::time::Instant::now(),
            frame_interval: (std::time::Instant::now(), std::time::Duration::from_micros(16_667)),
            next_idle_frame: std::time::Instant::now(),
            idle_interval: IDLE_FRAME,
            hidden_for_minimize: Vec::new(),
            view_wake: std::sync::Arc::default(),
        }
    }
}

/// Whether `event` counts as activity that ends idle pacing (`App::mark_active`). An engine wake does not: the drain
/// it runs marks the loop active only when it applies a message, as a tick's drain does.
fn is_activity(event: &UserEvent) -> bool {
    !matches!(event, UserEvent::ViewMessage)
}

/// The `Wake` a tab's reader calls (`process_manager::spawn_tab`): posts `UserEvent::ViewMessage` unless one is
/// already pending (`FramePacing::view_wake`).
pub(crate) fn view_wake(
    pending: std::sync::Arc<std::sync::atomic::AtomicBool>,
    proxy: EventLoopProxy<UserEvent>,
) -> crate::process_manager::view_io::Wake {
    // In a Mutex: winit declares its proxy `Send`, not `Sync`.
    let proxy = std::sync::Mutex::new(proxy);
    coalesced_wake(pending, move || {
        let proxy = proxy.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        proxy.send_event(UserEvent::ViewMessage).is_ok()
    })
}

/// `post` (true if it queued the event), unless `pending` says a post is already waiting to be handled.
fn coalesced_wake(
    pending: std::sync::Arc<std::sync::atomic::AtomicBool>,
    post: impl Fn() -> bool + Send + Sync + 'static,
) -> crate::process_manager::view_io::Wake {
    std::sync::Arc::new(move || {
        // A post that was not queued (the loop is gone, or the OS refused) clears the flag again: left set, it would mute
        // every later wake.
        if !pending.swap(true, std::sync::atomic::Ordering::SeqCst) && !post() {
            pending.store(false, std::sync::atomic::Ordering::SeqCst);
        }
    })
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let mut attrs = Window::default_attributes().with_title("manifold").with_window_icon(load_app_icon());
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
            // `with_active(false)`: shown with `SW_SHOWNOACTIVATE` rather than winit's
            // default `SW_SHOW`, which activated it and, whenever Windows allowed it, took
            // the OS foreground from the user's own app (scripts/foreground-watch.ps1).
            attrs = attrs
                .with_position(winit::dpi::PhysicalPosition::new(-32000, -32000))
                .with_decorations(false)
                .with_active(false);
            // A Windows-only window attribute; elsewhere the headless window shows in the task list.
            #[cfg(windows)]
            {
                attrs = attrs.with_skip_taskbar(true);
            }
        } else {
            // Restore the window to where the user last left it -- see
            // `Settings::window_width`'s own doc comment for why this is
            // `LogicalSize`, not `PhysicalSize`: winit/Windows converts a
            // logical size to whatever physical size is correct for the
            // monitor the window actually lands on, rather than reapplying
            // a stale physical pixel count that Per-Monitor-V2 would then
            // rescale (and this app would then re-save, compounding it
            // further on every launch). `None` (no saved geometry yet)
            // just leaves `attrs` untouched, falling back to winit's own
            // default the same as before this existed.
            if let (Some(width), Some(height)) = (self.settings.window_width, self.settings.window_height) {
                attrs = attrs.with_inner_size(winit::dpi::LogicalSize::new(width as f64, height as f64));
            }
            if let (Some(x), Some(y)) = (self.settings.window_x, self.settings.window_y) {
                attrs = attrs.with_position(winit::dpi::PhysicalPosition::new(x, y));
            }
        }
        // The accesskit adapter must exist before the window is shown, so the real window is
        // created hidden and shown once it is wired up (the headless one stays as it is: no
        // screen reader there).
        let real_window = !self.headless;
        let window = Arc::new(event_loop.create_window(attrs.with_visible(!real_window)).expect("create window"));
        apply_dark_titlebar(&window);
        self.gpu = Some(GpuState::new(window.clone()));
        let mut egui_state = egui_winit::State::new(
            self.egui_ctx.clone(),
            egui::ViewportId::ROOT,
            window.as_ref(),
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        if real_window {
            if let Some(proxy) = self.event_proxy.clone() {
                egui_state.init_accesskit(event_loop, window.as_ref(), proxy);
            }
            window.set_visible(true);
        }
        self.egui_state = Some(egui_state);
        self.window = Some(window);
        self.open_store();
        let restored = self.restore_session();
        if self.ensure_default_jar() {
            self.save_session();
        }
        if !restored {
            if self.settings.has_shown_first_launch_demo || !self.demo_available() {
                self.spawn_root_tab();
            } else {
                self.spawn_first_launch_demo();
                self.settings.has_shown_first_launch_demo = true;
                self.save_settings();
            }
        }
        self.prune_backing_dirs();
    }

    /// A clean exit (a window close, or headless `QUIT`): a window geometry
    /// change still waiting (`flush_pending_settings`), the tabs' last
    /// cookies (`final_cookie_pull`, at most 1 s), the store's last
    /// save and close, then everything tied to the window, while the event
    /// loop still exists. `run_app` drops the event loop (on Wayland, its
    /// display connection) before `main` drops `App`, and egui-winit's
    /// clipboard worker releases its Wayland objects on that connection when
    /// dropped: dropped after it, it crashed with SIGSEGV on every quit under
    /// WSLg. Children before the window: egui's state and the surface hold
    /// handles to it.
    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.flush_pending_settings();
        self.final_cookie_pull();
        self.begin_view_shutdown();
        self.flush_dirty_jars(None); // the last pull's changes, and any round still open
        self.close_store();
        self.egui_state = None;
        self.gpu = None;
        self.window = None;
    }

    /// A minimized window shows nothing, so instead of redrawing every
    /// frame (`ControlFlow::Poll`) the loop sleeps and ticks every
    /// `MINIMIZED_TICK`: just the bookkeeping that doesn't need a frame
    /// (engine messages, store, idle sleep), no egui pass and no present.
    /// Measured 2026-10-01: a minimized window still cost ~16% of a core.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let minimized = self.is_minimized();
        if minimized {
            event_loop.set_control_flow(ControlFlow::WaitUntil(std::time::Instant::now() + MINIMIZED_TICK));
            if !self.pacing.was_minimized {
                self.hide_tabs_for_minimize();
            }
            self.minimized_tick();
        } else if self.pacing.was_minimized {
            event_loop.set_control_flow(ControlFlow::Poll);
            for id in std::mem::take(&mut self.pacing.hidden_for_minimize) {
                if let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
                    node.tab.set_visible(true);
                }
            }
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        } else if self.is_idle() {
            // Nothing changed for a while: draw at `IDLE_FRAME` (or sooner when egui asks) instead of
            // every vsync (a frame costs ~4 ms of CPU in the present wait
            // even when nothing moved). Any event wakes the loop at once.
            let now = std::time::Instant::now();
            if now >= self.pacing.next_idle_frame {
                self.pacing.next_idle_frame = now + self.pacing.idle_interval;
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(self.pacing.next_idle_frame));
        } else if !self.headless {
            let due = if self.present_blocks() { None } else { Some(self.pacing.last_present + self.frame_interval()) };
            match due.filter(|d| std::time::Instant::now() < *d) {
                Some(due) => event_loop.set_control_flow(ControlFlow::WaitUntil(due)),
                None => {
                    event_loop.set_control_flow(ControlFlow::Poll);
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
            }
        }
        self.pacing.was_minimized = minimized;
        self.release_expired_cookie_holds();
        if self.cookies.jar_last_pull.is_none_or(|t| t.elapsed() >= JAR_PULL_EVERY) {
            self.jar_pull_all();
            self.cookies.jar_last_pull = Some(std::time::Instant::now());
        }
        // A pull round's changed jars, once its replies are in or `JAR_FLUSH_AFTER` has passed. The loop wakes at
        // least every `IDLE_FRAME` or `MINIMIZED_TICK` (100 ms), so a due jar is written about 100 ms later at most
        // while the loop turns (a modal file dialog or a long frame holds it).
        self.flush_dirty_jars(Some(std::time::Instant::now()));
        // A window drag's geometry, once it has been quiet for `SETTINGS_SAVE_DELAY`. The loop wakes at least
        // every idle frame or `MINIMIZED_TICK`, so it lands within about a second.
        self.flush_settings_if_due(std::time::Instant::now());
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        if is_activity(&event) {
            self.mark_active();
        }
        match event {
            UserEvent::ViewMessage => self.on_view_wake(),
            UserEvent::Accessibility(event) => {
                use egui_winit::accesskit_winit::WindowEvent as A11y;
                match event.window_event {
                    A11y::InitialTreeRequested => {
                        self.egui_ctx.enable_accesskit();
                        self.egui_ctx.request_repaint();
                    }
                    A11y::ActionRequested(request) => {
                        if let Some(state) = self.egui_state.as_mut() {
                            state.on_accesskit_action_request(request);
                        }
                        self.egui_ctx.request_repaint();
                    }
                    A11y::AccessibilityDeactivated => self.egui_ctx.disable_accesskit(),
                }
            }
            UserEvent::Debug(cmd) => {
                // egui hit-tests pointer input against the widgets of the
                // last frame, which may predate whatever changed since the
                // previous command (a tab that just spawned, suggestions
                // that just arrived); a CLICK on a node right after
                // WAITTABS missed 3 runs out of 3. So draw the current
                // state first.
                let pointer = matches!(
                    cmd,
                    DebugCommand::MouseMove { .. }
                        | DebugCommand::MouseDown { .. }
                        | DebugCommand::MouseUp { .. }
                        | DebugCommand::Click { .. }
                        | DebugCommand::DoubleClick { .. }
                        | DebugCommand::Scroll { .. }
                );
                if self.headless && pointer {
                    self.dispatch_window_event(event_loop, WindowEvent::RedrawRequested);
                }
                self.handle_debug_command(event_loop, cmd);
                // The off-screen headless window can get `RedrawRequested`
                // (WM_PAINT) only sporadically -- measured ~40 frames in
                // 20 s under load (another process on the GPU, engines
                // starting), ~60 fps on a quiet machine -- so several commands could
                // land in one frame: a `MOUSEDOWN` and the `MOUSEMOVE`s after
                // it were hit-tested at the pointer's final position, turning
                // a card drag into a background pan. Rendering a frame right
                // here gives every command its own frame.
                if self.headless {
                    self.dispatch_window_event(event_loop, WindowEvent::RedrawRequested);
                }
            }
            UserEvent::TabSpawned(outcome) => self.handle_tab_spawn_result(*outcome),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let consumed_by_egui = match (&mut self.egui_state, &self.window) {
            (Some(state), Some(window)) => state.on_window_event(window.as_ref(), &event).consumed,
            _ => false,
        };
        let in_tab_view = self.showing_tab_view();
        if !matches!(event, WindowEvent::RedrawRequested) {
            self.mark_active();
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Focused(focused) => self.on_window_focus(focused),
            // Fires when the window moves to a monitor with a different
            // scale factor (also on a live resolution/scaling change).
            // Confirmed live this was a real gap, not just theoretical:
            // instrumenting a live cross-monitor drag showed a transitional
            // `send_resize_to` call computed with the size already updated
            // for the new monitor but `window.scale_factor()` still
            // reporting the *old* monitor's value -- a genuine race between
            // winit's size and scale-factor updates that `Resized` alone
            // doesn't protect against. This handler re-triggers the same
            // tab-resize notification `Resized` does, so whichever event
            // actually lands last with the fully-settled state (typically
            // this one, since it specifically announces the scale change
            // completed) corrects any bad intermediate value the other
            // sent. `window.inner_size()` (not the stale pre-change size)
            // is used for the GPU surface for the same reason.
            WindowEvent::ScaleFactorChanged { .. } => {
                if let Some(gpu) = &mut self.gpu {
                    if let Some(size) = self.window.as_ref().map(|w| w.inner_size()) {
                        gpu.resize(size.width, size.height);
                    }
                }
                if let Some(id) = self.focused.clone() {
                    self.send_resize_to(&id);
                }
                if let Some(id) = self.split_tab.clone() {
                    self.send_resize_to(&id);
                }
            }
            WindowEvent::Resized(size) => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize(size.width, size.height);
                }
                // A minimized window reports 0x0; resizing a page to that
                // crashed the WebView2 engine ("Tab crashed, restarted" on
                // restore). The restore's own `Resized` sends the real size.
                if size.width > 0 && size.height > 0 {
                    if let Some(id) = self.focused.clone() {
                        self.send_resize_to(&id);
                    }
                    if let Some(id) = self.split_tab.clone() {
                        self.send_resize_to(&id);
                    }
                }
                // Not saved in headless mode -- see `Settings::window_width`'s
                // own doc comment for why that would corrupt a real saved
                // geometry with the deliberate off-screen placeholder size.
                // Converted to logical points here (dividing out `ppp`) --
                // see that same doc comment for why saving raw physical
                // pixels compounded into runaway window growth across
                // launches on a mixed-DPI multi-monitor setup.
                if !self.headless && size.width > 0 && size.height > 0 {
                    let ppp = self.ppp();
                    self.settings.window_width = Some((size.width as f64 / ppp).round() as u32);
                    self.settings.window_height = Some((size.height as f64 / ppp).round() as u32);
                    self.note_settings_changed();
                }
            }
            WindowEvent::Moved(position) => {
                if !self.headless {
                    self.settings.window_x = Some(position.x);
                    self.settings.window_y = Some(position.y);
                    self.note_settings_changed();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_pos = (position.x, position.y);
                let chrome_height = self.chrome_height_px() as f64;
                let in_primary_pane = self.split_divider_x_px().is_none_or(|divider| position.x < divider);
                if in_tab_view && !consumed_by_egui && position.y >= chrome_height && in_primary_pane {
                    if let Some(id) = self.focused.clone() {
                        if let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
                            node.tab.send(&HostToView::Input(InputEvent::MouseMove {
                                x: position.x,
                                y: position.y - chrome_height,
                                modifiers: self.key_modifiers,
                            }));
                        }
                    }
                }
            }
            // The page never receives the drop (spec "Drag and drop").
            // winit 0.30 sends no `CursorMoved` during an OLE drag, so the
            // drop point is read from the OS (`cursor_point`), falling back
            // to the last real `CursorMoved` off Windows or if that read
            // fails. `render_frame` saves once per batch (`drop_batch`).
            WindowEvent::DroppedFile(path) => {
                let point = self.cursor_point().unwrap_or_else(|| self.last_pointer_point());
                self.drop_file(path, point);
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let chrome_height = self.chrome_height_px() as f64;
                let (x, y) = self.cursor_pos;
                // `egui_winit` reports the right button as "consumed"
                // unconditionally (confirmed live via temporary logging:
                // `consumed_by_egui=true` even over this project's own raw
                // content quad, which isn't a real egui widget and has
                // nothing there to legitimately claim it) — bypassed for
                // right and middle specifically so a right/middle-click on
                // tab content still reaches `InputEvent::MouseDown`/
                // `MouseUp` the same way every left-click already does.
                // Middle was missing this even though every view-host's
                // own `translate_mouse_button`/native input path already
                // fully implements `MouseButton::Middle`.
                let egui_wants_it =
                    consumed_by_egui && button != WinitMouseButton::Right && button != WinitMouseButton::Middle;
                let in_primary_pane = self.split_divider_x_px().is_none_or(|divider| x < divider);
                let in_content = in_tab_view && !egui_wants_it && y >= chrome_height && in_primary_pane;
                if state == ElementState::Pressed && in_tab_view && !consumed_by_egui {
                    self.side_button_nav(button);
                }
                if let Some(button) = translate_mouse_button(button) {
                    // A release always goes to whichever tab is mid-drag,
                    // even when it lands outside the content area -- see
                    // `native_drag_tab`'s doc comment for the stuck-button
                    // bug that gating it the same way as a press caused.
                    let target = match state {
                        ElementState::Pressed if in_content => self.focused.clone(),
                        ElementState::Released => {
                            if matches!(button, MouseButton::Left) { self.native_drag_tab.take() } else { None }
                                .or_else(|| in_content.then(|| self.focused.clone()).flatten())
                        }
                        ElementState::Pressed => None,
                    };
                    if state == ElementState::Pressed && target.is_none() {
                        self.click_counter.reset();
                    }
                    if let Some(id) = target {
                        if let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
                            let (x, y) = (x, y - chrome_height);
                            let event = match state {
                                ElementState::Pressed => {
                                    let click_count =
                                        self.click_counter.press(&id, button, (x, y), std::time::Instant::now());
                                    InputEvent::MouseDown { x, y, button, modifiers: self.key_modifiers, click_count }
                                }
                                ElementState::Released => InputEvent::MouseUp {
                                    x,
                                    y,
                                    button,
                                    modifiers: self.key_modifiers,
                                    click_count: self.click_counter.current(),
                                },
                            };
                            node.tab.send(&HostToView::Input(event));
                            if state == ElementState::Pressed && matches!(button, MouseButton::Left) {
                                self.native_drag_tab = Some(id);
                            }
                        }
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let chrome_height = self.chrome_height_px() as f64;
                let (x, y) = self.cursor_pos;
                let in_primary_pane = self.split_divider_x_px().is_none_or(|divider| x < divider);
                if in_tab_view && !consumed_by_egui && y >= chrome_height && in_primary_pane {
                    if let Some(id) = self.focused.clone() {
                        if let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
                            // `LineDelta` is a fixed synthetic unit
                            // (`* 20.0`), not a measurement of anything;
                            // `PixelDelta` is already a real physical-pixel
                            // amount, which is the space every engine's
                            // surface (and so this event's `x`/`y`) is in --
                            // neither needs converting.
                            let (dx, dy) = match delta {
                                winit::event::MouseScrollDelta::LineDelta(x, y) => (x as f64 * 20.0, y as f64 * 20.0),
                                winit::event::MouseScrollDelta::PixelDelta(p) => (p.x, p.y),
                            };
                            node.tab.send(&HostToView::Input(InputEvent::Scroll {
                                x,
                                y: y - chrome_height,
                                delta_x: dx,
                                delta_y: dy,
                                modifiers: self.key_modifiers,
                            }));
                        }
                    }
                }
            }
            WindowEvent::ModifiersChanged(new_modifiers) => {
                self.key_modifiers = winit_modifiers_to_bits(new_modifiers.state());
            }
            WindowEvent::KeyboardInput { event, .. } => {
                // A key we take as a shortcut isn't also the page's: its press, text and release stay here.
                let swallowed = match (event.physical_key, event.state) {
                    (PhysicalKey::Code(code), ElementState::Pressed)
                        if crate::shortcuts::claims_chord(code, self.key_modifiers) =>
                    {
                        self.swallowed_keys.insert(code);
                        true
                    }
                    (PhysicalKey::Code(code), ElementState::Released) => self.swallowed_keys.remove(&code),
                    _ => false,
                };
                if in_tab_view && !consumed_by_egui && !swallowed {
                    if let Some(id) = self.focused.clone() {
                        if let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
                            if let Some(key) = translate_key_code(event.physical_key) {
                                let input = match event.state {
                                    ElementState::Pressed => InputEvent::KeyDown { key, modifiers: self.key_modifiers },
                                    ElementState::Released => InputEvent::KeyUp { key, modifiers: self.key_modifiers },
                                };
                                node.tab.send(&HostToView::Input(input));
                            }
                            // The produced text (after layout/shift/dead-key
                            // processing) is separate from the physical key
                            // above — a page's text field wants this, a
                            // shortcut wants the key. winit only reports
                            // `text` on the press, not the release.
                            if event.state == ElementState::Pressed {
                                if let Some(text) = &event.text {
                                    for ch in text.chars() {
                                        node.tab.send(&HostToView::Input(InputEvent::Char { codepoint: ch as u32 }));
                                    }
                                }
                            }
                        }
                    }
                }
            }
            // No frame while minimized, and no `request_redraw`, so the loop
            // stops spinning; `about_to_wait` ticks it instead.
            WindowEvent::RedrawRequested if self.is_minimized() => {}
            WindowEvent::RedrawRequested => {
                self.poll_store();
                self.poll_trail_focus();
                self.poll_view_installs();
                let focused_tab = self
                    .focused
                    .as_deref()
                    .and_then(|id| self.nodes.get(id))
                    .and_then(|n| n.as_tab())
                    .map(|t| format!("{:?} {}", t.tab.kind, crash_report::redacted_url(&t.url)));
                crash_report::record_state(format!(
                    "view_mode={:?} headless={} node_count={} focused_tab={:?}",
                    self.view_mode,
                    self.headless,
                    self.nodes.len(),
                    focused_tab
                ));
                self.drain_tab_events();
                // Checked before building this frame's UI so that, the
                // instant a `Transition` completes, this same frame already
                // renders the real destination view instead of an extra
                // stale transitioning frame.
                self.finish_transition_if_done();

                let egui_output = self.run_egui_frame();
                // egui asks for the next frame itself while something it
                // draws is animating.
                let repaint_in =
                    egui_output.as_ref().and_then(|o| o.viewport_output.values().map(|v| v.repaint_delay).min());
                if repaint_in.is_some_and(|d| d.is_zero()) {
                    self.mark_active();
                }
                self.pacing.idle_interval = repaint_in.map_or(IDLE_FRAME, |d| d.min(IDLE_FRAME));

                self.render_frame(egui_output);
                self.pacing.last_present = std::time::Instant::now();
                if !self.is_idle() && self.present_blocks() {
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }

                self.headless_end_of_frame(event_loop);
            }
            _ => {}
        }
    }
}

/// A monitor refresh rate in millihertz as a frame interval; 60 Hz when the rate is unknown or zero.
fn interval_from_millihertz(mhz: Option<u32>) -> std::time::Duration {
    mhz.filter(|m| *m > 0).map_or(std::time::Duration::from_micros(16_667), |m| {
        std::time::Duration::from_micros(1_000_000_000 / u64::from(m))
    })
}

/// Without activity for this long the loop slows down (see `about_to_wait`).
const IDLE_AFTER: std::time::Duration = std::time::Duration::from_secs(1);
/// An idle loop's frame interval.
pub(crate) const IDLE_FRAME: std::time::Duration = std::time::Duration::from_millis(100);
/// Engine messages one drain applies per tab; the rest wait for the next tick (survey E3). An applied message marks
/// the loop active, so that tick comes at the active rate.
pub(crate) const VIEW_MESSAGES_PER_DRAIN: usize = 64;
/// Engine frames per second that count as activity (video, an animation);
/// an idle page still sends a few.
const BUSY_FRAMES: u32 = 6;

/// How often every WebView2 and CEF tab is asked for its cookies (`App::jar_pull_all`).
const JAR_PULL_EVERY: std::time::Duration = std::time::Duration::from_secs(30);

/// How often a minimized window's loop wakes (see `about_to_wait`).
const MINIMIZED_TICK: std::time::Duration = std::time::Duration::from_millis(100);

/// A tab's newest frame, which `App::drain_view_ipc` collects and `App::upload_frames` uploads.
struct FrameUpdate {
    tab_id: String,
    kind: ViewKind,
    /// The frame file the pixels are in (`FrameReady::generation`).
    generation: u32,
    width: u32,
    height: u32,
    /// The rows that changed since the last upload; `None` is the whole frame.
    rows: Option<std::ops::Range<u32>>,
    pixels: Option<Vec<u8>>,
}

impl App {
    fn mark_active(&mut self) {
        self.pacing.last_activity = std::time::Instant::now();
    }

    /// `UserEvent::ViewMessage`: clears the pending flag first (a message arriving during the drain posts again),
    /// then applies what arrived (`drain_tab_events`).
    fn on_view_wake(&mut self) {
        // SeqCst on both sides: a reader whose `swap` still saw `true` must have its message seen by this drain.
        self.pacing.view_wake.store(false, std::sync::atomic::Ordering::SeqCst);
        self.drain_tab_events();
        // A non-blocking present mode leaves the active loop waiting out its frame interval; a page's own frame
        // draws at once instead (the page's frame rate bounds how often).
        if !self.present_blocks() {
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
    }

    /// Counts engine frames; a burst of them is activity.
    fn note_view_frames(&mut self, n: u32) {
        let now = std::time::Instant::now();
        if n > 0 {
            // An idle loop draws at its next tick; a frame just uploaded should not wait for it.
            self.pacing.next_idle_frame = now;
        }
        if now.duration_since(self.pacing.frame_burst.0) >= std::time::Duration::from_secs(1) {
            self.pacing.frame_burst = (now, 0);
        }
        self.pacing.frame_burst.1 += n;
        if self.pacing.frame_burst.1 >= BUSY_FRAMES {
            self.mark_active();
        }
    }

    /// Whether the loop can run at `IDLE_FRAME`: a real window, nothing
    /// happened for `IDLE_AFTER`, and no view transition is under way.
    fn is_idle(&self) -> bool {
        !self.headless && self.transition.is_none() && self.pacing.last_activity.elapsed() >= IDLE_AFTER
    }

    /// Whether a present blocks (`Fifo`), so the loop needs no cap of its own; true before the GPU exists.
    fn present_blocks(&self) -> bool {
        self.gpu.as_ref().is_none_or(GpuState::present_blocks)
    }

    /// The monitor's refresh interval (60 Hz when unknown), re-read at most once a second.
    fn frame_interval(&mut self) -> std::time::Duration {
        if self.pacing.frame_interval.0.elapsed() >= std::time::Duration::from_secs(1) {
            let mhz = self.window.as_ref().and_then(|w| w.current_monitor()).and_then(|m| m.refresh_rate_millihertz());
            self.pacing.frame_interval = (std::time::Instant::now(), interval_from_millihertz(mhz));
        }
        self.pacing.frame_interval.1
    }

    /// Whether the real (non-headless) window is minimized.
    fn is_minimized(&self) -> bool {
        !self.headless && self.window.as_ref().is_some_and(|w| w.is_minimized() == Some(true))
    }

    /// Tells the tabs on screen (the focused tab in Tab view or mid-
    /// transition, and the split pane) to stop capturing while minimized;
    /// `about_to_wait` resumes them on restore. Measured 2026-10-01: the
    /// engine and WebView2 kept using ~15% of a core behind a minimized window.
    fn hide_tabs_for_minimize(&mut self) {
        let mut ids = Vec::new();
        if self.showing_tab_view() || self.transition.is_some() {
            ids.extend(self.focused.clone());
        }
        ids.extend(self.split_tab.clone());
        for id in ids {
            if let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
                node.tab.set_visible(false);
                self.pacing.hidden_for_minimize.push(id);
            }
        }
    }

    /// What a frame does before building its UI, minus the UI itself.
    fn minimized_tick(&mut self) {
        self.poll_store();
        self.poll_trail_focus();
        self.poll_view_installs();
        self.drain_tab_events();
        self.finish_transition_if_done();
    }

    /// `RedrawRequested`: builds this frame's UI (`build_ui`), with the
    /// raw input adjusted first. `None` without a window or egui state yet.
    pub(crate) fn run_egui_frame(&mut self) -> Option<egui::FullOutput> {
        match (self.egui_state.take(), self.window.clone()) {
            (Some(mut state), Some(window)) => {
                let mut raw_input = state.take_egui_input(window.as_ref());
                // `egui::Scene` (the graph view's pan/zoom container)
                // hardcodes ctrl+scroll as its zoom gesture and plain
                // scroll as pan -- not configurable via its builder,
                // since `register_pan_and_zoom` reads
                // `zoom_delta()`/`smooth_scroll_delta()` straight off
                // `InputState`. Forcing every `MouseWheel` event's
                // modifiers to ctrl+command here, before egui ever
                // computes those derived deltas, makes plain scroll
                // register as the zoom gesture instead -- the graph
                // already has a separate pan gesture (dragging empty
                // background), so losing scroll-as-pan costs nothing.
                // Tab view's own page-content scroll is untouched:
                // that's a distinct `InputEvent::Scroll` forwarded to
                // the focused tab's view-host, not egui's Scene.
                // `egui` treats "is this scroll a zoom gesture" as a
                // single frame-wide flag (`InputState::begin_pass`
                // computes it once from the whole event list, not
                // per-widget or per-hover-region) -- forcing it
                // unconditionally would silently zero out
                // `smooth_scroll_delta` for *any* `ScrollArea` shown
                // over Graph view, not just `Scene`. Scoped to
                // `ViewMode::Graph` specifically, so the Settings
                // page's own `ScrollArea` (a separate `ViewMode` now)
                // always keeps normal scroll.
                if self.view_mode == ViewMode::Graph {
                    for event in &mut raw_input.events {
                        if let egui::Event::MouseWheel { modifiers, .. } = event {
                            modifiers.ctrl = true;
                            modifiers.command = true;
                        }
                    }
                }
                // The hidden headless window never has OS focus, and egui
                // reports no widget focus (`Response::has_focus`) in an
                // unfocused window, so the address bar could never take
                // the typed text.
                if self.headless {
                    raw_input.focused = true;
                }
                raw_input.events.append(&mut self.debug.debug_input_events);
                if self.address_bar.focused {
                    if let Some(text) = self.debug.debug_address_bar_typing.take() {
                        raw_input.events.push(debug_key_event(egui::Key::A, egui::Modifiers::COMMAND));
                        raw_input.events.push(egui::Event::Text(text));
                    }
                }
                self.update_window_title();
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
        }
    }

    /// The content quad's rect(s) (points, screen space) and
    /// which tab's texture to draw at each — a `Transition`
    /// overrides this to one entry, the animated in-between rect
    /// and that tab's id, regardless of `view_mode`; otherwise
    /// the normal Tab-view rule, which is two entries (primary +
    /// secondary, side by side) while a split is open, one
    /// otherwise, none in Graph view.
    pub(crate) fn content_panes_points(&self, ppp: f32) -> Vec<(egui::Rect, egui::Rect, String)> {
        if let Some(transition) = &self.transition {
            // Only a Tab has a GPU pane; an Image's transition is
            // painted through egui instead (see `build_ui`).
            if self.nodes.get(&transition.node_id).is_some_and(|n| n.as_tab().is_some()) {
                let (viewport, clip) = self.transition_content_rects_points(transition);
                vec![(viewport, clip, transition.node_id.clone())]
            } else {
                vec![]
            }
        } else if self.showing_tab_view() {
            let screen =
                self.window.as_ref().map(|w| w.inner_size()).unwrap_or(winit::dpi::PhysicalSize::new(1024, 768));
            let top = self.chrome_height_points();
            let full_rect = egui::Rect::from_min_size(
                egui::pos2(0.0, top),
                egui::vec2(screen.width as f32 / ppp, (screen.height as f32 / ppp - top).max(0.0)),
            );
            match (&self.focused, &self.split_tab) {
                (Some(focused_id), Some(split_id)) => {
                    let mid_x = full_rect.center().x;
                    let left = egui::Rect::from_min_max(full_rect.min, egui::pos2(mid_x, full_rect.max.y));
                    let right = egui::Rect::from_min_max(egui::pos2(mid_x, full_rect.min.y), full_rect.max);
                    vec![(left, left, focused_id.clone()), (right, right, split_id.clone())]
                }
                (Some(focused_id), None) => vec![(full_rect, full_rect, focused_id.clone())],
                (None, _) => vec![],
            }
        } else {
            vec![]
        }
    }

    /// `RedrawRequested`: draws the content panes and the egui output,
    /// taking a pending screenshot (and print-to-PDF) along the way.
    pub(crate) fn render_frame(&mut self, egui_output: Option<egui::FullOutput>) {
        // One save for the files dropped since the last frame, then reset
        // here, the one place every frame reaches, so no count goes stale.
        if self.drop_batch > 0 {
            self.save_session();
        }
        self.drop_batch = 0;
        let ppp = self.window.as_ref().map_or(1.0, |w| w.scale_factor()) as f32;
        let content_panes_pts = self.content_panes_points(ppp);
        let px = |rect: &egui::Rect| (rect.min.x * ppp, rect.min.y * ppp, rect.width() * ppp, rect.height() * ppp);
        let content_panes_px: Vec<(gpu::PanePx, &str)> = content_panes_pts
            .iter()
            .map(|(viewport, clip, id)| (gpu::PanePx { viewport: px(viewport), clip: px(clip) }, id.as_str()))
            .collect();

        let content_on_top = self.transition.is_some();
        // Not taken yet -- a transient surface-acquire failure
        // (Outdated/Occluded/...) means the screenshot was never
        // attempted this frame, and the request must survive to
        // retry on a later one instead of being silently dropped.
        let screenshot_path = self.debug.pending_screenshot.clone();
        let mut lost = None;
        if let Some(gpu) = &mut self.gpu {
            let outcome = match egui_output {
                Some(output) => gpu.render(
                    &content_panes_px,
                    content_on_top,
                    Some((&self.egui_ctx, output)),
                    screenshot_path.as_deref(),
                ),
                None => gpu.render(&content_panes_px, content_on_top, None, screenshot_path.as_deref()),
            };
            lost = gpu.take_device_lost();
            if outcome.surface_ok {
                self.debug.pending_screenshot = None;
                // The PNG is written by now (`render` saves it). A headless
                // `SCREENSHOT` (one with a waiter) isn't toasted: the toast
                // would sit over the window's bottom-left for 4s, in later
                // screenshots and under the script's next clicks.
                let mut headless = false;
                if outcome.screenshot_result.is_some() {
                    if let Some(waiter) = self.debug.screenshot_waiter.take() {
                        let _ = waiter.send(());
                        headless = true;
                    }
                }
                let screenshot_path = screenshot_path.filter(|_| !headless);
                self.handle_screenshot_result(screenshot_path, outcome.screenshot_result);
            }
        }
        if let Some(message) = lost {
            self.queue_toast(format!("The graphics device was lost ({message}). Restart manifold to draw again."));
        }
    }

    /// The pointer's position (window points) straight from the OS, for
    /// `WindowEvent::DroppedFile` -- winit 0.30 sends no `CursorMoved`
    /// during an OLE drag (confirmed live: `self.cursor_pos` still holds
    /// wherever the pointer was *before* the drag started), so the drop
    /// point has to be read some other way. `None` off Windows, or if the
    /// window handle/OS call fails; `last_pointer_point` is the fallback.
    #[cfg(target_os = "windows")]
    fn cursor_point(&self) -> Option<egui::Pos2> {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use windows_sys::Win32::{
            Foundation::POINT, Graphics::Gdi::ScreenToClient, UI::WindowsAndMessaging::GetCursorPos,
        };
        let RawWindowHandle::Win32(handle) = self.window.as_ref()?.window_handle().ok()?.as_raw() else { return None };
        let mut p = POINT { x: 0, y: 0 };
        // SAFETY: plain Win32 calls on our own window handle and a local
        // POINT that outlives them both.
        let ok = unsafe { GetCursorPos(&mut p) != 0 && ScreenToClient(handle.hwnd.get() as _, &mut p) != 0 };
        let ppp = self.ppp() as f32;
        ok.then(|| egui::pos2(p.x as f32 / ppp, p.y as f32 / ppp))
    }
    #[cfg(not(target_os = "windows"))]
    fn cursor_point(&self) -> Option<egui::Pos2> {
        None
    }

    /// `self.cursor_pos` (physical pixels, last real `CursorMoved`) as
    /// window points -- the fallback `WindowEvent::DroppedFile` uses when
    /// `cursor_point` can't read the OS cursor position directly.
    fn last_pointer_point(&self) -> egui::Pos2 {
        let ppp = self.ppp() as f32;
        egui::pos2(self.cursor_pos.0 as f32 / ppp, self.cursor_pos.1 as f32 / ppp)
    }

    /// Window focus changed. A chord's key-up can be lost while the window is
    /// in the background (Alt-Tab mid-chord); a key left in `swallowed_keys`
    /// would then swallow that key's next real release in the page. A mouse button's up can be lost the same way, and
    /// egui keeps it down: a connect-drag (and a line press) ends here, or the next ordinary click would link. A card
    /// drag ends as a release where the card is (`release_drag` saves its pos), never as a drop onto what is under it,
    /// and egui's own drag stops too: with the button still down it would carry on, the card following the pointer
    /// again (or a pan going on) until the next real release, which would then drop it.
    pub(crate) fn on_window_focus(&mut self, focused: bool) {
        if !focused {
            self.swallowed_keys.clear();
            self.cancel_connect();
            self.link.line_press = None;
            if self.interaction.dragging.is_some() {
                self.release_drag();
            }
            // Clears a press that is not a drag yet too, so the stale button can't start one.
            self.egui_ctx.stop_dragging();
        }
    }

    /// A screenshot `render_frame` just took: toasts where it went, or
    /// turns it into the pending PDF. Only the capture `start_print_to_pdf`
    /// asked for (its temp PNG) takes the PDF target; a failed one clears it.
    /// `screenshot_path` is `None` for a headless `SCREENSHOT`, which isn't toasted.
    pub(crate) fn handle_screenshot_result(
        &mut self,
        screenshot_path: Option<String>,
        result: Option<Result<(), String>>,
    ) {
        let Some(result) = result else { return };
        let Some(path) = screenshot_path else {
            if let Err(err) = result {
                eprintln!("screenshot: {err}");
            }
            return;
        };
        let pdf = self.debug.pending_print_to_pdf.take_if(|(png, _)| *png == path).map(|(_, pdf)| pdf);
        match (result, pdf) {
            (Ok(()), Some(pdf_path)) => {
                match pdf::write_image_as_pdf(std::path::Path::new(&path), std::path::Path::new(&pdf_path)) {
                    Ok(()) => self.queue_toast(format!("Saved {pdf_path}")),
                    Err(err) => {
                        eprintln!("print to pdf: {err}");
                        self.queue_toast(format!("Couldn't print to PDF: {err}"));
                    }
                }
                let _ = std::fs::remove_file(&path);
            }
            (Ok(()), None) => self.queue_toast(format!("Saved {path}")),
            (Err(err), Some(_)) => {
                eprintln!("print to pdf: capture failed: {err}");
                self.queue_toast(format!("Couldn't print to PDF: {err}"));
                let _ = std::fs::remove_file(&path);
            }
            (Err(err), None) => {
                eprintln!("screenshot: {err}");
                self.queue_toast(format!("Couldn't save the screenshot: {err}"));
            }
        }
    }

    /// `RedrawRequested`'s headless bookkeeping: keeps frames coming for
    /// pending debug input, and quits once `QUIT`'s frames have run.
    pub(crate) fn headless_end_of_frame(&mut self, event_loop: &ActiveEventLoop) {
        // `ADDRESSBAR` text is typed in the first frame the address
        // bar has focus; keep frames coming until then. Dropped if
        // Tab view went away, since the bar won't be built again.
        if self.debug.debug_address_bar_typing.is_some() {
            if self.showing_tab_view() || self.transition.is_some() {
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            } else {
                eprintln!("ADDRESSBAR: left Tab view before the address bar took focus; text dropped");
                self.debug.debug_address_bar_typing = None;
            }
        }

        self.release_idle_waiters();

        // See `quit_after_redraws`'s doc comment.
        if let Some(left) = self.debug.quit_after_redraws {
            if !self.debug_input_pending() || left == 0 {
                event_loop.exit();
            } else {
                self.debug.quit_after_redraws = Some(left - 1);
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
        }
    }

    /// Feeds a synthetic `WindowEvent` through the exact same handling real
    /// OS input goes through (`Self::window_event`) — used by
    /// `handle_debug_command` for mouse commands, so headless mouse-driven
    /// testing exercises real egui hit-testing/rendering-timing rather than
    /// bypassing it (see `debug_control`'s doc comment for why this matters:
    /// it's the only way headless testing could have caught the dropdown
    /// click-swallowing bug fixed in the autocomplete feature). A no-op if
    /// the window doesn't exist yet (shouldn't happen in practice — commands
    /// only arrive after `resumed` has created it).
    pub(crate) fn dispatch_window_event(&mut self, event_loop: &ActiveEventLoop, event: WindowEvent) {
        let Some(window) = &self.window else { return };
        let id = window.id();
        self.window_event(event_loop, id, event);
    }

    /// Debug input that still needs a frame: queued keys, `ADDRESSBAR`
    /// text not typed yet, a focus request, or a screenshot.
    pub(crate) fn debug_input_pending(&self) -> bool {
        !self.debug.debug_input_events.is_empty()
            || self.debug.debug_address_bar_typing.is_some()
            || self.pending_focus == Some(FocusTarget::AddressBar)
            || self.debug.pending_screenshot.is_some()
    }

    /// What `WAITIDLE` waits for: no debug input still needs a frame, no
    /// view transition is running, and the focused tab (if any) isn't loading.
    fn debug_idle(&self) -> bool {
        let loading =
            self.focused.as_ref().and_then(|id| self.nodes.get(id)).and_then(|n| n.as_tab()).is_some_and(|t| t.loading);
        !self.debug_input_pending() && self.transition.is_none() && !loading
    }

    /// Releases the `WAITIDLE` commands waiting, if `debug_idle` holds.
    pub(crate) fn release_idle_waiters(&mut self) {
        if !self.debug.idle_waiters.is_empty() && self.debug_idle() {
            for waiter in self.debug.idle_waiters.drain(..) {
                let _ = waiter.send(());
            }
        }
    }

    /// Moves the pointer to (x, y), then presses and releases `button`
    /// `count` times. All of it lands in one egui frame, so for `count` 2
    /// the second release is always within egui's double-click delay
    /// (0.3 s between frame times). Two `CLICK`s are a frame or more apart,
    /// and a slow headless frame pushes them past that.
    pub(crate) fn debug_click(
        &mut self,
        event_loop: &ActiveEventLoop,
        x: f64,
        y: f64,
        button: WinitMouseButton,
        count: u8,
        ctrl: bool,
    ) {
        self.dispatch_window_event(
            event_loop,
            WindowEvent::CursorMoved { device_id: DeviceId::dummy(), position: PhysicalPosition::new(x, y) },
        );
        if ctrl {
            self.dispatch_window_event(
                event_loop,
                WindowEvent::ModifiersChanged(winit::keyboard::ModifiersState::CONTROL.into()),
            );
        }
        for _ in 0..count {
            for state in [ElementState::Pressed, ElementState::Released] {
                self.dispatch_window_event(
                    event_loop,
                    WindowEvent::MouseInput { device_id: DeviceId::dummy(), state, button },
                );
            }
        }
        if ctrl {
            // The click's frame, with Ctrl still down, before it is let go.
            self.dispatch_window_event(event_loop, WindowEvent::RedrawRequested);
            self.dispatch_window_event(event_loop, WindowEvent::ModifiersChanged(Default::default()));
        }
    }

    /// Drains every tab's `events_rx` (not just the focused one — a
    /// background tab can still crash, or finish an in-flight swap, while
    /// suspended), applies every message but frames in arrival order through
    /// `handle_view_message`, and then uploads each tab's newest frame.
    /// Draining first means no tab node stays borrowed while a message is
    /// applied, so any message can touch any node. Frames go last because a
    /// crash or swap in the same drain replaces the tab they came from.
    /// The order is load-bearing: ipc may remove tabs before the idle
    /// sweep looks at them, the sweep and the fetches can remove tabs
    /// too, and frame uploads run last so none lands on a removed tab.
    pub(crate) fn drain_tab_events(&mut self) {
        let frame_updates = self.drain_view_ipc();
        self.idle_sweep();
        self.tick_fetchers();
        self.upload_frames(frame_updates);
    }

    /// Takes up to `VIEW_MESSAGES_PER_DRAIN` messages off each tab's `events_rx`, applies all but frames in
    /// arrival order, and returns each tab's newest frame (`drain_tab_events`).
    fn drain_view_ipc(&mut self) -> std::collections::HashMap<String, FrameUpdate> {
        // Keyed by tab id, not a plain `Vec` -- only the *latest* `FrameReady`
        // for a tab ever matters (each one fully supersedes the last in
        // `GpuState::upload_frame`'s own `tab_textures.insert`), matching the
        // same "keep only the newest, drop the rest" policy the WebView2
        // view-host's own capture code already applies one layer down (see
        // `capture.rs`'s `latest` field). A plain `Vec` here used to `push`
        // every buffered `FrameReady` this drain found and then process each
        // one in turn -- harmless while the main loop keeps up with the
        // view-host's own frame rate, but a real, confirmed bug once it
        // falls behind even slightly: found live via temporary
        // instrumentation showing bursts of up to 81 queued frames processed
        // in a single tick for one idle tab, each one a real, wasted GPU
        // texture+bind-group+thumbnail creation immediately thrown away by
        // the next -- severe enough burst GPU churn to both stall the main
        // thread for whole seconds at a time (Windows' own "(Not
        // Responding)", confirmed live) and outpace wgpu's own resource
        // reclamation, observed as continuous unbounded memory growth
        // (roughly 20MB/s from one idle default WebView2 tab). This bug
        // itself is engine-agnostic -- every engine's `FrameReady` messages
        // go through this same drain loop -- but WebView2 is the one that
        // actually triggers it in practice: its `Windows.Graphics.Capture`
        // pipeline captures continuously at the compositor's own refresh
        // rate regardless of whether the page changed, while CEF/Servo/
        // WebKit's paint-driven capture sends far fewer frames for a static,
        // idle page, staying well within what this loop could already keep
        // up with (confirmed live: an otherwise-identical idle-memory test
        // with CEF as the default engine stayed flat).
        // Since wave 3b the reader keeps one frame per tab itself (`ViewTab::take_frame`), so the burst described
        // above cannot build up in the first place.
        let mut frame_updates: std::collections::HashMap<String, FrameUpdate> = std::collections::HashMap::new();
        let mut messages: Vec<(String, ViewToHost)> = Vec::new();
        let now = std::time::Instant::now();
        let mut stuck: Vec<String> = Vec::new();
        for tab_id in &self.node_order {
            let Some(tab) = self.nodes.get(tab_id).and_then(|n| n.as_tab()) else {
                continue;
            };
            for msg in tab.tab.events_rx.try_iter().take(VIEW_MESSAGES_PER_DRAIN) {
                match msg {
                    // Only the last of a run of same-path Progress events is
                    // ever visible (applying one overwrites `bytes_received`),
                    // and each costs a scan of every node, so a run collapses
                    // into its newest.
                    ViewToHost::DownloadProgress { path, bytes_received } => {
                        if let Some((_, ViewToHost::DownloadProgress { path: last, bytes_received: last_bytes })) =
                            messages.last_mut()
                        {
                            if *last == path {
                                *last_bytes = bytes_received;
                                continue;
                            }
                        }
                        messages.push((tab_id.clone(), ViewToHost::DownloadProgress { path, bytes_received }));
                    }
                    msg => messages.push((tab_id.clone(), msg)),
                }
            }
            if let Some(frame) = tab.tab.take_frame() {
                frame_updates.insert(
                    tab_id.clone(),
                    FrameUpdate {
                        tab_id: tab_id.clone(),
                        kind: tab.tab.kind,
                        generation: frame.generation,
                        width: frame.width,
                        height: frame.height,
                        rows: frame.rows,
                        pixels: frame.pixels,
                    },
                );
            }
            if tab.tab.take_newly_unresponsive(now) {
                stuck.push(tab.url.clone());
            }
        }
        for url in stuck {
            self.queue_toast(format!(
                "Not responding: {}. Right-click its card to restart or end it.",
                Self::toast_url(&url)
            ));
        }
        // One `save_session()` for the whole drain rather than one per
        // `NavigationStateChanged`.
        let mut changed = false;
        if !messages.is_empty() {
            self.mark_active();
        }
        self.note_view_frames(frame_updates.len() as u32);
        for (tab_id, msg) in messages {
            let user_started = match &msg {
                ViewToHost::NavigationStateChanged { user_started, .. } => *user_started,
                _ => None,
            };
            let url_changed = self.handle_view_message(&tab_id, msg);
            if url_changed && self.shares_url_change(&tab_id, user_started) {
                // Ruling 3: every reported url change is shared, whichever
                // tab it came from -- share it here rather than leaving it
                // to `navigate_tab` (that only handles this instance's own
                // address-bar navigations, not the engine's own
                // confirmation this responds to, nor back/forward) --
                // except one a script or the store started. Shared as reported
                // now: a later unshared move (a redirect) is not (2b-3 Ruling 4).
                if let Some(url) = self.nodes.get(&tab_id).and_then(|n| n.as_tab()).map(|t| t.url.clone()) {
                    self.edit_url(&tab_id, &url);
                }
            }
            changed |= url_changed;
        }
        if changed {
            self.save_session();
        }
        frame_updates
    }

    /// Puts idle tabs to sleep (`sweep_idle_tabs`) when `idle_clock` says a sweep is due.
    fn idle_sweep(&mut self) {
        if self.idle_clock.due(std::time::Instant::now()) {
            let limit = self.settings.idle_sleep_minutes.map(|m| std::time::Duration::from_secs(u64::from(m) * 60));
            self.sweep_idle_tabs(limit);
        }
    }

    /// Waits for every queued image decode and applies the results, as the frames would.
    #[cfg(test)]
    pub(crate) fn finish_image_decodes(&mut self) {
        let end = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while self.image_decoder.pending() > 0 && std::time::Instant::now() < end {
            self.tick_fetchers();
            std::thread::yield_now();
        }
        self.tick_fetchers();
    }

    /// Applies what the favicon fetcher, PDF renderer and redirect fetcher finished since the last drain.
    fn tick_fetchers(&mut self) {
        for (origin, result) in self.favicon_fetcher.tick() {
            self.favicon_resolved.insert(origin.clone());
            if let Some(image) = result {
                if let Some(gpu) = &mut self.gpu {
                    gpu.upload_favicon(&origin, &image.rgba, image.width, image.height);
                }
            }
        }

        for (id, result) in self.image_decoder.tick() {
            // A node deleted (or already failed and retried) while its decode ran has nothing to receive it.
            if self.nodes.get(&id).and_then(|n| n.as_image()).is_none() {
                continue;
            }
            match result {
                Ok(decoded) => {
                    if let Some(gpu) = &mut self.gpu {
                        gpu.upload_image(&id, &decoded.rgba, decoded.width, decoded.height);
                    }
                    if let Some(image) = self.nodes.get_mut(&id).and_then(|n| n.as_image_mut()) {
                        (image.width, image.height, image.error) = (decoded.width, decoded.height, None);
                    }
                }
                Err(error) => {
                    self.queue_toast(error.clone());
                    if let Some(image) = self.nodes.get_mut(&id).and_then(|n| n.as_image_mut()) {
                        image.error = Some(error);
                    }
                }
            }
        }

        for (id, result) in self.pdf_renderer.tick() {
            // The node may have been deleted while this render was still
            // in flight -- skip a late-arriving result entirely rather
            // than uploading a texture nothing will ever free (`delete`'s
            // own `gpu.free_image` already ran, before this result could
            // possibly have arrived, so a late upload here would be a real
            // leak, not just wasted work).
            if self.nodes.get(&id).and_then(|n| n.as_pdf()).is_none() {
                continue;
            }
            match result {
                Ok(rendered) => {
                    if let Some(gpu) = &mut self.gpu {
                        gpu.upload_image(&id, &rendered.rgba, rendered.width, rendered.height);
                    }
                    if let Some(pdf) = self.nodes.get_mut(&id).and_then(|n| n.as_pdf_mut()) {
                        eprintln!(
                            "pdf: rendered {}: {}x{}, {} page(s)",
                            pdf.path, rendered.width, rendered.height, rendered.page_count
                        );
                        pdf.outcome = Some(Ok(PdfOutcome {
                            width: rendered.width,
                            height: rendered.height,
                            page_count: rendered.page_count,
                        }));
                    }
                }
                // Toasted like a failed image/JSON load, and kept as the
                // card's tooltip with the file named.
                Err(err) => {
                    let Some(path) = self.nodes.get(&id).and_then(|n| n.as_pdf()).map(|p| p.path.clone()) else {
                        continue;
                    };
                    eprintln!("pdf: couldn't open {path}: {err}");
                    let name = crate::json_file::file_name(std::path::Path::new(&path));
                    let err = format!("Couldn't open PDF {name}: {err}");
                    self.queue_toast(err.clone());
                    if let Some(pdf) = self.nodes.get_mut(&id).and_then(|n| n.as_pdf_mut()) {
                        pdf.outcome = Some(Err(err));
                    }
                    // No refetch here: a missing file plus a known URL is
                    // already handled before this can ever be reached --
                    // `materialize`'s own `exists()` check inserts the
                    // placeholder directly (never starting a render) and
                    // `retry_missing_file`'s URL branch covers a click on
                    // the card. Refetching here too would double-fetch a
                    // restore-time miss (two threads writing the same
                    // `<id>.pdf`) and loop forever on a `%PDF-`-shaped file
                    // that simply fails to render.
                }
            }
        }

        for FetchResult { job, body } in self.redirect_fetcher.tick() {
            let Some(body) = body else {
                // A tab whose fetch failed just stays an ordinary tab (see
                // `RedirectFetcher::tick`'s own doc comment) -- silent by
                // design. A Refresh failing this way would otherwise vanish
                // with no feedback at all, since it never reaches
                // `reload_fetched_node`'s own toast.
                if let FetchTarget::Node(id) = &job.target {
                    self.refresh_fetch_failed(id);
                }
                continue;
            };
            match job.target {
                // The tab may have been closed, or navigated somewhere else,
                // while this fetch was still in flight -- only convert it if
                // it's still the same tab still showing the same url that
                // triggered the fetch in the first place.
                FetchTarget::Tab(tab_id) => {
                    let still_current =
                        self.nodes.get(&tab_id).and_then(|n| n.as_tab()).is_some_and(|t| t.url == job.url);
                    if still_current {
                        self.redirect_tab_to_content(&tab_id, job.node_id, job.url, body);
                    }
                }
                FetchTarget::Node(id) => self.reload_fetched_node(&id, body),
            }
        }
    }

    /// Uploads each tab's newest frame to the GPU. Runs last in `drain_tab_events`: a message or fetch
    /// applied earlier in the same drain can remove the tab a frame came from.
    fn upload_frames(&mut self, frame_updates: std::collections::HashMap<String, FrameUpdate>) {
        for update in frame_updates.into_values() {
            // A `JsonBody`/`RedirectableContent` conversion earlier in this
            // same drain (or a plain tab close) can remove `update.tab_id`
            // before its `FrameReady` -- collected up front, above -- gets
            // here. Uploading it anyway would recreate a `tab_textures`
            // entry for a tab that no longer exists and that nothing ever
            // frees again (`remove_tab`'s `free_tab_texture` already ran).
            if !self.nodes.contains_key(&update.tab_id) {
                continue;
            }
            let format = if update.kind.desc().bgra {
                wgpu::TextureFormat::Bgra8UnormSrgb
            } else {
                wgpu::TextureFormat::Rgba8UnormSrgb
            };
            if let Some(pixels) = update.pixels {
                // RemoteTransport view-host (WebKit over WSL2): no shared
                // memory with this process, so the pixels travel inline in
                // the message itself.
                if let Some(gpu) = &mut self.gpu {
                    gpu.upload_frame(&update.tab_id, &pixels, update.width, update.height, format, update.rows);
                }
            } else if let Some(Ok(pixels)) = self
                .nodes
                .get_mut(&update.tab_id)
                .and_then(|n| n.as_tab_mut())
                .map(|t| t.tab.frame_pixels(update.generation, update.width, update.height))
            {
                // The tab keeps one mapping per frame file. A file the engine has already replaced (a resize) no
                // longer opens, or opens at the wrong size, so that frame is skipped; the next one names the new file.
                if let Some(gpu) = &mut self.gpu {
                    gpu.upload_frame(&update.tab_id, pixels, update.width, update.height, format, update.rows);
                }
            } else if let Some(gpu) = &mut self.gpu {
                // The skipped frame's changed rows never reached the texture: the next upload writes it whole.
                gpu.mark_stale(&update.tab_id);
            }
        }
    }

    /// The url tab `tab_id` shows, given one its engine reported (`url`)
    /// and the one it shows now (`current`). Right after `navigate_tab` an
    /// engine can still report the url it left, from before it started the
    /// navigation (CEF reports loading first): that keeps `current`, the
    /// typed url, rather than flashing back and adding the old url to
    /// history again. Any other url, or `NAVIGATE_START_WINDOW` passing,
    /// ends that. So does a finished load (`!loading`) at the old url after
    /// one reported loading: the navigation started and never left the page
    /// (an attachment download, a 204), and CEF reports nothing after it.
    pub(crate) fn reported_url(&mut self, tab_id: &str, url: String, loading: bool, current: String) -> String {
        const NAVIGATE_START_WINDOW: std::time::Duration = std::time::Duration::from_secs(2);
        let Some((from, at, started)) = self.spawns.navigated_from.get_mut(tab_id) else { return url };
        if *from == url && at.elapsed() < NAVIGATE_START_WINDOW && (loading || !*started) {
            *started |= loading;
            return current;
        }
        self.spawns.navigated_from.remove(tab_id);
        url
    }

    /// Applies one message from tab `tab_id`'s engine (see
    /// `drain_tab_events`, which uploads frames itself). Messages that
    /// update the tab do nothing once it's gone, e.g. after a crash earlier
    /// in the same drain. Returns whether something worth saving the
    /// session for happened: the tab's settled url changed (title/history-
    /// only changes and a same-url reload don't count), or a download
    /// completed (the store keeps completed downloads).
    pub(crate) fn handle_view_message(&mut self, tab_id: &str, msg: ViewToHost) -> bool {
        match msg {
            ViewToHost::FrameReady { .. } => {}
            ViewToHost::PageState(state) => {
                if let Some(Held::PageState(state)) = self.hold_for_cookies(tab_id, Held::PageState(state)) {
                    self.finish_sleep(tab_id, state);
                }
            }
            ViewToHost::Cookies(json) => {
                self.cookies.jar_pulls.remove(tab_id);
                self.cookies.jar_silent.remove(tab_id);
                self.apply_pulled_cookies(tab_id, &json);
                self.release_cookie_hold(tab_id);
            }
            ViewToHost::StateSnapshot(state) => {
                let target = self.nodes.get_mut(tab_id).and_then(|n| n.as_tab_mut()).and_then(|t| t.swap_target.take());
                if let Some(target) = target {
                    if let Some(Held::Snapshot(target, url)) =
                        self.hold_for_cookies(tab_id, Held::Snapshot(target, state.url))
                    {
                        self.finish_swap(tab_id, target, url);
                    }
                }
            }
            ViewToHost::NavigationStateChanged { url, title, can_go_back, can_go_forward, loading, .. } => {
                let Some(current) = self.nodes.get(tab_id).and_then(|n| n.as_tab()).map(|t| t.url.clone()) else {
                    return false;
                };
                let url = self.reported_url(tab_id, url, loading, current);
                let stale = self.nav_history.entry(tab_id.to_string()).or_default().observe(
                    &url,
                    &title,
                    loading,
                    can_go_forward,
                    std::time::Instant::now(),
                );
                let Some(node) = self.nodes.get_mut(tab_id).and_then(|n| n.as_tab_mut()) else {
                    return false;
                };
                let (url, title) = if stale { (node.url.clone(), node.title.clone()) } else { (url, title) };
                let url_changed = node.url != url;
                let skip = skip_convert_after(self.spawns.skip_convert.get(tab_id).copied(), url_changed, loading);
                match skip {
                    Some(settled) => self.spawns.skip_convert.insert(tab_id.to_string(), settled),
                    None => self.spawns.skip_convert.remove(tab_id),
                };
                if url_changed {
                    self.history.push(history::HistoryEntry { title: title.clone(), url: url.clone() });
                    if self.history.len() > history::MAX_HISTORY_ENTRIES {
                        self.history.remove(0);
                    }
                    self.history_ui.history_dirty = true;
                    self.history_ui.history_matches_cache = None;
                }
                node.url = url.clone();
                node.title = title;
                node.can_go_back = can_go_back;
                node.can_go_forward = can_go_forward;
                node.loading = loading;
                // The origin is known as soon as the url is, without waiting for the page to finish loading
                // (`favicon_fetch` guesses `/favicon.ico`); a typed navigation asks in `navigate_tab`.
                if url_changed {
                    self.request_favicons([url.as_str()]);
                }
                let restore = self.saved_scroll.get_mut(tab_id).is_some_and(|s| s.on_loading(loading));
                if restore {
                    if let Some(s) = self.saved_scroll.remove(tab_id) {
                        if let Some(tab) = self.nodes.get_mut(tab_id).and_then(|n| n.as_tab_mut()) {
                            tab.tab.send(&HostToView::RestoreScroll { x: s.x, y: s.y });
                        }
                    }
                }
                return url_changed;
            }
            ViewToHost::Crashed { reason } => {
                let Some(spawn) = self.restart_spawn(tab_id) else { return false };
                // A stale engine binary (protocol mismatch) or a host that cannot
                // start would fail the same way again, and a page or install that
                // crashes on every start would loop forever; each is left as a
                // failed card instead.
                let times = self.spawns.tab_crashes.entry(tab_id.to_string()).or_default();
                match crate::tabs::after_crash(&reason, times, std::time::Instant::now()) {
                    AfterCrash::Restart => {}
                    AfterCrash::GiveUp => {
                        eprintln!("view-host for {tab_id} crashed ({reason}); not restarting");
                        self.leave_failed_card(tab_id, &reason, LeftClosed::KeepsCrashing);
                        return false;
                    }
                    AfterCrash::CannotStart(why) => {
                        eprintln!("view-host for {tab_id} could not start ({why}); not restarting");
                        self.leave_failed_card(tab_id, why, LeftClosed::CannotStart);
                        return false;
                    }
                }
                let url = &spawn.url;
                // The origin only (`redacted_url`): a path or query can carry a token.
                eprintln!(
                    "view-host crashed ({reason}) for {tab_id}, restarting at {}",
                    crash_report::redacted_url(url)
                );
                // Previously silent from the actual app's own UI -- this only
                // ever showed up in the console, so a user with no terminal
                // open had no way to know a tab had crashed and quietly come
                // back, e.g. mid-navigation with unsaved page state lost.
                // Reuses the same toast infrastructure "Saved <path>" already
                // has (generalized to a plain message, see `save_toast`'s own
                // doc comment) rather than a second, separate notification
                // mechanism.
                self.queue_toast(format!("Tab crashed, restarted: {}", Self::toast_url(url)));
                self.restart_with(tab_id, spawn);
            }
            // Page-triggered new tab (target=_blank/window.open()/ctrl+click,
            // intercepted by CEF/WebKit/WebView2 - see ViewToHost::OpenTabRequested's
            // doc comment): a child of whichever tab requested it, staying in
            // the current view rather than auto-focusing, same as an explicit
            // "+ New Tab" click. Only web/internal schemes, only with a user
            // gesture, and only a few per source tab (`popup_gate.rs`).
            ViewToHost::OpenTabRequested { url, user_gesture } => {
                if !crate::popup_gate::page_tab_url_allowed(&url) {
                    match crate::popup_gate::scheme_for_log(&url) {
                        Some(scheme) => eprintln!("popup to a disallowed scheme dropped: {scheme}:"),
                        None => eprintln!("popup to a url with no scheme dropped"),
                    }
                } else {
                    match self.popup_gate.admit(tab_id, user_gesture, std::time::Instant::now()) {
                        crate::popup_gate::Admit::Open => {
                            self.spawn_tab_from(tab_id, &url, None);
                        }
                        crate::popup_gate::Admit::Drop { toast } => {
                            if toast {
                                self.queue_toast("Blocked pop-ups from this page".to_string());
                            }
                        }
                    }
                }
            }
            ViewToHost::FindResult { count, active_index } => {
                // Ignore a result from a tab that isn't the one the find bar
                // is currently searching — shouldn't normally happen
                // (`close_find_bar` sends `FindStop` on every focus change),
                // but a stray late result from just before a switch is cheap
                // to guard against.
                if self.focused.as_deref() == Some(tab_id) {
                    self.find.result = Some((count, active_index));
                }
            }
            ViewToHost::AudioPlayingChanged { playing } => {
                if let Some(tab) = self.nodes.get_mut(tab_id).and_then(|n| n.as_tab_mut()) {
                    tab.audio_playing = playing;
                }
            }
            // The engine's own in-memory page text, already sent over IPC --
            // saved and converted right away, same as any other content
            // kind. Written on the UI thread, unlike every other kind's
            // fetch: it's already in hand, so there's nothing to background.
            ViewToHost::JsonBody { body } => {
                let url = self.nodes.get(tab_id).and_then(|n| n.as_tab()).map(|tab| tab.url.clone());
                if self.spawns.skip_convert.contains_key(tab_id) {
                    return false;
                }
                if let (Some(url), Some(dir)) = (url, crate::profile::content_dir()) {
                    let node_id = self.alloc_node_id();
                    if let Some(web) = crate::redirect_fetch::save(
                        &dir,
                        &node_id,
                        crate::content::ContentKind::Json,
                        body.into_bytes(),
                    ) {
                        self.redirect_tab_to_content(tab_id, node_id, url, web);
                    }
                }
            }
            // See `manifold_view_protocol::ViewToHost::RedirectableContent`'s own doc
            // comment -- kicks off `manifold`'s own independent fetch of this
            // tab's current url; the actual tab-to-node conversion happens
            // once that completes, in `redirect_fetcher`'s own `tick()` drain
            // in `drain_tab_events`, not here.
            ViewToHost::RedirectableContent { content_type } => {
                use crate::content::ContentKind as K;
                if self.spawns.skip_convert.contains_key(tab_id) {
                    return false;
                }
                let url = self.nodes.get(tab_id).and_then(|n| n.as_tab()).map(|t| t.url.clone());
                let kind = crate::content::kind_of(&content_type);
                if let (Some(url), K::Image | K::Pdf | K::Json | K::Note) = (url, kind) {
                    let node_id = self.alloc_node_id();
                    let target = FetchTarget::Tab(tab_id.to_string());
                    self.redirect_fetcher.request(FetchJob { target, node_id, url, kind });
                }
            }
            ViewToHost::ContextMenuLink { x, y, href } => {
                let ppp = self.ppp() as f32;
                // Content-area coordinates -> window point space.
                // The engine echoes back the same physical-pixel
                // surface coordinates the right-click was sent in
                // (see `manifold_view_protocol::InputEvent`'s doc comment), so
                // this is the one place that has to divide out the
                // DPI scale: egui lays out in logical points. Then
                // shifted down past the toolbar, since Tab view's
                // content starts below it (see
                // `CHROME_HEIGHT_POINTS`'s other call sites). Only
                // meaningful for the tab currently on screen.
                if self.focused.as_deref() == Some(tab_id) {
                    self.menus.context_menu =
                        Some(ContextMenuState { x: x as f32 / ppp, y: y as f32 / ppp + CHROME_HEIGHT_POINTS, href });
                }
            }
            ViewToHost::DownloadStarted { path, total_bytes, url } => {
                self.insert_download_node(tab_id.to_string(), path, total_bytes, url);
            }
            // Each finds its node with `in_flight_download_id`: the in-flight
            // one this tab started, never a finished one with the same path.
            ViewToHost::DownloadProgress { path, bytes_received } => {
                if let Some(id) = self.in_flight_download_id(tab_id, &path) {
                    if let Some(dl) = self.nodes.get_mut(&id).and_then(|n| n.as_download_mut()) {
                        dl.bytes_received = bytes_received;
                    }
                }
            }
            ViewToHost::DownloadFailed { path } => {
                self.queue_toast(format!("Download failed: {path}"));
                if let Some(id) = self.in_flight_download_id(tab_id, &path) {
                    if let Some(dl) = self.nodes.get_mut(&id).and_then(|n| n.as_download_mut()) {
                        dl.failed = true;
                    }
                }
            }
            ViewToHost::DownloadCompleted { path } => {
                self.queue_toast(format!("Saved {path}"));
                if let Some(id) = self.in_flight_download_id(tab_id, &path) {
                    if let Some(dl) = self.nodes.get_mut(&id).and_then(|n| n.as_download_mut()) {
                        dl.completed = true;
                        return true;
                    }
                }
            }
        }
        false
    }
}

/// The app's own window/taskbar icon -- embedded at compile time (`include_
/// bytes!`, same posture `IBMPlexSerif-Regular.ttf` already uses) rather
/// than loaded from disk at runtime, so the binary stays self-contained.
/// `None` on decode failure rather than panicking -- a missing icon just
/// means winit's own default, not a reason to fail to start.
fn load_app_icon() -> Option<Icon> {
    let bytes = include_bytes!("../assets/icon/icon-64.png");
    let img = image::load_from_memory(bytes).ok()?.into_rgba8();
    let (width, height) = img.dimensions();
    Icon::from_rgba(img.into_raw(), width, height).ok()
}

#[cfg(test)]
mod idle_tests {
    use super::*;

    #[test]
    fn a_refresh_rate_becomes_a_frame_interval_with_a_60_hz_fallback() {
        let ms = |d: std::time::Duration| d.as_secs_f64() * 1000.0;
        assert!((ms(interval_from_millihertz(Some(60_000))) - 16.667).abs() < 0.01);
        assert!((ms(interval_from_millihertz(Some(144_000))) - 6.944).abs() < 0.01);
        assert!((ms(interval_from_millihertz(None)) - 16.667).abs() < 0.01);
        assert!((ms(interval_from_millihertz(Some(0))) - 16.667).abs() < 0.01);
    }

    #[test]
    fn a_burst_of_view_messages_posts_one_wake_until_it_is_handled() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::Arc;
        let pending = Arc::new(AtomicBool::new(false));
        let posted = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&posted);
        let wake = coalesced_wake(Arc::clone(&pending), move || {
            count.fetch_add(1, Ordering::SeqCst);
            true
        });
        for _ in 0..5 {
            wake();
        }
        assert_eq!(posted.load(Ordering::SeqCst), 1);
        pending.store(false, Ordering::Release); // what `on_view_wake` does first
        wake();
        assert_eq!(posted.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_view_wake_is_not_activity_but_a_command_is() {
        assert!(!is_activity(&UserEvent::ViewMessage));
        assert!(is_activity(&UserEvent::Debug(DebugCommand::Help)));
    }

    #[test]
    fn one_drain_applies_at_most_its_cap_per_tab() {
        use crate::node::TabPayload;
        use crate::process_manager::{ViewKind, ViewTab};
        let mut app = App::default();
        let (tab, _peer, feed) = ViewTab::fake_fed(ViewKind::Servo);
        let payload = TabPayload {
            tab,
            url: "https://a/".into(),
            title: String::new(),
            can_go_back: false,
            can_go_forward: false,
            loading: false,
            audio_playing: false,
            muted: false,
            zoom_factor: 1.0,
            swap_target: None,
        };
        app.test_node("t", crate::store::model::Kind::Tab, Runtime::Tab(payload));
        for i in 0..VIEW_MESSAGES_PER_DRAIN + 10 {
            feed.send(ViewToHost::AudioPlayingChanged { playing: i.is_multiple_of(2) }).unwrap();
        }
        app.drain_view_ipc();
        let left = app.nodes.get("t").and_then(|n| n.as_tab()).map_or(0, |t| t.tab.events_rx.try_iter().count());
        assert_eq!(left, 10, "the rest waits for the next drain");
    }

    #[test]
    fn a_failed_print_capture_clears_the_pdf_target() {
        let mut app = App::default();
        app.debug.pending_print_to_pdf = Some(("p.png".into(), "out.pdf".into()));
        app.handle_screenshot_result(Some("p.png".into()), Some(Err("no surface".into())));
        assert!(app.debug.pending_print_to_pdf.is_none());
        assert!(
            app.toasts.save_toast_queue.iter().any(|t| t.contains("Couldn't print")),
            "{:?}",
            app.toasts.save_toast_queue
        );
    }

    #[test]
    fn an_ordinary_screenshot_never_takes_a_pdf_target() {
        let mut app = App::default();
        app.debug.pending_print_to_pdf = Some(("p.png".into(), "out.pdf".into()));
        app.handle_screenshot_result(Some("shot.png".into()), Some(Ok(())));
        assert!(app.toasts.save_toast_queue.iter().any(|t| t == "Saved shot.png"));
        assert!(app.debug.pending_print_to_pdf.is_some(), "still waiting for its own capture");
    }

    #[test]
    fn losing_focus_forgets_swallowed_keys() {
        let mut app = App::default();
        app.swallowed_keys.insert(winit::keyboard::KeyCode::ArrowLeft);
        app.on_window_focus(false);
        assert!(app.swallowed_keys.is_empty());
    }

    #[test]
    fn a_failed_download_is_marked_and_no_longer_in_flight() {
        let mut app = App::default();
        app.insert_note_at("n".into(), None);
        app.insert_download_node("n".into(), "f.zip".into(), Some(10), None);
        let in_flight =
            |app: &App| app.nodes.values().any(|n| n.as_download().is_some_and(|d| !d.completed && !d.failed));
        assert!(in_flight(&app));
        app.handle_view_message("n", ViewToHost::DownloadFailed { path: "f.zip".into() });
        let download = app.nodes.values().find_map(|n| n.as_download()).unwrap();
        assert!(download.failed && !download.completed);
        assert!(!in_flight(&app));
    }

    #[test]
    fn a_redownload_updates_the_new_node_not_the_finished_one() {
        let mut app = App::default();
        app.insert_note_at("n".into(), None);
        app.insert_download_node("n".into(), "f.zip".into(), Some(10), None);
        app.handle_view_message("n", ViewToHost::DownloadCompleted { path: "f.zip".into() });
        app.insert_download_node("n".into(), "f.zip".into(), Some(20), None);
        app.handle_view_message("n", ViewToHost::DownloadProgress { path: "f.zip".into(), bytes_received: 7 });
        let downloads: Vec<_> = app.node_order.iter().filter_map(|id| app.nodes[id].as_download()).collect();
        assert_eq!(downloads.len(), 2);
        assert!(downloads[0].completed && downloads[0].bytes_received == 0, "the old node is untouched");
        assert_eq!(downloads[1].bytes_received, 7);
    }

    #[test]
    fn same_named_downloads_from_two_tabs_stay_apart() {
        let mut app = App::default();
        app.insert_note_at("a".into(), None);
        app.insert_note_at("b".into(), None);
        app.insert_download_node("a".into(), "f.zip".into(), None, None);
        app.insert_download_node("b".into(), "f.zip".into(), None, None);
        app.handle_view_message("a", ViewToHost::DownloadFailed { path: "f.zip".into() });
        let by_parent = |p: &str| {
            app.nodes
                .iter()
                .find(|(id, _)| app.opened_from(id) == Some(p))
                .and_then(|(_, n)| n.as_download())
                .unwrap()
                .failed
        };
        assert!(by_parent("a") && !by_parent("b"));
    }

    #[test]
    fn a_quiet_second_is_idle_and_a_burst_of_view_frames_is_not() {
        let mut app = App {
            pacing: FramePacing { last_activity: std::time::Instant::now() - IDLE_AFTER * 2, ..Default::default() },
            ..App::default()
        };
        assert!(app.is_idle());
        app.note_view_frames(BUSY_FRAMES - 1);
        assert!(app.is_idle(), "an idle page's few frames don't count");
        app.note_view_frames(1);
        assert!(!app.is_idle());
    }

    #[test]
    fn a_view_frame_in_an_idle_loop_asks_for_a_redraw_now() {
        let later = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let mut app = App { pacing: FramePacing { next_idle_frame: later, ..Default::default() }, ..App::default() };
        app.note_view_frames(0);
        assert_eq!(app.pacing.next_idle_frame, later, "no frame, no change");
        app.note_view_frames(1);
        assert!(app.pacing.next_idle_frame <= std::time::Instant::now(), "the next idle tick is due now");
    }

    #[test]
    fn input_ends_idle_and_headless_is_never_idle() {
        let mut app = App {
            pacing: FramePacing { last_activity: std::time::Instant::now() - IDLE_AFTER * 2, ..Default::default() },
            ..App::default()
        };
        app.mark_active();
        assert!(!app.is_idle());
        app.pacing.last_activity = std::time::Instant::now() - IDLE_AFTER * 2;
        app.headless = true;
        assert!(!app.is_idle());
    }
}
