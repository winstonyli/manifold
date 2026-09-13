use gtk::gdk;
use gtk::prelude::*;
use ipc_protocol::{modifiers, InputEvent, KeyCode, MouseButton};
use webkit2gtk::WebView;

/// Round 2 on WebKit input (see project memory for round 1's revert): the
/// prior attempt synthesized `GdkEventKey`/`Button`/`Motion` structs by hand
/// but never attached a `GdkDevice` to them, which produced a
/// `Gdk-WARNING: Event with type 8/9 not holding a GdkDevice` and — in
/// testing — a runaway CPU loop the moment a real keystroke was typed into
/// a page's input field. `gdk_event_set_device` (safe wrapper:
/// `Event::set_device`) is the fix under test here: it's a *generic* GDK
/// API specifically because raw `GdkEventKey` has no `device` field of its
/// own to poke directly (unlike `GdkEventButton`/`GdkEventMotion`, which
/// do — using the same generic setter for all three anyway, for one
/// consistent/simple code path).
///
/// `keyboard_device`/`pointer_device` come from `gdk::Seat::keyboard()`/
/// `pointer()` on the offscreen widget's own `gdk::Display`'s default seat
/// — resolved once by the caller (see `main.rs`), not on every event.
pub fn send_input(
    webview: &WebView,
    event: InputEvent,
    keyboard_device: Option<&gdk::Device>,
    pointer_device: Option<&gdk::Device>,
) {
    let Some(window) = webview.window() else {
        return;
    };

    match event {
        InputEvent::MouseMove { x, y } => {
            send_button_or_motion_event(
                &window,
                pointer_device,
                gdk::EventType::MotionNotify,
                x,
                y,
                0,
                gdk::ModifierType::empty(),
            );
        }
        InputEvent::MouseDown { x, y, button } => {
            send_button_or_motion_event(
                &window,
                pointer_device,
                gdk::EventType::ButtonPress,
                x,
                y,
                mouse_button_to_gdk(button),
                gdk::ModifierType::empty(),
            );
        }
        InputEvent::MouseUp { x, y, button } => {
            send_button_or_motion_event(
                &window,
                pointer_device,
                gdk::EventType::ButtonRelease,
                x,
                y,
                mouse_button_to_gdk(button),
                gdk::ModifierType::empty(),
            );
        }
        InputEvent::Scroll { x, y, delta_x, delta_y } => {
            send_scroll_event(&window, pointer_device, x, y, delta_x, delta_y);
        }
        InputEvent::KeyDown { key, modifiers: mods } => {
            send_key_event(&window, keyboard_device, gdk::EventType::KeyPress, key, mods);
        }
        InputEvent::KeyUp { key, modifiers: mods } => {
            send_key_event(&window, keyboard_device, gdk::EventType::KeyRelease, key, mods);
        }
        InputEvent::Char { codepoint } => {
            send_char_event(&window, keyboard_device, codepoint);
        }
    }
}

fn mouse_button_to_gdk(button: MouseButton) -> u32 {
    match button {
        MouseButton::Left => 1,
        MouseButton::Middle => 2,
        MouseButton::Right => 3,
    }
}

fn modifiers_to_gdk(bits: u8) -> gdk::ModifierType {
    let mut state = gdk::ModifierType::empty();
    if bits & modifiers::SHIFT != 0 {
        state |= gdk::ModifierType::SHIFT_MASK;
    }
    if bits & modifiers::CONTROL != 0 {
        state |= gdk::ModifierType::CONTROL_MASK;
    }
    if bits & modifiers::ALT != 0 {
        state |= gdk::ModifierType::MOD1_MASK;
    }
    state
}

/// Physical-key keyval only (e.g. `KeyCode::KeyA` -> lowercase `a`) — Shift
/// is conveyed via `state`/`ModifierType::SHIFT_MASK`, same as a real X11
/// keyboard driver would report; the actually-typed character (already
/// case-corrected) arrives separately via `InputEvent::Char`, matching how
/// `engine-host-cef`/`engine-host-servo` already split "physical key" from
/// "produced text" — see `ipc_protocol::InputEvent::Char`'s doc comment.
fn key_to_gdk_keyval(key: KeyCode) -> Option<u32> {
    use gdk::ffi::*;
    let keyval = match key {
        KeyCode::KeyA => GDK_KEY_a,
        KeyCode::KeyB => GDK_KEY_b,
        KeyCode::KeyC => GDK_KEY_c,
        KeyCode::KeyD => GDK_KEY_d,
        KeyCode::KeyE => GDK_KEY_e,
        KeyCode::KeyF => GDK_KEY_f,
        KeyCode::KeyG => GDK_KEY_g,
        KeyCode::KeyH => GDK_KEY_h,
        KeyCode::KeyI => GDK_KEY_i,
        KeyCode::KeyJ => GDK_KEY_j,
        KeyCode::KeyK => GDK_KEY_k,
        KeyCode::KeyL => GDK_KEY_l,
        KeyCode::KeyM => GDK_KEY_m,
        KeyCode::KeyN => GDK_KEY_n,
        KeyCode::KeyO => GDK_KEY_o,
        KeyCode::KeyP => GDK_KEY_p,
        KeyCode::KeyQ => GDK_KEY_q,
        KeyCode::KeyR => GDK_KEY_r,
        KeyCode::KeyS => GDK_KEY_s,
        KeyCode::KeyT => GDK_KEY_t,
        KeyCode::KeyU => GDK_KEY_u,
        KeyCode::KeyV => GDK_KEY_v,
        KeyCode::KeyW => GDK_KEY_w,
        KeyCode::KeyX => GDK_KEY_x,
        KeyCode::KeyY => GDK_KEY_y,
        KeyCode::KeyZ => GDK_KEY_z,
        KeyCode::Digit0 => GDK_KEY_0,
        KeyCode::Digit1 => GDK_KEY_1,
        KeyCode::Digit2 => GDK_KEY_2,
        KeyCode::Digit3 => GDK_KEY_3,
        KeyCode::Digit4 => GDK_KEY_4,
        KeyCode::Digit5 => GDK_KEY_5,
        KeyCode::Digit6 => GDK_KEY_6,
        KeyCode::Digit7 => GDK_KEY_7,
        KeyCode::Digit8 => GDK_KEY_8,
        KeyCode::Digit9 => GDK_KEY_9,
        KeyCode::Enter => GDK_KEY_Return,
        KeyCode::Backspace => GDK_KEY_BackSpace,
        KeyCode::Tab => GDK_KEY_Tab,
        KeyCode::Space => GDK_KEY_space,
        KeyCode::Escape => GDK_KEY_Escape,
        KeyCode::Delete => GDK_KEY_Delete,
        KeyCode::ArrowLeft => GDK_KEY_Left,
        KeyCode::ArrowRight => GDK_KEY_Right,
        KeyCode::ArrowUp => GDK_KEY_Up,
        KeyCode::ArrowDown => GDK_KEY_Down,
        KeyCode::Home => GDK_KEY_Home,
        KeyCode::End => GDK_KEY_End,
        KeyCode::PageUp => GDK_KEY_Page_Up,
        KeyCode::PageDown => GDK_KEY_Page_Down,
        KeyCode::ShiftLeft => GDK_KEY_Shift_L,
        KeyCode::ShiftRight => GDK_KEY_Shift_R,
        KeyCode::ControlLeft => GDK_KEY_Control_L,
        KeyCode::ControlRight => GDK_KEY_Control_R,
        KeyCode::AltLeft => GDK_KEY_Alt_L,
        KeyCode::AltRight => GDK_KEY_Alt_R,
    };
    Some(keyval as u32)
}

/// Builds and injects a `GdkEventButton`/`GdkEventMotion` (same field shape
/// for the purposes used here: window/send_event/time/x/y/state, plus
/// button-only fields for the button variant). `window.to_glib_full()` (not
/// `to_glib_none()`) for the `window` field: the event's own `Drop`
/// (`gdk_event_free`) unconditionally unrefs whatever pointer is stored
/// there, so it must own a real reference, not borrow one from `window`
/// that then gets double-freed.
fn send_button_or_motion_event(
    window: &gdk::Window,
    device: Option<&gdk::Device>,
    type_: gdk::EventType,
    x: f64,
    y: f64,
    button: u32,
    state: gdk::ModifierType,
) {
    use gdk::glib::translate::{IntoGlib, ToGlibPtr, ToGlibPtrMut};

    let mut ev = gdk::Event::new(type_);
    unsafe {
        match type_ {
            gdk::EventType::MotionNotify => {
                let any: *mut gdk::ffi::GdkEvent = ev.to_glib_none_mut().0;
                let raw = any as *mut gdk::ffi::GdkEventMotion;
                (*raw).window = window.to_glib_full();
                (*raw).send_event = 1;
                (*raw).time = gdk::ffi::GDK_CURRENT_TIME as u32;
                (*raw).x = x;
                (*raw).y = y;
                (*raw).x_root = x;
                (*raw).y_root = y;
                (*raw).state = state.into_glib();
                (*raw).is_hint = 0;
            }
            gdk::EventType::ButtonPress | gdk::EventType::ButtonRelease => {
                let any: *mut gdk::ffi::GdkEvent = ev.to_glib_none_mut().0;
                let raw = any as *mut gdk::ffi::GdkEventButton;
                (*raw).window = window.to_glib_full();
                (*raw).send_event = 1;
                (*raw).time = gdk::ffi::GDK_CURRENT_TIME as u32;
                (*raw).x = x;
                (*raw).y = y;
                (*raw).x_root = x;
                (*raw).y_root = y;
                (*raw).state = state.into_glib();
                (*raw).button = button;
            }
            _ => return,
        }
    }
    ev.set_device(device);
    ev.put();
}

/// `GdkScrollDirection::Smooth` + `delta_x`/`delta_y` is the modern,
/// pixel-precise scroll path (what a real trackpad/precision mouse wheel
/// reports) — the legacy `Up`/`Down`/`Left`/`Right` directions are a
/// coarser fallback for old discrete-notch mice that WebKitGTK also still
/// understands, but smooth is the better match for
/// `InputEvent::Scroll`'s own already-continuous `delta_x`/`delta_y`.
fn send_scroll_event(
    window: &gdk::Window,
    device: Option<&gdk::Device>,
    x: f64,
    y: f64,
    delta_x: f64,
    delta_y: f64,
) {
    use gdk::glib::translate::{IntoGlib, ToGlibPtr, ToGlibPtrMut};

    let mut ev = gdk::Event::new(gdk::EventType::Scroll);
    unsafe {
        let any: *mut gdk::ffi::GdkEvent = ev.to_glib_none_mut().0;
        let raw = any as *mut gdk::ffi::GdkEventScroll;
        (*raw).window = window.to_glib_full();
        (*raw).send_event = 1;
        (*raw).time = gdk::ffi::GDK_CURRENT_TIME as u32;
        (*raw).x = x;
        (*raw).y = y;
        (*raw).x_root = x;
        (*raw).y_root = y;
        (*raw).state = gdk::ModifierType::empty().into_glib();
        (*raw).direction = gdk::ScrollDirection::Smooth.into_glib();
        (*raw).delta_x = delta_x;
        (*raw).delta_y = delta_y;
        (*raw).is_stop = 0;
    }
    ev.set_device(device);
    ev.put();
}

fn send_key_event(
    window: &gdk::Window,
    device: Option<&gdk::Device>,
    type_: gdk::EventType,
    key: KeyCode,
    mods: u8,
) {
    use gdk::glib::translate::{IntoGlib, ToGlibPtr, ToGlibPtrMut};

    let Some(keyval) = key_to_gdk_keyval(key) else {
        return;
    };
    let mut ev = gdk::Event::new(type_);
    unsafe {
        let any: *mut gdk::ffi::GdkEvent = ev.to_glib_none_mut().0;
        let raw = any as *mut gdk::ffi::GdkEventKey;
        (*raw).window = window.to_glib_full();
        (*raw).send_event = 1;
        (*raw).time = gdk::ffi::GDK_CURRENT_TIME as u32;
        (*raw).state = modifiers_to_gdk(mods).into_glib();
        (*raw).keyval = keyval;
        (*raw).length = 0;
        (*raw).string = std::ptr::null_mut();
        (*raw).hardware_keycode = 0;
        (*raw).group = 0;
        (*raw).is_modifier = 0;
    }
    ev.set_device(device);
    ev.put();
}

/// `InputEvent::Char` (the actually-produced, already-case-corrected text)
/// is delivered as a `GDK_KEY_PRESS`+`GDK_KEY_RELEASE` pair carrying the
/// Unicode codepoint's own keyval — `gdk_unicode_to_keyval` maps a
/// codepoint to the matching X11 keysym (identity for ASCII, `0x1000000 |
/// codepoint` for anything above the direct-mapped Latin-1/keysym range),
/// which is what WebKitGTK's input handling actually reads text from.
fn send_char_event(window: &gdk::Window, device: Option<&gdk::Device>, codepoint: u32) {
    use gdk::glib::translate::{IntoGlib, ToGlibPtr, ToGlibPtrMut};

    let keyval = unsafe { gdk::ffi::gdk_unicode_to_keyval(codepoint) };
    for type_ in [gdk::EventType::KeyPress, gdk::EventType::KeyRelease] {
        let mut ev = gdk::Event::new(type_);
        unsafe {
            let any: *mut gdk::ffi::GdkEvent = ev.to_glib_none_mut().0;
            let raw = any as *mut gdk::ffi::GdkEventKey;
            (*raw).window = window.to_glib_full();
            (*raw).send_event = 1;
            (*raw).time = gdk::ffi::GDK_CURRENT_TIME as u32;
            (*raw).state = gdk::ModifierType::empty().into_glib();
            (*raw).keyval = keyval;
            (*raw).length = 0;
            (*raw).string = std::ptr::null_mut();
            (*raw).hardware_keycode = 0;
            (*raw).group = 0;
            (*raw).is_modifier = 0;
        }
        ev.set_device(device);
        ev.put();
    }
}
