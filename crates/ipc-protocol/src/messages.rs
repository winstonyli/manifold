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
/// for WebKitGTK, `keyboard_types::Code` for Servo). `manifold` translates
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
/// `Self::SHIFT` set). Combine with `|`; `manifold` is the only producer
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

/// Messages sent from the manifold process manager to an engine-host.
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
    /// `manifold` already knows the state it just asked for, same
    /// fire-and-forget shape as `SetZoom`. WebView2 (`ICoreWebView2_8::
    /// SetIsMuted` — the same interface `IsDocumentPlayingAudio` already
    /// uses) and WebKitGTK (`WebView::set_is_muted`, needing the
    /// `webkit2gtk` Cargo feature bumped to `v2_30`) both have a real
    /// native mute API. CEF has no such API short of the same full
    /// `AudioHandler` `AudioPlayingChanged`'s own doc comment already rules
    /// out -- but unlike that read, muting is a plain write, so it doesn't
    /// need `AudioHandler`, or even the `V8Context`/`ProcessMessage` round
    /// trip the audio-playing poll needs: `Frame::execute_java_script`
    /// (fire-and-forget, no return value required either) just sets
    /// `.muted` directly on every `<video>`/`<audio>` element, the same
    /// media-element-only approximation `AudioPlayingChanged`'s own CEF
    /// path uses. Servo's embedding API has no audio control at all --
    /// silent no-op there only.
    SetMuted { muted: bool },
    Shutdown,
}

/// Messages sent from an engine-host back to the manifold process manager.
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
    /// place — `manifold` spawns a real child tab for `url` instead.
    /// CEF, WebKit, and WebView2 (`ICoreWebView2::NewWindowRequested`, see
    /// `manifold-engine-webview2/src/popup.rs`) send this: their
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
    /// `active_index: 0`, so `manifold` shows just the count for a WebKit
    /// tab rather than "3 of 12".
    FindResult { count: u32, active_index: u32 },
    /// Whether this tab's page is currently producing audio — shown as a
    /// speaker icon on its graph node. Only sent when the value actually
    /// changes. WebView2 (`ICoreWebView2_8::IsDocumentPlayingAudio`/
    /// `add_IsDocumentPlayingAudioChanged`) and WebKitGTK (`WebView::
    /// is_playing_audio`/its `is-playing-audio` GObject notify) both have a
    /// real *passive* native API for this — reading/observing a value
    /// Chromium/WebKit already track internally, no different in kind from
    /// reading `title`, so those two never poll for it. CEF has no such API
    /// short of registering a `Client::audio_handler()`, which takes over
    /// delivering every raw PCM audio packet to the app instead of playing
    /// it through normal OS audio output — a real audio-routing feature,
    /// not a passive read, and a much bigger and riskier change than a tab
    /// icon warrants (get it wrong and CEF tabs go silent). CEF sends this
    /// too, by a different and much lighter route instead: a periodic
    /// `V8Context::eval` (`manifold-engine-cef`'s own polling timer, see
    /// `AUDIO_PLAYING_POLL_INTERVAL`) checking whether any `<video>`/
    /// `<audio>` element is currently playing and unmuted -- a real
    /// approximation, not the same guarantee the other two engines'
    /// passive reads give (misses audio the page produces through the Web
    /// Audio API alone with no media element behind it), but covers the
    /// common case without the `AudioHandler` risk. Servo's embedding API
    /// has no such property at all, passive or otherwise.
    AudioPlayingChanged { playing: bool },
    /// A right-click landed on a link — `href` is its resolved absolute
    /// target URL. `manifold` shows a minimal single-item "Open in new
    /// tab" menu at `(x, y)` when this arrives; a right-click that didn't
    /// land on a link sends nothing (no menu to show). All four engines
    /// send this now, each via whichever mechanism fits how that engine
    /// already reports/intercepts input, not one shared implementation:
    /// WebView2/WebKit drive every other input event through a script-
    /// execution API already (`ExecuteScript`/`run_javascript`), so this
    /// reuses that directly — `document.elementFromPoint(x, y)`, walking up
    /// to the nearest `<a href>` ancestor (see `input.rs` in each). CEF
    /// drives input via real native mouse-event injection instead (a
    /// completely different mechanism — see `manifold-engine-cef/src/input.rs`)
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
    /// chosen by `manifold`'s equivalent, WebKit's own default -- see each
    /// engine-host's own download-handling code). `total_bytes` is the
    /// expected final size if the server reported a real `Content-Length`;
    /// `None` for a chunked/unknown-length response, shown as a byte count
    /// with no percentage rather than a guessed bar. `path` is the stable
    /// key `DownloadProgress`/`DownloadCompleted` for this same download are
    /// reported under -- final by contract at this point (the engine has
    /// already decided the save location before this fires), so `manifold`
    /// never needs to re-derive it. WebView2, CEF, and WebKit all have a
    /// real native download-progress API; Servo's embedding surface has no
    /// download hook of any kind, so it never sends any `Download*` message.
    DownloadStarted { path: String, total_bytes: Option<u64> },
    /// Bytes received so far for an in-progress download, keyed by the same
    /// `path` its own `DownloadStarted` reported.
    DownloadProgress { path: String, bytes_received: u64 },
    /// A download finished — `path` is the same one its `DownloadStarted`
    /// reported. `manifold` shows a brief "Saved <path>" toast here and
    /// marks that download's own graph node/popup row as no longer in
    /// progress.
    DownloadCompleted { path: String },
    /// The page that just finished loading is a raw JSON response --
    /// `body` is its full text, so `manifold` can parse and show a real
    /// tree view instead of whatever a given engine's own default
    /// plain-text/JSON-viewer rendering looks like (inconsistent across
    /// engines, and not this project's own uniform UI). Detected the same
    /// way on every engine that sends it: once the load finishes, a script
    /// eval checks `document.contentType === 'application/json'` and, only
    /// then, reads back `document.body.innerText` -- `document.contentType`
    /// is a real DOM property every engine here already exposes to script,
    /// reflecting the actual HTTP response header, not a guess from the
    /// URL's own file extension. Sent by WebKit/WebView2/Servo via a script-
    /// eval-with-a-result-callback API -- see `ContextMenuLink`'s own doc
    /// comment for the same three-way split. CEF's `execute_java_script` has
    /// no way to return a value at all, but CEF sends this too, by a
    /// different route: a `RenderProcessHandler` in the renderer subprocess
    /// runs the same two checks via `V8Context::eval` and hands the result
    /// back to the browser process over a `ProcessMessage` -- no V8
    /// extension needed, just CEF's own real mechanism for a renderer to
    /// return a value at all (see `manifold-engine-cef`'s `render_process_
    /// handler.rs`). Cleared (an empty page) whenever the same tab's own
    /// URL changes for real, same as any other per-navigation state — see
    /// `App::json_body`'s own doc comment.
    JsonBody { body: String },
    /// The page that just finished loading is a raw, non-JSON response --
    /// detected the exact same way as `JsonBody` (once the load finishes, a
    /// script/`V8Context` eval reads `document.contentType`), just carrying
    /// that raw content-type string back instead of the page's own body.
    /// `manifold` never tries to read the response's actual bytes back
    /// out of the engine for this message -- it does its own independent
    /// `GET` of the tab's current url (same "manifold does its own
    /// uniform HTTP fetch, engine-agnostic" posture `favicon_fetch` already
    /// established for favicons), classifies `content_type` against a
    /// small, explicit allowlist (`redirect_fetch::classify` -- currently
    /// just an `image/` prefix), and, once that fetch succeeds, replaces
    /// the tab with a real graph node of the matching kind at the same
    /// position -- turning "navigated straight to a picture" into the same
    /// outcome a user picking that same file by hand would get, rather than
    /// leaving it sitting there as an ordinary tab showing whichever native
    /// viewer page each engine happens to render for it. Generalized from
    /// an image-only, payload-less version specifically so a future
    /// redirectable kind only ever means widening that one allowlist
    /// function -- no engine-host needs to change, since every engine
    /// already forwards the real content-type string regardless of what it
    /// says. Sent by all four engines (the same detection path `JsonBody`
    /// uses on each) for any content-type that isn't `application/json` --
    /// `manifold`'s own allowlist is what actually decides whether it
    /// means anything; an ordinary `text/html` page just gets classified as
    /// "nothing to do" and ignored. JSON deliberately isn't classified into
    /// this same automatic path despite being detected identically -- a
    /// JSON response is often a live, refetchable API call worth keeping as
    /// an ordinary tab, unlike an image or similar, which has nothing left
    /// to offer once redirected; converting one into a permanent node is a
    /// user-initiated action instead (`build_json_viewer`'s own "Save as
    /// Node" button), not something a plain navigation should silently do.
    RedirectableContent { content_type: String },
}
