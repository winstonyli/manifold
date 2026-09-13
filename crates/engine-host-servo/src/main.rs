use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use dpi::PhysicalSize;
use ipc_protocol::{EngineToHost, HostToEngine, InputEvent as HostInputEvent, SharedFrameBuffer};
use servo::{
    EventLoopWaker, InputEvent as ServoInputEvent, MouseButton as ServoMouseButton,
    MouseButtonAction, MouseButtonEvent, MouseMoveEvent, RenderingContext, ServoBuilder,
    SoftwareRenderingContext, WebView, WebViewBuilder, WebViewDelegate, WebViewPoint, WheelDelta,
    WheelEvent, WheelMode,
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

struct Delegate {
    frame_ready: RefCell<bool>,
}

impl WebViewDelegate for Delegate {
    fn notify_new_frame_ready(&self, _webview: WebView) {
        *self.frame_ready.borrow_mut() = true;
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
    let tab_id = std::env::var("ENGINE_TAB_ID").expect("ENGINE_TAB_ID env var required");
    let initial_url = std::env::var("ENGINE_INITIAL_URL")
        .unwrap_or_else(|_| "https://example.com".to_string());

    // Two one-directional control sockets back to browser-ui (it already
    // created both listeners before spawning this process — see
    // ipc_protocol::transport::listen_local_pair). Using two sockets
    // instead of one bidirectional stream means the reader thread below
    // never needs to duplicate a stream handle to read concurrently with
    // the main thread's sends.
    let (mut host_to_engine, mut control) = ipc_protocol::transport::connect_local_pair(&tab_id)
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
    });

    let webview = WebViewBuilder::new(&servo, rendering_context.clone() as Rc<dyn RenderingContext>)
        .url(Url::parse(&initial_url).expect("parse initial url"))
        .delegate(delegate.clone())
        .build();

    let mut frame_buffer = SharedFrameBuffer::create(&tab_id, DEFAULT_WIDTH, DEFAULT_HEIGHT)
        .expect("create shared frame buffer");
    let mut frame_id: u64 = 0;
    let mut width = DEFAULT_WIDTH;
    let mut height = DEFAULT_HEIGHT;
    let mut current_url = initial_url;

    loop {
        servo.spin_event_loop();

        while let Ok(msg) = rx.try_recv() {
            match msg {
                HostToEngine::Navigate { url, .. } => {
                    if let Ok(parsed) = Url::parse(&url) {
                        webview.load(parsed);
                        current_url = url;
                    }
                }
                HostToEngine::LoadHistoryEntry { index } => {
                    // Positive amount = forward, negative = backward; Servo's
                    // WebView only exposes relative go_back/go_forward, so
                    // absolute-index history replay is deferred to M4 when
                    // TabState round-tripping is wired up end to end.
                    let _ = index;
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
                    frame_buffer = SharedFrameBuffer::create(&tab_id, w, h)
                        .expect("recreate shared frame buffer at new size");
                }
                HostToEngine::Input(event) => {
                    if let Some(servo_event) = translate_input_event(event) {
                        webview.notify_input_event(servo_event);
                    }
                }
                HostToEngine::SetZoom { .. } => {
                    // TODO(M4): WebView doesn't expose a direct zoom setter in
                    // this API surface yet; revisit once tab UI needs it.
                }
                HostToEngine::GetState => {
                    // History/zoom still deferred to M4 (see the
                    // LoadHistoryEntry/SetZoom TODOs above) — url is the
                    // only piece M2's swap sequence actually replays.
                    let _ = control.send(&EngineToHost::StateSnapshot(ipc_protocol::TabState {
                        url: current_url.clone(),
                        history: vec![],
                        zoom: 1.0,
                    }));
                }
                HostToEngine::Shutdown => return,
            }
        }

        if *delegate.frame_ready.borrow() {
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
        // TODO(M1 follow-up): keyboard input needs mapping into Servo's
        // KeyboardEvent (wraps a keyboard-types::KeyEvent), which needs a
        // real keycode/modifier translation table from winit's key enum —
        // out of scope for M1's mouse+scroll verification.
        HostInputEvent::KeyDown { .. }
        | HostInputEvent::KeyUp { .. }
        | HostInputEvent::Char { .. } => None,
    }
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
