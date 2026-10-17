//! Translates winit input (mouse buttons, modifiers, key codes) into the
//! engine IPC protocol's types.

use super::*;

pub(crate) fn translate_mouse_button(button: WinitMouseButton) -> Option<MouseButton> {
    match button {
        WinitMouseButton::Left => Some(MouseButton::Left),
        WinitMouseButton::Right => Some(MouseButton::Right),
        WinitMouseButton::Middle => Some(MouseButton::Middle),
        _ => None,
    }
}

pub(crate) fn winit_modifiers_to_bits(state: ModifiersState) -> u8 {
    let mut bits = 0u8;
    if state.shift_key() {
        bits |= modifiers::SHIFT;
    }
    if state.control_key() {
        bits |= modifiers::CONTROL;
    }
    if state.alt_key() {
        bits |= modifiers::ALT;
    }
    if state.super_key() {
        bits |= modifiers::SUPER;
    }
    bits
}

/// Translates winit's cross-platform physical key into our own
/// engine-agnostic `manifold_view_protocol::KeyCode` — deliberately not forwarding
/// winit's `KeyCode` directly over IPC (not meant to be a stable wire
/// format across winit versions); see that type's own doc comment. Common
/// keys only, matching `KeyCode`'s own scope.
pub(crate) fn translate_key_code(key: PhysicalKey) -> Option<KeyCode> {
    let PhysicalKey::Code(code) = key else {
        return None;
    };
    Some(match code {
        WinitKeyCode::KeyA => KeyCode::KeyA,
        WinitKeyCode::KeyB => KeyCode::KeyB,
        WinitKeyCode::KeyC => KeyCode::KeyC,
        WinitKeyCode::KeyD => KeyCode::KeyD,
        WinitKeyCode::KeyE => KeyCode::KeyE,
        WinitKeyCode::KeyF => KeyCode::KeyF,
        WinitKeyCode::KeyG => KeyCode::KeyG,
        WinitKeyCode::KeyH => KeyCode::KeyH,
        WinitKeyCode::KeyI => KeyCode::KeyI,
        WinitKeyCode::KeyJ => KeyCode::KeyJ,
        WinitKeyCode::KeyK => KeyCode::KeyK,
        WinitKeyCode::KeyL => KeyCode::KeyL,
        WinitKeyCode::KeyM => KeyCode::KeyM,
        WinitKeyCode::KeyN => KeyCode::KeyN,
        WinitKeyCode::KeyO => KeyCode::KeyO,
        WinitKeyCode::KeyP => KeyCode::KeyP,
        WinitKeyCode::KeyQ => KeyCode::KeyQ,
        WinitKeyCode::KeyR => KeyCode::KeyR,
        WinitKeyCode::KeyS => KeyCode::KeyS,
        WinitKeyCode::KeyT => KeyCode::KeyT,
        WinitKeyCode::KeyU => KeyCode::KeyU,
        WinitKeyCode::KeyV => KeyCode::KeyV,
        WinitKeyCode::KeyW => KeyCode::KeyW,
        WinitKeyCode::KeyX => KeyCode::KeyX,
        WinitKeyCode::KeyY => KeyCode::KeyY,
        WinitKeyCode::KeyZ => KeyCode::KeyZ,
        WinitKeyCode::Digit0 => KeyCode::Digit0,
        WinitKeyCode::Digit1 => KeyCode::Digit1,
        WinitKeyCode::Digit2 => KeyCode::Digit2,
        WinitKeyCode::Digit3 => KeyCode::Digit3,
        WinitKeyCode::Digit4 => KeyCode::Digit4,
        WinitKeyCode::Digit5 => KeyCode::Digit5,
        WinitKeyCode::Digit6 => KeyCode::Digit6,
        WinitKeyCode::Digit7 => KeyCode::Digit7,
        WinitKeyCode::Digit8 => KeyCode::Digit8,
        WinitKeyCode::Digit9 => KeyCode::Digit9,
        WinitKeyCode::Enter => KeyCode::Enter,
        WinitKeyCode::Backspace => KeyCode::Backspace,
        WinitKeyCode::Tab => KeyCode::Tab,
        WinitKeyCode::Space => KeyCode::Space,
        WinitKeyCode::Escape => KeyCode::Escape,
        WinitKeyCode::Delete => KeyCode::Delete,
        WinitKeyCode::ArrowLeft => KeyCode::ArrowLeft,
        WinitKeyCode::ArrowRight => KeyCode::ArrowRight,
        WinitKeyCode::ArrowUp => KeyCode::ArrowUp,
        WinitKeyCode::ArrowDown => KeyCode::ArrowDown,
        WinitKeyCode::Home => KeyCode::Home,
        WinitKeyCode::End => KeyCode::End,
        WinitKeyCode::PageUp => KeyCode::PageUp,
        WinitKeyCode::PageDown => KeyCode::PageDown,
        WinitKeyCode::ShiftLeft => KeyCode::ShiftLeft,
        WinitKeyCode::ShiftRight => KeyCode::ShiftRight,
        WinitKeyCode::ControlLeft => KeyCode::ControlLeft,
        WinitKeyCode::ControlRight => KeyCode::ControlRight,
        WinitKeyCode::AltLeft => KeyCode::AltLeft,
        WinitKeyCode::AltRight => KeyCode::AltRight,
        _ => return None,
    })
}
