//! WebView2 engine-host (see the M6 plan). Structured to mirror
//! `engine-host-cef/src/main.rs`'s shape (fixed-tick poll loop,
//! `SharedNavState` + `dirty` flag) since CEF is the closest existing
//! analog: a real Windows process, no visible window, same-machine IPC.
//! Real frame capture via `Windows.Graphics.Capture` -- see `capture.rs`'s
//! doc comment for why it captures the host window directly rather than
//! the originally-planned DirectComposition Visual.

mod audio;
mod capture;
mod download;
mod find;
mod input;
mod nav_state;
mod popup;
mod window;

use capture::Capture;

use std::sync::mpsc;
use std::time::Duration;

use ipc_protocol::{transport, EngineToHost, HostToEngine, SharedFrameBuffer};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2Controller, ICoreWebView2Environment, ICoreWebView2_8,
    COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC,
};
use webview2_com::CreateCoreWebView2EnvironmentCompletedHandler;
use windows::core::{BOOL, HSTRING};
use windows::Win32::Foundation::RECT;

use nav_state::new_shared_nav_state;

const DEFAULT_WIDTH: u32 = 1024;
const DEFAULT_HEIGHT: u32 = 768;
/// Caps how often a captured frame is actually acted on and sent to
/// browser-ui -- see the cap's own call-site doc comment for why this
/// exists (unlike CEF/Servo/WebKit, WGC produces a new frame every vsync
/// regardless of whether content changed, which measurably overloaded
/// browser-ui without this). ~30fps is plenty for a tab preview/thumbnail
/// use case and well below what caused the observed overload.
const FRAME_INTERVAL: Duration = Duration::from_millis(33);

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

    // WebView2 spawns its own Chromium-like multi-process tree
    // (msedgewebview2.exe browser/renderer/GPU processes) that are NOT
    // direct children of this process in the Windows sense (they're
    // children of WebView2's own browser-process, a grandchild of us) --
    // confirmed live that a hard-killed engine-host-webview2.exe (e.g. an
    // external `timeout` cutting the whole tree, or a plain force-kill)
    // leaves those grandchildren running indefinitely, invisible to any
    // cleanup this process's own `Shutdown` handler could do (it never
    // gets to run). A Job Object with `KILL_ON_JOB_CLOSE`, assigned to this
    // process at startup, fixes this generally: Windows terminates every
    // process still in the job the moment this process's own job handle is
    // closed, which happens automatically on ANY exit path (graceful,
    // crashed, or force-killed) since the OS closes all of a dying
    // process's handles regardless of how it died. `_job`'s only purpose
    // is to stay alive for the whole process lifetime; never used again
    // after this.
    let _job = setup_kill_on_exit_job();

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

    // Plain *windowed* WebView2 hosting against a real, off-screen host
    // window -- see `window.rs`'s doc comment for why this ended up
    // simpler than the originally-planned Composition/Visual hosting once
    // `Windows.Graphics.Capture`'s actual constraints were discovered live.
    let hwnd = window::create_hidden_window().expect("create hidden host window");

    let environment = create_environment().expect("create CoreWebView2Environment");
    let (controller, webview) =
        create_controller(environment.clone(), hwnd).expect("create CoreWebView2Controller");

    resize_controller(&controller, DEFAULT_WIDTH, DEFAULT_HEIGHT);

    // Gives the webview logical focus programmatically -- no real OS window
    // activation (`SetForegroundWindow`) needed or wanted, this project's
    // off-screen host window deliberately never becomes the real foreground
    // window. Tried (unsuccessfully on its own) as a fix for raw WM_*
    // message injection in an earlier round -- see input.rs's doc comment
    // for the full history; kept here as cheap, harmless insurance since
    // some WebView2 behaviors genuinely do depend on logical focus state,
    // even though the actual input fix (input.rs's ExecuteScript approach)
    // doesn't need it.
    unsafe {
        let _ = controller.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
    }

    let nav_state = new_shared_nav_state(initial_url.clone());
    nav_state::register_handlers(&webview, nav_state.clone());
    let pending_popup = popup::register(&webview);
    // Best-effort -- an `ICoreWebView2_28`/`ICoreWebView2Environment15`-less
    // WebView2 Runtime just means this tab's find bar never reports results.
    let find_state = find::register(&environment, &webview).ok();
    let pending_audio_playing = audio::register(&webview).ok();
    let pending_context_menu: input::PendingContextMenu = Default::default();
    let pending_download = download::register(&webview).ok();

    unsafe {
        webview
            .Navigate(&HSTRING::from(&initial_url))
            .expect("initial navigate");
    }

    let mut capture = Capture::new(hwnd).expect("create capture pipeline");
    window::hide_from_taskbar(hwnd);
    let mut frame_buffer: Option<SharedFrameBuffer> = None;
    let mut frame_id: u64 = 0;
    let mut visible = true;
    // Unlike CEF/Servo (which only paint on invalidation, so a static page
    // naturally settles to a low steady-state frame rate) or WebKit (its
    // own GTK-driven capture), `Windows.Graphics.Capture` produces a new
    // frame on every compositor vsync *regardless of whether page content
    // actually changed* -- confirmed live: a real page can flood ~60-70
    // `FrameReady` messages/sec continuously, even fully static, which
    // measurably overloaded browser-ui's per-frame handling badly enough
    // to make it stop responding to headless stdin commands for tens of
    // seconds. `TryGetNextFrame` itself can't be told to slow down (no
    // rate-control knob exists on the capture session), so this throttles
    // at the polling level instead: skip acting on a new frame entirely
    // unless enough wall time has passed, capping our own outgoing rate
    // independent of the compositor's.
    let mut last_frame_sent = std::time::Instant::now() - FRAME_INTERVAL;

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
                HostToEngine::Reload => unsafe {
                    let _ = webview.Reload();
                },
                HostToEngine::GoForward => unsafe {
                    let mut can_go = BOOL(0);
                    let _ = webview.CanGoForward(&mut can_go);
                    if can_go.as_bool() {
                        let _ = webview.GoForward();
                    }
                },
                HostToEngine::Resize { width, height, .. } => {
                    window::resize(hwnd, width, height);
                    resize_controller(&controller, width, height);
                    capture.resize(width, height);
                }
                HostToEngine::Input(event) => {
                    input::send_input(&webview, event, &pending_context_menu);
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
                HostToEngine::FindStart { text, match_case } => {
                    if let Some(find) = &find_state {
                        find.start(&text, match_case);
                    }
                }
                HostToEngine::FindNext => {
                    if let Some(find) = &find_state {
                        find.next();
                    }
                }
                HostToEngine::FindPrevious => {
                    if let Some(find) = &find_state {
                        find.previous();
                    }
                }
                HostToEngine::FindStop => {
                    if let Some(find) = &find_state {
                        find.stop();
                    }
                }
                HostToEngine::OpenDevTools => unsafe {
                    let _ = webview.OpenDevToolsWindow();
                },
                HostToEngine::SetMuted { muted } => {
                    let webview8: windows::core::Result<ICoreWebView2_8> =
                        windows::core::Interface::cast(&webview);
                    if let Ok(webview8) = webview8 {
                        let _ = unsafe { webview8.SetIsMuted(muted) };
                    }
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

        for url in pending_popup.borrow_mut().drain(..) {
            let _ = control.send(&EngineToHost::OpenTabRequested { url });
        }

        if let Some(find) = &find_state {
            if let Some((count, active_index)) = find.pending().borrow_mut().take() {
                let _ = control.send(&EngineToHost::FindResult { count, active_index });
            }
        }

        if let Some(pending) = &pending_audio_playing {
            if let Some(playing) = pending.borrow_mut().take() {
                let _ = control.send(&EngineToHost::AudioPlayingChanged { playing });
            }
        }

        if let Some((x, y, href)) = pending_context_menu.borrow_mut().take() {
            let _ = control.send(&EngineToHost::ContextMenuLink { x, y, href });
        }

        if let Some(pending) = &pending_download {
            for event in pending.borrow_mut().drain(..) {
                let msg = match event {
                    download::DownloadEvent::Started { path, total_bytes } => {
                        EngineToHost::DownloadStarted { path, total_bytes }
                    }
                    download::DownloadEvent::Progress { path, bytes_received } => {
                        EngineToHost::DownloadProgress { path, bytes_received }
                    }
                    download::DownloadEvent::Completed { path } => EngineToHost::DownloadCompleted { path },
                };
                let _ = control.send(&msg);
            }
        }

        if visible && last_frame_sent.elapsed() >= FRAME_INTERVAL {
            if let Some((pixels, width, height)) = capture.try_get_latest_frame() {
                last_frame_sent = std::time::Instant::now();
                let needs_new_buffer = match &frame_buffer {
                    Some(buf) => buf.width() != width || buf.height() != height,
                    None => true,
                };
                if needs_new_buffer {
                    frame_buffer = SharedFrameBuffer::create(&socket_id, width, height).ok();
                }
                if let Some(buf) = &mut frame_buffer {
                    buf.write_frame(&pixels);
                    frame_id += 1;
                    let _ = control.send(&EngineToHost::FrameReady {
                        frame_id,
                        width,
                        height,
                        damage: None,
                        pixels: None,
                    });
                }
            }
        }

        std::thread::sleep(Duration::from_millis(8));
    }
}

/// See the call site's doc comment. Returns the job handle purely to keep
/// it alive for the process lifetime -- never queried or signaled again.
fn setup_kill_on_exit_job() -> windows::core::Result<windows::Win32::Foundation::HANDLE> {
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, SetInformationJobObject,
        JobObjectExtendedLimitInformation, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
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
