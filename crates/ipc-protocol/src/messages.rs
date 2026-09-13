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

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum InputEvent {
    MouseMove { x: f64, y: f64 },
    MouseDown { x: f64, y: f64, button: MouseButton },
    MouseUp { x: f64, y: f64, button: MouseButton },
    Scroll { x: f64, y: f64, delta_x: f64, delta_y: f64 },
    KeyDown { keycode: u32, modifiers: u8 },
    KeyUp { keycode: u32, modifiers: u8 },
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
