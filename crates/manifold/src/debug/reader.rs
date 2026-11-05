//! The stdin reader thread, the headless-mode state it feeds, and the UI-thread side that runs its jobs.

use super::*;

/// Runs one reader-thread op. `false` when the event loop is gone.
fn run_reader_op(proxy: &EventLoopProxy<UserEvent>, op: ReaderOp) -> bool {
    let (tx, rx) = std::sync::mpsc::channel();
    let (timeout, waiting_for, job): (_, String, Job) = match op {
        ReaderOp::Sleep(duration) => {
            std::thread::sleep(duration);
            return true;
        }
        // The app sends `()` on `tx` once `tab_spawns_in_flight` is zero.
        ReaderOp::WaitTabs(timeout) => (
            timeout,
            format!("WAITTABS: tab spawns still in flight after {timeout:?}; continuing"),
            Box::new(move |app, _| {
                if app.spawns.tab_spawns_in_flight == 0 {
                    let _ = tx.send(());
                } else {
                    app.spawns.tabs_spawned_waiters.push(tx);
                }
            }),
        ),
        // The app sends `()` on `tx` once `App::debug_idle` holds, checked when it arrives and at the end of every
        // frame.
        ReaderOp::WaitIdle(timeout) => (
            timeout,
            format!("WAITIDLE: input or a load still pending after {timeout:?}; continuing"),
            Box::new(move |app, _| {
                app.debug.idle_waiters.push(tx);
                app.debug.idle_frames = 0;
                // Pending input needs frames to land, and the check runs at the end of each.
                app.release_idle_waiters();
            }),
        ),
        // The app sends `()` on `tx` once `App::index_settled` holds, checked when it arrives and at the end of every
        // frame. An engine that never answers `QueryPageText` (Servo, WebKitGTK and CEF have no arm yet) keeps it from
        // holding until the timeout.
        ReaderOp::WaitIndex(timeout) => (
            timeout,
            format!("WAITINDEX: the index still behind after {timeout:?}; continuing"),
            Box::new(move |app, _| {
                app.debug.index_waiters.push(tx);
                app.release_index_waiters();
            }),
        ),
        // Frames can stall for seconds and the app holds one pending screenshot, so an unblocked second
        // `SCREENSHOT` (or a `QUIT`) would drop the first.
        ReaderOp::Screenshot(path) => (
            std::time::Duration::from_secs(30),
            format!("SCREENSHOT: {path} not taken after 30 s; continuing"),
            Box::new(move |app, _| {
                app.debug.pending_screenshot = Some(path);
                app.debug.screenshot_waiter = Some(tx);
                if let Some(window) = &app.window {
                    window.request_redraw();
                }
            }),
        ),
    };
    if proxy.send_event(UserEvent::Debug(DebugJob::new(false, job))).is_err() {
        return false;
    }
    if rx.recv_timeout(timeout).is_err() {
        eprintln!("{waiting_for}");
    }
    true
}

/// Spawns the stdin-reading thread for headless mode — one command per
/// line, forwarded to the app via `proxy` (the same `EventLoopProxy`
/// mechanism winit itself uses for user events, so commands land on the
/// real event loop like any other event). Stdin EOF (the driving script/
/// process closed its write end) sends `Quit` so a headless run driven by a
/// piped script exits cleanly instead of hanging forever.
pub fn spawn_stdin_reader(proxy: EventLoopProxy<UserEvent>) {
    std::thread::spawn(move || {
        // `--script <file>` reads the commands from a file instead of stdin,
        // for a run that has no stdin to pipe (the elevated `Samply` task).
        let args: Vec<String> = std::env::args().collect();
        let script = args.iter().position(|a| a == "--script").and_then(|i| args.get(i + 1));
        let input: Box<dyn std::io::BufRead> = match script {
            Some(path) => match std::fs::File::open(path) {
                Ok(file) => Box::new(std::io::BufReader::new(file)),
                Err(e) => {
                    eprintln!("--script {path}: {e}");
                    let _ = proxy.send_event(UserEvent::Debug(DebugJob::quit()));
                    return;
                }
            },
            None => Box::new(std::io::stdin().lock()),
        };
        for line in input.lines() {
            let Ok(line) = line else { break };
            match parse_line(&line) {
                Some(Line::Reader(op)) => {
                    if !run_reader_op(&proxy, op) {
                        break;
                    }
                }
                Some(Line::Run(job)) => {
                    if proxy.send_event(UserEvent::Debug(job)).is_err() {
                        break;
                    }
                }
                // A blank line (an artifact of the stream itself, not
                // something anyone typed) stays silent; anything else that
                // failed to parse gets a visible error instead of a
                // same-as-success no-op, so a typo doesn't read as "this
                // command does nothing."
                None if !line.trim().is_empty() => {
                    eprintln!("Unrecognized debug command: {line:?} -- try HELP");
                }
                None => {}
            }
        }
        let _ = proxy.send_event(UserEvent::Debug(DebugJob::quit()));
    });
}

/// Senders of the headless waits (`WAITTABS`, `WAITIDLE`, `WAITINDEX`) parked until their condition holds.
#[derive(Default)]
pub(crate) struct Waiters(Vec<std::sync::mpsc::Sender<()>>);

impl Waiters {
    pub(crate) fn push(&mut self, waiter: std::sync::mpsc::Sender<()>) {
        self.0.push(waiter);
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Sends `()` to every waiter and forgets them.
    pub(crate) fn release(&mut self) {
        for waiter in self.0.drain(..) {
            let _ = waiter.send(());
        }
    }
}

/// The headless debug channel's pending work (`App::debug`): synthetic input, deferred quits and the capture/idle waiters.
#[derive(Default)]
pub(crate) struct HeadlessDebug {
    /// Key events from headless `ENTER`/`ESCAPE`/`ARROWDOWN`/`ARROWUP`,
    /// added to the next frame's egui input exactly like real key presses
    /// (see `run_egui_frame`), so they take the same code paths and, like
    /// a real key, last one frame whether or not anything reads them.
    pub(crate) debug_input_events: Vec<egui::Event>,
    /// Text from a headless `ADDRESSBAR`, typed (select-all, then the text)
    /// once the address bar actually has egui focus: the command requests
    /// focus through `pending_focus`, which only takes hold in
    /// the frame after, so typing waits for a frame where
    /// `address_bar.focused` is true.
    pub(crate) debug_address_bar_typing: Option<String>,
    /// Set by a headless `QUIT` that arrived while debug input above (or
    /// `pending_screenshot`) was still waiting for a frame — e.g. `ENTER`+`QUIT`,
    /// or `SCREENSHOT`+`QUIT`, sent back-to-back with no delay. Exiting
    /// immediately in that case could drop the pending command/screenshot
    /// before any frame ever consumed it (`event_loop.exit()` gives no
    /// guarantee a `RedrawRequested` runs first). Holds how many more frames
    /// to wait: `headless_end_of_frame` exits once nothing is pending, or when
    /// this runs out (an `ADDRESSBAR` needs two frames, focus then typing;
    /// a screenshot on a stalled surface might never land).
    pub(crate) quit_after_redraws: Option<u8>,
    /// Set by a headless `SCREENSHOT` command; consumed on the next
    /// `RedrawRequested` and passed into `gpu::GpuState::render` (`render_frame`).
    pub(crate) pending_screenshot: Option<String>,
    /// The stdin reader waiting on `pending_screenshot` (see
    /// `ReaderOp::Screenshot`); sent `()` once it's been attempted.
    pub(crate) screenshot_waiter: Option<std::sync::mpsc::Sender<()>>,
    /// "Print to PDF": (temp PNG, PDF): the capture `start_print_to_pdf`
    /// asked for, and where its PDF goes. Paired so an unrelated screenshot
    /// can't take the PDF target. Set alongside `pending_screenshot` so the
    /// GPU capture needn't know PDF exists; once that capture completes
    /// (`handle_screenshot_result`), the temp PNG is converted via
    /// `pdf::write_image_as_pdf` and deleted. `None` the rest of the time,
    /// including for an ordinary "Screenshot" button click.
    pub(crate) pending_print_to_pdf: Option<(String, String)>,
    /// The headless `WAITIDLE` commands waiting for `debug_idle`.
    pub(crate) idle_waiters: Waiters,
    /// The headless `WAITINDEX` commands waiting for `index_settled`.
    pub(crate) index_waiters: Waiters,
    /// Consecutive frame ends at which `debug_idle` held, while `idle_waiters` waits; `QUIET_FRAMES` releases them.
    pub(crate) idle_frames: u8,
    /// Whether the last frame's egui output asked for the next frame at once: an animation, a smooth scroll or a
    /// layout still settling (a new `Area` or `Grid` measures itself over several frames).
    pub(crate) egui_wants_frame: bool,
    /// The named controls the last frame drew (`identity::controls_in`), for `EXPORTSTATE`'s `controls`. Filled
    /// only while accesskit is on: always under `--headless` or `--debug-commands`.
    pub(crate) controls: Vec<crate::identity::DrawnControl>,
}

impl App {
    /// Runs one parsed debug line (`DebugJob`), after letting a finished view transition end: a transition only ends
    /// in a frame, and frames can stall for seconds on a busy GPU, so one that ran out long ago could still be `Some`
    /// here and make the ENTER/ESCAPE/arrow guards drop their key.
    pub(crate) fn run_debug_job(&mut self, event_loop: &ActiveEventLoop, job: DebugJob) {
        self.finish_transition_if_done();
        (job.run)(self, event_loop);
    }
}

/// A key press as egui receives it from a keyboard, for headless key
/// commands (see `App::debug_input_events`).
pub(crate) fn debug_key_event(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }
}
