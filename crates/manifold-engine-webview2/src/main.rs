//! WebView2 engine-host (see the M6 plan). Structured to mirror
//! `manifold-engine-cef/src/main.rs`'s shape (fixed-tick poll loop,
//! `SharedNavState` + `dirty` flag) since CEF is the closest existing
//! analog: a real Windows process, no visible window, same-machine IPC.
//! WebView2 is hosted via the Composition controller against a
//! `Windows.UI.Composition` visual (`composition.rs`), and frames are
//! captured directly from that visual via
//! `GraphicsCaptureItem::CreateFromVisual` (`capture.rs`) rather than from
//! the host window -- see
//! `docs/superpowers/specs/2026-09-20-composition-hosting-migration-design.md`
//! for why.

mod audio;
mod capture;
mod composition;
mod download;
mod find;
mod input;
mod jar_cdp;
mod nav_state;
mod popup;
mod scheme;
mod window;

use capture::Capture;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use ipc_protocol::{transport, EngineToHost, HostToEngine, InputEvent, SharedFrameBuffer};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2CompositionController, ICoreWebView2Controller, ICoreWebView2Environment,
    ICoreWebView2Environment10, ICoreWebView2Environment3, ICoreWebView2_8,
    COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC,
};
use webview2_com::{
    CallDevToolsProtocolMethodCompletedHandler, CreateCoreWebView2CompositionControllerCompletedHandler,
    CreateCoreWebView2EnvironmentCompletedHandler, ExecuteScriptCompletedHandler,
};
use windows::core::{Interface, BOOL, HSTRING};
use windows::Win32::Foundation::RECT;
use windows::UI::Composition::ContainerVisual;

use nav_state::new_shared_nav_state;

const DEFAULT_WIDTH: u32 = 1024;
const DEFAULT_HEIGHT: u32 = 768;
/// Caps how often a captured frame is actually acted on and sent to
/// manifold -- see the cap's own call-site doc comment for why this
/// exists (unlike CEF/Servo/WebKit, WGC produces a new frame every vsync
/// regardless of whether content changed, which measurably overloaded
/// manifold without this). ~30fps is plenty for a tab preview/thumbnail
/// use case and well below what caused the observed overload.
const FRAME_INTERVAL: Duration = Duration::from_millis(33);
/// How long a host started with `ENGINE_AWAIT_COOKIES` waits for manifold's `SetCookies` before navigating.
const COOKIE_SEED_WAIT: Duration = Duration::from_secs(5);

fn main() {
    // Must happen before any window exists (`create_hidden_window` below is
    // the first one) -- `manifold`'s own main process is DPI-aware for free
    // via `winit` (it calls this same API internally), but this process
    // creates its host window with plain Win32 calls (`window.rs`), which
    // gets *no* DPI awareness by default. An unaware process is DPI-
    // *virtualized* by Windows: it's handed a logical/unscaled window size,
    // and the OS bitmap-stretches its rendered output to the real physical
    // size to compensate -- which is exactly blur, a size mismatch between
    // what this process thinks its window is and what it actually occupies
    // on screen (reported live as a "white bar"/content overflowing its
    // borders), and coordinates that don't line up with `manifold`'s own
    // now-DPI-aware idea of where the cursor is. Per-Monitor-V2 makes this
    // process report and receive real physical-pixel values throughout,
    // matching `manifold`'s own winit-provided awareness level.
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
    }

    // WebView2 is fundamentally a COM component -- unlike CEF/Servo (which
    // never touch raw COM from the Rust side), every `ICoreWebView2*` call
    // here needs an initialized apartment on this thread first, or every
    // single one fails with CO_E_NOTINITIALIZED (confirmed live: exactly
    // that HRESULT, 0x800401F0, on the very first environment-creation
    // call). Apartment-threaded (STA), not multi-threaded, since WebView2
    // is a UI-affine component tied to the thread that pumps its message
    // loop (see window::pump_messages, called from this same thread below).
    unsafe {
        let _ =
            windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
    }

    // DEVIATION from the brief: not called out in Task 3's brief text, but
    // required for `composition::create_composition_target` (Task 1) to
    // work at all in this real process -- confirmed live: without this,
    // `ICompositorDesktopInterop::CreateDesktopWindowTarget` fails with
    // `HRESULT(0x80070005)` ("Access is denied.") on every single launch,
    // crashing the engine-host in a respawn loop. `examples/
    // composition_capture_spike.rs` (Task 1's own verification vehicle,
    // since deleted by Task 6 along with the other throwaway spikes)
    // already did exactly this before creating its `Compositor` -- a plain
    // Win32 desktop app (unlike a packaged UWP app) has no `DispatcherQueue`
    // on its thread by default, and `Windows.UI.Composition`'s interop
    // requires one to exist. `_dispatcher_queue_controller`'s only purpose
    // is to stay alive for the whole process lifetime, same as `_job` below.
    unsafe {
        let _ = windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_SINGLETHREADED);
    }
    let _dispatcher_queue_controller = unsafe {
        windows::Win32::System::WinRT::CreateDispatcherQueueController(
            windows::Win32::System::WinRT::DispatcherQueueOptions {
                dwSize: std::mem::size_of::<windows::Win32::System::WinRT::DispatcherQueueOptions>() as u32,
                threadType: windows::Win32::System::WinRT::DQTYPE_THREAD_CURRENT,
                apartmentType: windows::Win32::System::WinRT::DQTAT_COM_NONE,
            },
        )
        .expect("create DispatcherQueueController for Composition hosting")
    };

    // WebView2 spawns its own Chromium-like multi-process tree
    // (msedgewebview2.exe browser/renderer/GPU processes) that are NOT
    // direct children of this process in the Windows sense (they're
    // children of WebView2's own browser-process, a grandchild of us) --
    // confirmed live that a hard-killed manifold-engine-webview2.exe (e.g. an
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
    let initial_url = std::env::var("ENGINE_INITIAL_URL").unwrap_or_else(|_| "https://example.com".to_string());

    // Same convention as manifold-engine-cef/-servo: manifold already created
    // both named pipes before spawning us (listen_local_pair), we just
    // connect. `host_to_engine`/`control` are one-directional (HostToEngine
    // in, EngineToHost out) -- see transport.rs's own doc comment.
    let (host_to_engine, mut control) = transport::connect_local_pair(&socket_id).expect("connect to manifold");
    let rx = ipc_protocol::transport::spawn_host_reader(host_to_engine);

    // Composition hosting against a real, off-screen host window -- see
    // docs/superpowers/specs/2026-09-20-composition-hosting-migration-design.md
    // for why this replaced the plain windowed hosting `window.rs`'s own
    // doc comment used to describe here (that doc comment was itself
    // written expecting this exact migration -- see `create_composition_controller`'s
    // own doc comment).
    let hwnd = window::create_hidden_window().expect("create hidden host window");
    let (_composition_target, root_visual) =
        composition::create_composition_target(hwnd, DEFAULT_WIDTH, DEFAULT_HEIGHT).expect("create composition target");

    let environment = create_environment().expect("create CoreWebView2Environment");
    let (comp_controller, controller, webview, in_private) =
        create_composition_controller(environment.clone(), hwnd).expect("create CoreWebView2CompositionController");
    unsafe {
        comp_controller.SetRootVisualTarget(&root_visual).expect("set composition root visual target");
    }

    resize_controller(&controller, &root_visual, DEFAULT_WIDTH, DEFAULT_HEIGHT);

    // Suppresses Chromium's own native right-click context menu. Confirmed
    // live (see Task 4's follow-up report): once `input.rs` started sending
    // genuine native `SendMouseInput` right-button events, WebView2's
    // browser process really did pop up its native "Back/Refresh/Save
    // as/.../Inspect" context menu as a real top-level `Chrome_WidgetWin_1`
    // window -- and since this host window is a real (if off-screen) window,
    // that popup renders at real, on-screen coordinates, landing right on
    // top of `manifold`'s own visible window in a real desktop screen
    // capture (invisible in `manifold`'s own composited-frame screenshots,
    // since it's a separate top-level window, not part of the captured
    // visual tree). `manifold` already has its own complete
    // link-context-menu UI built on this tab's JS-based `elementFromPoint`
    // href-detection (`input.rs`'s `MouseUp` right-click arm; see
    // `ipc_protocol::EngineToHost::ContextMenuLink`) -- live-verified after
    // this fix to still work correctly (the custom "Open in new tab" pill
    // still appears right where expected) -- so Chromium's native menu is
    // pure downside: a second, wrongly-positioned menu with no corresponding
    // custom-menu upside.
    //
    // Hard-failed rather than `let _ =`'d, same reasoning as
    // `SetRootVisualTarget` above: a one-time startup call whose silent
    // failure resurrects a real, already-diagnosed bug for the whole
    // process lifetime.
    unsafe {
        webview
            .Settings()
            .and_then(|settings| settings.SetAreDefaultContextMenusEnabled(false))
            .expect("disable WebView2's own native context menu");
    }

    // Gives the webview logical focus programmatically -- no real OS window
    // activation (`SetForegroundWindow`) needed or wanted, this project's
    // off-screen host window deliberately never becomes the real foreground
    // window. Tried (unsuccessfully on its own) as a fix for raw WM_*
    // message injection in an earlier round -- see input.rs's doc comment
    // for the full history.
    //
    // **Once at startup is enough**, despite the migration spec's original
    // expectation that native `SendMouseInput` would need this on every
    // mouse-down: confirmed live in Task 5 that a native click focuses a
    // plain `<input>` on its own (`:focus` outline appeared, and subsequent
    // `Char` events typed into it), with no per-dispatch `MoveFocus`
    // anywhere. Kept here because some WebView2 behaviors genuinely do
    // depend on the control having logical focus at all, which this
    // establishes once for the process lifetime.
    unsafe {
        let _ = controller.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
    }

    let nav_state = new_shared_nav_state(initial_url.clone());
    let pending_json_body = nav_state::new_shared_json_body();
    let pending_redirect_content_type = nav_state::new_shared_redirect_content_type();
    let pending_page_state: Rc<RefCell<Option<ipc_protocol::PageState>>> = Rc::new(RefCell::new(None));
    let pending_cookies: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    nav_state::register_handlers(
        &webview,
        nav_state.clone(),
        pending_json_body.clone(),
        pending_redirect_content_type.clone(),
    );
    let pending_popup = popup::register(&webview);
    // Best-effort -- an `ICoreWebView2_28`/`ICoreWebView2Environment15`-less
    // WebView2 Runtime just means this tab's find bar never reports results.
    let find_state = find::register(&environment, &webview).ok();
    let pending_audio_playing = audio::register(&webview).ok();
    let pending_context_menu: input::PendingContextMenu = Default::default();
    let drag_state: input::DragState = Default::default();
    let pending_download = download::register(&webview).ok();
    let _ = scheme::register(&environment, &webview);

    // Cookie-jar seeding: manifold sends `SetCookies` as its very first message to hosts started with
    // `ENGINE_AWAIT_COOKIES`. Wait for it (bounded) so the cookies are in place before the first navigation.
    // Any other first message is kept for the loop below rather than dropped.
    let mut first_message: Option<HostToEngine> = None;
    if std::env::var_os("ENGINE_AWAIT_COOKIES").is_some() {
        match wait_for_first_message(&rx, COOKIE_SEED_WAIT) {
            Some(HostToEngine::SetCookies(json)) => seed_cookies(&webview, in_private, &json),
            Some(other) => first_message = Some(other),
            None => eprintln!("jar: no seed received"),
        }
    }

    unsafe {
        webview.Navigate(&HSTRING::from(&initial_url)).expect("initial navigate");
    }

    let mut capture = Capture::new(&root_visual).expect("create capture pipeline");
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
    // measurably overloaded manifold's per-frame handling badly enough
    // to make it stop responding to headless stdin commands for tens of
    // seconds. `TryGetNextFrame` itself can't be told to slow down (no
    // rate-control knob exists on the capture session), so this throttles
    // at the polling level instead: skip acting on a new frame entirely
    // unless enough wall time has passed, capping our own outgoing rate
    // independent of the compositor's.
    let mut last_frame_sent = std::time::Instant::now() - FRAME_INTERVAL;

    loop {
        window::pump_messages();

        while let Some(msg) = first_message.take().or_else(|| rx.try_recv().ok()) {
            match msg {
                HostToEngine::Navigate { url, .. } => unsafe {
                    nav_state.borrow_mut().host_started = Some((false, std::time::Instant::now()));
                    let _ = webview.Navigate(&HSTRING::from(&url));
                },
                HostToEngine::GoBack => unsafe {
                    let mut can_go = BOOL(0);
                    let _ = webview.CanGoBack(&mut can_go);
                    if can_go.as_bool() {
                        nav_state.borrow_mut().host_started = Some((true, std::time::Instant::now()));
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
                        nav_state.borrow_mut().host_started = Some((true, std::time::Instant::now()));
                        let _ = webview.GoForward();
                    }
                },
                HostToEngine::Resize { width, height, .. } => {
                    window::resize(hwnd, width, height);
                    resize_controller(&controller, &root_visual, width, height);
                    capture.resize(width, height);
                }
                HostToEngine::Input(event) => {
                    if matches!(event, InputEvent::MouseDown { .. } | InputEvent::KeyDown { .. }) {
                        nav_state.borrow_mut().last_input = Some(std::time::Instant::now());
                    }
                    input::send_input(&webview, &comp_controller, event, &pending_context_menu, &drag_state);
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
                    let _ = control.send(&EngineToHost::StateSnapshot(ipc_protocol::TabState { url, zoom: 1.0 }));
                }
                // A seed that arrives after the first navigation (not the expected path) still goes in.
                HostToEngine::SetCookies(json) => seed_cookies(&webview, in_private, &json),
                HostToEngine::GetCookies => {
                    // On the shared profile the answer would hold the user's real cookies, and an empty answer
                    // would read as a logout: say nothing.
                    if in_private {
                        let slot = pending_cookies.clone();
                        let handler =
                            CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |error, result| {
                                // A failed or unreadable read sends nothing: an answer of `[]` would read as a
                                // logout of every cookie this tab held. Manifold's pull just times out.
                                let json = error
                                    .ok()
                                    .and_then(|_| serde_json::from_str::<serde_json::Value>(&result.to_string()).ok())
                                    .and_then(|parsed| jar_cdp::jar_from_cdp(&parsed));
                                match json {
                                    Some(json) => *slot.borrow_mut() = Some(json),
                                    None => eprintln!("jar: Network.getAllCookies failed; not answering"),
                                }
                                Ok(())
                            }));
                        let _ = unsafe {
                            webview.CallDevToolsProtocolMethod(
                                &HSTRING::from("Network.getAllCookies"),
                                &HSTRING::from("{}"),
                                &handler,
                            )
                        };
                    }
                }
                HostToEngine::QueryPageState => {
                    let pending = pending_page_state.clone();
                    let handler = ExecuteScriptCompletedHandler::create(Box::new(move |_error, result| {
                        // `ExecuteScript` returns the string JSON-encoded (quoted).
                        if let Ok(Some(text)) = serde_json::from_str::<Option<String>>(&result.to_string()) {
                            if let Some(state) = ipc_protocol::PageState::parse(&text) {
                                *pending.borrow_mut() = Some(state);
                            }
                        }
                        Ok(())
                    }));
                    let _ = unsafe { webview.ExecuteScript(&HSTRING::from(ipc_protocol::PAGE_STATE_SCRIPT), &handler) };
                }
                HostToEngine::RestoreScroll { x, y } => {
                    let handler = ExecuteScriptCompletedHandler::create(Box::new(|_error, _result| Ok(())));
                    let script = ipc_protocol::restore_scroll_script(x, y);
                    let _ = unsafe { webview.ExecuteScript(&HSTRING::from(script), &handler) };
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
                    let webview8: windows::core::Result<ICoreWebView2_8> = windows::core::Interface::cast(&webview);
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
                    user_started: state.user_started,
                });
                if std::mem::take(&mut state.finished) {
                    state.user_started = None;
                }
            }
        }

        if let Some(body) = pending_json_body.borrow_mut().take() {
            let _ = control.send(&EngineToHost::JsonBody { body });
        }

        if let Some(json) = pending_cookies.borrow_mut().take() {
            let _ = control.send(&EngineToHost::Cookies(json));
        }

        if let Some(state) = pending_page_state.borrow_mut().take() {
            let _ = control.send(&EngineToHost::PageState(state));
        }

        if let Some(content_type) = pending_redirect_content_type.borrow_mut().take() {
            let _ = control.send(&EngineToHost::RedirectableContent { content_type });
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
                    download::DownloadEvent::Started { path, total_bytes, url } => {
                        EngineToHost::DownloadStarted { path, total_bytes, url }
                    }
                    download::DownloadEvent::Progress { path, bytes_received } => {
                        EngineToHost::DownloadProgress { path, bytes_received }
                    }
                    download::DownloadEvent::Completed { path } => EngineToHost::DownloadCompleted { path },
                    download::DownloadEvent::Failed { path } => EngineToHost::DownloadFailed { path },
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
                    // Logged permanently -- a frame-size change is rare
                    // enough post-migration that seeing it in the log is
                    // useful signal, not noise.
                    eprintln!("[capture] captured frame size changed to {width}x{height}");
                    // Drop the old mapping before recreating -- `create`
                    // truncates the same backing file by name, and Windows
                    // rejects resizing a file with a live mapped section
                    // still open. Plain assignment wouldn't drop the old
                    // value until after this call already ran.
                    drop(frame_buffer);
                    frame_buffer = SharedFrameBuffer::create(&socket_id, width, height).ok();
                }
                if let Some(buf) = &mut frame_buffer {
                    buf.write_frame(&pixels);
                    frame_id += 1;
                    let _ =
                        control.send(&EngineToHost::FrameReady { frame_id, width, height, damage: None, pixels: None });
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

fn create_environment() -> webview2_com::Result<ICoreWebView2Environment> {
    let options = scheme::environment_options().map_err(webview2_com::Error::WindowsError)?;
    let (tx, rx) = mpsc::channel();
    CreateCoreWebView2EnvironmentCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| unsafe {
            webview2_com::Microsoft::Web::WebView2::Win32::CreateCoreWebView2EnvironmentWithOptions(
                windows::core::PCWSTR::null(),
                windows::core::PCWSTR::null(),
                &options,
                &handler,
            )
            .map_err(webview2_com::Error::WindowsError)
        }),
        Box::new(move |error_code, environment| {
            error_code?;
            tx.send(environment.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_POINTER)))
                .expect("send environment over mpsc channel");
            Ok(())
        }),
    )?;
    Ok(webview2_com::wait_with_pump(rx)??)
}

/// Waits up to `timeout` for the first message from manifold, pumping window messages meanwhile so WebView2's
/// async completions keep running. `None` on timeout or a closed channel.
fn wait_for_first_message(rx: &mpsc::Receiver<HostToEngine>, timeout: Duration) -> Option<HostToEngine> {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        window::pump_messages();
        match rx.try_recv() {
            Ok(msg) => return Some(msg),
            Err(mpsc::TryRecvError::Empty) => std::thread::sleep(Duration::from_millis(8)),
            Err(mpsc::TryRecvError::Disconnected) => return None,
        }
    }
    None
}

/// `Network.setCookies` from the neutral jar JSON, awaited. Only into an InPrivate tab: seeding the shared
/// profile would write into the user's real cookies. Never logs cookie values.
fn seed_cookies(webview: &ICoreWebView2, in_private: bool, jar_json: &str) {
    if !in_private {
        eprintln!("jar: not seeding the shared profile");
        return;
    }
    let cookies = jar_cdp::cdp_cookies_from_jar(jar_json);
    if cookies.is_empty() {
        return;
    }
    if set_cookies(webview, &cookies) {
        eprintln!("jar: seeded {} cookies", cookies.len());
        return;
    }
    // One cookie CDP rejects may fail the whole batch: set them one at a time so the rest still go in.
    let ok = cookies.iter().filter(|c| set_cookies(webview, std::slice::from_ref(c))).count();
    eprintln!("jar: batch seed failed; seeded {ok} of {} one at a time", cookies.len());
}

/// `Network.setCookies` with `cookies`, awaited. Whether it succeeded. Logs nothing (the params hold cookie values;
/// the caller logs counts).
fn set_cookies(webview: &ICoreWebView2, cookies: &[serde_json::Value]) -> bool {
    let params = jar_cdp::set_cookies_params(cookies);
    let webview = webview.clone();
    let ok = Rc::new(std::cell::Cell::new(false));
    let done = ok.clone();
    let res = CallDevToolsProtocolMethodCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| unsafe {
            webview
                .CallDevToolsProtocolMethod(
                    &HSTRING::from("Network.setCookies"),
                    &HSTRING::from(params.as_str()),
                    &handler,
                )
                .map_err(webview2_com::Error::WindowsError)
        }),
        Box::new(move |error_code, _result| {
            done.set(error_code.is_ok());
            Ok(())
        }),
    );
    res.is_ok() && ok.get()
}

/// Returns `(comp_controller, controller, webview, in_private)` -- `comp_controller`
/// (`ICoreWebView2CompositionController`) is used for `SendMouseInput`/
/// `SetRootVisualTarget`; `controller` (`ICoreWebView2Controller`) for
/// bounds/visibility/zoom/focus exactly as before (the object
/// `CreateCoreWebView2CompositionController` returns implements both
/// interfaces -- confirmed live this session, not assumed); `webview`
/// (`ICoreWebView2`) for navigation/events/keyboard input, also unchanged.
///
/// The controller is InPrivate (an in-memory profile, so the jar is the only source of cookies). If the runtime
/// lacks `ICoreWebView2Environment10` or the options fail, it falls back to the shared profile and `in_private`
/// is `false`; callers must then neither seed nor report cookies.
fn create_composition_controller(
    environment: ICoreWebView2Environment,
    parent: windows::Win32::Foundation::HWND,
) -> webview2_com::Result<(ICoreWebView2CompositionController, ICoreWebView2Controller, ICoreWebView2, bool)> {
    let environment3: ICoreWebView2Environment3 = environment.cast().map_err(webview2_com::Error::WindowsError)?;
    let private_options = environment.cast::<ICoreWebView2Environment10>().and_then(|env10| unsafe {
        let options = env10.CreateCoreWebView2ControllerOptions()?;
        options.SetIsInPrivateModeEnabled(true)?;
        Ok((env10, options))
    });
    let private_options = match private_options {
        Ok(p) => Some(p),
        Err(_) => {
            eprintln!("jar: InPrivate unavailable, using the shared profile");
            None
        }
    };
    let in_private = private_options.is_some();
    let (tx, rx) = mpsc::channel();
    CreateCoreWebView2CompositionControllerCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| unsafe {
            match &private_options {
                Some((env10, options)) => env10
                    .CreateCoreWebView2CompositionControllerWithOptions(parent, options, &handler)
                    .map_err(webview2_com::Error::WindowsError),
                None => environment3
                    .CreateCoreWebView2CompositionController(parent, &handler)
                    .map_err(webview2_com::Error::WindowsError),
            }
        }),
        Box::new(move |error_code, controller| {
            error_code?;
            tx.send(controller.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_POINTER)))
                .expect("send composition controller over mpsc channel");
            Ok(())
        }),
    )?;
    let comp_controller = webview2_com::wait_with_pump(rx)??;
    let controller: ICoreWebView2Controller = comp_controller.cast().map_err(webview2_com::Error::WindowsError)?;
    let webview = unsafe { controller.CoreWebView2() }?;
    Ok((comp_controller, controller, webview, in_private))
}

fn resize_controller(controller: &ICoreWebView2Controller, root_visual: &ContainerVisual, width: u32, height: u32) {
    let bounds = RECT { left: 0, top: 0, right: width as i32, bottom: height as i32 };
    unsafe {
        let _ = controller.SetBounds(bounds);
    }
    composition::resize_root_visual(root_visual, width, height);
}
