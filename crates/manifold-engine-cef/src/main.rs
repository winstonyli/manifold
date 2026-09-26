mod app;
mod client;
mod context_menu_handler;
mod download_handler;
mod find_handler;
mod input;
mod life_span_handler;
mod nav_handler;
mod render_handler;
mod render_process_handler;

use std::time::Duration;

use cef::args::Args;
use cef::*;
use ipc_protocol::{EngineToHost, HostToEngine, TabState};

use app::{AppBuilder, EngineApp};
use client::{new_shared_audio_playing, new_shared_json_body, new_shared_redirect_content_type, ClientBuilder};
use context_menu_handler::new_shared_context_menu_link;
use download_handler::new_shared_download_path;
use find_handler::new_shared_find_result;
use life_span_handler::{new_shared_closed, new_shared_pending_open_tab};
use nav_handler::{new_shared_muted, new_shared_nav_state};
use render_handler::EngineRenderHandler;
use render_process_handler::CHECK_AUDIO_PLAYING;

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
    // stay alive for the whole process lifetime.
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

    let settings = Settings {
        windowless_rendering_enabled: true as _,
        external_message_pump: true as _,
        // We disabled the `sandbox` cargo feature (it needs a separate
        // bootstrap-process bundling step we're not doing), so CEF must be
        // told explicitly not to expect one — otherwise it CHECK()-fails
        // almost immediately trying to initialize sandbox-dependent state.
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
        )),
        Some(&initial_url.as_str().into()),
        Some(&browser_settings),
        None,
        None,
    )
    .expect("create CEF browser");

    let mut buffer_size = (DEFAULT_WIDTH, DEFAULT_HEIGHT);
    let mut frame_buffer = ipc_protocol::SharedFrameBuffer::create(&socket_id, DEFAULT_WIDTH, DEFAULT_HEIGHT)
        .expect("create shared frame buffer");
    let mut frame_id: u64 = 0;
    // M5: background tabs stay alive but stop sending frames; see
    // HostToEngine::SetVisible's doc comment.
    let mut visible = true;
    // Set once `HostToEngine::Shutdown` calls `close_browser` -- the loop
    // keeps pumping (`do_message_loop_work` below) until `closed` flips,
    // which is what actually drives `on_before_close` to fire.
    let mut shutting_down = false;

    loop {
        do_message_loop_work();

        while let Ok(msg) = rx.try_recv() {
            match msg {
                HostToEngine::Navigate { url, .. } => {
                    if let Some(frame) = browser.main_frame() {
                        frame.load_url(Some(&url.as_str().into()));
                    }
                }
                HostToEngine::GoBack => {
                    if browser.can_go_back() != 0 {
                        browser.go_back();
                    }
                }
                HostToEngine::GoForward => {
                    if browser.can_go_forward() != 0 {
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
                    let _ = control.send(&EngineToHost::StateSnapshot(TabState {
                        url: nav_state.borrow().url.clone(),
                        zoom: 1.0,
                    }));
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
                    shutting_down = true;
                }
            }
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
                });
            }
        }

        for url in pending_open_tab.borrow_mut().drain(..) {
            let _ = control.send(&EngineToHost::OpenTabRequested { url });
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

        for event in download_path.borrow_mut().queue.drain(..) {
            let msg = match event {
                download_handler::DownloadEvent::Started { path, total_bytes } => {
                    EngineToHost::DownloadStarted { path, total_bytes }
                }
                download_handler::DownloadEvent::Progress { path, bytes_received } => {
                    EngineToHost::DownloadProgress { path, bytes_received }
                }
                download_handler::DownloadEvent::Completed { path } => EngineToHost::DownloadCompleted { path },
            };
            let _ = control.send(&msg);
        }

        if visible {
            if let Some(frame) = latest_frame.borrow_mut().take() {
                if buffer_size != (frame.width, frame.height) {
                    drop(frame_buffer);
                    frame_buffer = ipc_protocol::SharedFrameBuffer::create(&socket_id, frame.width, frame.height)
                        .expect("recreate shared frame buffer to match painted frame size");
                    buffer_size = (frame.width, frame.height);
                }
                frame_buffer.write_frame(&frame.bgra);
                frame_id += 1;
                let _ = control.send(&EngineToHost::FrameReady {
                    frame_id,
                    width: frame.width,
                    height: frame.height,
                    damage: None,
                    pixels: None,
                });
            }
        }

        if shutting_down && *closed.borrow() {
            cef::shutdown();
            return;
        }

        std::thread::sleep(Duration::from_millis(8));
    }
}
