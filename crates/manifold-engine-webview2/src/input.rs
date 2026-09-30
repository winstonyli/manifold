//! Input dispatch for WebView2: **mouse via native
//! `ICoreWebView2CompositionController::SendMouseInput`, keyboard/text via
//! `ICoreWebView2`'s CDP/`ExecuteScript`** (WebView2 has no native
//! keyboard-injection API on any hosting mode). The rest of this comment is
//! the history of how that split was arrived at over several rounds -- it
//! matters, because three of those rounds concluded the native path was
//! impossible, and the reason they were wrong is specific and easy to
//! re-lose.
//!
//! Three earlier rounds tried to simulate input at the OS/engine level:
//! raw `WM_*` messages to `Chrome_RenderWidgetHostHWND` (round 1, and round
//! 3 with `MoveFocus` added first) were silently ignored by Chromium's
//! actual input pipeline -- confirmed with real clicks that produced zero
//! effect, not just a hunch; Composition hosting's native `SendMouseInput`
//! (round 2) broke frame capture without fixing input either, and was
//! reverted. A web search on how other projects automate WebView2 without
//! a real focused window turned up the answer this project had been
//! missing at the time: **don't simulate input at all** -- drive the
//! interaction via JavaScript run directly in the page. This sidesteps the
//! entire OS/Composition input pipeline (and its apparent focus/
//! message-injection requirements) completely. (Superseded for mouse
//! events by the Task 4 migration described at the bottom of this comment;
//! still how text entry works.)
//!
//! **Verified live**: a dispatched `MouseUp` visibly changed real Wikipedia
//! page content (a fundraising banner expanded) -- unambiguous proof the
//! click reached the page for real, not just that the call didn't error.
//! Text entry (`Char`, via `execCommand('insertText', ...)`) is confirmed
//! live too: a `MouseUp` on DuckDuckGo's HTML search box focused it, then
//! dispatched `Char` events typed the real string into the field (visible
//! in the resulting screenshot, cursor included) -- both halves of the
//! click-then-type path work end to end, not just each in isolation.
//!
//! **`KeyDown`/`KeyUp`, later session pass: no longer no-ops.** The README's
//! own former "no CEF-style synthetic key-event injection API" claim was
//! wrong -- `ICoreWebView2::CallDevToolsProtocolMethod` is a real,
//! documented WebView2 API that runs *any* Chrome DevTools Protocol method
//! against this tab's own underlying page, `Input.dispatchKeyEvent` among
//! them. This is the exact mechanism tools like Puppeteer use for
//! `page.keyboard.press()` -- a CDP-injected key event is treated as trusted
//! by Chromium's own input pipeline, so (unlike a hand-dispatched DOM
//! `KeyboardEvent`) it *does* trigger default actions: Enter submits a form,
//! Backspace deletes a character, arrow keys scroll/move a text cursor.
//! `Char`'s own `execCommand('insertText', ...)` path is untouched --
//! `KeyDown`/`KeyUp` only need to cover the physical-key side now, the same
//! division of labor `ipc_protocol::InputEvent`'s own doc comment already
//! describes.
//!
//! **Mouse, later still (composition-hosting-migration Task 4): native
//! `SendMouseInput`, not `Selection`/`Range` JS.** An earlier spike found
//! `SendMouseInput` didn't extend a text selection during a drag under the
//! old *windowed*-hosting controller, which is why `MouseDown`/`MouseMove`/
//! `MouseUp`/`Scroll` moved to the JS path above at the time. Tasks 1-3 of
//! that migration then moved hosting itself to
//! `ICoreWebView2CompositionController` (for `Windows.Graphics.Capture`
//! frame capture), which is the controller `SendMouseInput` was always
//! meant for -- re-tried there, real button-down/up/move/wheel events drive
//! Chromium's own click/drag/scroll handling (including drag-selection)
//! directly, so mouse handling now sends those instead of running JS.
//! Keyboard is unaffected: WebView2 has no `SendKeyInput` equivalent on any
//! hosting mode, so `KeyDown`/`KeyUp`/`Char` stay exactly as described above.

use std::cell::Cell;
use std::cell::RefCell;
use std::rc::Rc;

use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2CompositionController, COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_DOWN,
    COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_UP, COREWEBVIEW2_MOUSE_EVENT_KIND_MOVE,
    COREWEBVIEW2_MOUSE_EVENT_KIND_RIGHT_BUTTON_DOWN, COREWEBVIEW2_MOUSE_EVENT_KIND_RIGHT_BUTTON_UP,
    COREWEBVIEW2_MOUSE_EVENT_KIND_WHEEL, COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_LEFT_BUTTON,
    COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_NONE, COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_RIGHT_BUTTON,
};
use webview2_com::{CallDevToolsProtocolMethodCompletedHandler, ExecuteScriptCompletedHandler};
use windows::core::HSTRING;
use windows::Win32::Foundation::POINT;

use ipc_protocol::{modifiers, InputEvent, KeyCode, MouseButton};

/// `(x, y, href)` for a right-click that landed on a link — see
/// `ipc_protocol::EngineToHost::ContextMenuLink`'s doc comment.
pub type PendingContextMenu = Rc<RefCell<Option<(f64, f64, String)>>>;

/// Is the left button currently down? Tracked across `MouseDown`/
/// `MouseMove`/`MouseUp` because `ipc_protocol::InputEvent::MouseMove`
/// carries no button state of its own (see its doc comment: that field
/// exists only for a native-injection engine forwarding modifier keys), so
/// this project's own engine-host has to remember it, to know whether to
/// include the left-button virtual key on a `SendMouseInput` move (so
/// Chromium's own drag/selection handling sees the button as held).
pub type DragState = Rc<Cell<bool>>;

fn exec(webview: &ICoreWebView2, script: String) {
    let handler = ExecuteScriptCompletedHandler::create(Box::new(|_error, _result| Ok(())));
    unsafe {
        let _ = webview.ExecuteScript(&HSTRING::from(script), &handler);
    }
}

/// `ExecuteScript`'s completion handler hands back the script's return
/// value JSON-encoded, not as a plain string — this project's own script
/// only ever returns a URL string or `null`, so a full JSON parser is
/// unwarranted; stripping one matched pair of outer quotes (real hrefs
/// don't contain literal `"` — browsers percent-encode those) is enough.
fn parse_json_string_or_null(json: &str) -> Option<String> {
    let trimmed = json.trim();
    if trimmed == "null" {
        return None;
    }
    trimmed.strip_prefix('"').and_then(|s| s.strip_suffix('"')).map(str::to_string)
}

/// Translates and dispatches one `ipc_protocol::InputEvent` -- mouse events
/// via `ICoreWebView2CompositionController::SendMouseInput`, keyboard/text
/// events via CDP/`ExecuteScript` on `webview` (see this module's doc
/// comment for why each picked its own mechanism).
pub fn send_input(
    webview: &ICoreWebView2,
    comp_controller: &ICoreWebView2CompositionController,
    event: InputEvent,
    pending_context_menu: &PendingContextMenu,
    drag_state: &DragState,
) {
    match event {
        InputEvent::MouseDown { x, y, button, .. } => {
            let (kind, keys) = match button {
                MouseButton::Left => {
                    (COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_DOWN, COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_LEFT_BUTTON)
                }
                MouseButton::Right => (
                    COREWEBVIEW2_MOUSE_EVENT_KIND_RIGHT_BUTTON_DOWN,
                    COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_RIGHT_BUTTON,
                ),
                MouseButton::Middle => return,
            };
            if matches!(button, MouseButton::Left) {
                drag_state.set(true);
            }
            unsafe {
                let _ = comp_controller.SendMouseInput(kind, keys, 0, POINT { x: x as i32, y: y as i32 });
            }
        }
        InputEvent::MouseMove { x, y, .. } => {
            let keys = if drag_state.get() {
                COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_LEFT_BUTTON
            } else {
                COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_NONE
            };
            unsafe {
                let _ = comp_controller.SendMouseInput(
                    COREWEBVIEW2_MOUSE_EVENT_KIND_MOVE,
                    keys,
                    0,
                    POINT { x: x as i32, y: y as i32 },
                );
            }
        }
        // Right-click's href-detection JS is kept (it doesn't depend on how
        // the click was delivered) but a real button-up is sent first.
        InputEvent::MouseUp { x, y, button: MouseButton::Right, .. } => {
            drag_state.set(false);
            unsafe {
                let _ = comp_controller.SendMouseInput(
                    COREWEBVIEW2_MOUSE_EVENT_KIND_RIGHT_BUTTON_UP,
                    COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_NONE,
                    0,
                    POINT { x: x as i32, y: y as i32 },
                );
            }
            // `elementFromPoint` is the one consumer left here that works in
            // CSS pixels, not the physical-surface pixels `InputEvent`
            // carries (see its doc comment) -- WebView2 sizes the page's CSS
            // viewport as bounds/`RasterizationScale`, and exposes exactly
            // that divisor as `devicePixelRatio`, so the page converts its
            // own coordinate rather than this process plumbing the scale
            // through from `Resize`.
            let script = format!(
                "(function(){{var r=window.devicePixelRatio||1;var e=document.elementFromPoint({x}/r,{y}/r);var a=e&&e.closest('a[href]');return a?a.href:null;}})();"
            );
            let pending = pending_context_menu.clone();
            let handler = ExecuteScriptCompletedHandler::create(Box::new(move |_error, result| {
                let json = result.to_string();
                if let Some(href) = parse_json_string_or_null(&json) {
                    *pending.borrow_mut() = Some((x, y, href));
                }
                Ok(())
            }));
            unsafe {
                let _ = webview.ExecuteScript(&HSTRING::from(script), &handler);
            }
        }
        InputEvent::MouseUp { x, y, button, .. } => {
            drag_state.set(false);
            let kind = match button {
                MouseButton::Left => COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_UP,
                MouseButton::Middle => return,
                MouseButton::Right => unreachable!("handled above"),
            };
            unsafe {
                let _ = comp_controller.SendMouseInput(
                    kind,
                    COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_NONE,
                    0,
                    POINT { x: x as i32, y: y as i32 },
                );
            }
        }
        InputEvent::Scroll { x, y, delta_y, .. } => unsafe {
            let _ = comp_controller.SendMouseInput(
                COREWEBVIEW2_MOUSE_EVENT_KIND_WHEEL,
                COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_NONE,
                delta_y as i32 as u32,
                POINT { x: x as i32, y: y as i32 },
            );
        },
        // See this module's own doc comment for why a CDP-injected key
        // event, unlike a hand-dispatched DOM `KeyboardEvent`, actually
        // triggers a key's default action.
        InputEvent::KeyDown { key, modifiers } => dispatch_key_event(webview, "keyDown", key, modifiers),
        InputEvent::KeyUp { key, modifiers } => dispatch_key_event(webview, "keyUp", key, modifiers),
        // `execCommand('insertText', ...)` is the one synthetic-input path
        // Chromium actually treats as "real" text entry into whatever
        // element currently has focus -- unlike a dispatched `input`/
        // `keypress` event, this one actually changes the field's value
        // and fires the same `input` event a real keystroke would, so
        // framework-bound inputs (React-style controlled inputs included)
        // see it correctly.
        // Confirmed live: click-focusing DuckDuckGo's HTML search box then
        // dispatching `Char` events actually typed the real string into the
        // field (screenshot showed the text and a real cursor).
        InputEvent::Char { codepoint } => {
            exec(webview, format!("document.execCommand('insertText', false, String.fromCodePoint({codepoint}));"));
        }
    }
}

/// `ipc_protocol::modifiers`' bit positions (`SHIFT`/`CONTROL`/`ALT`/`SUPER`)
/// don't match CDP's own `Input.dispatchKeyEvent`/`dispatchMouseEvent`
/// `modifiers` bitmask (`Alt=1, Ctrl=2, Meta=4, Shift=8`) -- translated
/// explicitly rather than assumed to line up.
fn cdp_modifiers(mods: u8) -> u32 {
    let mut out = 0;
    if mods & modifiers::ALT != 0 {
        out |= 1;
    }
    if mods & modifiers::CONTROL != 0 {
        out |= 2;
    }
    if mods & modifiers::SUPER != 0 {
        out |= 4;
    }
    if mods & modifiers::SHIFT != 0 {
        out |= 8;
    }
    out
}

fn dispatch_key_event(webview: &ICoreWebView2, cdp_type: &str, key: KeyCode, mods: u8) {
    // Left/right variants of a modifier share one generic virtual-key code, as real Windows key
    // messages do; `code` alone tells the page which side was pressed.
    let (vk, code, key_name) = (key.windows_vk_generic(), key.dom_code(), key.dom_key());
    let params = serde_json::json!({
        "type": cdp_type,
        "windowsVirtualKeyCode": vk,
        "nativeVirtualKeyCode": vk,
        "code": code,
        "key": key_name,
        "modifiers": cdp_modifiers(mods),
    })
    .to_string();
    let handler = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(|_error, _result| Ok(())));
    unsafe {
        let _ = webview.CallDevToolsProtocolMethod(
            &HSTRING::from("Input.dispatchKeyEvent"),
            &HSTRING::from(params),
            &handler,
        );
    }
}
