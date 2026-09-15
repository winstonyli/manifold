//! WebView2 engine-host, Phase 1 (see the M6 plan): IPC + navigation state
//! wired up fully; frame capture is a fixed solid-color placeholder until
//! the real `Windows.Graphics.Capture` pipeline lands in Phase 2 (see
//! `capture.rs`'s own doc comment once that file exists). Structured to
//! mirror `engine-host-cef/src/main.rs`'s shape (fixed-tick poll loop,
//! `SharedNavState` + `dirty` flag) since CEF is the closest existing
//! analog: a real Windows process, no visible window, same-machine IPC.

mod nav_state;
mod window;

use std::sync::mpsc;
use std::time::Duration;

use ipc_protocol::{transport, EngineToHost, HostToEngine, SharedFrameBuffer};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2Controller, ICoreWebView2Environment,
};
use webview2_com::CreateCoreWebView2EnvironmentCompletedHandler;
use windows::core::{BOOL, HSTRING};
use windows::Win32::Foundation::RECT;

use nav_state::new_shared_nav_state;

const DEFAULT_WIDTH: u32 = 1024;
const DEFAULT_HEIGHT: u32 = 768;

fn main() {
    // WebView2 is fundamentally a COM component -- unlike CEF/Servo (which
    // never touch raw COM from the Rust side), every `ICoreWebView2*` call
    // here needs an initialized apartment on this thread first, or every
    // single one fails with CO_E_NOTINITIALIZED (confirmed live: exactly
    // that HRESULT, 0x800401F0, on the very first environment-creation
    // call). Apartment-threaded (STA), not multi-threaded, since WebView2
    // is a UI-affine component tied to the thread that pumps its message
    // loop (see window::pump_messages, called from this same thread below).
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
        );
    }

    let socket_id = std::env::var("ENGINE_SOCKET_ID").expect("ENGINE_SOCKET_ID env var required");
    let initial_url =
        std::env::var("ENGINE_INITIAL_URL").unwrap_or_else(|_| "https://example.com".to_string());

    // Same convention as engine-host-cef/-servo: browser-ui already created
    // both named pipes before spawning us (listen_local_pair), we just
    // connect. `host_to_engine`/`control` are one-directional (HostToEngine
    // in, EngineToHost out) -- see transport.rs's own doc comment.
    let (mut host_to_engine, mut control) =
        transport::connect_local_pair(&socket_id).expect("connect to browser-ui");
    let (tx, rx) = mpsc::channel::<HostToEngine>();
    std::thread::spawn(move || loop {
        match host_to_engine.recv::<HostToEngine>() {
            Ok(msg) => {
                let is_shutdown = matches!(msg, HostToEngine::Shutdown);
                if tx.send(msg).is_err() || is_shutdown {
                    return;
                }
            }
            Err(_) => return,
        }
    });

    // Phase 1 deliberately uses plain *windowed* hosting (`CreateCoreWebView2Controller`)
    // against a real, off-screen, invisible window -- the exact same trick
    // browser-ui's own headless mode already uses successfully on this
    // machine (position far off-screen, no WS_VISIBLE, skip the taskbar).
    // Composition/Visual hosting (needed for the real Windows.Graphics.Capture
    // pipeline) is deferred to Phase 2 along with capture.rs -- switching the
    // controller-creation call is expected, bounded rework once that phase
    // starts, and far lower-risk than betting Phase 1's buildability on an
    // also-unproven composition-without-a-visual-target configuration.
    let hwnd = window::create_hidden_window().expect("create hidden host window");

    let environment = create_environment().expect("create CoreWebView2Environment");
    let (controller, webview) =
        create_controller(environment, hwnd).expect("create CoreWebView2Controller");

    resize_controller(&controller, DEFAULT_WIDTH, DEFAULT_HEIGHT);

    let nav_state = new_shared_nav_state(initial_url.clone());
    nav_state::register_handlers(&webview, nav_state.clone());

    unsafe {
        webview
            .Navigate(&HSTRING::from(&initial_url))
            .expect("initial navigate");
    }

    // Phase 1 placeholder: a fixed mid-gray frame, sent once so browser-ui
    // has *something* to display and the tab UI (title/loading spinner/
    // engine badge) can be fully exercised without the real capture
    // pipeline (Phase 2) existing yet.
    let mut frame_buffer = SharedFrameBuffer::create(&socket_id, DEFAULT_WIDTH, DEFAULT_HEIGHT)
        .expect("create shared frame buffer");
    let placeholder = vec![60u8; (DEFAULT_WIDTH * DEFAULT_HEIGHT * 4) as usize];
    let mut frame_id: u64 = 0;
    let mut visible = true;
    let mut sent_placeholder = false;

    loop {
        window::pump_messages();

        while let Ok(msg) = rx.try_recv() {
            match msg {
                HostToEngine::Navigate { url, .. } => unsafe {
                    let _ = webview.Navigate(&HSTRING::from(&url));
                },
                HostToEngine::GoBack => unsafe {
                    let mut can_go = BOOL(0);
                    let _ = webview.CanGoBack(&mut can_go);
                    if can_go.as_bool() {
                        let _ = webview.GoBack();
                    }
                },
                HostToEngine::GoForward => unsafe {
                    let mut can_go = BOOL(0);
                    let _ = webview.CanGoForward(&mut can_go);
                    if can_go.as_bool() {
                        let _ = webview.GoForward();
                    }
                },
                HostToEngine::Resize { width, height, .. } => {
                    resize_controller(&controller, width, height);
                    // Real resize handling (recreating the capture frame
                    // pool at the new size) lands in Phase 2 -- the
                    // placeholder buffer intentionally stays fixed-size.
                }
                HostToEngine::Input(_event) => {
                    // Phase 3 (see the M6 plan) -- input dispatch needs
                    // real rendered content to verify against, done after
                    // Phase 2's capture pipeline, not before.
                }
                HostToEngine::SetZoom { factor } => unsafe {
                    // Linear, unlike CEF's log-scale zoom_level -- direct
                    // match to this project's own `factor` semantics.
                    let _ = controller.SetZoomFactor(factor);
                },
                HostToEngine::SetVisible(v) => {
                    visible = v;
                }
                HostToEngine::GetState => {
                    let url = nav_state.borrow().url.clone();
                    let _ = control.send(&EngineToHost::StateSnapshot(ipc_protocol::TabState {
                        url,
                        zoom: 1.0,
                    }));
                }
                HostToEngine::Shutdown => {
                    unsafe {
                        let _ = controller.Close();
                    }
                    return;
                }
            }
        }

        {
            let mut state = nav_state.borrow_mut();
            if state.dirty {
                state.dirty = false;
                let _ = control.send(&EngineToHost::NavigationStateChanged {
                    url: state.url.clone(),
                    title: state.title.clone(),
                    can_go_back: state.can_go_back,
                    can_go_forward: state.can_go_forward,
                    loading: state.loading,
                });
            }
        }

        if visible && !sent_placeholder {
            frame_buffer.write_frame(&placeholder);
            frame_id += 1;
            let _ = control.send(&EngineToHost::FrameReady {
                frame_id,
                width: DEFAULT_WIDTH,
                height: DEFAULT_HEIGHT,
                damage: None,
                pixels: None,
            });
            sent_placeholder = true;
        }

        std::thread::sleep(Duration::from_millis(8));
    }
}

fn create_environment() -> webview2_com::Result<ICoreWebView2Environment> {
    let (tx, rx) = mpsc::channel();
    CreateCoreWebView2EnvironmentCompletedHandler::wait_for_async_operation(
        Box::new(|handler| unsafe {
            webview2_com::Microsoft::Web::WebView2::Win32::CreateCoreWebView2Environment(&handler)
                .map_err(webview2_com::Error::WindowsError)
        }),
        Box::new(move |error_code, environment| {
            error_code?;
            tx.send(environment.ok_or_else(|| {
                windows::core::Error::from(windows::Win32::Foundation::E_POINTER)
            }))
            .expect("send environment over mpsc channel");
            Ok(())
        }),
    )?;
    Ok(webview2_com::wait_with_pump(rx)??)
}

/// Returns `(controller, webview)` -- `ICoreWebView2Controller` is used for
/// bounds/visibility/zoom, `ICoreWebView2` (from `.CoreWebView2()`) for
/// navigation/events. Plain windowed hosting for Phase 1 (see main()'s doc
/// comment above `create_hidden_window` for why); Phase 2 swaps this for
/// `CreateCoreWebView2CompositionController`/`ICoreWebView2CompositionController`
/// once the real capture pipeline needs a composition visual tree.
fn create_controller(
    environment: ICoreWebView2Environment,
    parent: windows::Win32::Foundation::HWND,
) -> webview2_com::Result<(ICoreWebView2Controller, ICoreWebView2)> {
    use webview2_com::CreateCoreWebView2ControllerCompletedHandler;

    let (tx, rx) = mpsc::channel();
    CreateCoreWebView2ControllerCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| unsafe {
            environment
                .CreateCoreWebView2Controller(parent, &handler)
                .map_err(webview2_com::Error::WindowsError)
        }),
        Box::new(move |error_code, controller| {
            error_code?;
            tx.send(controller.ok_or_else(|| {
                windows::core::Error::from(windows::Win32::Foundation::E_POINTER)
            }))
            .expect("send controller over mpsc channel");
            Ok(())
        }),
    )?;
    let controller = webview2_com::wait_with_pump(rx)??;
    let webview = unsafe { controller.CoreWebView2() }?;
    Ok((controller, webview))
}

fn resize_controller(controller: &ICoreWebView2Controller, width: u32, height: u32) {
    let bounds = RECT {
        left: 0,
        top: 0,
        right: width as i32,
        bottom: height as i32,
    };
    unsafe {
        let _ = controller.SetBounds(bounds);
    }
}
