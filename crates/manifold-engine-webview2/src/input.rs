//! Input dispatch for WebView2 -- working, via `ICoreWebView2::
//! ExecuteScript` (round 4; see project memory for the full history).
//!
//! Three earlier rounds tried to simulate input at the OS/engine level:
//! raw `WM_*` messages to `Chrome_RenderWidgetHostHWND` (round 1, and round
//! 3 with `MoveFocus` added first) were silently ignored by Chromium's
//! actual input pipeline -- confirmed with real clicks that produced zero
//! effect, not just a hunch; Composition hosting's native `SendMouseInput`
//! (round 2) broke frame capture without fixing input either, and was
//! reverted. A web search on how other projects automate WebView2 without
//! a real focused window turned up the answer this project had been
//! missing: **don't simulate input at all** -- drive the interaction via
//! JavaScript run directly in the page. This sidesteps the entire OS/
//! Composition input pipeline (and its apparent focus/message-injection
//! requirements) completely.
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
//! **Drag text-selection and nested scroll, later still: `Selection`/`Range`
//! JS, not CDP.** A throwaway spike tried the other native lever available
//! -- `ICoreWebView2CompositionController::SendMouseInput`/`SendPointerInput`
//! -- and found it *doesn't* extend a text selection during a drag at all
//! (confirmed live: fully `isTrusted:true` events, correct coordinates and
//! button state throughout, `selection.rangeCount` shows an anchor was set
//! on mousedown, but `.toString()` stays empty after the drag) -- a genuine
//! WebView2/Chromium limitation in that hosting mode, not a mistake in the
//! spike, and it would've meant redoing this crate's whole hosting+capture
//! pipeline anyway (`SendMouseInput` only exists on the Composition-hosted
//! controller, which `capture.rs`'s `Windows.Graphics.Capture` pipeline
//! doesn't use). So this stays on the plain-JS path this whole module is
//! already built around: `document.caretRangeFromPoint` + `Selection.
//! collapse`/`.extend` reproduce a real drag-select using the same browser
//! primitives a native drag would, and a short walk up from `elementFromPoint`
//! to the first scrollable ancestor lets `Scroll` reach a nested scrollable
//! div instead of only ever the page itself.

use std::cell::Cell;
use std::cell::RefCell;
use std::rc::Rc;

use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2;
use webview2_com::{CallDevToolsProtocolMethodCompletedHandler, ExecuteScriptCompletedHandler};
use windows::core::HSTRING;

use ipc_protocol::{modifiers, InputEvent, KeyCode, MouseButton};

/// `(x, y, href)` for a right-click that landed on a link — see
/// `ipc_protocol::EngineToHost::ContextMenuLink`'s doc comment.
pub type PendingContextMenu = Rc<RefCell<Option<(f64, f64, String)>>>;

/// Tracks a left-button drag across `MouseDown`/`MouseMove`/`MouseUp` --
/// `ipc_protocol::InputEvent::MouseMove` carries no button state of its own
/// (see its doc comment: that field exists only for a native-injection
/// engine forwarding modifier keys), so this project's own engine-host has
/// to remember "is the left button currently down" itself. `moved` is what
/// tells `MouseUp` whether this was a real drag (skip the synthetic
/// `.click()` -- a real browser doesn't fire a click after dragging a
/// selection across a link either) or a plain stationary click (fire it, as
/// before).
#[derive(Clone, Copy, Default)]
pub struct Drag {
    active: bool,
    moved: bool,
}

pub type DragState = Rc<Cell<Drag>>;

/// Both the drag-anchor (`MouseDown`) and drag-extend (`MouseMove`) scripts
/// skip form controls -- `document.caretRangeFromPoint` returns `null` or a
/// range outside an `<input>`/`<textarea>`'s own internal text layout, so
/// acting on it there would move the selection *away* from where the user
/// clicked instead of focusing the field, breaking the click-to-focus path
/// `Char`'s own doc comment already depends on (confirmed live against
/// DuckDuckGo's search box).
fn skip_form_controls_guard(x: f64, y: f64) -> String {
    format!("var el=document.elementFromPoint({x},{y});if(el&&el.closest('input,textarea,select'))return;")
}

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
    trimmed
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .map(str::to_string)
}

/// Translates and dispatches one `ipc_protocol::InputEvent` as a small
/// snippet of JavaScript run via `ExecuteScript`.
pub fn send_input(
    webview: &ICoreWebView2,
    event: InputEvent,
    pending_context_menu: &PendingContextMenu,
    drag_state: &DragState,
) {
    match event {
        // `caretRangeFromPoint` + `Selection.collapse` reproduces a real
        // mousedown's caret placement -- see this module's doc comment for
        // why this is JS/`Selection` rather than a CDP mouse event.
        InputEvent::MouseDown { x, y, button: MouseButton::Left, .. } => {
            // TEMP diagnostic for the live offset bug -- prints the
            // received CSS-pixel coordinate here, and separately what the
            // page itself believes is at that point (tag/id and its own
            // `getBoundingClientRect`), so a report can be compared against
            // where the user actually clicked. Remove once resolved.
            eprintln!("[input-diag] MouseDown received x={x} y={y}");
            let diag_script = format!(
                "(function(){{var e=document.elementFromPoint({x},{y});if(!e)return 'null element at point';var r=e.getBoundingClientRect();return e.tagName+'#'+e.id+' rect=('+r.left.toFixed(1)+','+r.top.toFixed(1)+','+r.width.toFixed(1)+'x'+r.height.toFixed(1)+') text='+JSON.stringify((e.textContent||'').slice(0,30));}})();"
            );
            let handler = ExecuteScriptCompletedHandler::create(Box::new(move |_error, result| {
                eprintln!("[input-diag]   elementFromPoint({x},{y}) -> {result}");
                Ok(())
            }));
            unsafe {
                let _ = webview.ExecuteScript(&HSTRING::from(diag_script), &handler);
            }
            drag_state.set(Drag { active: true, moved: false });
            exec(
                webview,
                format!(
                    "(function(){{{guard}var r=document.caretRangeFromPoint({x},{y});if(r){{var s=window.getSelection();s.removeAllRanges();s.collapse(r.startContainer,r.startOffset);}}}})();",
                    guard = skip_form_controls_guard(x, y),
                ),
            );
        }
        // Right/middle mouse-down: no drag-selection to start (right-click's
        // own handling is on `MouseUp` below; middle-click is unhandled).
        InputEvent::MouseDown { .. } => {}
        // Only round-trips a script while a left-button drag is actually in
        // progress -- a plain hover move (the common case) stays free, same
        // as before this drag-selection support existed.
        InputEvent::MouseMove { x, y, .. } => {
            let mut drag = drag_state.get();
            if drag.active {
                drag.moved = true;
                drag_state.set(drag);
                exec(
                    webview,
                    format!(
                        "(function(){{{guard}var r=document.caretRangeFromPoint({x},{y});if(r){{window.getSelection().extend(r.startContainer,r.startOffset);}}}})();",
                        guard = skip_form_controls_guard(x, y),
                    ),
                );
            }
        }
        // The actual click happens on mouse-up (matches the natural
        // "press, release, click fires" order) -- `el.click()` is a real
        // synthetic click the page's own listeners and default actions
        // (link navigation, button submit, etc.) all respond to normally,
        // unlike a synthetic `MouseEvent` dispatched by hand (those don't
        // trigger default actions in Chromium, only listener callbacks).
        InputEvent::MouseUp { x, y, button: MouseButton::Right, .. } => {
            drag_state.set(Drag::default());
            let script = format!(
                "(function(){{var e=document.elementFromPoint({x},{y});var a=e&&e.closest('a[href]');return a?a.href:null;}})();"
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
        InputEvent::MouseUp { x, y, .. } => {
            let moved = drag_state.get().moved;
            drag_state.set(Drag::default());
            // A completed drag-selection isn't a click -- a real browser
            // doesn't fire one after dragging a selection across a link
            // either, and synthesizing one here would wrongly navigate/
            // submit just because the drag happened to end over such an
            // element.
            if !moved {
                // `.focus()` first, then `.click()` -- `.click()` alone
                // never focuses a plain `<input>`: it dispatches with
                // `isTrusted:false`, and Chromium's "untrusted events still
                // get their default action" carve-out (why `.click()` still
                // navigates links/submits forms below) doesn't extend to
                // focus assignment. `.focus()` is a plain DOM method, not an
                // event, so it isn't trust-gated at all, and it's a silent
                // no-op on non-focusable elements -- safe to call
                // unconditionally rather than special-casing form controls.
                exec(
                    webview,
                    format!(
                        "(function(){{var e=document.elementFromPoint({x},{y});if(e){{if(e.focus){{e.focus();}}e.click();}}}})();"
                    ),
                );
            }
        }
        InputEvent::Scroll { x, y, delta_x, delta_y, .. } => {
            // Walks up from the pointer's own element to the first
            // scrollable ancestor (matches a real wheel event's target
            // resolution) instead of always scrolling the page -- lets this
            // reach a nested scrollable panel, not only the document.
            exec(
                webview,
                format!(
                    "(function(){{var el=document.elementFromPoint({x},{y});while(el&&el!==document.body&&el!==document.documentElement){{var cs=getComputedStyle(el);var sy=(cs.overflowY==='auto'||cs.overflowY==='scroll')&&el.scrollHeight>el.clientHeight;var sx=(cs.overflowX==='auto'||cs.overflowX==='scroll')&&el.scrollWidth>el.clientWidth;if(sy||sx){{el.scrollBy({delta_x},{delta_y});return;}}el=el.parentElement;}}window.scrollBy({delta_x},{delta_y});}})();"
                ),
            );
        }
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
            exec(
                webview,
                format!(
                    "document.execCommand('insertText', false, String.fromCodePoint({codepoint}));"
                ),
            );
        }
    }
}

/// `windowsVirtualKeyCode`/`code`/`key` for `Input.dispatchKeyEvent` --
/// CDP's own field names, not this project's own naming. `nativeVirtualKeyCode`
/// is sent equal to `windowsVirtualKeyCode` (true on Windows, the only
/// platform WebView2 runs on). Left/right variants of a modifier key share
/// one generic virtual-key code (`VK_SHIFT`/`VK_CONTROL`/`VK_MENU`) the same
/// way real Windows key messages do -- `code` alone still tells the page
/// which physical side was pressed.
fn key_info(key: KeyCode) -> (u32, &'static str, &'static str) {
    match key {
        KeyCode::KeyA => (0x41, "KeyA", "a"),
        KeyCode::KeyB => (0x42, "KeyB", "b"),
        KeyCode::KeyC => (0x43, "KeyC", "c"),
        KeyCode::KeyD => (0x44, "KeyD", "d"),
        KeyCode::KeyE => (0x45, "KeyE", "e"),
        KeyCode::KeyF => (0x46, "KeyF", "f"),
        KeyCode::KeyG => (0x47, "KeyG", "g"),
        KeyCode::KeyH => (0x48, "KeyH", "h"),
        KeyCode::KeyI => (0x49, "KeyI", "i"),
        KeyCode::KeyJ => (0x4A, "KeyJ", "j"),
        KeyCode::KeyK => (0x4B, "KeyK", "k"),
        KeyCode::KeyL => (0x4C, "KeyL", "l"),
        KeyCode::KeyM => (0x4D, "KeyM", "m"),
        KeyCode::KeyN => (0x4E, "KeyN", "n"),
        KeyCode::KeyO => (0x4F, "KeyO", "o"),
        KeyCode::KeyP => (0x50, "KeyP", "p"),
        KeyCode::KeyQ => (0x51, "KeyQ", "q"),
        KeyCode::KeyR => (0x52, "KeyR", "r"),
        KeyCode::KeyS => (0x53, "KeyS", "s"),
        KeyCode::KeyT => (0x54, "KeyT", "t"),
        KeyCode::KeyU => (0x55, "KeyU", "u"),
        KeyCode::KeyV => (0x56, "KeyV", "v"),
        KeyCode::KeyW => (0x57, "KeyW", "w"),
        KeyCode::KeyX => (0x58, "KeyX", "x"),
        KeyCode::KeyY => (0x59, "KeyY", "y"),
        KeyCode::KeyZ => (0x5A, "KeyZ", "z"),
        KeyCode::Digit0 => (0x30, "Digit0", "0"),
        KeyCode::Digit1 => (0x31, "Digit1", "1"),
        KeyCode::Digit2 => (0x32, "Digit2", "2"),
        KeyCode::Digit3 => (0x33, "Digit3", "3"),
        KeyCode::Digit4 => (0x34, "Digit4", "4"),
        KeyCode::Digit5 => (0x35, "Digit5", "5"),
        KeyCode::Digit6 => (0x36, "Digit6", "6"),
        KeyCode::Digit7 => (0x37, "Digit7", "7"),
        KeyCode::Digit8 => (0x38, "Digit8", "8"),
        KeyCode::Digit9 => (0x39, "Digit9", "9"),
        KeyCode::Enter => (0x0D, "Enter", "Enter"),
        KeyCode::Backspace => (0x08, "Backspace", "Backspace"),
        KeyCode::Tab => (0x09, "Tab", "Tab"),
        KeyCode::Space => (0x20, "Space", " "),
        KeyCode::Escape => (0x1B, "Escape", "Escape"),
        KeyCode::Delete => (0x2E, "Delete", "Delete"),
        KeyCode::ArrowLeft => (0x25, "ArrowLeft", "ArrowLeft"),
        KeyCode::ArrowUp => (0x26, "ArrowUp", "ArrowUp"),
        KeyCode::ArrowRight => (0x27, "ArrowRight", "ArrowRight"),
        KeyCode::ArrowDown => (0x28, "ArrowDown", "ArrowDown"),
        KeyCode::PageUp => (0x21, "PageUp", "PageUp"),
        KeyCode::PageDown => (0x22, "PageDown", "PageDown"),
        KeyCode::End => (0x23, "End", "End"),
        KeyCode::Home => (0x24, "Home", "Home"),
        KeyCode::ShiftLeft => (0x10, "ShiftLeft", "Shift"),
        KeyCode::ShiftRight => (0x10, "ShiftRight", "Shift"),
        KeyCode::ControlLeft => (0x11, "ControlLeft", "Control"),
        KeyCode::ControlRight => (0x11, "ControlRight", "Control"),
        KeyCode::AltLeft => (0x12, "AltLeft", "Alt"),
        KeyCode::AltRight => (0x12, "AltRight", "Alt"),
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
    let (vk, code, key_name) = key_info(key);
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
