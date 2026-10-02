//! The winit event loop: the ApplicationHandler impl, each frame's egui
//! pass and render, window-event dispatch, the headless debug commands and
//! the engine processes' messages.

use super::*;
use crate::nav_history::Dir;
use crate::sleep_state::SleepWhy;

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
            attrs = attrs
                .with_position(winit::dpi::PhysicalPosition::new(-32000, -32000))
                .with_decorations(false)
                .with_skip_taskbar(true);
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
        if !self.restore_session() {
            if self.settings.has_shown_first_launch_demo {
                self.spawn_root_tab();
            } else {
                self.spawn_first_launch_demo();
                self.settings.has_shown_first_launch_demo = true;
                self.save_settings();
            }
        }
        self.prune_content_dir();
    }

    /// A clean exit (a window close, or headless `QUIT`): the store's last
    /// save and close.
    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.close_store();
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
            if !self.was_minimized {
                self.hide_tabs_for_minimize();
            }
            self.minimized_tick();
        } else if self.was_minimized {
            event_loop.set_control_flow(ControlFlow::Poll);
            for id in std::mem::take(&mut self.hidden_for_minimize) {
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
            if now >= self.next_idle_frame {
                self.next_idle_frame = now + self.idle_interval;
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_idle_frame));
        } else if !self.headless {
            event_loop.set_control_flow(ControlFlow::Poll);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
        self.was_minimized = minimized;
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        self.mark_active();
        match event {
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
                    self.save_settings();
                }
            }
            WindowEvent::Moved(position) => {
                if !self.headless {
                    self.settings.window_x = Some(position.x);
                    self.settings.window_y = Some(position.y);
                    self.save_settings();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_pos = (position.x, position.y);
                let chrome_height = self.chrome_height_px() as f64;
                let in_primary_pane = self.split_divider_x_px().is_none_or(|divider| position.x < divider);
                if in_tab_view && !consumed_by_egui && position.y >= chrome_height && in_primary_pane {
                    if let Some(id) = self.focused.clone() {
                        if let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
                            node.tab.send(&HostToEngine::Input(InputEvent::MouseMove {
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
                // Middle was missing this even though every engine-host's
                // own `translate_mouse_button`/native input path already
                // fully implements `MouseButton::Middle`.
                let egui_wants_it =
                    consumed_by_egui && button != WinitMouseButton::Right && button != WinitMouseButton::Middle;
                let in_primary_pane = self.split_divider_x_px().is_none_or(|divider| x < divider);
                let in_content = in_tab_view && !egui_wants_it && y >= chrome_height && in_primary_pane;
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
                    if let Some(id) = target {
                        if let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
                            let (x, y) = (x, y - chrome_height);
                            let event = match state {
                                ElementState::Pressed => {
                                    InputEvent::MouseDown { x, y, button, modifiers: self.key_modifiers }
                                }
                                ElementState::Released => {
                                    InputEvent::MouseUp { x, y, button, modifiers: self.key_modifiers }
                                }
                            };
                            node.tab.send(&HostToEngine::Input(event));
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
                            node.tab.send(&HostToEngine::Input(InputEvent::Scroll {
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
                                        node.tab.send(&HostToEngine::Input(InputEvent::Char { codepoint: ch as u32 }));
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
                self.poll_engine_installs();
                let focused_tab = self
                    .focused
                    .as_deref()
                    .and_then(|id| self.nodes.get(id))
                    .and_then(|n| n.as_tab())
                    .map(|t| format!("{:?} {}", t.tab.kind, t.url));
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
                self.idle_interval = repaint_in.map_or(IDLE_FRAME, |d| d.min(IDLE_FRAME));

                self.render_frame(egui_output);
                if !self.is_idle() {
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

/// Without activity for this long the loop slows down (see `about_to_wait`).
const IDLE_AFTER: std::time::Duration = std::time::Duration::from_secs(1);
/// An idle loop's frame interval.
pub(crate) const IDLE_FRAME: std::time::Duration = std::time::Duration::from_millis(100);
/// Engine frames per second that count as activity (video, an animation);
/// an idle page still sends a few.
const BUSY_FRAMES: u32 = 6;

/// How often a minimized window's loop wakes (see `about_to_wait`).
const MINIMIZED_TICK: std::time::Duration = std::time::Duration::from_millis(100);

impl App {
    fn mark_active(&mut self) {
        self.last_activity = std::time::Instant::now();
    }

    /// Counts engine frames; a burst of them is activity.
    fn note_engine_frames(&mut self, n: u32) {
        let now = std::time::Instant::now();
        if now.duration_since(self.frame_burst.0) >= std::time::Duration::from_secs(1) {
            self.frame_burst = (now, 0);
        }
        self.frame_burst.1 += n;
        if self.frame_burst.1 >= BUSY_FRAMES {
            self.mark_active();
        }
    }

    /// Whether the loop can run at `IDLE_FRAME`: a real window, nothing
    /// happened for `IDLE_AFTER`, and no view transition is under way.
    fn is_idle(&self) -> bool {
        !self.headless && self.transition.is_none() && self.last_activity.elapsed() >= IDLE_AFTER
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
                self.hidden_for_minimize.push(id);
            }
        }
    }

    /// What a frame does before building its UI, minus the UI itself.
    fn minimized_tick(&mut self) {
        self.poll_store();
        self.poll_engine_installs();
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
                // the focused tab's engine-host, not egui's Scene.
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
                raw_input.events.append(&mut self.debug_input_events);
                if self.address_bar.focused {
                    if let Some(text) = self.debug_address_bar_typing.take() {
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
        let screenshot_path = self.pending_screenshot.clone();
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
            if outcome.surface_ok {
                self.pending_screenshot = None;
                // The PNG is written by now (`render` saves it). A headless
                // `SCREENSHOT` (one with a waiter) isn't toasted: the toast
                // would sit over the window's bottom-left for 4s, in later
                // screenshots and under the script's next clicks.
                let mut headless = false;
                if outcome.screenshot_result.is_some() {
                    if let Some(waiter) = self.screenshot_waiter.take() {
                        let _ = waiter.send(());
                        headless = true;
                    }
                }
                let screenshot_path = screenshot_path.filter(|_| !headless);
                self.handle_screenshot_result(screenshot_path, outcome.screenshot_result);
            }
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

    /// A screenshot `render_frame` just took: toasts where it went, or
    /// turns it into the pending PDF.
    pub(crate) fn handle_screenshot_result(
        &mut self,
        screenshot_path: Option<String>,
        result: Option<Result<(), String>>,
    ) {
        match result {
            Some(Ok(())) => {
                if let Some(path) = screenshot_path {
                    if let Some(pdf_path) = self.pending_print_to_pdf.take() {
                        match pdf::write_image_as_pdf(std::path::Path::new(&path), std::path::Path::new(&pdf_path)) {
                            Ok(()) => {
                                self.queue_toast(format!("Saved {pdf_path}"));
                            }
                            Err(err) => eprintln!("print to pdf: {err}"),
                        }
                        let _ = std::fs::remove_file(&path);
                    } else {
                        self.queue_toast(format!("Saved {path}"));
                    }
                }
            }
            Some(Err(err)) => eprintln!("screenshot: {err}"),
            None => {}
        }
    }

    /// `RedrawRequested`'s headless bookkeeping: keeps frames coming for
    /// pending debug input, and quits once `QUIT`'s frames have run.
    pub(crate) fn headless_end_of_frame(&mut self, event_loop: &ActiveEventLoop) {
        // `ADDRESSBAR` text is typed in the first frame the address
        // bar has focus; keep frames coming until then. Dropped if
        // Tab view went away, since the bar won't be built again.
        if self.debug_address_bar_typing.is_some() {
            if self.showing_tab_view() || self.transition.is_some() {
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            } else {
                eprintln!("ADDRESSBAR: left Tab view before the address bar took focus; text dropped");
                self.debug_address_bar_typing = None;
            }
        }

        // See `quit_after_redraws`'s doc comment.
        if let Some(left) = self.quit_after_redraws {
            if !self.debug_input_pending() || left == 0 {
                event_loop.exit();
            } else {
                self.quit_after_redraws = Some(left - 1);
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
        !self.debug_input_events.is_empty()
            || self.debug_address_bar_typing.is_some()
            || self.pending_focus == Some(FocusTarget::AddressBar)
            || self.pending_screenshot.is_some()
    }

    /// Applies one headless `DebugCommand` (see `debug_control`'s doc
    /// comment for the overall design: mouse commands are real synthetic
    /// `WindowEvent`s, keyboard/text commands call the equivalent effect
    /// directly since `winit::event::KeyEvent` has no public constructor).
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
    ) {
        self.dispatch_window_event(
            event_loop,
            WindowEvent::CursorMoved { device_id: DeviceId::dummy(), position: PhysicalPosition::new(x, y) },
        );
        for _ in 0..count {
            for state in [ElementState::Pressed, ElementState::Released] {
                self.dispatch_window_event(
                    event_loop,
                    WindowEvent::MouseInput { device_id: DeviceId::dummy(), state, button },
                );
            }
        }
    }

    pub(crate) fn handle_debug_command(&mut self, event_loop: &ActiveEventLoop, cmd: DebugCommand) {
        // A transition only ends in a frame, and frames can stall for
        // seconds on a busy GPU, so one that ran out long ago could still
        // be `Some` here and make the ENTER/ESCAPE/arrow guards below drop
        // their command. It's timed by the clock, so end it by the clock.
        self.finish_transition_if_done();
        match cmd {
            DebugCommand::MouseMove { x, y } => {
                self.dispatch_window_event(
                    event_loop,
                    WindowEvent::CursorMoved { device_id: DeviceId::dummy(), position: PhysicalPosition::new(x, y) },
                );
            }
            DebugCommand::MouseDown { x, y, button } => {
                self.dispatch_window_event(
                    event_loop,
                    WindowEvent::CursorMoved { device_id: DeviceId::dummy(), position: PhysicalPosition::new(x, y) },
                );
                self.dispatch_window_event(
                    event_loop,
                    WindowEvent::MouseInput { device_id: DeviceId::dummy(), state: ElementState::Pressed, button },
                );
            }
            DebugCommand::MouseUp { x, y, button } => {
                self.dispatch_window_event(
                    event_loop,
                    WindowEvent::CursorMoved { device_id: DeviceId::dummy(), position: PhysicalPosition::new(x, y) },
                );
                self.dispatch_window_event(
                    event_loop,
                    WindowEvent::MouseInput { device_id: DeviceId::dummy(), state: ElementState::Released, button },
                );
            }
            DebugCommand::Click { x, y, button } => self.debug_click(event_loop, x, y, button, 1),
            DebugCommand::DoubleClick { x, y } => self.debug_click(event_loop, x, y, WinitMouseButton::Left, 2),
            DebugCommand::Scroll { x, y, delta_x, delta_y } => {
                self.dispatch_window_event(
                    event_loop,
                    WindowEvent::CursorMoved { device_id: DeviceId::dummy(), position: PhysicalPosition::new(x, y) },
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
            // A click on the address bar, then select-all and typing: focus
            // is requested now, and the text is typed as egui `Text` input in
            // the first frame the bar has it (see `debug_address_bar_typing`).
            DebugCommand::AddressBar(text) if self.showing_tab_view() => {
                self.pending_focus = Some(FocusTarget::AddressBar);
                self.debug_address_bar_typing = Some(text);
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            DebugCommand::AddressBar(_) => eprintln!("debug: ADDRESSBAR ignored: not in Tab view"),
            // Real key presses for the next frame's egui input, so they go
            // through the same checks a keyboard does, in any view.
            DebugCommand::Enter => {
                self.debug_input_events.push(debug_key_event(egui::Key::Enter, egui::Modifiers::NONE))
            }
            DebugCommand::Escape => {
                self.debug_input_events.push(debug_key_event(egui::Key::Escape, egui::Modifiers::NONE))
            }
            DebugCommand::ArrowDown => {
                self.debug_input_events.push(debug_key_event(egui::Key::ArrowDown, egui::Modifiers::NONE))
            }
            DebugCommand::ArrowRight => {
                self.debug_input_events.push(debug_key_event(egui::Key::ArrowRight, egui::Modifiers::NONE))
            }
            DebugCommand::DigitKey(d) => {
                let key = [
                    egui::Key::Num0,
                    egui::Key::Num1,
                    egui::Key::Num2,
                    egui::Key::Num3,
                    egui::Key::Num4,
                    egui::Key::Num5,
                    egui::Key::Num6,
                    egui::Key::Num7,
                    egui::Key::Num8,
                    egui::Key::Num9,
                ][d as usize];
                self.debug_input_events.push(debug_key_event(key, egui::Modifiers::NONE))
            }
            DebugCommand::SpaceKey => {
                self.debug_input_events.push(debug_key_event(egui::Key::Space, egui::Modifiers::NONE))
            }
            DebugCommand::ArrowLeft => {
                self.debug_input_events.push(debug_key_event(egui::Key::ArrowLeft, egui::Modifiers::NONE))
            }
            DebugCommand::ArrowUp => {
                self.debug_input_events.push(debug_key_event(egui::Key::ArrowUp, egui::Modifiers::NONE))
            }
            DebugCommand::TabKey(shift) => {
                let modifiers = if shift { egui::Modifiers::SHIFT } else { egui::Modifiers::NONE };
                self.debug_input_events.push(debug_key_event(egui::Key::Tab, modifiers))
            }
            DebugCommand::ShiftF10 => {
                self.debug_input_events.push(debug_key_event(egui::Key::F10, egui::Modifiers::SHIFT))
            }
            DebugCommand::GraphFocusLeft => self.move_graph_focus(egui::vec2(-1.0, 0.0)),
            DebugCommand::GraphFocusRight => self.move_graph_focus(egui::vec2(1.0, 0.0)),
            DebugCommand::GraphFocusUp => self.move_graph_focus(egui::vec2(0.0, -1.0)),
            DebugCommand::GraphFocusDown => self.move_graph_focus(egui::vec2(0.0, 1.0)),
            DebugCommand::GraphOpenFocused => self.open_focused_graph_node(),
            DebugCommand::TypeIntoPage(text) => {
                if let Some(id) = self.focused.clone() {
                    if let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
                        for ch in text.chars() {
                            node.tab.send(&HostToEngine::Input(InputEvent::Char { codepoint: ch as u32 }));
                        }
                    }
                }
            }
            DebugCommand::KeyPage(key) => {
                if let Some(id) = self.focused.clone() {
                    if let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
                        node.tab.send(&HostToEngine::Input(InputEvent::KeyDown { key, modifiers: self.key_modifiers }));
                        node.tab.send(&HostToEngine::Input(InputEvent::KeyUp { key, modifiers: self.key_modifiers }));
                    }
                }
            }
            DebugCommand::ReopenTab => self.reopen_closed_shortcut(),
            DebugCommand::ZoomIn => self.zoom_in_shortcut(),
            DebugCommand::ZoomOut => self.zoom_out_shortcut(),
            DebugCommand::ZoomReset => self.zoom_reset_shortcut(),
            DebugCommand::OpenDevTools => self.open_devtools_shortcut(),
            DebugCommand::NewTab => self.new_tab_shortcut(),
            DebugCommand::CloseTab => self.close_focused_tab_shortcut(),
            DebugCommand::FocusAddressBar => self.focus_address_bar_shortcut(),
            DebugCommand::NextTab => self.cycle_focused_tab(true),
            DebugCommand::PrevTab => self.cycle_focused_tab(false),
            DebugCommand::OpenFindBar => self.open_find_bar_shortcut(),
            DebugCommand::FindText(text) => {
                self.find.text = text;
                self.send_find_start();
            }
            DebugCommand::FindMatchCase(match_case) => {
                self.find.match_case = match_case;
                self.send_find_start();
            }
            DebugCommand::FindNext => self.send_find_navigate(true),
            DebugCommand::FindPrevious => self.send_find_navigate(false),
            DebugCommand::CloseFindBar => self.close_find_bar(),
            DebugCommand::SetVisible(visible) => {
                if let Some(id) = self.focused.clone() {
                    if let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
                        node.tab.set_visible(visible);
                    }
                }
            }
            // Only ever intercepted by the stdin reader itself.
            DebugCommand::Screenshot(_) | DebugCommand::Sleep(_) => {}
            DebugCommand::NotifyWhenScreenshotTaken(path, waiter) => {
                self.pending_screenshot = Some(path);
                self.screenshot_waiter = Some(waiter);
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            DebugCommand::Resize(width, height) => {
                if let Some(window) = &self.window {
                    let _ = window.request_inner_size(PhysicalSize::new(width, height));
                }
            }
            DebugCommand::QuickSwitchOpen => {
                self.quick_switcher = Some(QuickSwitcher { query: String::new(), selected: 0 });
                self.pending_focus = Some(FocusTarget::QuickSwitcher);
            }
            DebugCommand::QuickSwitchText(text) => {
                if let Some(switcher) = &mut self.quick_switcher {
                    switcher.query = text;
                    switcher.selected = 0;
                }
            }
            DebugCommand::QuickSwitchDown => self.quick_switcher_move(1),
            DebugCommand::QuickSwitchUp => self.quick_switcher_move(-1),
            DebugCommand::QuickSwitchEnter => self.quick_switcher_confirm(),
            DebugCommand::QuickSwitchEscape => self.quick_switcher = None,
            DebugCommand::SessionNameText(text) => self.session_name_draft = text,
            DebugCommand::SaveSessionAs(name) => self.save_named_session(&name),
            DebugCommand::OpenSession(name) => {
                self.load_named_session(&name);
                self.close_settings_page();
            }
            DebugCommand::GraphFilterText(text) => self.graph_filter = text,
            DebugCommand::HistoryFilterText(text) => self.history_filter = text,
            DebugCommand::BookmarksFilterText(text) => self.bookmarks_filter = text,
            DebugCommand::ToggleView => self.toggle_view_mode_shortcut(),
            DebugCommand::ToggleSettingsPage => self.toggle_settings_page(),
            DebugCommand::ExportSession(path) => {
                self.export_session_to(std::path::Path::new(&path));
            }
            DebugCommand::ExportState(path) => {
                let focused_url = self
                    .focused
                    .as_ref()
                    .and_then(|id| self.nodes.get(id))
                    .and_then(|n| n.as_tab())
                    .map(|t| t.url.clone());
                let suggestions: Vec<String> = self
                    .address_bar
                    .combined()
                    .into_iter()
                    .map(|s| match s {
                        Suggestion::OpenTab { tab_id, .. } => format!("tab:{tab_id}"),
                        Suggestion::Search { query } => query,
                    })
                    .collect();
                // The current level's cards, in draw order, with where each
                // one's center is on screen: what a headless test needs to
                // aim a click or drag at a card.
                let cards: Vec<serde_json::Value> = self
                    .level_items()
                    .into_iter()
                    .filter_map(|item| {
                        let screen = self.graph_world_rect_to_screen(item.rect);
                        if item.kind == LevelItemKind::JsonCard {
                            return Some(serde_json::json!({
                                "id": item.id,
                                "kind": "JsonCard",
                                "screen": [screen.center().x, screen.center().y],
                                "rect": [screen.min.x, screen.min.y, screen.max.x, screen.max.y],
                            }));
                        }
                        let LevelItemKind::Node(kind) = item.kind else { return None };
                        let node = self.nodes.get(&item.id)?;
                        Some(serde_json::json!({
                            "id": item.id,
                            "kind": format!("{kind:?}"),
                            "group": node.group,
                            "parent": node.parent,
                            "color": node.color.is_some(),
                            "pinned": node.payload.pinned(),
                            "editing": node.as_note().is_some_and(|n| n.editing),
                            "screen": [screen.center().x, screen.center().y],
                            "rect": [screen.min.x, screen.min.y, screen.max.x, screen.max.y],
                        }))
                    })
                    .collect();
                let mut selected_tabs: Vec<&String> = self.selected_tabs.iter().collect();
                selected_tabs.sort();
                let state = serde_json::json!({
                    "tab_view": self.showing_tab_view(),
                    "transition": self.transition.is_some(),
                    "focused_url": focused_url,
                    "address_bar_text": self.address_bar.text,
                    "suggestions": suggestions,
                    "highlighted": self.address_bar.highlighted,
                    "current_group": self.current_group,
                    "cards": cards,
                    "json_level": self.json_level_state(),
                    "selected_tabs": selected_tabs,
                    // The toast showing, or else the next queued one.
                    "toast": self.save_toast.as_ref().map(|(message, _)| message).or(self.save_toast_queue.front()),
                });
                if let Err(err) = std::fs::write(&path, state.to_string()) {
                    eprintln!("EXPORTSTATE: couldn't write {path}: {err}");
                }
            }
            DebugCommand::ImportSession(path) => match crate::json_file::load(std::path::Path::new(&path), "session") {
                Ok(json) => self.pending_import = Some(json),
                Err(err) => self.queue_toast(err),
            },
            DebugCommand::ConfirmImport => {
                if let Some(json) = self.pending_import.take() {
                    self.replace_running_session(&json);
                }
            }
            DebugCommand::CancelImport => {
                self.pending_import = None;
            }
            DebugCommand::RenameGroup => self.rename_current_group_shortcut(),
            DebugCommand::BookmarkFolderText(text) => {
                if let Some((_, draft)) = &mut self.renaming_bookmark_folder {
                    *draft = text;
                }
            }
            DebugCommand::DismissPeek => self.dismiss_hover_peek(),
            DebugCommand::ExportGroup(path) => {
                if let Some((group_id, _)) = self
                    .node_menu
                    .clone()
                    .filter(|(id, _)| self.nodes.get(id).is_some_and(|node| node.as_group().is_some()))
                {
                    if let Some(json) = self.export_group_json(&group_id) {
                        write_json_export(&json, std::path::Path::new(&path), "group");
                    }
                }
            }
            DebugCommand::ImportGroup(path) => {
                match crate::json_file::load::<store::json::JsonDoc>(std::path::Path::new(&path), "group") {
                    Ok(json) => self.import_group_json(&json),
                    Err(err) => self.queue_toast(err),
                }
            }
            DebugCommand::DuplicateTab => self.duplicate_focused_tab_shortcut(),
            DebugCommand::ExportBookmarks(path) => {
                bookmarks::save_to(&self.bookmarks, std::path::Path::new(&path));
            }
            DebugCommand::ImportBookmarks(path) => match bookmarks::load_from(std::path::Path::new(&path)) {
                Ok(imported) => {
                    self.bookmarks = imported;
                    bookmarks::save(&self.bookmarks);
                }
                Err(err) => self.queue_toast(err),
            },
            DebugCommand::PrintToPdf(path) => self.start_print_to_pdf(path),
            DebugCommand::TogglePin => self.pin_focused_tab_shortcut(),
            DebugCommand::Reload => self.reload_focused_tab_shortcut(),
            DebugCommand::GoBack | DebugCommand::GoForward => {
                if let Some(id) = self.focused.clone() {
                    let dir = if matches!(cmd, DebugCommand::GoBack) { Dir::Back } else { Dir::Forward };
                    self.navigate_history(&id, dir);
                }
            }
            DebugCommand::NewGroup => self.new_group_shortcut(),
            DebugCommand::NewNote => self.new_note_shortcut(),
            DebugCommand::AddFile(path, None) => {
                if self.add_file_at(path.into(), None).is_ok() {
                    self.save_session();
                }
            }
            // Placed like a real drop at that window point. No
            // `save_session` here either -- same reasoning as
            // `WindowEvent::DroppedFile`, so a script sending several
            // `ADDFILE`s with a point still gets one save per batch, not
            // one per file.
            DebugCommand::AddFile(path, Some((x, y))) => {
                self.drop_file(path.into(), egui::pos2(x, y));
            }
            // Only ever intercepted by the stdin reader itself.
            DebugCommand::WaitTabs(_) => {}
            DebugCommand::NotifyWhenTabsSpawned(waiter) => {
                if self.tab_spawns_in_flight == 0 {
                    let _ = waiter.send(());
                } else {
                    self.tabs_spawned_waiters.push(waiter);
                }
            }
            DebugCommand::AddDownload(path) => {
                if let Some(tab_id) = self.focused.clone() {
                    self.insert_download_node(tab_id, path, None, None);
                }
            }
            DebugCommand::Split(kind) => self.start_split(kind),
            DebugCommand::CloseSplit => self.close_split(),
            DebugCommand::JsonSelect(pointer) => self.apply_json_edit(JsonEdit::Select(pointer)),
            DebugCommand::JsonToggle(pointer) => self.apply_json_edit(JsonEdit::Toggle(pointer)),
            DebugCommand::JsonMore(pointer) => self.apply_json_edit(JsonEdit::More(pointer)),
            DebugCommand::JsonRelayout => self.apply_json_edit(JsonEdit::Relayout),
            DebugCommand::JsonSearch(text) => self.set_json_search(text),
            DebugCommand::CloseGroup(id) => {
                if self.nodes.get(&id).is_some_and(|n| n.as_group().is_some()) {
                    self.close_node(&id);
                }
            }
            DebugCommand::IdleSweep(secs) => self.sweep_idle_tabs(Some(std::time::Duration::from_secs(secs))),
            DebugCommand::SleepNode(id) => {
                if self.nodes.get(&id).is_some_and(|n| n.as_group().is_some()) {
                    self.sleep_group(&id);
                } else {
                    self.request_sleep(&id, SleepWhy::Manual);
                }
            }
            DebugCommand::TriggerFirstLaunchDemo => self.spawn_first_launch_demo(),
            DebugCommand::Help => debug_control::print_help(),
            DebugCommand::CloseAllLevel => self.close_all_at_current_level(),
            DebugCommand::ToggleShortcutsHelp => {
                self.shortcuts_help_open = !self.shortcuts_help_open;
            }
            DebugCommand::Quit => {
                // See `quit_after_redraws`'s doc comment: exiting right
                // now could drop a pending Enter/Escape/arrow command, or a
                // just-requested Screenshot that hasn't been captured yet
                // (`pending_screenshot` is only consumed inside a
                // `RedrawRequested` — a `Screenshot` immediately followed by
                // `Quit`, with no frame in between, would otherwise silently
                // lose the screenshot the same way an unconsumed Enter
                // could).
                if self.debug_input_pending() {
                    self.quit_after_redraws = Some(3);
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                } else {
                    event_loop.exit();
                }
            }
        }
    }

    /// Drains every tab's `events_rx` (not just the focused one — a
    /// background tab can still crash, or finish an in-flight swap, while
    /// suspended), applies every message but frames in arrival order through
    /// `handle_engine_message`, and then uploads each tab's newest frame.
    /// Draining first means no tab node stays borrowed while a message is
    /// applied, so any message can touch any node. Frames go last because a
    /// crash or swap in the same drain replaces the tab they came from.
    pub(crate) fn drain_tab_events(&mut self) {
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

        // Keyed by tab id, not a plain `Vec` -- only the *latest* `FrameReady`
        // for a tab ever matters (each one fully supersedes the last in
        // `GpuState::upload_frame`'s own `tab_textures.insert`), matching the
        // same "keep only the newest, drop the rest" policy the WebView2
        // engine-host's own capture code already applies one layer down (see
        // `capture.rs`'s `latest` field). A plain `Vec` here used to `push`
        // every buffered `FrameReady` this drain found and then process each
        // one in turn -- harmless while the main loop keeps up with the
        // engine-host's own frame rate, but a real, confirmed bug once it
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
        let mut frame_updates: std::collections::HashMap<String, FrameUpdate> = std::collections::HashMap::new();
        let mut messages: Vec<(String, EngineToHost)> = Vec::new();
        for tab_id in &self.node_order {
            let Some(tab) = self.nodes.get(tab_id).and_then(|n| n.as_tab()) else {
                continue;
            };
            while let Ok(msg) = tab.tab.events_rx.try_recv() {
                match msg {
                    EngineToHost::FrameReady { width, height, pixels, .. } => {
                        frame_updates.insert(
                            tab_id.clone(),
                            FrameUpdate {
                                tab_id: tab_id.clone(),
                                socket_id: tab.tab.socket_id.clone(),
                                kind: tab.tab.kind,
                                width,
                                height,
                                pixels,
                            },
                        );
                    }
                    // Only the last of a run of same-path Progress events is
                    // ever visible (applying one overwrites `bytes_received`),
                    // and each costs a scan of every node, so a run collapses
                    // into its newest.
                    EngineToHost::DownloadProgress { path, bytes_received } => {
                        if let Some((_, EngineToHost::DownloadProgress { path: last, bytes_received: last_bytes })) =
                            messages.last_mut()
                        {
                            if *last == path {
                                *last_bytes = bytes_received;
                                continue;
                            }
                        }
                        messages.push((tab_id.clone(), EngineToHost::DownloadProgress { path, bytes_received }));
                    }
                    msg => messages.push((tab_id.clone(), msg)),
                }
            }
        }
        // One `save_session()` for the whole drain rather than one per
        // `NavigationStateChanged`.
        let mut changed = false;
        if !messages.is_empty() {
            self.mark_active();
        }
        self.note_engine_frames(frame_updates.len() as u32);
        for (tab_id, msg) in messages {
            let user_started = match &msg {
                EngineToHost::NavigationStateChanged { user_started, .. } => *user_started,
                _ => None,
            };
            let url_changed = self.handle_engine_message(&tab_id, msg);
            if url_changed && self.shares_url_change(&tab_id, user_started) {
                // Ruling 3: every reported url change is shared, whichever
                // tab it came from -- mark it here rather than leaving it
                // to `navigate_tab` (that only handles this instance's own
                // address-bar navigations, not the engine's own
                // confirmation this responds to, nor back/forward) --
                // except one a script or the store started.
                self.mark(&tab_id, field::URL);
            }
            changed |= url_changed;
        }
        if changed {
            self.save_session();
        }

        if self.idle_clock.due(std::time::Instant::now()) {
            let limit = self.settings.idle_sleep_minutes.map(|m| std::time::Duration::from_secs(u64::from(m) * 60));
            self.sweep_idle_tabs(limit);
        }

        for (origin, result) in self.favicon_fetcher.tick() {
            self.favicon_resolved.insert(origin.clone());
            if let Some(image) = result {
                if let Some(gpu) = &mut self.gpu {
                    gpu.upload_favicon(&origin, &image.rgba, image.width, image.height);
                }
            }
        }

        for (id, result) in self.pdf_fetcher.tick() {
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
                // RemoteTransport engine-host (WebKit over WSL2): no shared
                // memory with this process, so the pixels travel inline in
                // the message itself.
                if let Some(gpu) = &mut self.gpu {
                    gpu.upload_frame(&update.tab_id, &pixels, update.width, update.height, format);
                }
            } else if let Ok(buf) =
                SharedFrameBuffer::open_existing(frame_path(&update.socket_id), update.width, update.height)
            {
                // `open_existing` refuses a file of the wrong size (the engine-host
                // truncates-then-grows it on resize), so a frame caught mid-resize
                // is skipped and the next redraw tries again.
                if let Some(gpu) = &mut self.gpu {
                    gpu.upload_frame(&update.tab_id, buf.as_slice(), update.width, update.height, format);
                }
            }
        }
    }

    /// The url tab `tab_id` shows, given one its engine reported (`url`)
    /// and the one it shows now (`current`). Right after `navigate_tab` an
    /// engine can still report the url it left, from before it started the
    /// navigation (CEF reports loading first): that keeps `current`, the
    /// typed url, rather than flashing back and adding the old url to
    /// history again. Any other url, or `NAVIGATE_START_WINDOW` passing,
    /// ends that.
    pub(crate) fn reported_url(&mut self, tab_id: &str, url: String, current: String) -> String {
        const NAVIGATE_START_WINDOW: std::time::Duration = std::time::Duration::from_secs(2);
        let Some((from, at)) = self.navigated_from.get(tab_id) else { return url };
        if *from == url && at.elapsed() < NAVIGATE_START_WINDOW {
            return current;
        }
        self.navigated_from.remove(tab_id);
        url
    }

    /// Applies one message from tab `tab_id`'s engine (see
    /// `drain_tab_events`, which uploads frames itself). Messages that
    /// update the tab do nothing once it's gone, e.g. after a crash earlier
    /// in the same drain. Returns whether something worth saving the
    /// session for happened: the tab's settled url changed (title/history-
    /// only changes and a same-url reload don't count), or a download
    /// completed (the store keeps completed downloads).
    pub(crate) fn handle_engine_message(&mut self, tab_id: &str, msg: EngineToHost) -> bool {
        match msg {
            EngineToHost::FrameReady { .. } => {}
            EngineToHost::PageState(state) => self.finish_sleep(tab_id, state),
            EngineToHost::StateSnapshot(state) => {
                let target = self.nodes.get_mut(tab_id).and_then(|n| n.as_tab_mut()).and_then(|t| t.swap_target.take());
                if let Some(target) = target {
                    self.finish_swap(tab_id, target, state.url);
                }
            }
            EngineToHost::NavigationStateChanged { url, title, can_go_back, can_go_forward, loading, .. } => {
                let Some(current) = self.nodes.get(tab_id).and_then(|n| n.as_tab()).map(|t| t.url.clone()) else {
                    return false;
                };
                let url = self.reported_url(tab_id, url, current);
                self.nav_history.entry(tab_id.to_string()).or_default().observe(&url, loading);
                let Some(node) = self.nodes.get_mut(tab_id).and_then(|n| n.as_tab_mut()) else {
                    return false;
                };
                let url_changed = node.url != url;
                let skip = skip_convert_after(self.skip_convert.get(tab_id).copied(), url_changed, loading);
                match skip {
                    Some(settled) => self.skip_convert.insert(tab_id.to_string(), settled),
                    None => self.skip_convert.remove(tab_id),
                };
                if url_changed {
                    self.history.push(history::HistoryEntry { title: title.clone(), url: url.clone() });
                    if self.history.len() > history::MAX_HISTORY_ENTRIES {
                        self.history.remove(0);
                    }
                    self.history_dirty = true;
                    // The origin is known as soon as the url is --
                    // no need to wait for the page to actually
                    // finish loading, unlike a page's own favicon
                    // link tag this project doesn't parse anyway
                    // (see `favicon_fetch`'s own doc comment).
                    // `favicon_resolved`/`FaviconFetcher`'s own
                    // `in_flight` set both dedupe repeat requests,
                    // so this can fire on every url change with no
                    // extra bookkeeping needed here.
                    if let Some(origin) = favicon_fetch::url_origin(&url) {
                        if !self.favicon_resolved.contains(&origin) {
                            self.favicon_fetcher.request(origin);
                        }
                    }
                }
                node.url = url;
                node.title = title;
                node.can_go_back = can_go_back;
                node.can_go_forward = can_go_forward;
                node.loading = loading;
                let restore = self.saved_scroll.get_mut(tab_id).is_some_and(|s| s.on_loading(loading));
                if restore {
                    if let Some(s) = self.saved_scroll.remove(tab_id) {
                        if let Some(tab) = self.nodes.get_mut(tab_id).and_then(|n| n.as_tab_mut()) {
                            tab.tab.send(&HostToEngine::RestoreScroll { x: s.x, y: s.y });
                        }
                    }
                }
                return url_changed;
            }
            EngineToHost::Crashed { reason } => {
                let Some(spawn) = self.nodes.get(tab_id).and_then(|n| {
                    let t = n.as_tab()?;
                    TabSpawn::respawn(tab_id, n, t.tab.kind, t.url.clone())
                }) else {
                    return false;
                };
                let url = &spawn.url;
                eprintln!("engine-host crashed ({reason}) for {tab_id}, restarting at {url}");
                // Previously silent from the actual app's own UI -- this only
                // ever showed up in the console, so a user with no terminal
                // open had no way to know a tab had crashed and quietly come
                // back, e.g. mid-navigation with unsaved page state lost.
                // Reuses the same toast infrastructure "Saved <path>" already
                // has (generalized to a plain message, see `save_toast`'s own
                // doc comment) rather than a second, separate notification
                // mechanism.
                self.queue_toast(format!("Tab crashed, restarted: {url}"));
                self.nodes.remove(tab_id);
                // Same stale-state guard `destroy_tab`/`finish_swap` apply — see
                // `finish_swap`'s own comment on this.
                if self.dragging.as_deref() == Some(tab_id) {
                    self.dragging = None;
                }
                if self.hovered.as_deref() == Some(tab_id) {
                    self.hovered = None;
                }
                self.respawn_tab(spawn);
            }
            // Page-triggered new tab (target=_blank/window.open()/ctrl+click,
            // intercepted by CEF/WebKit - see EngineToHost::OpenTabRequested's
            // doc comment): a child of whichever tab requested it, staying in
            // the current view rather than auto-focusing, same as an explicit
            // "+ New Tab" click.
            EngineToHost::OpenTabRequested { url } => {
                let parent = Some(tab_id.to_string());
                let group = self.group_of(&parent);
                self.spawn_new_tab(parent, group, &url);
            }
            EngineToHost::FindResult { count, active_index } => {
                // Ignore a result from a tab that isn't the one the find bar
                // is currently searching — shouldn't normally happen
                // (`close_find_bar` sends `FindStop` on every focus change),
                // but a stray late result from just before a switch is cheap
                // to guard against.
                if self.focused.as_deref() == Some(tab_id) {
                    self.find.result = Some((count, active_index));
                }
            }
            EngineToHost::AudioPlayingChanged { playing } => {
                if let Some(tab) = self.nodes.get_mut(tab_id).and_then(|n| n.as_tab_mut()) {
                    tab.audio_playing = playing;
                }
            }
            // The engine's own in-memory page text, already sent over IPC --
            // saved and converted right away, same as any other content
            // kind. Written on the UI thread, unlike every other kind's
            // fetch: it's already in hand, so there's nothing to background.
            EngineToHost::JsonBody { body } => {
                let url = self.nodes.get(tab_id).and_then(|n| n.as_tab()).map(|tab| tab.url.clone());
                if self.skip_convert.contains_key(tab_id) {
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
            // See `ipc_protocol::EngineToHost::RedirectableContent`'s own doc
            // comment -- kicks off `manifold`'s own independent fetch of this
            // tab's current url; the actual tab-to-node conversion happens
            // once that completes, in `redirect_fetcher`'s own `tick()` drain
            // in `drain_tab_events`, not here.
            EngineToHost::RedirectableContent { content_type } => {
                use crate::content::ContentKind as K;
                if self.skip_convert.contains_key(tab_id) {
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
            EngineToHost::ContextMenuLink { x, y, href } => {
                let ppp = self.ppp() as f32;
                // Content-area coordinates -> window point space.
                // The engine echoes back the same physical-pixel
                // surface coordinates the right-click was sent in
                // (see `ipc_protocol::InputEvent`'s doc comment), so
                // this is the one place that has to divide out the
                // DPI scale: egui lays out in logical points. Then
                // shifted down past the toolbar, since Tab view's
                // content starts below it (see
                // `CHROME_HEIGHT_POINTS`'s other call sites). Only
                // meaningful for the tab currently on screen.
                if self.focused.as_deref() == Some(tab_id) {
                    self.context_menu =
                        Some(ContextMenuState { x: x as f32 / ppp, y: y as f32 / ppp + CHROME_HEIGHT_POINTS, href });
                }
            }
            EngineToHost::DownloadStarted { path, total_bytes, url } => {
                self.insert_download_node(tab_id.to_string(), path, total_bytes, url);
            }
            EngineToHost::DownloadProgress { path, bytes_received } => {
                if let Some(dl) = self.nodes.values_mut().find_map(|n| n.as_download_mut().filter(|d| d.path == path)) {
                    dl.bytes_received = bytes_received;
                }
            }
            EngineToHost::DownloadFailed { path } => {
                self.queue_toast(format!("Download failed: {path}"));
                if let Some(dl) = self.nodes.values_mut().find_map(|n| n.as_download_mut().filter(|d| d.path == path)) {
                    dl.failed = true;
                }
            }
            EngineToHost::DownloadCompleted { path } => {
                self.queue_toast(format!("Saved {path}"));
                if let Some(dl) = self.nodes.values_mut().find_map(|n| n.as_download_mut().filter(|d| d.path == path)) {
                    dl.completed = true;
                    return true;
                }
            }
        }
        false
    }
}

#[cfg(test)]
mod idle_tests {
    use super::*;

    #[test]
    fn a_failed_download_is_marked_and_no_longer_in_flight() {
        let mut app = App::default();
        app.insert_note_at("n".into(), None);
        app.insert_download_node("n".into(), "f.zip".into(), Some(10), None);
        let in_flight =
            |app: &App| app.nodes.values().any(|n| n.as_download().is_some_and(|d| !d.completed && !d.failed));
        assert!(in_flight(&app));
        app.handle_engine_message("n", EngineToHost::DownloadFailed { path: "f.zip".into() });
        let download = app.nodes.values().find_map(|n| n.as_download()).unwrap();
        assert!(download.failed && !download.completed);
        assert!(!in_flight(&app));
    }

    #[test]
    fn a_quiet_second_is_idle_and_a_burst_of_engine_frames_is_not() {
        let mut app = App { last_activity: std::time::Instant::now() - IDLE_AFTER * 2, ..App::default() };
        assert!(app.is_idle());
        app.note_engine_frames(BUSY_FRAMES - 1);
        assert!(app.is_idle(), "an idle page's few frames don't count");
        app.note_engine_frames(1);
        assert!(!app.is_idle());
    }

    #[test]
    fn input_ends_idle_and_headless_is_never_idle() {
        let mut app = App { last_activity: std::time::Instant::now() - IDLE_AFTER * 2, ..App::default() };
        app.mark_active();
        assert!(!app.is_idle());
        app.last_activity = std::time::Instant::now() - IDLE_AFTER * 2;
        app.headless = true;
        assert!(!app.is_idle());
    }
}
