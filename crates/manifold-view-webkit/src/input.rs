use std::cell::RefCell;
use std::rc::Rc;

use gtk::gdk;
use gtk::prelude::*;
use javascriptcore::ValueExt;
use manifold_view_protocol::{modifiers, InputEvent, KeyCode, MouseButton};
use webkit2gtk::{WebView, WebViewExt};

/// `(x, y, href)` for a right-click that landed on a link — see
/// `manifold_view_protocol::ViewToHost::ContextMenuLink`'s doc comment.
pub type PendingContextMenu = Rc<RefCell<Option<(f64, f64, String)>>>;

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
    pending_context_menu: &PendingContextMenu,
) {
    let Some(window) = webview.window() else {
        return;
    };

    // A right-click here also dispatches a genuine native `ButtonRelease`
    // below (same as every other button) -- WebKitGTK's own real
    // `context-menu` signal fires from that and is suppressed elsewhere
    // (see `main.rs`) so no real popup window ever tries to show. This is
    // a separate, script-driven query for the one thing this minimal
    // feature actually needs: whether the click landed on a link.
    if let InputEvent::MouseUp { x, y, button: MouseButton::Right, .. } = event {
        let pending = pending_context_menu.clone();
        let zoom = webview.zoom_level().max(0.01);
        // InputEvent coordinates are surface pixels; elementFromPoint takes CSS pixels, which page zoom scales.
        webview.run_javascript(
            &format!(
                "(function(){{var e=document.elementFromPoint({x}/{zoom},{y}/{zoom});var a=e&&e.closest('a[href]');return a?a.href:null;}})();"
            ),
            gtk::gio::Cancellable::NONE,
            move |result| {
                if let Ok(js_result) = result {
                    if let Some(value) = js_result.js_value() {
                        if value.is_string() {
                            *pending.borrow_mut() = Some((x, y, value.to_str().to_string()));
                        }
                    }
                }
            },
        );
    }

    match event {
        InputEvent::MouseMove { x, y, modifiers } => {
            send_button_or_motion_event(
                &window,
                pointer_device,
                gdk::EventType::MotionNotify,
                x,
                y,
                0,
                modifiers_to_gdk(modifiers),
            );
        }
        InputEvent::MouseDown { x, y, button, modifiers, click_count } => {
            let (button, state) = (mouse_button_to_gdk(button), modifiers_to_gdk(modifiers));
            send_button_or_motion_event(&window, pointer_device, gdk::EventType::ButtonPress, x, y, button, state);
            // GDK3 follows the second press of a double click with a
            // `2BUTTON_PRESS`, and that is what WebKitGTK counts as a double
            // click (word selection, `dblclick`).
            if click_count == 2 {
                send_button_or_motion_event(
                    &window,
                    pointer_device,
                    gdk::EventType::DoubleButtonPress,
                    x,
                    y,
                    button,
                    state,
                );
            }
        }
        InputEvent::MouseUp { x, y, button, modifiers, .. } => {
            send_button_or_motion_event(
                &window,
                pointer_device,
                gdk::EventType::ButtonRelease,
                x,
                y,
                mouse_button_to_gdk(button),
                modifiers_to_gdk(modifiers),
            );
        }
        InputEvent::Scroll { x, y, delta_x, delta_y, .. } => {
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

/// Letters/digits/space (`KeyCode::produces_text`) deliberately do *not*
/// map to their own real keyval here (`GDK_KEY_a` etc.) — GTK's
/// input-method handling reads a `GdkEventKey`'s keyval directly to decide
/// what text to insert, so a real letter keyval on `KeyDown` inserts text
/// *in addition to* the separate `InputEvent::Char` pair below
/// (double-insert, the same class of bug Servo hit — see project memory).
/// Worse, in testing this left WebKitGTK's input-method context in a state
/// where it silently dropped every keystroke after the first.
/// `GDK_KEY_VoidSymbol` (X11's "this key has no keysym" placeholder) is
/// GDK's nearest equivalent to Servo's fix
/// (`Key::Named(NamedKey::Unidentified)`) — a valid key event for
/// shortcut-matching purposes (which key off `KeyCode` on the manifold
/// side, never a keyval) that produces no text of its own, leaving
/// `InputEvent::Char`'s `send_char_event` as the only thing that ever
/// inserts real text, mirroring how CEF/Servo already split "physical key"
/// from "produced text".
fn key_to_gdk_keyval(key: KeyCode) -> Option<u32> {
    use gdk::ffi::*;
    if key.produces_text() {
        return Some(GDK_KEY_VoidSymbol as u32);
    }
    let keyval = match key {
        KeyCode::Enter => GDK_KEY_Return,
        KeyCode::Backspace => GDK_KEY_BackSpace,
        KeyCode::Tab => GDK_KEY_Tab,
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
        // Text keys returned above; a new non-text key lands here until it gets its own arm.
        _ => GDK_KEY_VoidSymbol,
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
            gdk::EventType::ButtonPress | gdk::EventType::DoubleButtonPress | gdk::EventType::ButtonRelease => {
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
fn send_scroll_event(window: &gdk::Window, device: Option<&gdk::Device>, x: f64, y: f64, delta_x: f64, delta_y: f64) {
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

fn send_key_event(window: &gdk::Window, device: Option<&gdk::Device>, type_: gdk::EventType, key: KeyCode, mods: u8) {
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
/// codepoint` for anything above the direct-mapped Latin-1/keysym range).
/// This is the *only* thing that inserts text on WebKit (see
/// `key_to_gdk_keyval`'s doc comment) — `KeyDown`/`KeyUp` deliberately
/// don't, for letters/digits/space.
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
