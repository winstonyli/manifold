mod app;
mod client;
mod render_handler;

use std::sync::mpsc;
use std::time::Duration;

use cef::args::Args;
use cef::*;
use ipc_protocol::{EngineToHost, HostToEngine, TabState};

use app::{AppBuilder, EngineApp};
use client::ClientBuilder;
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
    let tab_id = std::env::var("ENGINE_TAB_ID").expect("ENGINE_TAB_ID env var required");
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
    let (mut host_to_engine, mut control) = ipc_protocol::transport::connect_local_pair(&tab_id)
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
        Some(&mut ClientBuilder::build(render_handler)),
        Some(&initial_url.as_str().into()),
        Some(&browser_settings),
        None,
        None,
    )
    .expect("create CEF browser");

    let mut current_url = initial_url;
    let mut buffer_size = (DEFAULT_WIDTH, DEFAULT_HEIGHT);
    let mut frame_buffer = ipc_protocol::SharedFrameBuffer::create(&tab_id, DEFAULT_WIDTH, DEFAULT_HEIGHT)
        .expect("create shared frame buffer");
    let mut frame_id: u64 = 0;

    loop {
        do_message_loop_work();

        while let Ok(msg) = rx.try_recv() {
            match msg {
                HostToEngine::Navigate { url, .. } => {
                    if let Some(frame) = browser.main_frame() {
                        frame.load_url(Some(&url.as_str().into()));
                        current_url = url;
                    }
                }
                HostToEngine::LoadHistoryEntry { index } => {
                    // TODO(M4): same deferral as engine-host-servo — Browser
                    // only exposes relative go_back/go_forward here.
                    let _ = index;
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
                HostToEngine::Input(_event) => {
                    // TODO(M2 follow-up): map ipc_protocol::InputEvent to
                    // CEF's send_mouse_move_event/send_mouse_click_event/
                    // send_mouse_wheel_event on browser.host() — deferred
                    // until the swap sequence itself is verified working,
                    // matching engine-host-servo's original M1 sequencing.
                }
                HostToEngine::SetZoom { .. } => {}
                HostToEngine::GetState => {
                    let _ = control.send(&EngineToHost::StateSnapshot(TabState {
                        url: current_url.clone(),
                        history: vec![],
                        zoom: 1.0,
                    }));
                }
                HostToEngine::Shutdown => {
                    cef::shutdown();
                    return;
                }
            }
        }

        if let Some(frame) = latest_frame.borrow_mut().take() {
            if buffer_size != (frame.width, frame.height) {
                drop(frame_buffer);
                frame_buffer =
                    ipc_protocol::SharedFrameBuffer::create(&tab_id, frame.width, frame.height)
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

        std::thread::sleep(Duration::from_millis(8));
    }
}
