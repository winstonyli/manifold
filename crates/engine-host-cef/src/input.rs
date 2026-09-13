use cef::*;
use ipc_protocol::{modifiers, InputEvent, KeyCode, MouseButton};

/// Windows virtual-key codes (`VK_*` from `winbase.h`/Chromium's
/// `ui/events/keycodes/keyboard_codes.h`) — CEF's `KeyEvent::windows_key_code`
/// wants these specifically (CEF documents it as "the VKEY_* value from
/// Chromium's keyboard_codes.h header (VK_* values on Windows)"), regardless
/// of host platform, since we only ever run engine-host-cef on Windows.
fn key_code_to_vk(key: KeyCode) -> i32 {
    match key {
        KeyCode::KeyA => 0x41,
        KeyCode::KeyB => 0x42,
        KeyCode::KeyC => 0x43,
        KeyCode::KeyD => 0x44,
        KeyCode::KeyE => 0x45,
        KeyCode::KeyF => 0x46,
        KeyCode::KeyG => 0x47,
        KeyCode::KeyH => 0x48,
        KeyCode::KeyI => 0x49,
        KeyCode::KeyJ => 0x4A,
        KeyCode::KeyK => 0x4B,
        KeyCode::KeyL => 0x4C,
        KeyCode::KeyM => 0x4D,
        KeyCode::KeyN => 0x4E,
        KeyCode::KeyO => 0x4F,
        KeyCode::KeyP => 0x50,
        KeyCode::KeyQ => 0x51,
        KeyCode::KeyR => 0x52,
        KeyCode::KeyS => 0x53,
        KeyCode::KeyT => 0x54,
        KeyCode::KeyU => 0x55,
        KeyCode::KeyV => 0x56,
        KeyCode::KeyW => 0x57,
        KeyCode::KeyX => 0x58,
        KeyCode::KeyY => 0x59,
        KeyCode::KeyZ => 0x5A,
        KeyCode::Digit0 => 0x30,
        KeyCode::Digit1 => 0x31,
        KeyCode::Digit2 => 0x32,
        KeyCode::Digit3 => 0x33,
        KeyCode::Digit4 => 0x34,
        KeyCode::Digit5 => 0x35,
        KeyCode::Digit6 => 0x36,
        KeyCode::Digit7 => 0x37,
        KeyCode::Digit8 => 0x38,
        KeyCode::Digit9 => 0x39,
        KeyCode::Enter => 0x0D,       // VK_RETURN
        KeyCode::Backspace => 0x08,   // VK_BACK
        KeyCode::Tab => 0x09,         // VK_TAB
        KeyCode::Space => 0x20,       // VK_SPACE
        KeyCode::Escape => 0x1B,      // VK_ESCAPE
        KeyCode::Delete => 0x2E,      // VK_DELETE
        KeyCode::ArrowLeft => 0x25,   // VK_LEFT
        KeyCode::ArrowUp => 0x26,     // VK_UP
        KeyCode::ArrowRight => 0x27,  // VK_RIGHT
        KeyCode::ArrowDown => 0x28,   // VK_DOWN
        KeyCode::Home => 0x24,        // VK_HOME
        KeyCode::End => 0x23,         // VK_END
        KeyCode::PageUp => 0x21,      // VK_PRIOR
        KeyCode::PageDown => 0x22,    // VK_NEXT
        KeyCode::ShiftLeft => 0xA0,   // VK_LSHIFT
        KeyCode::ShiftRight => 0xA1,  // VK_RSHIFT
        KeyCode::ControlLeft => 0xA2, // VK_LCONTROL
        KeyCode::ControlRight => 0xA3, // VK_RCONTROL
        KeyCode::AltLeft => 0xA4,     // VK_LMENU
        KeyCode::AltRight => 0xA5,    // VK_RMENU
    }
}

/// CEF's `EventFlags`/`modifiers` fields are plain `u32`s combining
/// `EVENTFLAG_SHIFT_DOWN` (2) / `EVENTFLAG_CONTROL_DOWN` (4) /
/// `EVENTFLAG_ALT_DOWN` (8) / `EVENTFLAG_COMMAND_DOWN` (128) — read
/// straight from `cef-dll-sys`'s raw enum values, since the ergonomic
/// `EventFlags` wrapper type doesn't expose named bit constants itself.
fn modifiers_to_event_flags(bits: u8) -> u32 {
    let mut flags = 0u32;
    if bits & modifiers::SHIFT != 0 {
        flags |= 2;
    }
    if bits & modifiers::CONTROL != 0 {
        flags |= 4;
    }
    if bits & modifiers::ALT != 0 {
        flags |= 8;
    }
    if bits & modifiers::SUPER != 0 {
        flags |= 128;
    }
    flags
}

fn mouse_button_to_cef(button: MouseButton) -> MouseButtonType {
    match button {
        MouseButton::Left => MouseButtonType::LEFT,
        MouseButton::Right => MouseButtonType::RIGHT,
        MouseButton::Middle => MouseButtonType::MIDDLE,
    }
}

fn key_event(type_: KeyEventType, key: KeyCode, modifiers: u8, character: u16) -> KeyEvent {
    KeyEvent {
        size: std::mem::size_of::<KeyEvent>(),
        type_,
        modifiers: modifiers_to_event_flags(modifiers),
        windows_key_code: key_code_to_vk(key),
        native_key_code: 0,
        is_system_key: 0,
        character,
        unmodified_character: character,
        focus_on_editable_field: 0,
    }
}

/// Translates and dispatches one `ipc_protocol::InputEvent` onto a live CEF
/// `Browser` — mirrors `engine-host-servo`'s `translate_input_event`, just
/// dispatching directly instead of returning a value, since CEF's
/// `BrowserHost` methods are fire-and-forget (`send_*_event`) rather than a
/// single `notify_input_event` call.
pub fn send_input(browser: &Browser, event: InputEvent) {
    let Some(host) = browser.host() else {
        return;
    };
    match event {
        InputEvent::MouseMove { x, y } => {
            let mouse_event = MouseEvent {
                x: x as i32,
                y: y as i32,
                modifiers: 0,
            };
            host.send_mouse_move_event(Some(&mouse_event), 0);
        }
        InputEvent::MouseDown { x, y, button } => {
            let mouse_event = MouseEvent {
                x: x as i32,
                y: y as i32,
                modifiers: 0,
            };
            host.send_mouse_click_event(Some(&mouse_event), mouse_button_to_cef(button), 0, 1);
        }
        InputEvent::MouseUp { x, y, button } => {
            let mouse_event = MouseEvent {
                x: x as i32,
                y: y as i32,
                modifiers: 0,
            };
            host.send_mouse_click_event(Some(&mouse_event), mouse_button_to_cef(button), 1, 1);
        }
        InputEvent::Scroll {
            x,
            y,
            delta_x,
            delta_y,
        } => {
            let mouse_event = MouseEvent {
                x: x as i32,
                y: y as i32,
                modifiers: 0,
            };
            host.send_mouse_wheel_event(Some(&mouse_event), delta_x as i32, delta_y as i32);
        }
        InputEvent::KeyDown { key, modifiers } => {
            // CEF distinguishes RAWKEYDOWN (what the OS message loop would
            // deliver first) from KEYDOWN; KEYDOWN is what actually drives
            // most page-visible behavior (keydown DOM events, focus
            // handling) and is the one every other embedding sends for a
            // physical key press.
            let e = key_event(KeyEventType::KEYDOWN, key, modifiers, 0);
            host.send_key_event(Some(&e));
        }
        InputEvent::KeyUp { key, modifiers } => {
            let e = key_event(KeyEventType::KEYUP, key, modifiers, 0);
            host.send_key_event(Some(&e));
        }
        InputEvent::Char { codepoint } => {
            // CEF's KEYEVENT_CHAR takes the produced character directly in
            // `character` - the physical key is irrelevant here (matches
            // KeyDown/KeyUp's own key field being unused for CHAR events in
            // CEF's own model).
            let e = KeyEvent {
                size: std::mem::size_of::<KeyEvent>(),
                type_: KeyEventType::CHAR,
                modifiers: 0,
                windows_key_code: codepoint as i32,
                native_key_code: 0,
                is_system_key: 0,
                character: codepoint as u16,
                unmodified_character: codepoint as u16,
                focus_on_editable_field: 0,
            };
            host.send_key_event(Some(&e));
        }
    }
}
