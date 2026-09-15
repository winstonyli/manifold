mod app;
mod client;
mod find_handler;
mod input;
mod life_span_handler;
mod nav_handler;
mod render_handler;

use std::sync::mpsc;
use std::time::Duration;

use cef::args::Args;
use cef::*;
use ipc_protocol::{EngineToHost, HostToEngine, TabState};

use app::{AppBuilder, EngineApp};
use client::ClientBuilder;
use find_handler::new_shared_find_result;
use life_span_handler::new_shared_pending_open_tab;
use nav_handler::{new_shared_favicon_url, new_shared_nav_state};
use render_handler::EngineRenderHandler;

const DEFAULT_WIDTH: u32 = 1024;
const DEFAULT_HEIGHT: u32 = 768;

fn main() {
    // Matches cefsimple's shared::load_cef(): initializes the CEF API
    // version before any other CEF call.
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);

    let args = Args::new();
    let cmd = args.as_cmd_line().expect("cef command line");
    let type_switch = CefString::from("type");
    let is_browser_process = cmd.has_switch(Some(&type_switch)) != 1;

    // Matches cefsimple's shared::run_main(): the first execute_process
    // call passes no App at all (None) — the real App is only handed to
    // initialize(), below, for the actual browser process.
    let ret = execute_process(Some(args.as_main_args()), None, std::ptr::null_mut());

    if !is_browser_process {
        // A CEF subprocess (renderer/gpu/utility/...): execute_process has
        // already run its whole lifetime by the time this returns. There is
        // no tab_id/initial_url to parse — those are only ever passed to
        // the top-level browser-process invocation that browser-ui spawns.
        std::process::exit(if ret >= 0 { 0 } else { 1 });
    }
    assert_eq!(ret, -1, "cannot execute browser process");

    // Read via env vars, not argv: this process's argv is CEF's own command
    // line (parsed above via Args::new()/as_cmd_line()) — mixing in our own
    // positional args risks CEF's Chromium-style parser misinterpreting
    // them as its own flags/URLs-to-open.
    // ENGINE_SOCKET_ID names our control pipes/shared memory — normally
    // equal to ENGINE_TAB_ID, but browser-ui may pass a different id if a
    // stale engine-host from a prior crashed run still held ENGINE_TAB_ID's
    // pipes (see process_manager::spawn_tab's fallback). Only this id
    // matters here; we never need ENGINE_TAB_ID itself.
    let socket_id = std::env::var("ENGINE_SOCKET_ID").expect("ENGINE_SOCKET_ID env var required");
    let initial_url = std::env::var("ENGINE_INITIAL_URL")
        .unwrap_or_else(|_| "https://example.com".to_string());

    let mut cef_app = AppBuilder::build(EngineApp::new());
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
        initialize(
            Some(args.as_main_args()),
            Some(&settings),
            Some(&mut cef_app),
            std::ptr::null_mut(),
        ),
        1,
        "cef initialize failed"
    );

    // Two one-directional control sockets back to browser-ui — same
    // convention as engine-host-servo (see ipc_protocol::transport).
    let (mut host_to_engine, mut control) =
        ipc_protocol::transport::connect_local_pair(&socket_id)
            .expect("connect to browser-ui control sockets");
    let (tx, rx) = mpsc::channel::<HostToEngine>();
    std::thread::spawn(move || loop {
        match host_to_engine.recv::<HostToEngine>() {
            Ok(msg) => {
                let is_shutdown = matches!(msg, HostToEngine::Shutdown);
                if tx.send(msg).is_err() || is_shutdown {
                    break;
                }
            }
            Err(_) => break,
        }
    });

    let (render_handler, size, latest_frame) =
        EngineRenderHandler::new(DEFAULT_WIDTH, DEFAULT_HEIGHT);
    let nav_state = new_shared_nav_state(initial_url.clone());
    let pending_open_tab = new_shared_pending_open_tab();
    let favicon_url = new_shared_favicon_url();
    let find_result = new_shared_find_result();
    // Remembers the most recent `FindStart`'s term/case-sensitivity —
    // `BrowserHost::find` needs both repeated on every call, even a plain
    // `FindNext`/`FindPrevious` continuation of an already-running search.
    let mut find_term: Option<(String, bool)> = None;

    let window_info = WindowInfo {
        windowless_rendering_enabled: true as _,
        shared_texture_enabled: false as _,
        ..Default::default()
    };
    let browser_settings = BrowserSettings {
        windowless_frame_rate: 60,
        ..Default::default()
    };
    let browser = browser_host_create_browser_sync(
        Some(&window_info),
        Some(&mut ClientBuilder::build(
            render_handler,
            nav_state.clone(),
            pending_open_tab.clone(),
            favicon_url.clone(),
            find_result.clone(),
        )),
        Some(&initial_url.as_str().into()),
        Some(&browser_settings),
        None,
        None,
    )
    .expect("create CEF browser");

    let mut buffer_size = (DEFAULT_WIDTH, DEFAULT_HEIGHT);
    let mut frame_buffer =
        ipc_protocol::SharedFrameBuffer::create(&socket_id, DEFAULT_WIDTH, DEFAULT_HEIGHT)
            .expect("create shared frame buffer");
    let mut frame_id: u64 = 0;
    // M5: background tabs stay alive but stop sending frames; see
    // HostToEngine::SetVisible's doc comment.
    let mut visible = true;

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
                HostToEngine::Resize {
                    width: w,
                    height: h,
                    ..
                } => {
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
                HostToEngine::Shutdown => {
                    cef::shutdown();
                    return;
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

        if let Some(url) = pending_open_tab.borrow_mut().take() {
            let _ = control.send(&EngineToHost::OpenTabRequested { url });
        }

        if let Some((count, active_index)) = find_result.borrow_mut().take() {
            let _ = control.send(&EngineToHost::FindResult { count, active_index });
        }

        // Blocking HTTP fetch + decode, right here in the tick loop rather
        // than a background thread: favicon changes are rare (once or
        // twice per real navigation, not per frame) and the URL itself
        // already went through a short deny-list of untrusted response
        // sizes/timeouts below, so an occasional few-hundred-ms stall in
        // frame capture/input handling here is an acceptable tradeoff
        // against adding real async/thread-pool infrastructure for it.
        if let Some(url) = favicon_url.borrow_mut().take() {
            if let Some((rgba, width, height)) = fetch_favicon(&url) {
                let _ = control.send(&EngineToHost::FaviconChanged { rgba, width, height });
            }
        }

        if visible {
            if let Some(frame) = latest_frame.borrow_mut().take() {
                if buffer_size != (frame.width, frame.height) {
                    drop(frame_buffer);
                    frame_buffer = ipc_protocol::SharedFrameBuffer::create(
                        &socket_id,
                        frame.width,
                        frame.height,
                    )
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

        std::thread::sleep(Duration::from_millis(8));
    }
}

/// CEF's `DisplayHandler::on_favicon_urlchange` only ever hands over a
/// URL (unlike Servo/WebKit's `favicon()` getters, which hand over
/// already-decoded pixels) — fetch it over HTTP and decode it ourselves.
/// A short timeout and a response-size cap keep a slow or hostile favicon
/// URL from blocking this process's tick loop for long or exhausting
/// memory; any failure (network, decode, oversized) just means no favicon
/// for this change, not a crash — favicons are a cosmetic nice-to-have,
/// never load-bearing.
fn fetch_favicon(url: &str) -> Option<(Vec<u8>, u32, u32)> {
    let response = ureq::get(url)
        .config()
        .timeout_global(Some(Duration::from_secs(3)))
        .build()
        .call()
        .ok()?;
    let bytes = response
        .into_body()
        .with_config()
        .limit(2_000_000)
        .read_to_vec()
        .ok()?;
    let image = image::load_from_memory(&bytes).ok()?.into_rgba8();
    let (width, height) = (image.width(), image.height());
    Some((image.into_raw(), width, height))
}
