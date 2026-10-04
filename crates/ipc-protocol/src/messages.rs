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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

// One row per key: the variant (which is also its DOM `KeyboardEvent.code`), its Windows virtual-key
// code, and its DOM `KeyboardEvent.key`. Generates the enum, `KeyCode::ALL` and the accessors, so the
// table exists once. A doc comment on a row lands on its variant.
macro_rules! key_codes {
    ($($(#[$m:meta])* $name:ident $vk:literal $key:literal;)*) => {
        /// A physical key, engine-agnostic — deliberately not winit's own `KeyCode`
        /// (not guaranteed FFI/serialization-stable across winit versions) or any
        /// one engine's native representation (Windows `VK_*` for CEF, a GDK keyval
        /// for WebKitGTK, `keyboard_types::Code` for Servo). `manifold` translates
        /// winit's key into this; each engine-host translates this into whatever
        /// its own embedding API wants. Two encodings are standards rather than one
        /// engine's own, so they live here with the table (`key_codes!`): the DOM
        /// `code`/`key` strings and Windows virtual-key codes. GDK keyvals and
        /// Servo's `Code`/`Key` stay in their engine crates.
        ///
        /// Deliberately not exhaustive — common keys only (letters, digits,
        /// navigation, the usual modifiers and editing keys) covers real usage
        /// (typing into a page's form fields, basic shortcuts) without trying to
        /// enumerate every key on every keyboard layout.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        pub enum KeyCode {
            $($(#[$m])* $name,)*
        }

        impl KeyCode {
            /// Every key, in declaration order.
            pub const ALL: &'static [KeyCode] = &[$(KeyCode::$name,)*];

            /// The DOM `KeyboardEvent.code` (`"KeyA"`, `"ShiftLeft"`), which is the variant's own name.
            pub fn dom_code(self) -> &'static str {
                match self {
                    $(KeyCode::$name => stringify!($name),)*
                }
            }

            /// The DOM `KeyboardEvent.key` (`"a"`, `"Enter"`, `" "`). Left and right modifiers share one.
            pub fn dom_key(self) -> &'static str {
                match self {
                    $(KeyCode::$name => $key,)*
                }
            }

            /// The Windows virtual-key code (`VK_*`). Modifiers use the side-specific codes
            /// (`VK_LSHIFT`, ...); `windows_vk_generic` folds them to `VK_SHIFT` and friends.
            pub fn windows_vk(self) -> u32 {
                match self {
                    $(KeyCode::$name => $vk,)*
                }
            }
        }
    };
}

key_codes! {
    KeyA 0x41 "a";
    KeyB 0x42 "b";
    KeyC 0x43 "c";
    KeyD 0x44 "d";
    KeyE 0x45 "e";
    KeyF 0x46 "f";
    KeyG 0x47 "g";
    KeyH 0x48 "h";
    KeyI 0x49 "i";
    KeyJ 0x4A "j";
    KeyK 0x4B "k";
    KeyL 0x4C "l";
    KeyM 0x4D "m";
    KeyN 0x4E "n";
    KeyO 0x4F "o";
    KeyP 0x50 "p";
    KeyQ 0x51 "q";
    KeyR 0x52 "r";
    KeyS 0x53 "s";
    KeyT 0x54 "t";
    KeyU 0x55 "u";
    KeyV 0x56 "v";
    KeyW 0x57 "w";
    KeyX 0x58 "x";
    KeyY 0x59 "y";
    KeyZ 0x5A "z";
    Digit0 0x30 "0";
    Digit1 0x31 "1";
    Digit2 0x32 "2";
    Digit3 0x33 "3";
    Digit4 0x34 "4";
    Digit5 0x35 "5";
    Digit6 0x36 "6";
    Digit7 0x37 "7";
    Digit8 0x38 "8";
    Digit9 0x39 "9";
    Enter 0x0D "Enter";
    Backspace 0x08 "Backspace";
    Tab 0x09 "Tab";
    Space 0x20 " ";
    Escape 0x1B "Escape";
    Delete 0x2E "Delete";
    ArrowLeft 0x25 "ArrowLeft";
    ArrowRight 0x27 "ArrowRight";
    ArrowUp 0x26 "ArrowUp";
    ArrowDown 0x28 "ArrowDown";
    Home 0x24 "Home";
    End 0x23 "End";
    PageUp 0x21 "PageUp";
    PageDown 0x22 "PageDown";
    ShiftLeft 0xA0 "Shift";
    ShiftRight 0xA1 "Shift";
    ControlLeft 0xA2 "Control";
    ControlRight 0xA3 "Control";
    AltLeft 0xA4 "Alt";
    AltRight 0xA5 "Alt";
}

impl KeyCode {
    /// `windows_vk` with left/right modifiers folded to the generic `VK_SHIFT` (0x10),
    /// `VK_CONTROL` (0x11) and `VK_MENU` (0x12), as real Windows key messages and CDP carry them.
    pub fn windows_vk_generic(self) -> u32 {
        match self.windows_vk() {
            0xA0 | 0xA1 => 0x10,
            0xA2 | 0xA3 => 0x11,
            0xA4 | 0xA5 => 0x12,
            vk => vk,
        }
    }
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

/// Every `x`/`y` below (and `Scroll`'s `delta_x`/`delta_y`) is in **physical
/// pixels of the engine's own rendered surface**, relative to its top-left
/// corner — the exact same space as `HostToEngine::Resize`'s `width`/
/// `height`. Not CSS pixels, and not DPI-independent logical points.
///
/// That is the one space that needs no per-engine translation: each
/// engine-host renders a surface of precisely the pixel size `Resize` gave
/// it (CEF pins `device_scale_factor` to 1.0, Servo/WebKit size their
/// offscreen surface directly in those pixels), and `manifold` composites
/// that surface 1:1 into its content quad, so a physical offset into the
/// on-screen content is already the correct value to send. WebView2 is the
/// one engine that keeps its own separate CSS space (it picks a
/// `RasterizationScale` from the host monitor's DPI, so the page's CSS
/// viewport is the surface divided by that) — `SendMouseInput` takes the
/// raw surface coordinate regardless, and the one remaining script-based
/// consumer there divides by `devicePixelRatio` itself.
///
/// Confirmed live on a 125%-scaled monitor: dividing these by the scale
/// factor before sending (an earlier fix aimed at WebView2's
/// `elementFromPoint` path, back when that drove clicks) made every native
/// click land at 80% of the intended distance from the content's top-left
/// corner.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[rustfmt::skip]
pub enum InputEvent {
    /// `modifiers` (see the `modifiers` module) are the modifier keys held
    /// at the time of this mouse event; every engine forwards them, so
    /// Ctrl+click/Shift+click reach the page instead of a bare click.
    MouseMove { x: f64, y: f64, modifiers: u8 },
    /// `click_count` is 1 for a single press, 2 for the second of a double
    /// click, and so on (`manifold`'s `ClickCounter`); `MouseUp` repeats
    /// its press's count.
    MouseDown { x: f64, y: f64, button: MouseButton, modifiers: u8, click_count: u8 },
    MouseUp { x: f64, y: f64, button: MouseButton, modifiers: u8, click_count: u8 },
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
#[rustfmt::skip]
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
    /// Asks for the page's scroll position and dirty-form flag
    /// (`PAGE_STATE_SCRIPT`); the engine answers with `EngineToHost::PageState`.
    /// Sent before a tab sleeps. An engine that cannot answer stays silent
    /// and the host gives up after a few seconds.
    QueryPageState,
    /// Seeds the engine's cookie jar from a JSON array of `jar::Cookie`. Sent once, first, right after connect, to
    /// hosts started with `ENGINE_AWAIT_COOKIES=1`, which wait for it before their first navigation.
    ///
    /// The payload holds cookie values (session credentials): the derived `Debug` would print them, so never format
    /// this message (or `EngineToHost::Cookies`) with `{:?}`.
    SetCookies(String),
    /// Asks for every cookie in the engine's jar; the engine answers with `EngineToHost::Cookies`.
    /// An engine that cannot answer, or whose read failed or may be incomplete, stays silent: an answer is only ever a
    /// complete read, since a cookie missing from it can be taken as deleted.
    GetCookies,
    /// Scrolls the page to `(x, y)` once, with retries (`restore_scroll_script`).
    /// Sent after a woken tab's first load.
    RestoreScroll { x: f64, y: f64 },
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

/// The script every engine that reports `JsonBody`/`RedirectableContent` (all but CEF, which
/// evals the two properties itself) runs once a load finishes. It returns `"json\n<body>"` for a
/// JSON response, `"type\n<content type>"` for any other, and null when the page has none.
pub const PAGE_KIND_SCRIPT: &str = "(function(){var t=document.contentType;if(t==='application/json'){return 'json\\n'+document.body.innerText;}if(t){return 'type\\n'+t;}return null;})();";

/// What `PAGE_KIND_SCRIPT` returned, split by its prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageKindReply {
    /// A JSON response: its `document.body.innerText`.
    Json(String),
    /// Any other response: its content type.
    Other(String),
}

impl PageKindReply {
    /// Parses the script's string result; `None` for any other text.
    pub fn parse(text: &str) -> Option<PageKindReply> {
        if let Some(body) = text.strip_prefix("json\n") {
            Some(PageKindReply::Json(body.to_string()))
        } else {
            text.strip_prefix("type\n").map(|t| PageKindReply::Other(t.to_string()))
        }
    }
}

/// Returns `"<scrollX> <scrollY> <1|0>"`: where the page is scrolled and
/// whether any form field differs from its default (the dirty-form check
/// of idle sleep). A text/number input, textarea or select counts when its
/// value differs from its default; a checkbox or radio when `checked` does;
/// a `contenteditable` element when it has any text. Button-like inputs are
/// ignored. Pages that keep state outside form elements are not detected.
pub const PAGE_STATE_SCRIPT: &str = "(function(){var d=false;var els=document.querySelectorAll('input,textarea,select,[contenteditable]');for(var i=0;i<els.length&&!d;i++){var e=els[i];var t=(e.type||'').toLowerCase();if(e.tagName==='INPUT'){if(t==='button'||t==='submit'||t==='reset'||t==='image'||t==='hidden'||t==='file'){continue;}if(t==='checkbox'||t==='radio'){d=e.checked!==e.defaultChecked;}else{d=e.value!==e.defaultValue;}}else if(e.tagName==='TEXTAREA'){d=e.value!==e.defaultValue;}else if(e.tagName==='SELECT'){for(var j=0;j<e.options.length;j++){if(e.options[j].selected!==e.options[j].defaultSelected){d=true;break;}}}else if(e.isContentEditable){d=(e.textContent||'')!=='';}}return (window.scrollX||0)+' '+(window.scrollY||0)+' '+(d?1:0);})();";

/// A tab's page state at the moment of `HostToEngine::QueryPageState`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageState {
    pub scroll_x: f64,
    pub scroll_y: f64,
    pub dirty_form: bool,
}

impl PageState {
    /// Parses `PAGE_STATE_SCRIPT`'s result; `None` for any other text.
    pub fn parse(text: &str) -> Option<PageState> {
        let mut parts = text.split_whitespace();
        let scroll_x: f64 = parts.next()?.parse().ok().filter(|v: &f64| v.is_finite())?;
        let scroll_y: f64 = parts.next()?.parse().ok().filter(|v: &f64| v.is_finite())?;
        let dirty_form = match parts.next()? {
            "1" => true,
            "0" => false,
            _ => return None,
        };
        parts.next().is_none().then_some(PageState { scroll_x, scroll_y, dirty_form })
    }
}

/// Scrolls to `(x, y)`, retrying on animation frames for up to 3 s until the
/// page is tall (wide) enough to reach it, then to the nearest reachable
/// point. Gives up at once if the user scrolls first (wheel, key, touch).
/// A non-finite coordinate is replaced by 0, so no `NaN`/`inf` reaches the
/// page as source.
pub fn restore_scroll_script(x: f64, y: f64) -> String {
    let f = |v: f64| if v.is_finite() { v } else { 0.0 };
    format!(
        "(function(x,y){{var stop=false;var end=Date.now()+3000;['wheel','keydown','touchstart'].forEach(function(n){{addEventListener(n,function(){{stop=true;}},{{once:true,passive:true}});}});function go(){{if(stop){{return;}}var de=document.documentElement;var reach=de.scrollHeight-innerHeight>=y&&de.scrollWidth-innerWidth>=x;if(reach||Date.now()>=end){{scrollTo(x,y);return;}}scrollTo(x,y);requestAnimationFrame(go);}}go();}})({},{});",
        f(x),
        f(y)
    )
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
#[rustfmt::skip]
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
        /// How the current navigation started, so the host shares only the
        /// url changes a person made (spec "Fields", `url`). `Some(true)`:
        /// a link click, a form submit, back/forward. `Some(false)`: a page
        /// script without a gesture, or the host's own `Navigate`. Redirects
        /// keep the label of the navigation they continue. `None`: the
        /// engine can't tell (Servo, WebKit), or no navigation is under way.
        user_started: Option<bool>,
    },
    StateSnapshot(TabState),
    PageState(PageState),
    /// The engine's whole cookie jar as a JSON array of `jar::Cookie` (name, value, domain, path, expires,
    /// secure, http_only, same_site), the answer to `HostToEngine::GetCookies`.
    Cookies(String),
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
    /// `user_gesture`: the engine saw a user action (click, key) behind the
    /// request; the host drops requests without one (`popup_gate.rs`).
    OpenTabRequested { url: String, user_gesture: bool },
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
    /// project never prompts for a save location: WebView2 uses Edge's own
    /// Downloads-folder default and naming; CEF and WebKit use
    /// `crate::downloads` -- `~/Downloads`, the suggested name's last
    /// component, numbered `name (n).ext` if taken). `total_bytes` is the
    /// expected final size if the server reported a real `Content-Length`;
    /// `None` for a chunked/unknown-length response, shown as a byte count
    /// with no percentage rather than a guessed bar. `path` is the stable
    /// key `DownloadProgress`/`DownloadCompleted` for this same download are
    /// reported under -- final by contract at this point (the engine has
    /// already decided the save location before this fires), so `manifold`
    /// never needs to re-derive it. WebView2, CEF, and WebKit all have a
    /// real native download-progress API; Servo's embedding surface has no
    /// download hook of any kind, so it never sends any `Download*` message.
    /// `url` is what the engine is fetching, when it can say (shown as the card's source line).
    DownloadStarted { path: String, total_bytes: Option<u64>, url: Option<String> },
    /// Bytes received so far for an in-progress download, keyed by the same
    /// `path` its own `DownloadStarted` reported.
    DownloadProgress { path: String, bytes_received: u64 },
    /// A download finished — `path` is the same one its `DownloadStarted`
    /// reported. `manifold` shows a brief "Saved `<path>`" toast here and
    /// marks that download's own graph node/popup row as no longer in
    /// progress.
    DownloadCompleted { path: String },
    /// A download ended without finishing (cancelled, interrupted, network error) -- `path` is the one its
    /// `DownloadStarted` reported. `manifold` marks the card failed and stops treating it as in flight.
    DownloadFailed { path: String },
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
    /// handler.rs`). The host converts the tab to a `Json` node from this
    /// same body, right away -- see
    /// `RedirectableContent`'s own doc comment for why JSON takes this
    /// separate route instead of that message's own refetch-and-check path.
    JsonBody { body: String },
    /// The page that just finished loading is a raw, non-JSON response --
    /// detected the exact same way as `JsonBody` (once the load finishes, a
    /// script/`V8Context` eval reads `document.contentType`), just carrying
    /// that raw content-type string back instead of the page's own body.
    /// `manifold` never tries to read the response's actual bytes back
    /// out of the engine for this message -- it does its own independent
    /// `GET` of the tab's current url (same "manifold does its own
    /// uniform HTTP fetch, engine-agnostic" posture `favicon_fetch` already
    /// established for favicons), classifies `content_type` with
    /// `content::kind_of` (the host's one MIME table), refetches the url,
    /// checks the bytes and, when they pass, replaces the tab with the node
    /// the table names at the same position. Sent by all four engines for
    /// any content type but `application/json`, which comes as `JsonBody`
    /// instead and converts from that body.
    RedirectableContent { content_type: String },
}

#[cfg(test)]
mod key_code_tests {
    use super::*;

    #[test]
    fn dom_code_is_the_variant_name_and_vk_codes_are_distinct_per_key() {
        for &key in KeyCode::ALL {
            assert_eq!(key.dom_code(), format!("{key:?}"));
        }
        let mut vks: Vec<u32> = KeyCode::ALL.iter().map(|k| k.windows_vk()).collect();
        vks.sort();
        vks.dedup();
        assert_eq!(vks.len(), KeyCode::ALL.len());
    }

    #[test]
    fn spot_checks_against_the_old_per_engine_tables() {
        assert_eq!(KeyCode::KeyA.windows_vk(), 0x41);
        assert_eq!(KeyCode::Digit9.windows_vk(), 0x39);
        assert_eq!(KeyCode::PageUp.windows_vk(), 0x21);
        assert_eq!(KeyCode::AltRight.windows_vk(), 0xA5);
        assert_eq!(KeyCode::AltRight.windows_vk_generic(), 0x12);
        assert_eq!(KeyCode::ShiftLeft.dom_key(), "Shift");
        assert_eq!(KeyCode::Space.dom_key(), " ");
        assert_eq!(KeyCode::KeyQ.dom_key(), "q");
    }

    // The macro also emits `ALL`, `dom_code` and `windows_vk`, which this one-key table never calls.
    #[allow(dead_code)]
    mod attrs_pass_through {
        // A doc comment on a variant must compile: `key_codes!` forwards attributes.
        use serde::{Deserialize, Serialize};
        key_codes! {
            /// A documented key.
            KeyA 0x41 "a";
        }

        #[test]
        fn a_documented_variant_compiles() {
            assert_eq!(KeyCode::KeyA.dom_key(), "a");
        }
    }
}

#[cfg(test)]
mod cookie_message_tests {
    use super::*;
    use crate::channel::MessageChannel;
    use std::io::Cursor;

    /// Sends `msg` through the real postcard framing and reads it back.
    fn round_trip<T: Serialize + serde::de::DeserializeOwned>(msg: &T) -> T {
        let mut out = MessageChannel::new(Cursor::new(Vec::new()));
        out.send(msg).unwrap();
        MessageChannel::new(Cursor::new(out.into_inner().into_inner())).recv().unwrap()
    }

    #[test]
    fn the_cookie_messages_round_trip() {
        assert!(matches!(round_trip(&HostToEngine::GetCookies), HostToEngine::GetCookies));
        assert!(matches!(round_trip(&HostToEngine::SetCookies("[]".into())), HostToEngine::SetCookies(s) if s == "[]"));
        assert!(matches!(round_trip(&EngineToHost::Cookies("[]".into())), EngineToHost::Cookies(s) if s == "[]"));
    }
}

#[cfg(test)]
mod page_kind_tests {
    use super::*;

    #[test]
    fn page_kind_reply_splits_on_the_scripts_prefixes() {
        assert_eq!(PageKindReply::parse("json\n{\"a\":1}"), Some(PageKindReply::Json("{\"a\":1}".into())));
        assert_eq!(PageKindReply::parse("type\nimage/png"), Some(PageKindReply::Other("image/png".into())));
        assert_eq!(PageKindReply::parse("text/html"), None);
        assert_eq!(PageKindReply::parse(""), None);
    }
}

#[cfg(test)]
mod page_state_tests {
    use super::*;

    #[test]
    fn page_state_parses_the_scripts_three_numbers() {
        assert_eq!(
            PageState::parse("12.5 340 1"),
            Some(PageState { scroll_x: 12.5, scroll_y: 340.0, dirty_form: true })
        );
        assert_eq!(PageState::parse("0 0 0"), Some(PageState { scroll_x: 0.0, scroll_y: 0.0, dirty_form: false }));
    }

    #[test]
    fn page_state_rejects_anything_else() {
        for bad in ["", "1 2", "a b c", "1 2 3 4", "1 2 2", "NaN 0 0", "inf 0 0"] {
            assert_eq!(PageState::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn restore_scroll_script_embeds_only_finite_numbers() {
        let s = restore_scroll_script(1.5, 2400.0);
        assert!(s.contains("1.5") && s.contains("2400"), "{s}");
        // A non-finite value must not reach the page as JS source.
        assert_eq!(restore_scroll_script(f64::NAN, 5.0), restore_scroll_script(0.0, 5.0));
        assert_eq!(restore_scroll_script(1.0, f64::INFINITY), restore_scroll_script(1.0, 0.0));
    }
}
