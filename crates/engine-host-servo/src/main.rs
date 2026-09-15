use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use dpi::PhysicalSize;
use ipc_protocol::{
    modifiers as host_modifiers, EngineToHost, HostToEngine, InputEvent as HostInputEvent,
    KeyCode as HostKeyCode, SharedFrameBuffer,
};
use servo::{
    Code, EventLoopWaker, InputEvent as ServoInputEvent, Key, KeyState, KeyboardEvent, Location,
    Modifiers, MouseButton as ServoMouseButton, MouseButtonAction, MouseButtonEvent,
    MouseMoveEvent, NamedKey, RenderingContext, ServoBuilder, SoftwareRenderingContext, WebView,
    WebViewBuilder, WebViewDelegate, WebViewPoint, WheelDelta, WheelEvent, WheelMode,
};
use url::Url;
use webrender_api::units::DevicePoint;

const DEFAULT_WIDTH: u32 = 1024;
const DEFAULT_HEIGHT: u32 = 768;

/// M1 drives Servo with a busy-poll loop rather than a real wake-based
/// event loop, so this waker is intentionally a no-op. Revisit if CPU
/// usage from polling becomes a problem.
#[derive(Clone)]
struct NoopWaker;

impl EventLoopWaker for NoopWaker {
    fn clone_box(&self) -> Box<dyn EventLoopWaker> {
        Box::new(self.clone())
    }
    fn wake(&self) {}
}

#[derive(Default)]
struct NavState {
    url: String,
    title: String,
    can_go_back: bool,
    can_go_forward: bool,
    loading: bool,
    dirty: bool,
}

struct Delegate {
    frame_ready: RefCell<bool>,
    nav: RefCell<NavState>,
    /// Set by `notify_favicon_changed`, polled and drained by the main
    /// loop the same way `nav`/`frame_ready` are — `(rgba, width, height)`.
    favicon: RefCell<Option<(Vec<u8>, u32, u32)>>,
}

impl WebViewDelegate for Delegate {
    fn notify_new_frame_ready(&self, _webview: WebView) {
        *self.frame_ready.borrow_mut() = true;
    }

    fn notify_url_changed(&self, _webview: WebView, url: Url) {
        let mut nav = self.nav.borrow_mut();
        nav.url = url.to_string();
        nav.dirty = true;
    }

    fn notify_page_title_changed(&self, _webview: WebView, title: Option<String>) {
        let mut nav = self.nav.borrow_mut();
        nav.title = title.unwrap_or_default();
        nav.dirty = true;
    }

    fn notify_load_status_changed(&self, _webview: WebView, status: servo::LoadStatus) {
        let mut nav = self.nav.borrow_mut();
        nav.loading = !matches!(status, servo::LoadStatus::Complete);
        nav.dirty = true;
    }

    fn notify_history_changed(&self, _webview: WebView, entries: Vec<Url>, current: usize) {
        let mut nav = self.nav.borrow_mut();
        nav.can_go_back = current > 0;
        nav.can_go_forward = current + 1 < entries.len();
        nav.dirty = true;
    }

    fn notify_favicon_changed(&self, webview: WebView) {
        if let Some(image) = webview.favicon() {
            *self.favicon.borrow_mut() =
                Some((to_rgba(image.data(), image.format), image.width, image.height));
        }
    }
}

/// Servo's favicon can arrive in any of several pixel formats depending on
/// the source image; `EngineToHost::FaviconChanged` is always plain RGBA8
/// (matching `FrameReady`'s convention), so normalize here rather than
/// pushing format-awareness onto browser-ui.
fn to_rgba(data: &[u8], format: servo::PixelFormat) -> Vec<u8> {
    match format {
        servo::PixelFormat::RGBA8 => data.to_vec(),
        servo::PixelFormat::BGRA8 => {
            let mut out = data.to_vec();
            for px in out.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
            out
        }
        servo::PixelFormat::RGB8 => data
            .chunks_exact(3)
            .flat_map(|rgb| [rgb[0], rgb[1], rgb[2], 255])
            .collect(),
        servo::PixelFormat::KA8 => data
            .chunks_exact(2)
            .flat_map(|ka| [ka[0], ka[0], ka[0], ka[1]])
            .collect(),
        servo::PixelFormat::K8 => data.iter().flat_map(|&k| [k, k, k, 255]).collect(),
    }
}

fn main() {
    // Servo needs a process-level TLS crypto provider installed before
    // anything touches HTTPS (matches servo's own components/servo/examples/
    // winit_minimal.rs, which does this before constructing anything else).
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("install rustls crypto provider");

    // Env vars, not argv — matches engine-host-cef, whose argv must stay
    // reserved for CEF's own command-line parsing. Kept consistent here too
    // so browser-ui's process manager has one spawn convention for every
    // engine-host.
    // ENGINE_SOCKET_ID names our control pipes/shared memory — normally
    // equal to ENGINE_TAB_ID, but browser-ui may pass a different id if a
    // stale engine-host from a prior crashed run still held ENGINE_TAB_ID's
    // pipes (see process_manager::spawn_tab's fallback). Only this id
    // matters here; we never need ENGINE_TAB_ID itself.
    let socket_id = std::env::var("ENGINE_SOCKET_ID").expect("ENGINE_SOCKET_ID env var required");
    let initial_url = std::env::var("ENGINE_INITIAL_URL")
        .unwrap_or_else(|_| "https://example.com".to_string());

    // Two one-directional control sockets back to browser-ui (it already
    // created both listeners before spawning this process — see
    // ipc_protocol::transport::listen_local_pair). Using two sockets
    // instead of one bidirectional stream means the reader thread below
    // never needs to duplicate a stream handle to read concurrently with
    // the main thread's sends.
    let (mut host_to_engine, mut control) =
        ipc_protocol::transport::connect_local_pair(&socket_id)
            .expect("connect to browser-ui control sockets");

    // A dedicated IO thread owns receiving HostToEngine messages so the
    // main thread stays free to drive Servo's event loop (Servo is not
    // Send, so it must live entirely on this thread).
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

    let rendering_context = Rc::new(
        SoftwareRenderingContext::new(PhysicalSize::new(DEFAULT_WIDTH, DEFAULT_HEIGHT))
            .expect("create SoftwareRenderingContext"),
    );
    // Matches servo's own headless test harness
    // (components/servo/tests/common/mod.rs): make_current() right after
    // construction, nothing else. No prepare_for_rendering() call — an
    // earlier attempt added one alongside make_current() and it made things
    // worse, not better; the reference code doesn't call it at all here.
    rendering_context
        .make_current()
        .expect("make rendering context current");

    let servo = ServoBuilder::default()
        .event_loop_waker(Box::new(NoopWaker))
        .build();

    let delegate = Rc::new(Delegate {
        frame_ready: RefCell::new(false),
        nav: RefCell::new(NavState {
            url: initial_url.clone(),
            ..Default::default()
        }),
        favicon: RefCell::new(None),
    });

    let webview = WebViewBuilder::new(&servo, rendering_context.clone() as Rc<dyn RenderingContext>)
        .url(Url::parse(&initial_url).expect("parse initial url"))
        .delegate(delegate.clone())
        .build();

    let mut frame_buffer = SharedFrameBuffer::create(&socket_id, DEFAULT_WIDTH, DEFAULT_HEIGHT)
        .expect("create shared frame buffer");
    let mut frame_id: u64 = 0;
    let mut width = DEFAULT_WIDTH;
    let mut height = DEFAULT_HEIGHT;
    // M5: background tabs stay alive but stop sending frames; browser-ui
    // defaults every newly spawned tab to visible and explicitly turns it
    // off right after if it isn't the focused one, so `true` here matches
    // pre-M5 single-tab behavior for any caller that never sends this.
    let mut visible = true;

    loop {
        servo.spin_event_loop();

        while let Ok(msg) = rx.try_recv() {
            match msg {
                HostToEngine::Navigate { url, .. } => {
                    if let Ok(parsed) = Url::parse(&url) {
                        webview.load(parsed);
                    }
                }
                HostToEngine::GoBack => {
                    webview.go_back(1);
                }
                HostToEngine::GoForward => {
                    webview.go_forward(1);
                }
                HostToEngine::Resize {
                    width: w,
                    height: h,
                    ..
                } => {
                    width = w;
                    height = h;
                    rendering_context.resize(PhysicalSize::new(w, h));
                    webview.resize(PhysicalSize::new(w, h));
                    // Must unmap the old buffer before truncating/recreating
                    // its backing file — Windows refuses to resize a file
                    // that still has a mapped section open, and assignment
                    // below wouldn't drop the old value until after the
                    // right-hand side (the new create() call) already ran.
                    drop(frame_buffer);
                    frame_buffer = SharedFrameBuffer::create(&socket_id, w, h)
                        .expect("recreate shared frame buffer at new size");
                }
                HostToEngine::Input(event) => {
                    if let Some(servo_event) = translate_input_event(event) {
                        webview.notify_input_event(servo_event);
                    }
                }
                HostToEngine::SetZoom { factor } => {
                    // `set_page_zoom` is a plain linear multiplier (1.0 =
                    // 100%, clamped internally to [0.1, 10.0]) — matches
                    // `factor`'s own semantics directly, no conversion
                    // needed (contrast CEF's log-scale zoom_level).
                    webview.set_page_zoom(factor as f32);
                }
                HostToEngine::SetVisible(v) => {
                    // Resuming from background: force one repaint even if
                    // `notify_new_frame_ready` never fired while hidden
                    // (static page, nothing changed) — otherwise the
                    // capture-and-FrameReady-send block below stays gated
                    // shut forever and browser-ui never gets a fresh
                    // texture to display.
                    if v && !visible {
                        *delegate.frame_ready.borrow_mut() = true;
                    }
                    visible = v;
                }
                HostToEngine::GetState => {
                    // Zoom still deferred (see the SetZoom TODO above) — url
                    // is the only piece M2's swap sequence actually replays.
                    let _ = control.send(&EngineToHost::StateSnapshot(ipc_protocol::TabState {
                        url: delegate.nav.borrow().url.clone(),
                        zoom: 1.0,
                    }));
                }
                // Servo's embedding API (the pinned 0.5.0 `WebView`) has no
                // find-in-page method at all — silent no-op, same per-engine
                // support gap as popup interception (see
                // `ipc_protocol::EngineToHost::OpenTabRequested`'s doc
                // comment). `browser-ui`'s find bar still opens on a Servo
                // tab; it just never receives a `FindResult`.
                // Same gap for DevTools: no inspector/DevTools method
                // exists in Servo's embedding API either. Same gap again
                // for mute: no audio control at all in this embedding API.
                HostToEngine::FindStart { .. }
                | HostToEngine::FindNext
                | HostToEngine::FindPrevious
                | HostToEngine::FindStop
                | HostToEngine::OpenDevTools
                | HostToEngine::SetMuted { .. } => {}
                HostToEngine::Shutdown => return,
            }
        }

        {
            let mut nav = delegate.nav.borrow_mut();
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

        if let Some((rgba, width, height)) = delegate.favicon.borrow_mut().take() {
            let _ = control.send(&EngineToHost::FaviconChanged { rgba, width, height });
        }

        if visible && *delegate.frame_ready.borrow() {
            *delegate.frame_ready.borrow_mut() = false;
            webview.paint();
            let rect = euclid::Box2D::from_size(euclid::Size2D::new(width as i32, height as i32));
            if let Some(image) = rendering_context.read_to_image(rect) {
                frame_buffer.write_frame(image.as_raw());
                frame_id += 1;
                let _ = control.send(&EngineToHost::FrameReady {
                    frame_id,
                    width,
                    height,
                    damage: None,
                    pixels: None,
                });
            }
            rendering_context.present();
        }

        std::thread::sleep(Duration::from_millis(8));
    }
}

fn translate_input_event(event: HostInputEvent) -> Option<ServoInputEvent> {
    match event {
        HostInputEvent::MouseMove { x, y } => Some(ServoInputEvent::MouseMove(
            MouseMoveEvent::new(webview_point(x, y)),
        )),
        HostInputEvent::MouseDown { x, y, button } => Some(ServoInputEvent::MouseButton(
            MouseButtonEvent::new(
                MouseButtonAction::Down,
                translate_mouse_button(button),
                webview_point(x, y),
            ),
        )),
        HostInputEvent::MouseUp { x, y, button } => Some(ServoInputEvent::MouseButton(
            MouseButtonEvent::new(
                MouseButtonAction::Up,
                translate_mouse_button(button),
                webview_point(x, y),
            ),
        )),
        HostInputEvent::Scroll {
            x,
            y,
            delta_x,
            delta_y,
        } => Some(ServoInputEvent::Wheel(WheelEvent::new(
            WheelDelta {
                x: delta_x,
                y: delta_y,
                z: 0.0,
                mode: WheelMode::DeltaPixel,
            },
            webview_point(x, y),
        ))),
        HostInputEvent::KeyDown { key, modifiers } => Some(ServoInputEvent::Keyboard(
            KeyboardEvent::new_without_event(
                KeyState::Down,
                key_to_servo_key(key),
                key_to_servo_code(key),
                Location::Standard,
                translate_modifiers(modifiers),
                false,
                false,
            ),
        )),
        HostInputEvent::KeyUp { key, modifiers } => Some(ServoInputEvent::Keyboard(
            KeyboardEvent::new_without_event(
                KeyState::Up,
                key_to_servo_key(key),
                key_to_servo_code(key),
                Location::Standard,
                translate_modifiers(modifiers),
                false,
                false,
            ),
        )),
        // Text insertion drives off `Key::Character`, which is what a page's
        // `<input>`/`<textarea>` actually reads — not `KeyDown`'s `code`
        // (see `key_to_servo_key`'s doc comment on why that's a best-effort
        // guess and not the real produced text for most keys).
        HostInputEvent::Char { codepoint } => {
            let ch = char::from_u32(codepoint)?;
            Some(ServoInputEvent::Keyboard(
                KeyboardEvent::from_state_and_key(KeyState::Down, Key::Character(ch.to_string())),
            ))
        }
    }
}

/// The *logical* key (what a DOM `KeyboardEvent.key` would read) for a
/// `KeyDown`/`KeyUp` — used for arrow/enter/backspace/etc. shortcut
/// handling, where the physical key's identity is what matters. Letters,
/// digits, and space map to `Key::Named(NamedKey::Unidentified)` rather
/// than a guessed `Key::Character` — the *correct* produced text arrives
/// separately via `InputEvent::Char` (see `translate_input_event`'s `Char`
/// arm), and sending a guessed character here too double-inserts text
/// (confirmed by hand: typing showed up doubled, e.g. "Hello" as
/// "hHeellllooo", before this was `Unidentified`). Shortcuts mostly key
/// off `code` anyway, not `key`, so `Unidentified` here costs nothing real.
fn key_to_servo_key(key: HostKeyCode) -> Key {
    match key {
        HostKeyCode::KeyA => Key::Named(NamedKey::Unidentified),
        HostKeyCode::KeyB => Key::Named(NamedKey::Unidentified),
        HostKeyCode::KeyC => Key::Named(NamedKey::Unidentified),
        HostKeyCode::KeyD => Key::Named(NamedKey::Unidentified),
        HostKeyCode::KeyE => Key::Named(NamedKey::Unidentified),
        HostKeyCode::KeyF => Key::Named(NamedKey::Unidentified),
        HostKeyCode::KeyG => Key::Named(NamedKey::Unidentified),
        HostKeyCode::KeyH => Key::Named(NamedKey::Unidentified),
        HostKeyCode::KeyI => Key::Named(NamedKey::Unidentified),
        HostKeyCode::KeyJ => Key::Named(NamedKey::Unidentified),
        HostKeyCode::KeyK => Key::Named(NamedKey::Unidentified),
        HostKeyCode::KeyL => Key::Named(NamedKey::Unidentified),
        HostKeyCode::KeyM => Key::Named(NamedKey::Unidentified),
        HostKeyCode::KeyN => Key::Named(NamedKey::Unidentified),
        HostKeyCode::KeyO => Key::Named(NamedKey::Unidentified),
        HostKeyCode::KeyP => Key::Named(NamedKey::Unidentified),
        HostKeyCode::KeyQ => Key::Named(NamedKey::Unidentified),
        HostKeyCode::KeyR => Key::Named(NamedKey::Unidentified),
        HostKeyCode::KeyS => Key::Named(NamedKey::Unidentified),
        HostKeyCode::KeyT => Key::Named(NamedKey::Unidentified),
        HostKeyCode::KeyU => Key::Named(NamedKey::Unidentified),
        HostKeyCode::KeyV => Key::Named(NamedKey::Unidentified),
        HostKeyCode::KeyW => Key::Named(NamedKey::Unidentified),
        HostKeyCode::KeyX => Key::Named(NamedKey::Unidentified),
        HostKeyCode::KeyY => Key::Named(NamedKey::Unidentified),
        HostKeyCode::KeyZ => Key::Named(NamedKey::Unidentified),
        HostKeyCode::Digit0 => Key::Named(NamedKey::Unidentified),
        HostKeyCode::Digit1 => Key::Named(NamedKey::Unidentified),
        HostKeyCode::Digit2 => Key::Named(NamedKey::Unidentified),
        HostKeyCode::Digit3 => Key::Named(NamedKey::Unidentified),
        HostKeyCode::Digit4 => Key::Named(NamedKey::Unidentified),
        HostKeyCode::Digit5 => Key::Named(NamedKey::Unidentified),
        HostKeyCode::Digit6 => Key::Named(NamedKey::Unidentified),
        HostKeyCode::Digit7 => Key::Named(NamedKey::Unidentified),
        HostKeyCode::Digit8 => Key::Named(NamedKey::Unidentified),
        HostKeyCode::Digit9 => Key::Named(NamedKey::Unidentified),
        HostKeyCode::Space => Key::Named(NamedKey::Unidentified),
        HostKeyCode::Enter => Key::Named(NamedKey::Enter),
        HostKeyCode::Backspace => Key::Named(NamedKey::Backspace),
        HostKeyCode::Tab => Key::Named(NamedKey::Tab),
        HostKeyCode::Escape => Key::Named(NamedKey::Escape),
        HostKeyCode::Delete => Key::Named(NamedKey::Delete),
        HostKeyCode::ArrowLeft => Key::Named(NamedKey::ArrowLeft),
        HostKeyCode::ArrowRight => Key::Named(NamedKey::ArrowRight),
        HostKeyCode::ArrowUp => Key::Named(NamedKey::ArrowUp),
        HostKeyCode::ArrowDown => Key::Named(NamedKey::ArrowDown),
        HostKeyCode::Home => Key::Named(NamedKey::Home),
        HostKeyCode::End => Key::Named(NamedKey::End),
        HostKeyCode::PageUp => Key::Named(NamedKey::PageUp),
        HostKeyCode::PageDown => Key::Named(NamedKey::PageDown),
        HostKeyCode::ShiftLeft | HostKeyCode::ShiftRight => Key::Named(NamedKey::Shift),
        HostKeyCode::ControlLeft | HostKeyCode::ControlRight => Key::Named(NamedKey::Control),
        HostKeyCode::AltLeft | HostKeyCode::AltRight => Key::Named(NamedKey::Alt),
    }
}

fn key_to_servo_code(key: HostKeyCode) -> Code {
    match key {
        HostKeyCode::KeyA => Code::KeyA,
        HostKeyCode::KeyB => Code::KeyB,
        HostKeyCode::KeyC => Code::KeyC,
        HostKeyCode::KeyD => Code::KeyD,
        HostKeyCode::KeyE => Code::KeyE,
        HostKeyCode::KeyF => Code::KeyF,
        HostKeyCode::KeyG => Code::KeyG,
        HostKeyCode::KeyH => Code::KeyH,
        HostKeyCode::KeyI => Code::KeyI,
        HostKeyCode::KeyJ => Code::KeyJ,
        HostKeyCode::KeyK => Code::KeyK,
        HostKeyCode::KeyL => Code::KeyL,
        HostKeyCode::KeyM => Code::KeyM,
        HostKeyCode::KeyN => Code::KeyN,
        HostKeyCode::KeyO => Code::KeyO,
        HostKeyCode::KeyP => Code::KeyP,
        HostKeyCode::KeyQ => Code::KeyQ,
        HostKeyCode::KeyR => Code::KeyR,
        HostKeyCode::KeyS => Code::KeyS,
        HostKeyCode::KeyT => Code::KeyT,
        HostKeyCode::KeyU => Code::KeyU,
        HostKeyCode::KeyV => Code::KeyV,
        HostKeyCode::KeyW => Code::KeyW,
        HostKeyCode::KeyX => Code::KeyX,
        HostKeyCode::KeyY => Code::KeyY,
        HostKeyCode::KeyZ => Code::KeyZ,
        HostKeyCode::Digit0 => Code::Digit0,
        HostKeyCode::Digit1 => Code::Digit1,
        HostKeyCode::Digit2 => Code::Digit2,
        HostKeyCode::Digit3 => Code::Digit3,
        HostKeyCode::Digit4 => Code::Digit4,
        HostKeyCode::Digit5 => Code::Digit5,
        HostKeyCode::Digit6 => Code::Digit6,
        HostKeyCode::Digit7 => Code::Digit7,
        HostKeyCode::Digit8 => Code::Digit8,
        HostKeyCode::Digit9 => Code::Digit9,
        HostKeyCode::Enter => Code::Enter,
        HostKeyCode::Backspace => Code::Backspace,
        HostKeyCode::Tab => Code::Tab,
        HostKeyCode::Space => Code::Space,
        HostKeyCode::Escape => Code::Escape,
        HostKeyCode::Delete => Code::Delete,
        HostKeyCode::ArrowLeft => Code::ArrowLeft,
        HostKeyCode::ArrowRight => Code::ArrowRight,
        HostKeyCode::ArrowUp => Code::ArrowUp,
        HostKeyCode::ArrowDown => Code::ArrowDown,
        HostKeyCode::Home => Code::Home,
        HostKeyCode::End => Code::End,
        HostKeyCode::PageUp => Code::PageUp,
        HostKeyCode::PageDown => Code::PageDown,
        HostKeyCode::ShiftLeft => Code::ShiftLeft,
        HostKeyCode::ShiftRight => Code::ShiftRight,
        HostKeyCode::ControlLeft => Code::ControlLeft,
        HostKeyCode::ControlRight => Code::ControlRight,
        HostKeyCode::AltLeft => Code::AltLeft,
        HostKeyCode::AltRight => Code::AltRight,
    }
}

fn translate_modifiers(bits: u8) -> Modifiers {
    let mut modifiers = Modifiers::empty();
    if bits & host_modifiers::SHIFT != 0 {
        modifiers |= Modifiers::SHIFT;
    }
    if bits & host_modifiers::CONTROL != 0 {
        modifiers |= Modifiers::CONTROL;
    }
    if bits & host_modifiers::ALT != 0 {
        modifiers |= Modifiers::ALT;
    }
    if bits & host_modifiers::SUPER != 0 {
        // `Modifiers::SUPER` is deprecated in `keyboard_types` (legacy name
        // from an earlier draft of the UI Events spec) in favor of `META`,
        // which is what the DOM's `KeyboardEvent.metaKey` actually checks.
        modifiers |= Modifiers::META;
    }
    modifiers
}

fn webview_point(x: f64, y: f64) -> WebViewPoint {
    WebViewPoint::Device(DevicePoint::new(x as f32, y as f32))
}

fn translate_mouse_button(button: ipc_protocol::MouseButton) -> ServoMouseButton {
    // Our pinned `servo = "0.5.0"` still uses Left/Middle/Right; the
    // W3C-aligned Primary/Auxiliary/Secondary rename (visible on
    // doc.servo.org, which tracks the `main` branch) hasn't been published
    // to crates.io yet.
    match button {
        ipc_protocol::MouseButton::Left => ServoMouseButton::Left,
        ipc_protocol::MouseButton::Middle => ServoMouseButton::Middle,
        ipc_protocol::MouseButton::Right => ServoMouseButton::Right,
    }
}
