//! The winit event loop: the ApplicationHandler impl, each frame's egui
//! pass and render, window-event dispatch, the headless debug commands and
//! the engine processes' messages.

use super::*;
use crate::debug_control::debug_key_event;
use crate::tabs::{AfterCrash, Held, LeftClosed};
use winit::window::Icon;

mod debug_input;
mod fetchers;
mod frame;
mod pacing;
mod view_ipc;
mod view_message;

pub(crate) use fetchers::Workers;
pub(crate) use pacing::*;

/// What the window events leave for forwarding input to pages (`App::page_input`).
#[derive(Default)]
pub(crate) struct PageInput {
    /// The last real `CursorMoved` position, in physical pixels.
    pub(crate) cursor_pos: (f64, f64),
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
    /// of the engine-side `DragState` each native-input view-host keeps.
    pub(crate) native_drag_tab: Option<NodeId>,
    /// Bitflags (see `manifold_view_protocol::modifiers`), tracked from winit's
    /// `WindowEvent::ModifiersChanged` so every forwarded `KeyDown`/`KeyUp`
    /// carries the modifier state at the time of that key, not just the
    /// currently-pressed key itself.
    pub(crate) key_modifiers: u8,
    /// Click counts for page presses (`InputEvent::MouseDown::click_count`).
    pub(crate) click_counter: click_count::ClickCounter,
    /// Physical keys of a shortcut chord pressed in Tab view, whose release (and typed text) must not
    /// reach the page either (`shortcuts::claims_chord`).
    pub(crate) swallowed_keys: std::collections::HashSet<WinitKeyCode>,
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
            if let (Some(width), Some(height)) = (self.settings.value.window_width, self.settings.value.window_height) {
                attrs = attrs.with_inner_size(winit::dpi::LogicalSize::new(width as f64, height as f64));
            }
            if let (Some(x), Some(y)) = (self.settings.value.window_x, self.settings.value.window_y) {
                attrs = attrs.with_position(winit::dpi::PhysicalPosition::new(x, y));
            }
        }
        // The accesskit adapter must exist before the window is shown, so the real window is
        // created hidden and shown once it is wired up (the headless one stays as it is: no
        // screen reader there).
        let real_window = !self.headless;
        let window = Arc::new(event_loop.create_window(attrs.with_visible(!real_window)).expect("create window"));
        window.set_theme(window_theme(self.settings.value.theme));
        self.gpu = Some(GpuState::new(window.clone()));
        let mut egui_state = egui_winit::State::new(
            self.egui_ctx.clone(),
            egui::ViewportId::ROOT,
            window.as_ref(),
            Some(window.scale_factor() as f32),
            window.theme(),
            None,
        );
        if real_window {
            if let Some(proxy) = self.event_proxy.clone() {
                egui_state.init_accesskit(event_loop, window.as_ref(), proxy);
            }
            window.set_visible(true);
        }
        // Debug commands find controls by name in the accesskit tree (`App::debug.controls`), screen reader or not.
        if self.headless || self.debug_commands {
            self.egui_ctx.enable_accesskit();
        }
        if self.headless {
            crate::theme::steady_caret(&self.egui_ctx);
        }
        self.egui_state = Some(egui_state);
        self.window = Some(window);
        self.open_store();
        let restored = self.restore_session();
        if self.ensure_default_jar() {
            self.save_session();
        }
        if !restored {
            if self.settings.value.has_shown_first_launch_demo {
                self.spawn_root_tab();
            } else {
                self.spawn_first_launch_demo();
                self.settings.value.has_shown_first_launch_demo = true;
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
        telemetry::flush();
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
    /// Polling a minimized window costs about 16% of a core.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let minimized = self.is_minimized();
        if minimized {
            event_loop.set_control_flow(ControlFlow::WaitUntil(std::time::Instant::now() + MINIMIZED_TICK));
            self.minimized_tick();
        } else if self.pacing.was_minimized {
            // The next frame's `sync_tab_visibility` shows the tabs on screen again.
            event_loop.set_control_flow(ControlFlow::Poll);
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
            tracing::trace!(target: "input", "user event");
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
                    A11y::AccessibilityDeactivated if !(self.headless || self.debug_commands) => {
                        self.egui_ctx.disable_accesskit()
                    }
                    A11y::AccessibilityDeactivated => {}
                }
            }
            UserEvent::Debug(cmd) => {
                // Pointer input is hit-tested against the last frame's widgets, so draw the current state first
                // (`DebugJob::pointer`).
                let pointer = cmd.pointer;
                if self.headless && pointer {
                    self.dispatch_window_event(event_loop, WindowEvent::RedrawRequested);
                }
                self.run_debug_job(event_loop, cmd);
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
            tracing::trace!(target: "input", "window event");
            self.mark_active();
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Focused(focused) => self.on_window_focus(focused),
            WindowEvent::ScaleFactorChanged { .. } => self.on_scale_factor_changed(),
            WindowEvent::Resized(size) => self.on_resized(size),
            WindowEvent::Moved(position) => {
                if !self.headless {
                    self.settings.value.window_x = Some(position.x);
                    self.settings.value.window_y = Some(position.y);
                    self.note_settings_changed();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.page_input.cursor_pos = (position.x, position.y);
                if in_tab_view && !consumed_by_egui && self.pointer_in_content(position.x, position.y) {
                    let chrome_height = self.chrome_height_px() as f64;
                    let modifiers = self.page_input.key_modifiers;
                    if let Some(node) = self.focused_tab_mut() {
                        node.tab.send(&HostToView::Input(InputEvent::MouseMove {
                            x: position.x,
                            y: position.y - chrome_height,
                            modifiers,
                        }));
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
                self.on_mouse_input(state, button, in_tab_view, consumed_by_egui)
            }
            WindowEvent::MouseWheel { delta, .. } => self.on_wheel(delta, in_tab_view && !consumed_by_egui),
            WindowEvent::ModifiersChanged(new_modifiers) => {
                self.page_input.key_modifiers = winit_modifiers_to_bits(new_modifiers.state());
            }
            WindowEvent::KeyboardInput { event, .. } => self.on_key(&event, in_tab_view && !consumed_by_egui),
            // No frame while minimized, and no `request_redraw`, so the loop
            // stops spinning; `about_to_wait` ticks it instead.
            WindowEvent::RedrawRequested if self.is_minimized() => {}
            WindowEvent::RedrawRequested => {
                let _frame = tracing::info_span!("frame").entered();
                self.poll_store();
                self.poll_trail_focus();
                self.poll_view_installs();
                self.sync_host_settings();
                let focused_tab = self
                    .focused
                    .and_then(|id| self.nodes.get(&id))
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

                let egui_output = tracing::info_span!("egui").in_scope(|| self.run_egui_frame());
                // egui asks for the next frame itself while something it
                // draws is animating.
                let repaint_in =
                    egui_output.as_ref().and_then(|o| o.viewport_output.values().map(|v| v.repaint_delay).min());
                self.debug.egui_wants_frame = repaint_in.is_some_and(|d| d.is_zero());
                if self.debug.egui_wants_frame {
                    // What keeps an idle window drawing: the `request_repaint` call sites, as `file:line reason`.
                    if tracing::enabled!(target: "repaint", tracing::Level::TRACE) {
                        let causes: Vec<String> =
                            self.egui_ctx.repaint_causes().iter().map(ToString::to_string).collect();
                        tracing::trace!(target: "repaint", label = causes.join(" | ").as_str());
                    }
                }
                // An animation asking for the next frame at once is drawn at the display's rate, not counted as
                // activity: a blinking caret asks every half second, and counting it kept the loop out of idle.
                let animation_pace = self.frame_interval();
                self.pacing.idle_interval = repaint_in.map_or(IDLE_FRAME, |d| d.max(animation_pace).min(IDLE_FRAME));

                tracing::info_span!("render").in_scope(|| self.render_frame(egui_output));
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

impl App {
    fn focused_tab_mut(&mut self) -> Option<&mut TabPayload> {
        let id = self.focused?;
        self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut())
    }

    /// Whether the pointer at `(x, y)` (physical window pixels) is over the primary pane's content: below the
    /// chrome and, while a split is open, left of its divider.
    fn pointer_in_content(&self, x: f64, y: f64) -> bool {
        let in_primary_pane = self.split_divider_x_px().is_none_or(|divider| x < divider);
        y >= self.chrome_height_px() as f64 && in_primary_pane
    }

    fn resize_panes(&mut self) {
        if let Some(id) = self.focused {
            self.send_resize_to(id);
        }
        if let Some(id) = self.split_tab {
            self.send_resize_to(id);
        }
    }

    /// Fires when the window moves to a monitor with a different
    /// scale factor (also on a live resolution/scaling change).
    /// Confirmed live this was a real gap, not just theoretical:
    /// instrumenting a live cross-monitor drag showed a transitional
    /// `send_resize_to` call computed with the size already updated
    /// for the new monitor but `window.scale_factor()` still
    /// reporting the *old* monitor's value -- a genuine race between
    /// winit's size and scale-factor updates that `Resized` alone
    /// doesn't protect against. This handler re-triggers the same
    /// tab-resize notification `Resized` does, so whichever event
    /// actually lands last with the fully-settled state (typically
    /// this one, since it specifically announces the scale change
    /// completed) corrects any bad intermediate value the other
    /// sent. `window.inner_size()` (not the stale pre-change size)
    /// is used for the GPU surface for the same reason.
    fn on_scale_factor_changed(&mut self) {
        if let Some(gpu) = &mut self.gpu {
            if let Some(size) = self.window.as_ref().map(|w| w.inner_size()) {
                gpu.resize(size.width, size.height);
            }
        }
        self.resize_panes();
    }

    fn on_resized(&mut self, size: winit::dpi::PhysicalSize<u32>) {
        if let Some(gpu) = &mut self.gpu {
            gpu.resize(size.width, size.height);
        }
        // A minimized window reports 0x0; resizing a page to that
        // crashed the WebView2 engine ("Tab crashed, restarted" on
        // restore). The restore's own `Resized` sends the real size.
        if size.width > 0 && size.height > 0 {
            self.resize_panes();
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
            self.settings.value.window_width = Some((size.width as f64 / ppp).round() as u32);
            self.settings.value.window_height = Some((size.height as f64 / ppp).round() as u32);
            self.note_settings_changed();
        }
    }

    fn on_mouse_input(
        &mut self,
        state: ElementState,
        button: WinitMouseButton,
        in_tab_view: bool,
        consumed_by_egui: bool,
    ) {
        let chrome_height = self.chrome_height_px() as f64;
        let (x, y) = self.page_input.cursor_pos;
        // `egui_winit` reports the right button as "consumed"
        // unconditionally (`consumed_by_egui=true` even over this project's own raw
        // content quad, which isn't a real egui widget and has
        // nothing there to legitimately claim it) — bypassed for
        // right and middle specifically so a right/middle-click on
        // tab content still reaches `InputEvent::MouseDown`/
        // `MouseUp` the same way every left-click already does.
        // Middle was missing this even though every view-host's
        // own `translate_mouse_button`/native input path already
        // fully implements `MouseButton::Middle`.
        let egui_wants_it = consumed_by_egui && button != WinitMouseButton::Right && button != WinitMouseButton::Middle;
        let in_content = in_tab_view && !egui_wants_it && self.pointer_in_content(x, y);
        if state == ElementState::Pressed && in_tab_view && !consumed_by_egui {
            self.side_button_nav(button);
        }
        let Some(button) = translate_mouse_button(button) else { return };
        // A release always goes to whichever tab is mid-drag,
        // even when it lands outside the content area -- see
        // `native_drag_tab`'s doc comment for the stuck-button
        // bug that gating it the same way as a press caused.
        let target = match state {
            ElementState::Pressed if in_content => self.focused,
            ElementState::Released => {
                if matches!(button, MouseButton::Left) { self.page_input.native_drag_tab.take() } else { None }
                    .or_else(|| in_content.then_some(self.focused).flatten())
            }
            ElementState::Pressed => None,
        };
        if state == ElementState::Pressed && target.is_none() {
            self.page_input.click_counter.reset();
        }
        let Some(id) = target else { return };
        let modifiers = self.page_input.key_modifiers;
        if let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
            let (x, y) = (x, y - chrome_height);
            let event = match state {
                ElementState::Pressed => {
                    let click_count =
                        self.page_input.click_counter.press(id, button, (x, y), std::time::Instant::now());
                    InputEvent::MouseDown { x, y, button, modifiers, click_count }
                }
                ElementState::Released => InputEvent::MouseUp {
                    x,
                    y,
                    button,
                    modifiers,
                    click_count: self.page_input.click_counter.current(),
                },
            };
            node.tab.send(&HostToView::Input(event));
            if state == ElementState::Pressed && matches!(button, MouseButton::Left) {
                self.page_input.native_drag_tab = Some(id);
            }
        }
    }

    /// `to_page`: the tab view is showing and egui didn't take the event.
    fn on_wheel(&mut self, delta: winit::event::MouseScrollDelta, to_page: bool) {
        let (x, y) = self.page_input.cursor_pos;
        if !(to_page && self.pointer_in_content(x, y)) {
            return;
        }
        let chrome_height = self.chrome_height_px() as f64;
        let modifiers = self.page_input.key_modifiers;
        if let Some(node) = self.focused_tab_mut() {
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
                modifiers,
            }));
        }
    }

    /// `to_page`: the tab view is showing and egui didn't take the event.
    fn on_key(&mut self, event: &winit::event::KeyEvent, to_page: bool) {
        // A key we take as a shortcut isn't also the page's: its press, text and release stay here.
        let swallowed = match (event.physical_key, event.state) {
            (PhysicalKey::Code(code), ElementState::Pressed)
                if crate::shortcuts::claims_chord(code, self.page_input.key_modifiers) =>
            {
                self.page_input.swallowed_keys.insert(code);
                true
            }
            (PhysicalKey::Code(code), ElementState::Released) => self.page_input.swallowed_keys.remove(&code),
            _ => false,
        };
        if !to_page || swallowed {
            return;
        }
        let modifiers = self.page_input.key_modifiers;
        if let Some(node) = self.focused_tab_mut() {
            if let Some(key) = translate_key_code(event.physical_key) {
                let input = match event.state {
                    ElementState::Pressed => InputEvent::KeyDown { key, modifiers },
                    ElementState::Released => InputEvent::KeyUp { key, modifiers },
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
