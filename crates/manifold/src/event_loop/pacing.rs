//! Frame pacing: the event loop's user events, the idle and minimize state, and the wake coalescing a view host's messages go through.

use super::*;

/// The event loop's real user-event type: headless debug commands, plus
/// `TabSpawned` — see its own doc comment for why view-host spawning needs
/// to be async in the first place.
pub(crate) enum UserEvent {
    Debug(DebugJob),
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
            view_wake: std::sync::Arc::default(),
        }
    }
}

/// Whether `event` counts as activity that ends idle pacing (`App::mark_active`). An engine wake does not: the drain
/// it runs marks the loop active only when it applies a message, as a tick's drain does.
pub(super) fn is_activity(event: &UserEvent) -> bool {
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
pub(super) const JAR_PULL_EVERY: std::time::Duration = std::time::Duration::from_secs(30);

/// How often a minimized window's loop wakes (see `about_to_wait`).
pub(super) const MINIMIZED_TICK: std::time::Duration = std::time::Duration::from_millis(100);

impl App {
    pub(super) fn mark_active(&mut self) {
        self.pacing.last_activity = std::time::Instant::now();
    }

    /// `UserEvent::ViewMessage`: clears the pending flag first (a message arriving during the drain posts again),
    /// then applies what arrived (`drain_tab_events`).
    pub(super) fn on_view_wake(&mut self) {
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
    pub(super) fn note_view_frames(&mut self, n: u32) {
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
    pub(super) fn is_idle(&self) -> bool {
        !self.headless && self.transition.is_none() && self.pacing.last_activity.elapsed() >= IDLE_AFTER
    }

    /// Whether a present blocks (`Fifo`), so the loop needs no cap of its own; true before the GPU exists.
    pub(super) fn present_blocks(&self) -> bool {
        self.gpu.as_ref().is_none_or(GpuState::present_blocks)
    }

    /// The monitor's refresh interval (60 Hz when unknown), re-read at most once a second.
    pub(super) fn frame_interval(&mut self) -> std::time::Duration {
        if self.pacing.frame_interval.0.elapsed() >= std::time::Duration::from_secs(1) {
            let mhz = self.window.as_ref().and_then(|w| w.current_monitor()).and_then(|m| m.refresh_rate_millihertz());
            self.pacing.frame_interval = (std::time::Instant::now(), interval_from_millihertz(mhz));
        }
        self.pacing.frame_interval.1
    }

    /// Whether the real (non-headless) window is minimized.
    pub(super) fn is_minimized(&self) -> bool {
        !self.headless && self.window.as_ref().is_some_and(|w| w.is_minimized() == Some(true))
    }

    /// What a frame does before building its UI, minus the UI itself; and every tab stops capturing
    /// (`tab_visible`), since otherwise the engine and WebView2 keep using ~15% of a core behind a minimized window.
    pub(super) fn minimized_tick(&mut self) {
        self.poll_store();
        self.poll_trail_focus();
        self.poll_view_installs();
        self.drain_tab_events();
        self.finish_transition_if_done();
        self.sync_tab_visibility(true, std::time::Instant::now());
    }
}

#[cfg(test)]
mod tests {
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
        assert!(is_activity(&UserEvent::Debug(DebugJob::quit())));
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
