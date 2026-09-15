use std::io::BufRead;

use ipc_protocol::KeyCode;
use winit::event::MouseButton;
use winit::event_loop::EventLoopProxy;

use crate::UserEvent;

/// A headless-mode automation command, one per stdin line (see `parse_line`
/// for the text grammar). Exists so live GUI testing on this project can
/// stop fighting Windows' foreground-lock/focus contention (see the M3/M5
/// gotchas in project memory: `SetForegroundWindow` can be silently refused
/// while the user is active on their own machine, making `SendKeys`/
/// `mouse_event`-based automation unreliable) — these commands drive the
/// exact same code paths real OS input would (see `main.rs`'s
/// `handle_debug_command`), just via a channel instead of the OS, so a
/// hidden/headless window never needs real focus at all.
///
/// Mouse commands are dispatched as genuine synthetic `winit::event::
/// WindowEvent`s (see `App::dispatch_window_event`) — deliberately not a
/// logic-bypassing shortcut — since that's the only way to actually exercise
/// egui's real hit-testing/rendering-timing, which is exactly the code path
/// that had the click-swallowing dropdown bug fixed in the autocomplete
/// feature. Keyboard/text commands are the one exception: `winit::event::
/// KeyEvent` has no public constructor outside the winit crate itself (its
/// `platform_specific` field is `pub(crate)`), so there is no way to
/// synthesize a real `WindowEvent::KeyboardInput` from application code at
/// all. Those commands instead call the exact equivalent effect directly
/// (send the same `InputEvent`s to a tab, or run the same address-bar
/// update/navigate logic) — this still exercises all of *this project's*
/// logic, just not winit/egui's own keyboard event plumbing (which was never
/// the source of a bug here).
#[derive(Debug, Clone)]
pub enum DebugCommand {
    MouseMove { x: f64, y: f64 },
    MouseDown { x: f64, y: f64, button: MouseButton },
    MouseUp { x: f64, y: f64, button: MouseButton },
    /// A full click (move + press + release) in one command — the common
    /// case, and avoids needing three round-trips through stdin for it.
    Click { x: f64, y: f64, button: MouseButton },
    Scroll { x: f64, y: f64, delta_x: f64, delta_y: f64 },
    /// Sets the address bar's text and focus state directly, then recomputes
    /// suggestions exactly as `response.changed()` would — see this module's
    /// doc comment for why address-bar *text entry* is a direct call rather
    /// than a synthetic `KeyboardInput` event.
    AddressBar(String),
    /// Simulates pressing Enter in the address bar (navigates using its
    /// current text), regardless of whether it has "real" egui focus —
    /// headless address-bar focus is set directly by `AddressBar`, not
    /// through a real click-driven egui focus transfer.
    Enter,
    /// Simulates pressing Escape — dismisses the suggestion dropdown if one
    /// is showing, otherwise leaves Tab view for Graph view (same behavior
    /// as a real Escape keypress).
    Escape,
    /// Simulates pressing Down/Up in the address bar — moves the
    /// highlighted suggestion row (wrapping at both ends) the same way a
    /// real arrow-key press would; `Enter` afterward accepts whichever row
    /// ends up highlighted.
    ArrowDown,
    ArrowUp,
    /// Sends each character as an `InputEvent::Char` to the focused tab's
    /// page — for typing into a page's own text field, not the address bar.
    TypeIntoPage(String),
    /// Sends a KeyDown+KeyUp pair of the named key to the focused tab's page.
    KeyPage(KeyCode),
    /// `Ctrl+T` equivalent — see `App::new_tab_shortcut`. A real
    /// `WindowEvent::KeyboardInput` can't be synthesized from application
    /// code (see this module's own doc comment), so this and the four
    /// shortcut commands below call the exact same effect directly instead,
    /// same as `TypeIntoPage`/`KeyPage` already do for page-level input.
    NewTab,
    /// `Ctrl+W` equivalent — see `App::close_focused_tab_shortcut`.
    CloseTab,
    /// `Ctrl+L` equivalent — see `App::focus_address_bar_shortcut`.
    FocusAddressBar,
    /// `Ctrl+Tab` equivalent — see `App::cycle_focused_tab`.
    NextTab,
    /// `Ctrl+Shift+Tab` equivalent — see `App::cycle_focused_tab`.
    PrevTab,
    /// `Ctrl+F` equivalent — see `App::open_find_bar_shortcut`.
    OpenFindBar,
    /// Sets the find bar's search text directly and sends `FindStart` to
    /// the focused tab — same "direct call, not a synthetic key event"
    /// reason as `AddressBar`.
    FindText(String),
    /// `Enter`/`Shift+Enter` in the find bar.
    FindNext,
    FindPrevious,
    /// `Escape` (or the find bar's own close button) — sends `FindStop` and
    /// closes the bar.
    CloseFindBar,
    /// Sends `HostToEngine::SetVisible` to the focused tab directly —
    /// bypassing `focus_tab`/`leave_to_graph`'s own visibility changes,
    /// which always come bundled with a `Resize` (Tab vs. Graph view's
    /// chrome heights differ). Exists for testing the `SetVisible(false)`→
    /// `true` repaint path in isolation, without a same-transaction resize
    /// able to mask a bug in it (see the M5 `SetVisible`-resume fix's own
    /// verification-gap note in project memory/README: a real user's
    /// Escape-then-refocus always resizes too, so a clean repro needs a
    /// way to toggle visibility *without* one).
    SetVisible(bool),
    /// Renders the next frame and saves it as a PNG at this path — includes
    /// chrome and any open dropdown, not just tab content, since it reads
    /// back the same composited frame a real screenshot would show.
    Screenshot(String),
    Resize(u32, u32),
    Quit,
}

fn parse_mouse_button(name: &str) -> MouseButton {
    match name.to_ascii_lowercase().as_str() {
        "right" => MouseButton::Right,
        "middle" => MouseButton::Middle,
        _ => MouseButton::Left,
    }
}

/// Maps a debug command's key name to `ipc_protocol::KeyCode` — the same
/// engine-agnostic key set every real engine-host translation table already
/// covers (see that type's own doc comment), not winit's own `KeyCode`.
fn parse_key_code(name: &str) -> Option<KeyCode> {
    Some(match name {
        "Enter" => KeyCode::Enter,
        "Backspace" => KeyCode::Backspace,
        "Tab" => KeyCode::Tab,
        "Space" => KeyCode::Space,
        "Escape" => KeyCode::Escape,
        "Delete" => KeyCode::Delete,
        "ArrowLeft" => KeyCode::ArrowLeft,
        "ArrowRight" => KeyCode::ArrowRight,
        "ArrowUp" => KeyCode::ArrowUp,
        "ArrowDown" => KeyCode::ArrowDown,
        "Home" => KeyCode::Home,
        "End" => KeyCode::End,
        "PageUp" => KeyCode::PageUp,
        "PageDown" => KeyCode::PageDown,
        "ShiftLeft" => KeyCode::ShiftLeft,
        "ShiftRight" => KeyCode::ShiftRight,
        "ControlLeft" => KeyCode::ControlLeft,
        "ControlRight" => KeyCode::ControlRight,
        "AltLeft" => KeyCode::AltLeft,
        "AltRight" => KeyCode::AltRight,
        _ => return parse_single_char_key(name),
    })
}

/// `KeyA`..`KeyZ` and `Digit0`..`Digit9` follow a regular naming scheme, so
/// generate the match from the single character instead of spelling out 36
/// arms — also accepts a bare letter/digit (`"a"`, `"5"`) for convenience.
fn parse_single_char_key(name: &str) -> Option<KeyCode> {
    let mut chars = name.chars();
    let ch = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    match ch.to_ascii_uppercase() {
        c @ 'A'..='Z' => Some(match c {
            'A' => KeyCode::KeyA, 'B' => KeyCode::KeyB, 'C' => KeyCode::KeyC, 'D' => KeyCode::KeyD,
            'E' => KeyCode::KeyE, 'F' => KeyCode::KeyF, 'G' => KeyCode::KeyG, 'H' => KeyCode::KeyH,
            'I' => KeyCode::KeyI, 'J' => KeyCode::KeyJ, 'K' => KeyCode::KeyK, 'L' => KeyCode::KeyL,
            'M' => KeyCode::KeyM, 'N' => KeyCode::KeyN, 'O' => KeyCode::KeyO, 'P' => KeyCode::KeyP,
            'Q' => KeyCode::KeyQ, 'R' => KeyCode::KeyR, 'S' => KeyCode::KeyS, 'T' => KeyCode::KeyT,
            'U' => KeyCode::KeyU, 'V' => KeyCode::KeyV, 'W' => KeyCode::KeyW, 'X' => KeyCode::KeyX,
            'Y' => KeyCode::KeyY, 'Z' => KeyCode::KeyZ,
            _ => unreachable!(),
        }),
        c @ '0'..='9' => Some(match c {
            '0' => KeyCode::Digit0, '1' => KeyCode::Digit1, '2' => KeyCode::Digit2, '3' => KeyCode::Digit3,
            '4' => KeyCode::Digit4, '5' => KeyCode::Digit5, '6' => KeyCode::Digit6, '7' => KeyCode::Digit7,
            '8' => KeyCode::Digit8, '9' => KeyCode::Digit9,
            _ => unreachable!(),
        }),
        _ => None,
    }
}

/// Parses one line of the headless debug-command text protocol. Unknown
/// commands or malformed arguments are silently ignored (returns `None`) —
/// good enough for a testing tool where a bad line should be a no-op, not a
/// crash.
pub fn parse_line(line: &str) -> Option<DebugCommand> {
    // Strip a leading BOM — several Windows-side tools that can drive this
    // protocol (PowerShell's default `StandardInput` stream encoding among
    // them, confirmed live: a `Process.StandardInput.WriteLine` from
    // PowerShell prepends `\u{FEFF}` to the very first line) write one
    // unconditionally, and it isn't whitespace so `trim()` alone leaves it
    // stuck to the command name.
    let line = line.strip_prefix('\u{feff}').unwrap_or(line);
    let line = line.trim();
    let (cmd, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
    let rest = rest.trim();
    let mut parts = rest.split_whitespace();
    match cmd.to_ascii_uppercase().as_str() {
        "MOUSEMOVE" => Some(DebugCommand::MouseMove {
            x: parts.next()?.parse().ok()?,
            y: parts.next()?.parse().ok()?,
        }),
        "MOUSEDOWN" => Some(DebugCommand::MouseDown {
            x: parts.next()?.parse().ok()?,
            y: parts.next()?.parse().ok()?,
            button: parts.next().map(parse_mouse_button).unwrap_or(MouseButton::Left),
        }),
        "MOUSEUP" => Some(DebugCommand::MouseUp {
            x: parts.next()?.parse().ok()?,
            y: parts.next()?.parse().ok()?,
            button: parts.next().map(parse_mouse_button).unwrap_or(MouseButton::Left),
        }),
        "CLICK" => Some(DebugCommand::Click {
            x: parts.next()?.parse().ok()?,
            y: parts.next()?.parse().ok()?,
            button: parts.next().map(parse_mouse_button).unwrap_or(MouseButton::Left),
        }),
        "SCROLL" => Some(DebugCommand::Scroll {
            x: parts.next()?.parse().ok()?,
            y: parts.next()?.parse().ok()?,
            delta_x: parts.next()?.parse().ok()?,
            delta_y: parts.next()?.parse().ok()?,
        }),
        "ADDRESSBAR" => Some(DebugCommand::AddressBar(rest.to_string())),
        "ENTER" => Some(DebugCommand::Enter),
        "ESCAPE" => Some(DebugCommand::Escape),
        "ARROWDOWN" => Some(DebugCommand::ArrowDown),
        "ARROWUP" => Some(DebugCommand::ArrowUp),
        "TYPE" => Some(DebugCommand::TypeIntoPage(rest.to_string())),
        "KEYPAGE" => Some(DebugCommand::KeyPage(parse_key_code(parts.next()?)?)),
        "NEWTAB" => Some(DebugCommand::NewTab),
        "CLOSETAB" => Some(DebugCommand::CloseTab),
        "FOCUSADDRESS" => Some(DebugCommand::FocusAddressBar),
        "NEXTTAB" => Some(DebugCommand::NextTab),
        "PREVTAB" => Some(DebugCommand::PrevTab),
        "OPENFINDBAR" => Some(DebugCommand::OpenFindBar),
        "FINDTEXT" => Some(DebugCommand::FindText(rest.to_string())),
        "FINDNEXT" => Some(DebugCommand::FindNext),
        "FINDPREVIOUS" => Some(DebugCommand::FindPrevious),
        "CLOSEFINDBAR" => Some(DebugCommand::CloseFindBar),
        "SETVISIBLE" => Some(DebugCommand::SetVisible(match parts.next()? {
            "true" => true,
            "false" => false,
            _ => return None,
        })),
        "SCREENSHOT" => Some(DebugCommand::Screenshot(rest.to_string())).filter(|_| !rest.is_empty()),
        "RESIZE" => Some(DebugCommand::Resize(parts.next()?.parse().ok()?, parts.next()?.parse().ok()?)),
        "QUIT" => Some(DebugCommand::Quit),
        _ => None,
    }
}

/// Spawns the stdin-reading thread for headless mode — one command per
/// line, forwarded to the app via `proxy` (the same `EventLoopProxy`
/// mechanism winit itself uses for user events, so commands land on the
/// real event loop like any other event). Stdin EOF (the driving script/
/// process closed its write end) sends `Quit` so a headless run driven by a
/// piped script exits cleanly instead of hanging forever.
pub fn spawn_stdin_reader(proxy: EventLoopProxy<UserEvent>) {
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
            if let Some(cmd) = parse_line(&line) {
                if proxy.send_event(UserEvent::Debug(cmd)).is_err() {
                    break;
                }
            }
        }
        let _ = proxy.send_event(UserEvent::Debug(DebugCommand::Quit));
    });
}
