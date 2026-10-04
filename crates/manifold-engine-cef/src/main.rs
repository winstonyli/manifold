mod app;
mod client;
mod context_menu_handler;
mod download_handler;
mod find_handler;
mod input;
mod jar_cookies;
mod life_span_handler;
mod nav_handler;
mod render_handler;
mod render_process_handler;

use std::time::Duration;

use cef::args::Args;
use cef::*;
use ipc_protocol::{EngineToHost, HostToEngine, InputEvent, TabState};

use app::{AppBuilder, EngineApp};
use client::{
    new_shared_audio_playing, new_shared_json_body, new_shared_page_state, new_shared_redirect_content_type,
    ClientBuilder,
};
use context_menu_handler::new_shared_context_menu_link;
use download_handler::new_shared_download_path;
use find_handler::new_shared_find_result;
use life_span_handler::{new_shared_closed, new_shared_pending_open_tab};
use nav_handler::{new_shared_muted, new_shared_nav_state};
use render_handler::EngineRenderHandler;
use render_process_handler::{CHECK_AUDIO_PLAYING, CHECK_PAGE_STATE};

/// How long a host started with `ENGINE_AWAIT_COOKIES` waits for manifold's `SetCookies` before navigating.
const COOKIE_SEED_WAIT: Duration = Duration::from_secs(5);
const DEFAULT_WIDTH: u32 = 1024;
const DEFAULT_HEIGHT: u32 = 768;
/// How often to poll for audio-playing state -- see `ipc_protocol::
/// EngineToHost::AudioPlayingChanged`'s own doc comment for why CEF needs a
/// poll at all instead of a real native event. A full second is generous
/// (nothing here needs sub-second precision) against the real cost of an
/// extra `V8Context::eval` round trip every tick.
const AUDIO_PLAYING_POLL_INTERVAL: Duration = Duration::from_millis(1000);

/// Same job-object setup `engine-host-webview2` already uses for its own
/// multi-process tree -- see this function's own call site for why.
#[cfg(windows)]
fn setup_kill_on_exit_job() -> windows::core::Result<windows::Win32::Foundation::HANDLE> {
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows::Win32::System::Threading::GetCurrentProcess;

    unsafe {
        let job = CreateJobObjectW(None, None)?;
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const std::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )?;
        AssignProcessToJobObject(job, GetCurrentProcess())?;
        Ok(job)
    }
}

fn main() {
    // Matches cefsimple's shared::load_cef(): initializes the CEF API
    // version before any other CEF call.
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);

    let args = Args::new();
    let cmd = args.as_cmd_line().expect("cef command line");
    let type_switch = CefString::from("type");
    let is_browser_process = cmd.has_switch(Some(&type_switch)) != 1;

    // Unlike cefsimple's shared::run_main() (which passes no App at all to
    // this first execute_process call, since its own App has nothing a
    // subprocess needs), this one now does need a real App here too: a
    // renderer subprocess is this same exe re-launched by CEF with
    // `--type=renderer`, and only the App passed to *this* call ever gets
    // asked for a `render_process_handler` (see `app.rs` and
    // `render_process_handler.rs`, added for CEF's side of JSON viewing).
    // Harmless for the actual browser-process invocation below despite
    // being the same instance -- CEF never calls that getter outside a
    // renderer subprocess.
    let mut cef_app = AppBuilder::build(EngineApp::new());
    let ret = execute_process(Some(args.as_main_args()), Some(&mut cef_app), std::ptr::null_mut());

    if !is_browser_process {
        // A CEF subprocess (renderer/gpu/utility/...): execute_process has
        // already run its whole lifetime by the time this returns. There is
        // no tab_id/initial_url to parse — those are only ever passed to
        // the top-level browser-process invocation that manifold spawns.
        std::process::exit(if ret >= 0 { 0 } else { 1 });
    }
    assert_eq!(ret, -1, "cannot execute browser process");

    // CEF's own renderer/GPU/utility subprocesses are spawned by CEF
    // itself, not tracked by anything else in this process -- a Job Object
    // with KILL_ON_JOB_CLOSE, assigned to this (browser) process only,
    // fixes that generally: Windows terminates every process still in the
    // job the moment this process's own job handle closes, on any exit
    // path (graceful, crashed, or force-killed), since child processes
    // created by a process already in a job join the same job
    // automatically. Same fix `engine-host-webview2` already applies for
    // its own equivalent multi-process tree; `_job`'s only purpose is to
    // stay alive for the whole process lifetime. Elsewhere there is no job;
    // on Linux CEF's subprocesses exited by themselves within 5 s of a
    // `kill -9` of this process (WSL2, 2026-10-02; the engine-install spec's
    // Platforms section).
    #[cfg(windows)]
    let _job = setup_kill_on_exit_job();

    // Read via env vars, not argv: this process's argv is CEF's own command
    // line (parsed above via Args::new()/as_cmd_line()) — mixing in our own
    // positional args risks CEF's Chromium-style parser misinterpreting
    // them as its own flags/URLs-to-open.
    // ENGINE_SOCKET_ID names our control pipes/shared memory — normally
    // equal to ENGINE_TAB_ID, but manifold may pass a different id if a
    // stale engine-host from a prior crashed run still held ENGINE_TAB_ID's
    // pipes (see process_manager::spawn_tab's fallback). Only this id
    // matters here; we never need ENGINE_TAB_ID itself.
    let socket_id = std::env::var("ENGINE_SOCKET_ID").expect("ENGINE_SOCKET_ID env var required");
    let initial_url = std::env::var("ENGINE_INITIAL_URL").unwrap_or_else(|_| "https://example.com".to_string());
    // ENGINE_CACHE_DIR is this process's own `root_cache_path`, fresh per
    // spawn and deleted afterwards (see process_manager::spawn_tab).
    // Chromium's process singleton lets one browser process at a time use a
    // given root, so CEF's machine-wide default would allow one CEF tab at
    // a time. `cache_path` stays empty: browsing data lives in memory, since
    // the directory doesn't outlive the tab anyway.
    let cache_dir = std::env::var("ENGINE_CACHE_DIR").expect("ENGINE_CACHE_DIR env var required");

    let settings = Settings {
        root_cache_path: cache_dir.as_str().into(),
        windowless_rendering_enabled: true as _,
        external_message_pump: true as _,
        // The `sandbox` cargo feature is off (it needs a separate bootstrap
        // process this project doesn't bundle), so CEF must be told not to
        // expect one, or it CHECK()-fails at startup. Renderers run
        // unsandboxed: README, Known limitations.
        no_sandbox: true as _,
        ..Default::default()
    };
    assert_eq!(
        initialize(Some(args.as_main_args()), Some(&settings), Some(&mut cef_app), std::ptr::null_mut(),),
        1,
        "cef initialize failed"
    );

    // Two one-directional control sockets back to manifold — same
    // convention as manifold-engine-servo (see ipc_protocol::transport).
    let (host_to_engine, mut control) =
        ipc_protocol::transport::connect_local_pair(&socket_id).expect("connect to manifold control sockets");
    let rx = ipc_protocol::transport::spawn_host_reader(host_to_engine);

    let (render_handler, size, latest_frame) = EngineRenderHandler::new(DEFAULT_WIDTH, DEFAULT_HEIGHT);
    let nav_state = new_shared_nav_state(initial_url.clone());
    let muted_state = new_shared_muted();
    let pending_open_tab = new_shared_pending_open_tab();
    let find_result = new_shared_find_result();
    let download_path = new_shared_download_path();
    let closed = new_shared_closed();
    let context_menu_link = new_shared_context_menu_link();
    let json_body = new_shared_json_body();
    let redirect_content_type = new_shared_redirect_content_type();
    let audio_playing = new_shared_audio_playing();
    let page_state = new_shared_page_state();
    let mut last_reported_audio_playing = false;
    let mut last_audio_poll = std::time::Instant::now();
    // Remembers the most recent `FindStart`'s term/case-sensitivity —
    // `BrowserHost::find` needs both repeated on every call, even a plain
    // `FindNext`/`FindPrevious` continuation of an already-running search.
    let mut find_term: Option<(String, bool)> = None;

    let window_info = WindowInfo {
        windowless_rendering_enabled: true as _,
        shared_texture_enabled: false as _,
        ..Default::default()
    };
    let browser_settings = BrowserSettings { windowless_frame_rate: 60, ..Default::default() };
    // Cookie-jar seeding: manifold sends `SetCookies` as its very first message to hosts started with
    // `ENGINE_AWAIT_COOKIES`. Wait for it (bounded) so the cookies are in place before the first navigation.
    // Any other first message is kept for the loop below rather than dropped.
    let mut first_message: Option<HostToEngine> = None;
    // Whether this host was seeded with cookies or has reported some (`jar_cookies::JarPull::poll`).
    let mut may_hold_cookies = false;
    if std::env::var_os("ENGINE_AWAIT_COOKIES").is_some() {
        match ipc_protocol::transport::wait_for_first_message(&rx, COOKIE_SEED_WAIT, do_message_loop_work) {
            Some(HostToEngine::SetCookies(json)) => may_hold_cookies |= jar_cookies::seed(&json) > 0,
            Some(other) => first_message = Some(other),
            None => eprintln!("jar: no seed received"),
        }
    }
    let mut pull: Option<jar_cookies::JarPull> = None;

    let browser = browser_host_create_browser_sync(
        Some(&window_info),
        Some(&mut ClientBuilder::build(
            render_handler,
            nav_state.clone(),
            muted_state.clone(),
            pending_open_tab.clone(),
            find_result.clone(),
            download_path.clone(),
            closed.clone(),
            context_menu_link.clone(),
            json_body.clone(),
            redirect_content_type.clone(),
            audio_playing.clone(),
            page_state.clone(),
        )),
        Some(&initial_url.as_str().into()),
        Some(&browser_settings),
        None,
        None,
    )
    .expect("create CEF browser");

    let mut buffer_size = (DEFAULT_WIDTH, DEFAULT_HEIGHT);
    // `None` only while a recreate at a new size fails (retried on the next frame); the first one is fatal.
    let mut frame_buffer = Some(
        ipc_protocol::SharedFrameBuffer::create(&socket_id, DEFAULT_WIDTH, DEFAULT_HEIGHT)
            .expect("create shared frame buffer"),
    );
    let mut frame_id: u64 = 0;
    // M5: background tabs stay alive but stop sending frames; see
    // HostToEngine::SetVisible's doc comment.
    let mut visible = true;
    // Set once `HostToEngine::Shutdown` calls `close_browser` -- the loop
    // keeps pumping (`do_message_loop_work` below) until `closed` flips,
    // which is what actually drives `on_before_close` to fire, or until
    // `SHUTDOWN_GRACE` passes (`shutdown_due`).
    let mut shutting_down: Option<std::time::Instant> = None;

    loop {
        do_message_loop_work();

        while let Some(msg) = first_message.take().or_else(|| rx.try_recv().ok()) {
            match msg {
                HostToEngine::Navigate { url, .. } => {
                    nav_state.borrow_mut().label.host_started = Some((false, std::time::Instant::now()));
                    if let Some(frame) = browser.main_frame() {
                        frame.load_url(Some(&url.as_str().into()));
                    }
                }
                HostToEngine::GoBack => {
                    if browser.can_go_back() != 0 {
                        nav_state.borrow_mut().label.host_started = Some((true, std::time::Instant::now()));
                        browser.go_back();
                    }
                }
                HostToEngine::GoForward => {
                    if browser.can_go_forward() != 0 {
                        nav_state.borrow_mut().label.host_started = Some((true, std::time::Instant::now()));
                        browser.go_forward();
                    }
                }
                HostToEngine::Reload => {
                    browser.reload();
                }
                HostToEngine::Resize { width: w, height: h, .. } => {
                    *size.borrow_mut() = (w as f32, h as f32);
                    if let Some(host) = browser.host() {
                        host.was_resized();
                    }
                    // Note: this does NOT resize `frame_buffer` itself.
                    // CEF's next `on_paint` after `was_resized()` is what
                    // actually determines the real output size (it may
                    // still deliver one more frame at the *old* size first)
                    // — the buffer is resized lazily below, keyed off each
                    // painted frame's own reported dimensions, which is the
                    // only way to avoid writing/reporting mismatched sizes.
                }
                HostToEngine::Input(event) => {
                    if matches!(event, InputEvent::MouseDown { .. } | InputEvent::KeyDown { .. }) {
                        nav_state.borrow_mut().label.last_input = Some(std::time::Instant::now());
                    }
                    input::send_input(&browser, event);
                }
                HostToEngine::SetZoom { factor } => {
                    // CEF's own `zoom_level` is log-scale (Chromium's
                    // internal representation, each whole step ~20%
                    // larger/smaller), not a linear multiplier like
                    // WebKit's/Servo's `factor` — convert so the same
                    // `factor` value means the same visual result on every
                    // engine: `zoom_level = log_1.2(factor)`.
                    if let Some(host) = browser.host() {
                        host.set_zoom_level(factor.max(0.01).ln() / 1.2_f64.ln());
                    }
                }
                HostToEngine::SetVisible(v) => {
                    visible = v;
                }
                HostToEngine::GetState => {
                    let _ =
                        control.send(&EngineToHost::StateSnapshot(TabState { url: nav_state.borrow().url.clone() }));
                }
                HostToEngine::SetCookies(json) => may_hold_cookies |= jar_cookies::seed(&json) > 0,
                HostToEngine::GetCookies => pull = Some(jar_cookies::start_pull()),
                HostToEngine::QueryPageState => {
                    if let Some(frame) = browser.main_frame() {
                        if let Some(mut message) = process_message_create(Some(&CHECK_PAGE_STATE.into())) {
                            frame.send_process_message(ProcessId::RENDERER, Some(&mut message));
                        }
                    }
                }
                HostToEngine::RestoreScroll { x, y } => {
                    if let Some(frame) = browser.main_frame() {
                        frame.execute_java_script(
                            Some(&ipc_protocol::restore_scroll_script(x, y).as_str().into()),
                            None,
                            0,
                        );
                    }
                }
                HostToEngine::FindStart { text, match_case } => {
                    if let Some(host) = browser.host() {
                        host.find(Some(&text.as_str().into()), true as _, match_case as _, false as _);
                    }
                    find_term = Some((text, match_case));
                }
                HostToEngine::FindNext => {
                    if let (Some(host), Some((text, match_case))) = (browser.host(), &find_term) {
                        host.find(Some(&text.as_str().into()), true as _, *match_case as _, true as _);
                    }
                }
                HostToEngine::FindPrevious => {
                    if let (Some(host), Some((text, match_case))) = (browser.host(), &find_term) {
                        host.find(Some(&text.as_str().into()), false as _, *match_case as _, true as _);
                    }
                }
                HostToEngine::FindStop => {
                    find_term = None;
                    if let Some(host) = browser.host() {
                        host.stop_finding(true as _);
                    }
                }
                HostToEngine::OpenDevTools => {
                    // All `None`: a default (real, windowed, OS-visible)
                    // `WindowInfo` rather than inheriting this browser's own
                    // `windowless_rendering_enabled` client/settings --
                    // DevTools is meant to be an actual on-screen window,
                    // not something this project's off-screen capture
                    // pipeline should try to composite.
                    if let Some(host) = browser.host() {
                        host.show_dev_tools(None, None, None, None);
                    }
                }
                // A real native mute would still mean the full `AudioHandler`
                // `ipc_protocol::HostToEngine::SetMuted`'s own doc comment
                // rules out -- but muting is a *write*, unlike the audio-
                // playing indicator's read, so it doesn't need a value back
                // at all: `Frame::execute_java_script` (fire-and-forget,
                // already available on every engine) just sets `.muted` on
                // every `<video>`/`<audio>` element directly. Same
                // media-element-only approximation the audio-playing check
                // below uses -- misses audio from the Web Audio API alone
                // with no element behind it, but covers the common case
                // (YouTube, most players) a `<video>`/`<audio>` tag actually
                // backs.
                HostToEngine::SetMuted { muted } => {
                    // Recorded so `nav_handler`'s `on_loading_state_change`
                    // can re-apply it on every future navigation -- this
                    // JS write only ever affects elements that already
                    // exist right now, unlike WebView2/WebKit's real
                    // view-level mute property.
                    muted_state.set(muted);
                    if let Some(frame) = browser.main_frame() {
                        let code = format!(
                            "document.querySelectorAll('video,audio').forEach(function(el){{el.muted={muted};}});"
                        );
                        frame.execute_java_script(Some(&code.as_str().into()), None, 0);
                    }
                }
                HostToEngine::Shutdown => {
                    // Must call `close_browser` and wait for
                    // `LifeSpanHandler::on_before_close` before
                    // `cef::shutdown()` -- CEF's own documented contract.
                    // Calling `cef::shutdown()` immediately, with the
                    // browser/renderer still alive, risks a crash or hang
                    // inside libcef itself. `force_close: true` since
                    // there's nothing here (no unsaved-changes prompt) that
                    // would need a graceful, cancelable close.
                    if let Some(host) = browser.host() {
                        host.close_browser(true as _);
                    }
                    // A repeated `Shutdown` does not restart the clock.
                    shutting_down.get_or_insert_with(std::time::Instant::now);
                }
            }
        }

        match pull.as_ref().map(|p| p.poll(may_hold_cookies)) {
            Some(jar_cookies::PullState::Answer(json)) => {
                let n = serde_json::from_str::<Vec<serde_json::Value>>(&json).map_or(0, |v| v.len());
                eprintln!("jar: reported {n} cookies");
                may_hold_cookies |= n > 0;
                let _ = control.send(&EngineToHost::Cookies(json));
                pull = None;
            }
            Some(jar_cookies::PullState::Silent) => {
                eprintln!("jar: no complete cookie read; not answering");
                pull = None;
            }
            Some(jar_cookies::PullState::Pending) | None => {}
        }

        {
            let mut nav = nav_state.borrow_mut();
            if nav.dirty {
                nav.dirty = false;
                let _ = control.send(&EngineToHost::NavigationStateChanged {
                    url: nav.url.clone(),
                    title: nav.title.clone(),
                    can_go_back: nav.can_go_back,
                    can_go_forward: nav.can_go_forward,
                    loading: nav.loading,
                    user_started: nav.label.user_started,
                });
                if std::mem::take(&mut nav.label.finished) {
                    nav.label.user_started = None;
                }
            }
        }

        for (url, user_gesture) in pending_open_tab.borrow_mut().drain(..) {
            let _ = control.send(&EngineToHost::OpenTabRequested { url, user_gesture });
        }

        if let Some((count, active_index)) = find_result.borrow_mut().take() {
            let _ = control.send(&EngineToHost::FindResult { count, active_index });
        }

        if let Some((x, y, href)) = context_menu_link.borrow_mut().take() {
            let _ = control.send(&EngineToHost::ContextMenuLink { x, y, href });
        }

        if let Some(body) = json_body.borrow_mut().take() {
            let _ = control.send(&EngineToHost::JsonBody { body });
        }

        if let Some(content_type) = redirect_content_type.borrow_mut().take() {
            let _ = control.send(&EngineToHost::RedirectableContent { content_type });
        }

        if let Some(text) = page_state.borrow_mut().take() {
            if let Some(state) = ipc_protocol::PageState::parse(&text) {
                let _ = control.send(&EngineToHost::PageState(state));
            }
        }

        // See `AUDIO_PLAYING_POLL_INTERVAL`'s own doc comment for why this
        // is a poll, not a real event -- fires regardless of `visible`
        // (background-tab audio still needs an accurate indicator, same as
        // every other engine's own passive native read).
        if last_audio_poll.elapsed() >= AUDIO_PLAYING_POLL_INTERVAL {
            last_audio_poll = std::time::Instant::now();
            if let Some(frame) = browser.main_frame() {
                if let Some(mut message) = process_message_create(Some(&CHECK_AUDIO_PLAYING.into())) {
                    frame.send_process_message(ProcessId::RENDERER, Some(&mut message));
                }
            }
        }
        // Only sent on an actual change, matching `EngineToHost::
        // AudioPlayingChanged`'s own documented contract -- `audio_playing`
        // itself is overwritten on every reply regardless (see
        // `SharedAudioPlaying`'s own doc comment), so the dedupe has to
        // happen here, not there.
        if let Some(playing) = *audio_playing.borrow() {
            if playing != last_reported_audio_playing {
                last_reported_audio_playing = playing;
                let _ = control.send(&EngineToHost::AudioPlayingChanged { playing });
            }
        }

        let download_msgs = std::mem::take(&mut download_path.borrow_mut().queue);
        for msg in ipc_protocol::downloads::coalesce_progress(download_msgs) {
            let _ = control.send(&msg);
        }

        if visible {
            // Taken in its own statement: the `RefMut` would otherwise live through the block (edition 2021),
            // and putting a frame back for the retry below would panic on a second borrow.
            let painted = latest_frame.borrow_mut().take();
            if let Some(frame) = painted {
                if frame_buffer.is_none() || buffer_size != (frame.width, frame.height) {
                    // Unmapped before `create` truncates the file by name.
                    drop(frame_buffer.take());
                    frame_buffer = match ipc_protocol::SharedFrameBuffer::create(&socket_id, frame.width, frame.height)
                    {
                        Ok(buffer) => Some(buffer),
                        Err(e) => {
                            // The host still maps the old file (stalled on upload): retried on the next frame.
                            eprintln!(
                                "frame buffer: recreate at {}x{} failed, retrying: {e}",
                                frame.width, frame.height
                            );
                            None
                        }
                    };
                    buffer_size = (frame.width, frame.height);
                }
                if let Some(buffer) = frame_buffer.as_mut() {
                    buffer.write_frame(&frame.bgra);
                    frame_id += 1;
                    let _ = control.send(&EngineToHost::FrameReady {
                        frame_id,
                        width: frame.width,
                        height: frame.height,
                        damage: None,
                        pixels: None,
                    });
                } else {
                    // Kept for the retry unless a newer paint already replaced it.
                    latest_frame.borrow_mut().get_or_insert(frame);
                }
            }
        }

        if let Some(since) = shutting_down {
            match shutdown_due(since.elapsed(), *closed.borrow()) {
                Some(ShutdownPath::Clean) => {
                    cef::shutdown();
                    return;
                }
                Some(ShutdownPath::Forced) => {
                    // `cef::shutdown()` with the browser still alive can hang inside libcef:
                    // leave instead, and let the job object take the subprocesses.
                    eprintln!("shutdown: no on_before_close after {SHUTDOWN_GRACE:?}; exiting");
                    std::process::exit(0);
                }
                None => {}
            }
        }

        std::thread::sleep(Duration::from_millis(8));
    }
}

/// How long a `Shutdown` waits for `on_before_close` before the host exits
/// anyway. It matters when manifold died: `spawn_host_reader` synthesizes
/// the `Shutdown`, and a host that waited for ever would be the orphan.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);

#[derive(Debug, PartialEq, Eq)]
enum ShutdownPath {
    /// The browser closed: CEF's documented order, then `cef::shutdown()`.
    Clean,
    /// It never did within `SHUTDOWN_GRACE`.
    Forced,
}

/// Which way a host that got `Shutdown` `elapsed` ago leaves, if it leaves yet: cleanly once the browser `closed`,
/// forced once `SHUTDOWN_GRACE` passed without that.
fn shutdown_due(elapsed: Duration, closed: bool) -> Option<ShutdownPath> {
    if closed {
        Some(ShutdownPath::Clean)
    } else if elapsed >= SHUTDOWN_GRACE {
        Some(ShutdownPath::Forced)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shutdown_waits_for_the_close_then_gives_up() {
        assert_eq!(shutdown_due(Duration::ZERO, true), Some(ShutdownPath::Clean));
        assert_eq!(shutdown_due(Duration::from_secs(1), false), None);
        assert_eq!(shutdown_due(SHUTDOWN_GRACE, false), Some(ShutdownPath::Forced));
    }
}
