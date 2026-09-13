use serde::{Deserialize, Serialize};

/// Engine-agnostic snapshot of a tab. Cookies/localStorage/IndexedDB are
/// intentionally excluded: they live inside each engine's own storage
/// backend and do not transfer across an engine swap. A full navigable
/// history list is excluded too — CEF and WebKitGTK's embedding APIs (and
/// Servo's, going by the pinned 0.5.0 `WebView`) only expose *relative*
/// back/forward navigation plus can-go-back/can-go-forward flags, not an
/// enumerable/indexable entry list, so there is nothing to snapshot beyond
/// the current URL. See `HostToEngine::GoBack`/`GoForward`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TabState {
    pub url: String,
    pub zoom: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// A physical key, engine-agnostic — deliberately not winit's own `KeyCode`
/// (not guaranteed FFI/serialization-stable across winit versions) or any
/// one engine's native representation (Windows `VK_*` for CEF, a GDK keyval
/// for WebKitGTK, `keyboard_types::Code` for Servo). `browser-ui` translates
/// winit's key into this; each engine-host translates this into whatever
/// its own embedding API wants — see each engine-host's own translation
/// table, kept local to that crate the same way `MouseButton` translation
/// already is.
///
/// Deliberately not exhaustive — common keys only (letters, digits,
/// navigation, the usual modifiers and editing keys) covers real usage
/// (typing into a page's form fields, basic shortcuts) without trying to
/// enumerate every key on every keyboard layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyCode {
    KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM,
    KeyN, KeyO, KeyP, KeyQ, KeyR, KeyS, KeyT, KeyU, KeyV, KeyW, KeyX, KeyY, KeyZ,
    Digit0, Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9,
    Enter, Backspace, Tab, Space, Escape, Delete,
    ArrowLeft, ArrowRight, ArrowUp, ArrowDown,
    Home, End, PageUp, PageDown,
    ShiftLeft, ShiftRight, ControlLeft, ControlRight, AltLeft, AltRight,
}

/// Bitflags for `InputEvent::KeyDown`/`KeyUp`'s `modifiers` field — the
/// modifier keys held *at the time of* this key event, not the event's own
/// key (a `Shift` keypress itself also arrives as a normal `KeyDown` with
/// `Self::SHIFT` set). Combine with `|`; `browser-ui` is the only producer
/// (tracked from winit's `WindowEvent::ModifiersChanged`).
pub mod modifiers {
    pub const SHIFT: u8 = 1 << 0;
    pub const CONTROL: u8 = 1 << 1;
    pub const ALT: u8 = 1 << 2;
    pub const SUPER: u8 = 1 << 3;
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum InputEvent {
    MouseMove { x: f64, y: f64 },
    MouseDown { x: f64, y: f64, button: MouseButton },
    MouseUp { x: f64, y: f64, button: MouseButton },
    Scroll { x: f64, y: f64, delta_x: f64, delta_y: f64 },
    KeyDown { key: KeyCode, modifiers: u8 },
    KeyUp { key: KeyCode, modifiers: u8 },
    /// The actual text a keypress produced (after layout/shift/dead-key
    /// processing) — what a page's text field should actually insert.
    /// Sent alongside `KeyDown`, not instead of it: shortcuts need the
    /// physical key, typing needs the produced character, and they don't
    /// always correspond 1:1 (e.g. Shift+A produces one `KeyDown{KeyA}`
    /// plus one `Char{'A'}`, not two key events).
    Char { codepoint: u32 },
}

/// Messages sent from the browser-ui process manager to an engine-host.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HostToEngine {
    Navigate { url: String, referrer: Option<String> },
    GoBack,
    GoForward,
    Resize { width: u32, height: u32, dpi_scale: f64 },
    Input(InputEvent),
    SetZoom { factor: f64 },
    /// M5: background (non-focused) tabs stay alive but stop capturing and
    /// sending frames — only the currently focused tab is ever `true`. The
    /// engine's own event/message-loop pump keeps running regardless (the
    /// page keeps executing, title/url keep updating), only the final
    /// capture-and-send step is gated on this.
    SetVisible(bool),
    GetState,
    Shutdown,
}

/// Messages sent from an engine-host back to the browser-ui process manager.
///
/// `FrameReady`'s `pixels` field distinguishes the two transports: a
/// same-machine engine-host (Servo, CEF) leaves it `None` and writes pixels
/// into the `SharedFrameBuffer` named after the tab id instead (cheaper —
/// no copy through serialization); a WSL2-hosted engine-host (WPE/WebKit,
/// over `RemoteTransport`) has no shared memory with the Windows host
/// process, so it sends the raw pixel bytes inline here instead.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EngineToHost {
    FrameReady {
        frame_id: u64,
        width: u32,
        height: u32,
        damage: Option<(u32, u32, u32, u32)>,
        pixels: Option<Vec<u8>>,
    },
    NavigationStateChanged {
        url: String,
        title: String,
        can_go_back: bool,
        can_go_forward: bool,
        loading: bool,
    },
    FaviconChanged { bytes: Vec<u8> },
    StateSnapshot(TabState),
    Crashed { reason: String },
}
