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

use std::cell::RefCell;
use std::rc::Rc;

use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2;
use webview2_com::ExecuteScriptCompletedHandler;
use windows::core::HSTRING;

use ipc_protocol::{InputEvent, MouseButton};

/// `(x, y, href)` for a right-click that landed on a link — see
/// `ipc_protocol::EngineToHost::ContextMenuLink`'s doc comment.
pub type PendingContextMenu = Rc<RefCell<Option<(f64, f64, String)>>>;

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
) {
    match event {
        // Hover-only moves aren't worth a script round-trip for this
        // project's actual use (links/buttons/forms, not hover-driven
        // UI) -- skipped, matching the "click-only" scope of this round.
        InputEvent::MouseMove { .. } => {}
        InputEvent::MouseDown { .. } => {}
        // The actual click happens on mouse-up (matches the natural
        // "press, release, click fires" order) -- `el.click()` is a real
        // synthetic click the page's own listeners and default actions
        // (link navigation, button submit, etc.) all respond to normally,
        // unlike a synthetic `MouseEvent` dispatched by hand (those don't
        // trigger default actions in Chromium, only listener callbacks).
        InputEvent::MouseUp { x, y, button: MouseButton::Right } => {
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
            exec(
                webview,
                format!(
                    "(function(){{var e=document.elementFromPoint({x},{y});if(e){{e.click();}}}})();"
                ),
            );
        }
        InputEvent::Scroll { delta_x, delta_y, .. } => {
            exec(webview, format!("window.scrollBy({delta_x},{delta_y});"));
        }
        // Real KeyDown/KeyUp DOM events *can* be dispatched this way, but
        // synthetic events never trigger a browser's default action for a
        // key (Enter-submits-a-form, Backspace-deletes-a-character, arrow-
        // key scrolling, etc. all stay inert) -- only a page's own explicit
        // `keydown`/`keyup` listeners would ever see these. Kept anyway
        // since some effect is better than none for a page that does
        // listen; real text entry goes through `Char` below instead, which
        // *does* have a working default-action equivalent.
        InputEvent::KeyDown { .. } | InputEvent::KeyUp { .. } => {}
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
