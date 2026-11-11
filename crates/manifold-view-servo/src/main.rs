use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dpi::PhysicalSize;
use manifold_view_protocol::wake::WakeEvent;
use manifold_view_protocol::{
    modifiers as host_modifiers, HostToView, InputEvent as HostInputEvent, KeyCode as HostKeyCode, ScrollDelta,
    ViewToHost,
};
use servo::{
    Code, EventLoopWaker, InputEvent as ServoInputEvent, JSValue, Key, KeyState, KeyboardEvent, Location, Modifiers,
    MouseButton as ServoMouseButton, MouseButtonAction, MouseButtonEvent, MouseMoveEvent, NamedKey, RenderingContext,
    ServoBuilder, SoftwareRenderingContext, WebViewBuilder, WebViewPoint, WheelDelta, WheelEvent, WheelMode,
};
use url::Url;

mod pages;
mod views;
use views::Delegate;
use webrender_api::units::DevicePoint;

const DEFAULT_WIDTH: u32 = 1024;
const DEFAULT_HEIGHT: u32 = 768;
/// CSS px a wheel notch scrolls: Edge's 100 at Windows' default 3 lines a notch.
const NOTCH_PX: f64 = 100.0;

/// Ends the host loop's wait: Servo calls `wake` from any thread when it wants `spin_event_loop`.
#[derive(Clone)]
struct LoopWaker(Arc<WakeEvent>);

impl EventLoopWaker for LoopWaker {
    fn clone_box(&self) -> Box<dyn EventLoopWaker> {
        Box::new(self.clone())
    }
    fn wake(&self) {
        self.0.set();
    }
}

fn main() {
    // Servo needs a process-level TLS crypto provider installed before
    // anything touches HTTPS (matches servo's own components/servo/examples/
    // winit_minimal.rs, which does this before constructing anything else).
    rustls::crypto::aws_lc_rs::default_provider().install_default().expect("install rustls crypto provider");

    // Env vars, not argv, as for every view-host (`manifold_view_protocol::host_env`).
    let manifold_view_protocol::HostEnv { socket_id, initial_url, settings, .. } =
        manifold_view_protocol::HostEnv::from_env();
    manifold_view_protocol::host_env::apply(&settings);

    // Two one-directional control sockets back to manifold (it already
    // created both listeners before spawning this process — see
    // manifold_view_protocol::transport::listen_local_pair). Using two sockets
    // instead of one bidirectional stream means the reader thread below
    // never needs to duplicate a stream handle to read concurrently with
    // the main thread's sends.
    let (host_to_view, mut control) =
        manifold_view_protocol::transport::connect_local_pair(&socket_id).expect("connect to manifold control sockets");

    // A dedicated IO thread owns receiving HostToView messages so the
    // main thread stays free to drive Servo's event loop (Servo is not
    // Send, so it must live entirely on this thread). Each message ends the loop's wait, as Servo's own wakes do.
    let wake = Arc::new(WakeEvent::new().expect("wake event"));
    let rx = manifold_view_protocol::transport::spawn_host_reader(host_to_view, {
        let wake = wake.clone();
        move || wake.set()
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
    rendering_context.make_current().expect("make rendering context current");

    let servo = ServoBuilder::default()
        .event_loop_waker(Box::new(LoopWaker(wake.clone())))
        .protocol_registry(pages::registry())
        .build();
    // Servo's own `log` output reaches the per-tab log only through its logger: errors by default, more with
    // `RUST_LOG` (e.g. `servo_constellation=warn` shows a load the constellation drops).
    servo.setup_logging();

    // Drained as late as possible before the view is built (see `take_start_url`); the rest is handled by the loop.
    let (initial_url, early) = take_start_url(initial_url, rx.try_iter().collect());
    let mut early = early.into_iter();

    let delegate = Delegate::new(initial_url.clone());

    // Filled by the `QueryPageState` script callback, drained and sent each tick.
    let pending_page_state: Rc<RefCell<Option<manifold_view_protocol::PageState>>> = Rc::new(RefCell::new(None));
    // Filled by the `QueryPageText` script callback, drained and sent each tick.
    let pending_page_text: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));

    // As the WebView2 host: an initial URL that doesn't parse opens about:blank instead of panicking.
    let start = Url::parse(&initial_url).unwrap_or_else(|err| {
        eprintln!("initial url {initial_url:?} doesn't parse ({err}); opening about:blank");
        Url::parse("about:blank").expect("about:blank parses")
    });
    let webview = WebViewBuilder::new(&servo, rendering_context.clone() as Rc<dyn RenderingContext>)
        .url(start)
        .delegate(delegate.clone())
        .build();

    let mut sink = manifold_view_protocol::FrameSink::shared(&socket_id);
    let mut width = DEFAULT_WIDTH;
    let mut height = DEFAULT_HEIGHT;

    loop {
        servo.spin_event_loop();

        while let Some(msg) = early.next().or_else(|| rx.try_recv().ok()) {
            match msg {
                HostToView::Navigate { url, .. } => {
                    if let Ok(parsed) = Url::parse(&url) {
                        webview.load(parsed);
                    }
                }
                HostToView::GoBack => {
                    if delegate.nav.borrow().can_go_back {
                        webview.go_back(1);
                    }
                }
                HostToView::GoForward => {
                    if delegate.nav.borrow().can_go_forward {
                        webview.go_forward(1);
                    }
                }
                HostToView::Reload => {
                    webview.reload();
                }
                HostToView::Resize { width: w, height: h, .. } => {
                    // A minimized window delivers a real 0x0 `inner_size()`
                    // on Windows — nothing to render either way, and a
                    // zero-length shared frame buffer can't be mapped at
                    // all, so skip the resize entirely rather than
                    // panicking on the next real frame this tab would need.
                    if w == 0 || h == 0 {
                        continue;
                    }
                    width = w;
                    height = h;
                    // Only the webview: it resizes the rendering context itself, and Servo's painter skips its
                    // viewport update when the context already has the new size (resizing it here left the page
                    // laid out at the old width).
                    webview.resize(PhysicalSize::new(w, h));
                    // A static page paints nothing by itself after a resize, so the old frame would stay on screen
                    // stretched: force one paint at the new size (as `SetVisible` does).
                    *delegate.frame_ready.borrow_mut() = true;
                }
                HostToView::Input(event) => {
                    if let Some(servo_event) = translate_input_event(event) {
                        webview.notify_input_event(servo_event);
                    }
                }
                HostToView::SetZoom { factor } => {
                    // `set_page_zoom` is a plain linear multiplier (1.0 =
                    // 100%, clamped internally to [0.1, 10.0]) — matches
                    // `factor`'s own semantics directly, no conversion
                    // needed (contrast CEF's log-scale zoom_level).
                    webview.set_page_zoom(factor as f32);
                }
                HostToView::SetVisible(v) => {
                    // Resuming from background: force one repaint even if
                    // `notify_new_frame_ready` never fired while hidden
                    // (static page, nothing changed) — otherwise the
                    // capture-and-FrameReady-send block below stays gated
                    // shut forever and manifold never gets a fresh
                    // texture to display.
                    if v && !sink.visible() {
                        *delegate.frame_ready.borrow_mut() = true;
                    }
                    sink.set_visible(v);
                }
                HostToView::GetState => {
                    for m in delegate.nav.borrow_mut().answer_get_state() {
                        let _ = control.send(&m);
                    }
                }
                // This engine has no cookie jar to seed or read; stays silent.
                HostToView::SetCookies(_) | HostToView::GetCookies => {}
                HostToView::QueryPageState => {
                    let pending = pending_page_state.clone();
                    webview.evaluate_javascript(manifold_view_protocol::PAGE_STATE_SCRIPT, move |result| {
                        if let Ok(JSValue::String(text)) = result {
                            if let Some(state) = manifold_view_protocol::PageState::parse(&text) {
                                *pending.borrow_mut() = Some(state);
                            }
                        }
                    });
                }
                HostToView::QueryPageText => {
                    let pending = pending_page_text.clone();
                    webview.evaluate_javascript(manifold_view_protocol::PAGE_TEXT_SCRIPT, move |result| {
                        if let Ok(JSValue::String(raw)) = result {
                            if let Some(text) = manifold_view_protocol::page_text_reply(Some(&raw)) {
                                *pending.borrow_mut() = Some(text);
                            }
                        }
                    });
                }
                HostToView::RestoreScroll { x, y } => {
                    webview.evaluate_javascript(manifold_view_protocol::restore_scroll_script(x, y), |_| {});
                }
                // Servo's embedding API (`WebView`) has no
                // find-in-page method at all — silent no-op, same per-engine
                // support gap as popup interception (see
                // `manifold_view_protocol::ViewToHost::OpenTabRequested`'s doc
                // comment). `manifold`'s find bar still opens on a Servo
                // tab; it just never receives a `FindResult`.
                // Same gap for DevTools: no inspector/DevTools method
                // exists in Servo's embedding API either. Same gap again
                // for mute: no audio control at all in this embedding API.
                HostToView::FindStart { .. }
                | HostToView::FindNext
                | HostToView::FindPrevious
                | HostToView::FindStop
                | HostToView::OpenDevTools
                | HostToView::SetMuted { .. } => {}
                HostToView::SetSettings(settings) => manifold_view_protocol::host_env::apply(&settings),
                HostToView::Shutdown => return,
            }
        }

        // Servo's `NavigationRequest` carries only the url, so after the host's own first navigation
        // (`NavState::new`) `nav.label` stays at its default and every message says `user_started: None`.
        if let Some(m) = delegate.nav.borrow_mut().take_message() {
            let _ = control.send(&m);
        }

        if let Some((x, y, href)) = delegate.context_menu_link.borrow_mut().take() {
            let _ = control.send(&ViewToHost::ContextMenuLink { x, y, href });
        }

        if let Some(body) = delegate.json_body.borrow_mut().take() {
            let _ = control.send(&ViewToHost::JsonBody { body });
        }

        if let Some(state) = pending_page_state.borrow_mut().take() {
            let _ = control.send(&ViewToHost::PageState(state));
        }

        if let Some(text) = pending_page_text.borrow_mut().take() {
            let _ = control.send(&ViewToHost::PageText { text });
        }

        if let Some(content_type) = delegate.redirect_content_type.borrow_mut().take() {
            let _ = control.send(&ViewToHost::RedirectableContent { content_type });
        }

        if sink.visible() && *delegate.frame_ready.borrow() {
            *delegate.frame_ready.borrow_mut() = false;
            webview.paint();
            let rect = euclid::Box2D::from_size(euclid::Size2D::new(width as i32, height as i32));
            if let Some(image) = rendering_context.read_to_image(rect) {
                sink.offer(image.as_raw(), image.width() as usize * 4, image.width(), image.height());
            }
            rendering_context.present();
        }
        // Outside the paint gate: a failed create is retried on later turns without a new paint (at worst at the
        // wait's 1 s backstop).
        if let Some(msg) = sink.take() {
            let _ = control.send(&msg);
        }

        // Nothing in this loop is due on a clock: it waits for Servo or a message, and the wait's backstop bounds a
        // missed wake.
        wake.wait(Instant::now() + Duration::from_secs(60));
    }
}

/// Servo drops a load sent before a new view's first document commits (PARKED "Servo drops a load sent before the
/// previous one commits", case (a)), so the newest `Navigate` that arrived while the host started (one whose url
/// parses, as the loop requires) becomes the view's first load instead. Returns that url and the other early
/// messages, in order.
fn take_start_url(initial_url: String, early: Vec<HostToView>) -> (String, Vec<HostToView>) {
    let mut start = initial_url;
    let mut rest = Vec::new();
    for msg in early {
        match msg {
            HostToView::Navigate { url, .. } => {
                if Url::parse(&url).is_ok() {
                    start = url;
                }
            }
            other => rest.push(other),
        }
    }
    (start, rest)
}

fn translate_input_event(event: HostInputEvent) -> Option<ServoInputEvent> {
    match event {
        HostInputEvent::MouseMove { x, y, .. } => {
            Some(ServoInputEvent::MouseMove(MouseMoveEvent::new(webview_point(x, y))))
        }
        HostInputEvent::MouseDown { x, y, button, .. } => Some(ServoInputEvent::MouseButton(MouseButtonEvent::new(
            MouseButtonAction::Down,
            translate_mouse_button(button),
            webview_point(x, y),
        ))),
        HostInputEvent::MouseUp { x, y, button, .. } => Some(ServoInputEvent::MouseButton(MouseButtonEvent::new(
            MouseButtonAction::Up,
            translate_mouse_button(button),
            webview_point(x, y),
        ))),
        HostInputEvent::Scroll { x, y, delta, .. } => {
            // Servo scrolls by the pixels it is given whatever the mode (the mode is what the page's `wheel` event
            // reports), and leaves a notch's distance to the embedder: servoshell uses 76 px, this host Edge's 100
            // (`wheel::WheelUnits`), so a notch goes as far as in the Chromium tabs. Its CSS px are the surface's.
            // Both are sent as pixels, as Edge reports a notch to the page.
            let (x_px, y_px) = match delta {
                ScrollDelta::Lines { x, y } => (x * NOTCH_PX, y * NOTCH_PX),
                ScrollDelta::Pixels { x, y } => (x, y),
            };
            Some(ServoInputEvent::Wheel(WheelEvent::new(
                WheelDelta { x: x_px, y: y_px, z: 0.0, mode: WheelMode::DeltaPixel },
                webview_point(x, y),
            )))
        }
        HostInputEvent::KeyDown { key, modifiers } => {
            Some(ServoInputEvent::Keyboard(KeyboardEvent::new_without_event(
                KeyState::Down,
                key_to_servo_key(key),
                key_to_servo_code(key),
                Location::Standard,
                translate_modifiers(modifiers),
                false,
                false,
            )))
        }
        HostInputEvent::KeyUp { key, modifiers } => Some(ServoInputEvent::Keyboard(KeyboardEvent::new_without_event(
            KeyState::Up,
            key_to_servo_key(key),
            key_to_servo_code(key),
            Location::Standard,
            translate_modifiers(modifiers),
            false,
            false,
        ))),
        // Text insertion drives off `Key::Character`, which is what a page's
        // `<input>`/`<textarea>` actually reads — not `KeyDown`'s `code`
        // (see `key_to_servo_key`'s doc comment on why that's a best-effort
        // guess and not the real produced text for most keys).
        HostInputEvent::Char { codepoint } => {
            let ch = char::from_u32(codepoint)?;
            Some(ServoInputEvent::Keyboard(KeyboardEvent::from_state_and_key(
                KeyState::Down,
                Key::Character(ch.to_string()),
            )))
        }
    }
}

/// The *logical* key (what a DOM `KeyboardEvent.key` would read) for a
/// `KeyDown`/`KeyUp` — used for arrow/enter/backspace/etc. shortcut
/// handling, where the physical key's identity is what matters. Letters,
/// digits, and space (`KeyCode::produces_text`) map to
/// `Key::Named(NamedKey::Unidentified)` rather than a guessed
/// `Key::Character` — the *correct* produced text arrives separately via
/// `InputEvent::Char` (see `translate_input_event`'s `Char` arm), and
/// sending a guessed character here too double-inserts text (confirmed by
/// hand: typing showed up doubled, e.g. "Hello" as "hHeellllooo", before
/// this was `Unidentified`). Shortcuts mostly key off `code` anyway, not
/// `key`, so `Unidentified` here costs nothing real.
fn key_to_servo_key(key: HostKeyCode) -> Key {
    if key.produces_text() {
        return Key::Named(NamedKey::Unidentified);
    }
    match key {
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
        // Text keys returned above; a new non-text key lands here until it gets its own arm.
        _ => Key::Named(NamedKey::Unidentified),
    }
}

/// `KeyCode::dom_code` is the DOM `KeyboardEvent.code` string, which is what `Code` parses.
fn key_to_servo_code(key: HostKeyCode) -> Code {
    key.dom_code().parse().expect("every KeyCode's DOM code is a keyboard_types::Code")
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

fn translate_mouse_button(button: manifold_view_protocol::MouseButton) -> ServoMouseButton {
    match button {
        manifold_view_protocol::MouseButton::Left => ServoMouseButton::Primary,
        manifold_view_protocol::MouseButton::Middle => ServoMouseButton::Auxiliary,
        manifold_view_protocol::MouseButton::Right => ServoMouseButton::Secondary,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_newest_early_navigate_becomes_the_start_url() {
        let early = vec![
            HostToView::Navigate { url: "https://a.test/".into(), referrer: None },
            HostToView::SetVisible(true),
            HostToView::Navigate { url: "https://b.test/".into(), referrer: None },
            HostToView::Navigate { url: "not a url".into(), referrer: None },
        ];
        let (start, rest) = take_start_url("https://initial.test/".into(), early);
        assert_eq!(start, "https://b.test/");
        assert!(matches!(rest[..], [HostToView::SetVisible(true)]), "{rest:?}");
    }

    #[test]
    fn without_an_early_navigate_the_initial_url_stays() {
        let (start, rest) = take_start_url("https://initial.test/".into(), vec![HostToView::Reload]);
        assert_eq!(start, "https://initial.test/");
        assert!(matches!(rest[..], [HostToView::Reload]), "{rest:?}");
    }

    #[test]
    fn every_host_key_has_a_servo_code() {
        for &key in HostKeyCode::ALL {
            assert_eq!(key_to_servo_code(key).to_string(), key.dom_code());
        }
    }
}
