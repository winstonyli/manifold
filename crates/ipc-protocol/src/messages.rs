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
    /// `modifiers` (see the `modifiers` module) are the modifier keys held
    /// *at the time of* this mouse event — added so a native-input-
    /// injection engine (currently only CEF; the others are script-driven
    /// and read modifier state some other way) can forward e.g. Ctrl+click/
    /// Shift+click to the page instead of always injecting a bare click.
    MouseMove { x: f64, y: f64, modifiers: u8 },
    MouseDown { x: f64, y: f64, button: MouseButton, modifiers: u8 },
    MouseUp { x: f64, y: f64, button: MouseButton, modifiers: u8 },
    Scroll { x: f64, y: f64, delta_x: f64, delta_y: f64, modifiers: u8 },
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
    /// Reloads the currently loaded page — every engine here exposes a
    /// plain no-argument reload on its own embedding API (`WebView::
    /// reload`/`Browser::reload`/`WebViewExt::reload`/`ICoreWebView2::
    /// Reload`), unlike `GoBack`/`GoForward`'s can-go-back/forward guard,
    /// so there's nothing to check before sending this.
    Reload,
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
    /// Begin (or replace) a find-in-page search for `text` in this tab,
    /// jumping to the nearest match. Sent when the find bar's text changes,
    /// or the bar opens with existing text. Supported by CEF, WebKit, and
    /// WebView2 — all three have a real native find API (see each
    /// engine-host's own find-handling code); Servo's embedding API has
    /// none, so this is a silent no-op there, same per-engine support gap
    /// as `EngineToHost::OpenTabRequested`.
    FindStart { text: String, match_case: bool },
    /// Advance to the next/previous match of the current find session (the
    /// most recent `FindStart`'s term) — Enter/Shift+Enter in the find bar.
    FindNext,
    FindPrevious,
    /// Ends the find session: clears highlighted matches/selection. Sent
    /// when the find bar closes.
    FindStop,
    /// Opens this engine's own native DevTools UI — a separate real window
    /// the engine manages itself, not part of the tab's own captured
    /// content, so nothing comes back over `EngineToHost` for this. `F12`.
    /// CEF and WebView2 both have a real API for this
    /// (`BrowserHost::show_dev_tools`/`ICoreWebView2::OpenDevToolsWindow`);
    /// WebKitGTK's `WebInspector` needs `developer-extras` enabled on the
    /// `WebView`'s settings first (done once at startup) before `.show()`
    /// does anything. Servo's embedding API has no DevTools/inspector
    /// method at all — silent no-op, same per-engine gap as find-in-page.
    OpenDevTools,
    /// Mutes/unmutes this tab's audio output — host-initiated (the mute
    /// button), unlike `EngineToHost::AudioPlayingChanged`'s passive
    /// engine-reported state, so there's no matching `EngineToHost` reply:
    /// `browser-ui` already knows the state it just asked for, same
    /// fire-and-forget shape as `SetZoom`. WebView2 (`ICoreWebView2_8::
    /// SetIsMuted` — the same interface `IsDocumentPlayingAudio` already
    /// uses) and WebKitGTK (`WebView::set_is_muted`, needing the
    /// `webkit2gtk` Cargo feature bumped to `v2_30`) both have a real
    /// native mute API. CEF's only equivalent would again be a full
    /// `AudioHandler` taking over raw PCM delivery (see
    /// `AudioPlayingChanged`'s doc comment for why that's out of scope for
    /// a toggle this minor); Servo's embedding API has no audio control at
    /// all. Silent no-op on both, same per-engine gap as the audio
    /// indicator itself.
    SetMuted { muted: bool },
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
    StateSnapshot(TabState),
    Crashed { reason: String },
    /// The page asked to open a new tab (`target=_blank`, `window.open()`,
    /// ctrl+click) and the engine-host intercepted and cancelled it in
    /// place — `browser-ui` spawns a real child tab for `url` instead.
    /// CEF, WebKit, and WebView2 (`ICoreWebView2::NewWindowRequested`, see
    /// `engine-host-webview2/src/popup.rs`) send this: their
    /// popup-interception APIs all hand over the target URL before creating
    /// anything. Servo's equivalent
    /// (`WebViewDelegate::request_create_new`) doesn't expose a URL until
    /// you've already constructed a same-process auxiliary `WebView` —
    /// doesn't fit this project's one-process-per-tab architecture, so
    /// Servo tabs don't send this (page-triggered popups fall back to
    /// whatever Servo's own unhandled-request default behavior is).
    OpenTabRequested { url: String },
    /// A find-in-page result update for the current search term: `count`
    /// matches found, `active_index` the 1-based position of the currently
    /// selected match among them (0 if `count` is 0). WebKitGTK's
    /// `WebKitFindController` only ever reports a total count, never which
    /// match is currently selected — its engine-host always sends
    /// `active_index: 0`, so `browser-ui` shows just the count for a WebKit
    /// tab rather than "3 of 12".
    FindResult { count: u32, active_index: u32 },
    /// Whether this tab's page is currently producing audio — shown as a
    /// speaker icon on its graph node. Only sent when the value actually
    /// changes (a real native event/notify property on the engine side),
    /// not polled. WebView2 (`ICoreWebView2_8::IsDocumentPlayingAudio`/
    /// `add_IsDocumentPlayingAudioChanged`) and WebKitGTK (`WebView::
    /// is_playing_audio`/its `is-playing-audio` GObject notify) both have a
    /// real *passive* API for this — reading/observing a value Chromium/
    /// WebKit already track internally, no different in kind from reading
    /// `title`. CEF's only equivalent is registering a `Client::
    /// audio_handler()`, which takes over delivering every raw PCM audio
    /// packet to the app instead of playing it through the normal OS audio
    /// output — a real audio-routing feature, not a passive read, and a
    /// much bigger and riskier change than a tab icon warrants (get it
    /// wrong and CEF tabs go silent), so CEF tabs never send this. Servo's
    /// embedding API has no such property at all.
    AudioPlayingChanged { playing: bool },
    /// A right-click landed on a link — `href` is its resolved absolute
    /// target URL. `browser-ui` shows a minimal single-item "Open in new
    /// tab" menu at `(x, y)` when this arrives; a right-click that didn't
    /// land on a link sends nothing (no menu to show). All four engines
    /// send this now, each via whichever mechanism fits how that engine
    /// already reports/intercepts input, not one shared implementation:
    /// WebView2/WebKit drive every other input event through a script-
    /// execution API already (`ExecuteScript`/`run_javascript`), so this
    /// reuses that directly — `document.elementFromPoint(x, y)`, walking up
    /// to the nearest `<a href>` ancestor (see `input.rs` in each). CEF
    /// drives input via real native mouse-event injection instead (a
    /// completely different mechanism — see `engine-host-cef/src/input.rs`)
    /// but has its own native `ContextMenuHandler::on_before_context_menu`
    /// API that hands over `link_url`/`xcoord`/`ycoord` directly, no script
    /// round-trip needed (`context_menu_handler.rs`); `run_context_menu`
    /// always returns "handled" so CEF never tries to show a real popup —
    /// windowless/OSR mode has no window to anchor one to anyway. Servo
    /// requests an `EmbedderControl::ContextMenu` for every right-click,
    /// carrying a structured `ContextMenuElementInformation` (`link_url`
    /// among other fields) directly; letting it drop without an explicit
    /// `.select()`/`.dismiss()` still tells Servo "dismissed, no selection"
    /// (its own `Drop` impl), the same "read the data, suppress the real
    /// menu" outcome the other three engines reach by different means.
    ContextMenuLink { x: f64, y: f64, href: String },
    /// A download started -- `path` is where the engine will save it (this
    /// project never prompts for a save location; each engine picks its own
    /// default: WebView2's own real Downloads-folder default, CEF's
    /// `%USERPROFILE%\Downloads` plus the server's suggested filename
    /// chosen by `browser-ui`'s equivalent, WebKit's own default -- see each
    /// engine-host's own download-handling code). `total_bytes` is the
    /// expected final size if the server reported a real `Content-Length`;
    /// `None` for a chunked/unknown-length response, shown as a byte count
    /// with no percentage rather than a guessed bar. `path` is the stable
    /// key `DownloadProgress`/`DownloadCompleted` for this same download are
    /// reported under -- final by contract at this point (the engine has
    /// already decided the save location before this fires), so `browser-ui`
    /// never needs to re-derive it. WebView2, CEF, and WebKit all have a
    /// real native download-progress API; Servo's embedding surface has no
    /// download hook of any kind, so it never sends any `Download*` message.
    DownloadStarted { path: String, total_bytes: Option<u64> },
    /// Bytes received so far for an in-progress download, keyed by the same
    /// `path` its own `DownloadStarted` reported.
    DownloadProgress { path: String, bytes_received: u64 },
    /// A download finished — `path` is the same one its `DownloadStarted`
    /// reported. `browser-ui` shows a brief "Saved <path>" toast here and
    /// marks that download's own graph node/popup row as no longer in
    /// progress.
    DownloadCompleted { path: String },
    /// The page that just finished loading is a raw JSON response --
    /// `body` is its full text, so `browser-ui` can parse and show a real
    /// tree view instead of whatever a given engine's own default
    /// plain-text/JSON-viewer rendering looks like (inconsistent across
    /// engines, and not this project's own uniform UI). Detected the same
    /// way on every engine that sends it: once the load finishes, a script
    /// eval checks `document.contentType === 'application/json'` and, only
    /// then, reads back `document.body.innerText` -- `document.contentType`
    /// is a real DOM property every engine here already exposes to script,
    /// reflecting the actual HTTP response header, not a guess from the
    /// URL's own file extension. Sent by WebKit/WebView2/Servo, which all
    /// already have (or, for Servo, expose but hadn't yet used) a script-
    /// eval-with-a-result-callback API -- see `ContextMenuLink`'s own doc
    /// comment for the same three-way split. CEF's `execute_java_script`
    /// has no way to return a value at all (would need a real V8 extension
    /// to bind a native callback the script could call into, a much bigger
    /// change than this feature warrants on its own) — deferred, so CEF
    /// tabs never send this. Cleared (an empty page) whenever the same
    /// tab's own URL changes for real, same as any other per-navigation
    /// state — see `App::json_body`'s own doc comment.
    JsonBody { body: String },
}
