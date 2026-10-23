use cef::*;
use manifold_view_protocol::{modifiers, InputEvent, KeyCode, MouseButton};

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
        windows_key_code: key.windows_vk() as i32,
        native_key_code: 0,
        is_system_key: 0,
        character,
        unmodified_character: character,
        focus_on_editable_field: 0,
    }
}

/// `codepoint` as UTF-16 code units: Windows delivers a character outside
/// the BMP as two WM_CHARs (a surrogate pair), and CEF's KEYEVENT_CHAR takes
/// one unit each. Empty for a value that isn't a character.
fn utf16_units(codepoint: u32) -> Vec<u16> {
    char::from_u32(codepoint).map(|c| c.encode_utf16(&mut [0; 2]).to_vec()).unwrap_or_default()
}

/// Translates and dispatches one `manifold_view_protocol::InputEvent` onto a live CEF
/// `Browser` — mirrors `manifold-view-servo`'s `translate_input_event`, just
/// dispatching directly instead of returning a value, since CEF's
/// `BrowserHost` methods are fire-and-forget (`send_*_event`) rather than a
/// single `notify_input_event` call.
pub fn send_input(browser: &Browser, event: InputEvent) {
    let Some(host) = browser.host() else {
        return;
    };
    match event {
        InputEvent::MouseMove { x, y, modifiers } => {
            let mouse_event = MouseEvent { x: x as i32, y: y as i32, modifiers: modifiers_to_event_flags(modifiers) };
            host.send_mouse_move_event(Some(&mouse_event), 0);
        }
        InputEvent::MouseDown { x, y, button, modifiers, click_count } => {
            let mouse_event = MouseEvent { x: x as i32, y: y as i32, modifiers: modifiers_to_event_flags(modifiers) };
            host.send_mouse_click_event(Some(&mouse_event), mouse_button_to_cef(button), 0, click_count as i32);
        }
        InputEvent::MouseUp { x, y, button, modifiers, click_count } => {
            let mouse_event = MouseEvent { x: x as i32, y: y as i32, modifiers: modifiers_to_event_flags(modifiers) };
            host.send_mouse_click_event(Some(&mouse_event), mouse_button_to_cef(button), 1, click_count as i32);
        }
        InputEvent::Scroll { x, y, delta_x, delta_y, modifiers } => {
            let mouse_event = MouseEvent { x: x as i32, y: y as i32, modifiers: modifiers_to_event_flags(modifiers) };
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
            // CEF's own model). One event per UTF-16 unit, so an emoji
            // arrives as its surrogate pair (`utf16_units`).
            for unit in utf16_units(codepoint) {
                let e = KeyEvent {
                    size: std::mem::size_of::<KeyEvent>(),
                    type_: KeyEventType::CHAR,
                    modifiers: 0,
                    windows_key_code: unit as i32,
                    native_key_code: 0,
                    is_system_key: 0,
                    character: unit,
                    unmodified_character: unit,
                    focus_on_editable_field: 0,
                };
                host.send_key_event(Some(&e));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn astral_characters_become_surrogate_pairs() {
        assert_eq!(utf16_units(0x1F600), vec![0xD83D, 0xDE00]);
        assert_eq!(utf16_units('a' as u32), vec![0x61]);
        assert!(utf16_units(0xD800).is_empty(), "a lone surrogate isn't a character");
    }
}
