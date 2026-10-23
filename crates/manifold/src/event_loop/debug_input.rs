//! The headless debug commands' input: window events fed by a probe, idle waiters and the timed click.

use super::*;

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
    /// view transition is running, the Graph camera isn't easing or gliding, no address-bar suggestion is on its
    /// way, and the focused tab (if any) isn't loading. It must hold for `QUIET_FRAMES` frame ends in a row (see `release_idle_waiters`).
    fn debug_idle(&self) -> bool {
        let loading =
            self.focused.as_ref().and_then(|id| self.nodes.get(id)).and_then(|n| n.as_tab()).is_some_and(|t| t.loading);
        !self.debug_input_pending()
            && self.transition.is_none()
            && !self.camera.is_moving()
            && !self.address_bar.fetcher.is_busy()
            && !loading
    }

    /// Releases the `WAITIDLE` commands waiting once `debug_idle` has held for `QUIET_FRAMES` frame ends in a row,
    /// and until then asks for the next frame: a headless window draws only when asked, and the camera, queued
    /// input and loads all advance on frames. Several frames, because one that sizes a new card is not the frame
    /// that places it, and a click aimed at what the first drew can miss.
    pub(crate) fn release_idle_waiters(&mut self) {
        const QUIET_FRAMES: u8 = 3;
        if self.debug.idle_waiters.is_empty() {
            return;
        }
        self.debug.idle_frames = if self.debug_idle() { self.debug.idle_frames.saturating_add(1) } else { 0 };
        if self.debug.idle_frames >= QUIET_FRAMES {
            for waiter in self.debug.idle_waiters.drain(..) {
                let _ = waiter.send(());
            }
        } else if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Moves the pointer to (x, y), then presses and releases `button`
    /// `count` times. All of it lands in one egui frame, so for `count` 2
    /// the second release is always within egui's double-click delay
    /// (0.3 s between frame times). Two `CLICK`s are a frame or more apart,
    /// and a slow headless frame pushes them past that. A double click waits
    /// out the triple-click window of the click before it, or egui would
    /// count it as a triple. Egui measures that window in its own frame times,
    /// so the wait reads egui's `time_since_last_click` (after a frame, so it
    /// is current), which counts every click, not only the debug ones.
    pub(crate) fn debug_click(
        &mut self,
        event_loop: &ActiveEventLoop,
        x: f64,
        y: f64,
        button: WinitMouseButton,
        count: u8,
        ctrl: bool,
    ) {
        // Egui calls a release a triple click when the click before the last was under twice its double-click delay
        // ago, wherever that click was, so a double click needs that long since any earlier click. The margin covers
        // the frame after the wait starting later than the frame read here.
        const TRIPLE_CLICK_WINDOW: f32 = 0.6;
        const MARGIN: f32 = 0.05;
        if count >= 2 {
            self.dispatch_window_event(event_loop, WindowEvent::RedrawRequested);
            let since = self.egui_ctx.input(|i| i.pointer.time_since_last_click());
            if since < TRIPLE_CLICK_WINDOW {
                std::thread::sleep(std::time::Duration::from_secs_f32(TRIPLE_CLICK_WINDOW + MARGIN - since));
            }
        }
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
}
