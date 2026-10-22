//! Each frame: the egui pass, the render, the OS cursor and window focus, screenshots and the headless end of frame.

use super::*;

impl App {
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
                // the typed text. A window under `--debug-commands` may
                // not have it either (an elevated profiling task cannot
                // take the foreground).
                if self.headless || self.debug_commands {
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
    pub(super) fn cursor_point(&self) -> Option<egui::Pos2> {
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
    pub(super) fn cursor_point(&self) -> Option<egui::Pos2> {
        None
    }

    /// `self.cursor_pos` (physical pixels, last real `CursorMoved`) as
    /// window points -- the fallback `WindowEvent::DroppedFile` uses when
    /// `cursor_point` can't read the OS cursor position directly.
    pub(super) fn last_pointer_point(&self) -> egui::Pos2 {
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
                self.interaction.drag_origin.clear();
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
