//! Throwaway spike -- NOT wired into the real engine-host, NOT meant to be
//! kept. Answers one narrow question left open after round 2's abandoned
//! `ICoreWebView2CompositionController::SendMouseInput` attempt (see
//! `src/input.rs`'s own doc comment for that history): was round 2's "didn't
//! fix input either" a fundamental wall, or a fixable detail (missing
//! `MoveFocus`, wrong coordinate handling, missing `LEAVE` events -- all
//! called out in Microsoft's own guidance and in a real WebView2Feedback
//! issue about non-origin coordinates)?
//!
//! This spike does the minimum needed to isolate that: real Composition
//! (`Windows.UI.Composition`) hosting -- not windowed hosting, since
//! `SendMouseInput`/`SendPointerInput`/`SendKeyInput` only exist on
//! `ICoreWebView2CompositionController` -- with `MoveFocus` called
//! explicitly before sending any input, then a drag (`LEFT_BUTTON_DOWN` ->
//! several `MOVE`s -> `LEFT_BUTTON_UP`) over plain text, a plain click on a
//! button, and a `WHEEL` event at a deliberately *non-origin* point (the one
//! coordinate that a real WebView2Feedback issue reports as broken in some
//! versions). Success is read back the same way `src/input.rs` already
//! verifies success -- `ExecuteScript` returning real page state -- not by
//! eyeballing a screenshot.
//!
//! No capture pipeline here at all: this only answers the input question.
//! Whether Composition hosting can be paired with a working capture
//! pipeline is a separate, larger question -- capture.rs's own doc comment
//! explains why the windowed-HWND + `Windows.Graphics.Capture` pipeline it
//! uses today doesn't carry over to Composition hosting for free.
//!
//! Run with `cargo run -p manifold-engine-webview2 --example composition_input_spike`.

use std::sync::mpsc;
use std::time::Duration;

use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2CompositionController, ICoreWebView2Controller,
    ICoreWebView2Environment, ICoreWebView2Environment3, COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_DOWN,
    COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_UP, COREWEBVIEW2_MOUSE_EVENT_KIND_MOVE,
    COREWEBVIEW2_MOUSE_EVENT_KIND_WHEEL, COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_LEFT_BUTTON,
    COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_NONE, COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC,
    COREWEBVIEW2_POINTER_EVENT_KIND_DOWN, COREWEBVIEW2_POINTER_EVENT_KIND_UP,
    COREWEBVIEW2_POINTER_EVENT_KIND_UPDATE,
};
use webview2_com::{
    CreateCoreWebView2CompositionControllerCompletedHandler, CreateCoreWebView2EnvironmentCompletedHandler,
    ExecuteScriptCompletedHandler, NavigationCompletedEventHandler,
};
use windows::core::{w, Interface, Result, HSTRING, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::HBRUSH;
use windows::Win32::System::WinRT::Composition::ICompositorDesktopInterop;
use windows::Win32::System::WinRT::{
    CreateDispatcherQueueController, RoInitialize, DQTAT_COM_NONE, DQTYPE_THREAD_CURRENT,
    RO_INIT_SINGLETHREADED,
};
use windows::Win32::UI::Input::Pointer::{
    POINTER_FLAG_DOWN, POINTER_FLAG_FIRSTBUTTON, POINTER_FLAG_INCONTACT, POINTER_FLAG_INRANGE,
    POINTER_FLAG_NEW, POINTER_FLAG_PRIMARY, POINTER_FLAG_UP, POINTER_FLAG_UPDATE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, PeekMessageW, RegisterClassW, ShowWindow,
    TranslateMessage, CS_HREDRAW, CS_VREDRAW, MSG, PM_REMOVE, PT_MOUSE, SW_SHOWNOACTIVATE,
    WINDOW_EX_STYLE, WNDCLASSW, WS_OVERLAPPEDWINDOW,
};
use windows::UI::Composition::Compositor;
use windows_numerics::Vector2;

const WIDTH: u32 = 900;
const HEIGHT: u32 = 500;
const CLASS_NAME: PCWSTR = w!("ManifoldCompositionInputSpikeHost");

/// A single line of `x` characters wide enough to drag a selection across,
/// a button whose `onclick` proves a real trusted click reached the page
/// (the same click-fires-default-action bar round 4 already holds itself
/// to), and a nested scrollable div positioned away from the origin -- the
/// exact case a real WebView2Feedback issue reports `SendMouseInput` wheel
/// events silently failing at.
const TEST_HTML: &str = r#"<!doctype html>
<html><body style="margin:0;padding:0;font-family:sans-serif;">
<button id="btn" style="position:absolute;left:20px;top:20px;width:260px;height:50px;font-size:18px;"
  onclick="window.__clicked = (window.__clicked||0) + 1;">click me</button>
<p id="text" style="position:absolute;left:20px;top:110px;width:820px;font-family:monospace;font-size:24px;white-space:pre;margin:0;">xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx</p>
<div id="scrollbox" style="position:absolute;left:20px;top:180px;width:200px;height:120px;
  overflow:auto;border:1px solid black;">
<div style="height:2000px;">tall</div>
</div>
<script>
window.__scrollTop = 0;
document.getElementById('scrollbox').addEventListener('scroll', function (e) {
  window.__scrollTop = e.target.scrollTop;
});
window.__events = [];
['mousedown','mousemove','mouseup'].forEach(function (type) {
  document.getElementById('text').addEventListener(type, function (e) {
    window.__events.push(type + '@' + e.clientX + ',' + e.clientY + ' buttons=' + e.buttons + ' trusted=' + e.isTrusted);
  });
});
window.__clickTrusted = null;
document.getElementById('btn').addEventListener('click', function (e) {
  window.__clickTrusted = e.isTrusted;
});
</script>
</body></html>"#;

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn create_window() -> Result<HWND> {
    unsafe {
        let instance = windows::Win32::System::LibraryLoader::GetModuleHandleW(None)?;
        let class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: CLASS_NAME,
            hbrBackground: HBRUSH::default(),
            ..Default::default()
        };
        let _ = RegisterClassW(&class);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS_NAME,
            w!("manifold-composition-input-spike"),
            WS_OVERLAPPEDWINDOW,
            -32000,
            -32000,
            WIDTH as i32,
            HEIGHT as i32,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        Ok(hwnd)
    }
}

fn pump_messages() {
    unsafe {
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Pumps messages for up to `timeout` waiting for `done()` to return true --
/// every wait in this spike (navigation, script results) goes through this
/// instead of a blocking call, since WebView2's own async completion
/// handlers are delivered through this thread's message pump.
fn pump_until(timeout: Duration, mut done: impl FnMut() -> bool) {
    let start = std::time::Instant::now();
    while !done() && start.elapsed() < timeout {
        pump_messages();
        std::thread::sleep(Duration::from_millis(15));
    }
}

fn create_environment() -> webview2_com::Result<ICoreWebView2Environment> {
    let (tx, rx) = mpsc::channel();
    CreateCoreWebView2EnvironmentCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| unsafe {
            webview2_com::Microsoft::Web::WebView2::Win32::CreateCoreWebView2EnvironmentWithOptions(
                PCWSTR::null(),
                PCWSTR::null(),
                None,
                &handler,
            )
            .map_err(webview2_com::Error::WindowsError)
        }),
        Box::new(move |error_code, environment| {
            error_code?;
            tx.send(environment.ok_or_else(|| {
                windows::core::Error::from(windows::Win32::Foundation::E_POINTER)
            }))
            .expect("send environment over mpsc channel");
            Ok(())
        }),
    )?;
    Ok(webview2_com::wait_with_pump(rx)??)
}

/// Composition hosting's own completed-handler chain: `ICoreWebView2Environment3::
/// CreateCoreWebView2CompositionController` (mouse/pointer/key input methods only
/// exist on `ICoreWebView2CompositionController`, so plain windowed hosting's
/// `CreateCoreWebView2Controller` can't be used here at all).
fn create_composition_controller(
    environment: &ICoreWebView2Environment,
    parent: HWND,
) -> webview2_com::Result<(ICoreWebView2CompositionController, ICoreWebView2Controller, ICoreWebView2)> {
    let environment3: ICoreWebView2Environment3 =
        environment.cast().map_err(webview2_com::Error::WindowsError)?;
    let (tx, rx) = mpsc::channel();
    CreateCoreWebView2CompositionControllerCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| unsafe {
            environment3
                .CreateCoreWebView2CompositionController(parent, &handler)
                .map_err(webview2_com::Error::WindowsError)
        }),
        Box::new(move |error_code, controller| {
            error_code?;
            tx.send(controller.ok_or_else(|| {
                windows::core::Error::from(windows::Win32::Foundation::E_POINTER)
            }))
            .expect("send composition controller over mpsc channel");
            Ok(())
        }),
    )?;
    let comp_controller = webview2_com::wait_with_pump(rx)??;
    // The object `CreateCoreWebView2CompositionController` hands back
    // implements both `ICoreWebView2CompositionController` (input) and
    // `ICoreWebView2Controller` (bounds/focus/visibility) -- documented
    // WebView2 behavior, confirmed here by a plain `.cast()` rather than
    // assumed.
    let controller: ICoreWebView2Controller =
        comp_controller.cast().map_err(webview2_com::Error::WindowsError)?;
    let webview = unsafe { controller.CoreWebView2() }?;
    Ok((comp_controller, controller, webview))
}

fn exec_and_read(webview: &ICoreWebView2, script: &str) -> String {
    let (tx, rx) = mpsc::channel();
    let handler = ExecuteScriptCompletedHandler::create(Box::new(move |_error, result| {
        let _ = tx.send(result);
        Ok(())
    }));
    unsafe {
        let _ = webview.ExecuteScript(&HSTRING::from(script), &handler);
    }
    let mut got = None;
    pump_until(Duration::from_secs(2), || {
        if let Ok(r) = rx.try_recv() {
            got = Some(r);
            true
        } else {
            false
        }
    });
    got.unwrap_or_default()
}

fn main() -> Result<()> {
    unsafe {
        RoInitialize(RO_INIT_SINGLETHREADED).ok();
    }

    // Required before creating any `Compositor` from a classic Win32
    // message-pump thread -- the standard, documented pattern for hosting
    // WinRT Composition outside a UWP/WinUI app.
    let queue_options = windows::Win32::System::WinRT::DispatcherQueueOptions {
        dwSize: std::mem::size_of::<windows::Win32::System::WinRT::DispatcherQueueOptions>() as u32,
        threadType: DQTYPE_THREAD_CURRENT,
        apartmentType: DQTAT_COM_NONE,
    };
    let _dispatcher_queue_controller = unsafe { CreateDispatcherQueueController(queue_options)? };

    let hwnd = create_window()?;

    let compositor = Compositor::new()?;
    let desktop_interop: ICompositorDesktopInterop = compositor.cast()?;
    let target = unsafe { desktop_interop.CreateDesktopWindowTarget(hwnd, false)? };
    let root = compositor.CreateContainerVisual()?;
    root.SetSize(Vector2 { X: WIDTH as f32, Y: HEIGHT as f32 })?;
    target.SetRoot(&root)?;

    println!("[spike] environment...");
    let environment = create_environment().expect("create environment");
    println!("[spike] composition controller...");
    let (comp_controller, controller, webview) =
        create_composition_controller(&environment, hwnd).expect("create composition controller");

    unsafe {
        comp_controller.SetRootVisualTarget(&root)?;
        controller.SetBounds(RECT { left: 0, top: 0, right: WIDTH as i32, bottom: HEIGHT as i32 })?;
        controller.SetIsVisible(true)?;
    }

    println!("[spike] navigating...");
    let navigated = std::rc::Rc::new(std::cell::Cell::new(false));
    {
        let navigated = navigated.clone();
        let handler = NavigationCompletedEventHandler::create(Box::new(move |_sender, _args| {
            navigated.set(true);
            Ok(())
        }));
        let mut token = Default::default();
        unsafe {
            let _ = webview.add_NavigationCompleted(&handler, &mut token);
        }
    }
    unsafe {
        webview.NavigateToString(&HSTRING::from(TEST_HTML))?;
    }
    pump_until(Duration::from_secs(5), || navigated.get());
    if !navigated.get() {
        println!("[spike] FAIL: navigation never completed");
        return Ok(());
    }

    // This is the one call round 2 apparently never confirmed was actually
    // in place -- Microsoft's own guidance is explicit that focus is
    // managed through `ICoreWebView2Controller` even while input goes
    // through `ICoreWebView2CompositionController`.
    println!("[spike] MoveFocus...");
    unsafe {
        controller.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC)?;
    }
    pump_messages();
    std::thread::sleep(Duration::from_millis(200));

    // 1) Plain click on the button -- the simplest possible go/no-go signal
    //    for whether SendMouseInput reaches Chromium's real input pipeline
    //    at all (a trusted click, not a hand-dispatched DOM event).
    println!("[spike] click...");
    send_click(&comp_controller, 150, 45);
    pump_messages();
    std::thread::sleep(Duration::from_millis(200));
    let clicked = exec_and_read(&webview, "String(window.__clicked || 0)");
    println!("[spike]   __clicked = {clicked}");
    let click_trusted = exec_and_read(&webview, "String(window.__clickTrusted)");
    println!("[spike]   click event.isTrusted = {click_trusted}");

    // 2) Drag-select across the text line. Diagnostics first: confirm the
    //    coordinates actually land on the `<p>` text at all, since a wrong
    //    y-coordinate (font metrics assumed rather than measured) would
    //    produce the same "empty selection" symptom as SendMouseInput
    //    itself not working -- these are two very different verdicts and
    //    must not be conflated.
    // `ExecuteScript`'s own completion value is already JSON-encoded (see
    // `input.rs`'s `parse_json_string_or_null` doc comment) -- wrapping the
    // script's return value in an extra `JSON.stringify` here would
    // double-encode it and make a plain string-equality check against `""`
    // wrong, which is exactly the bug this comment is replacing.
    let hit_from = exec_and_read(&webview, "(document.elementFromPoint(25,122)||{}).id || ''");
    let hit_to = exec_and_read(&webview, "(document.elementFromPoint(500,122)||{}).id || ''");
    println!("[spike]   elementFromPoint(25,122) id = {hit_from}, (500,122) id = {hit_to}");
    println!("[spike] drag-select...");
    send_drag_select(&comp_controller, (25, 122), (500, 122));
    pump_messages();
    std::thread::sleep(Duration::from_millis(200));
    let selection = exec_and_read(&webview, "window.getSelection().toString()");
    println!("[spike]   selection = {selection:?} (raw len {})", selection.len());
    let events = exec_and_read(&webview, "window.__events.join(' | ')");
    println!("[spike]   raw DOM events seen = {events}");
    let range_count = exec_and_read(&webview, "String(window.getSelection().rangeCount)");
    println!("[spike]   selection.rangeCount = {range_count}");

    // 2b) Same drag, but via `SendPointerInput` (the newer API) instead of
    //     legacy `SendMouseInput`, in case Chromium's selection-extension
    //     logic specifically keys off pointer events. Selection is cleared
    //     first so this attempt's result can't be confused with 2)'s.
    let _ = exec_and_read(&webview, "window.getSelection().removeAllRanges(); '1'");
    println!("[spike] drag-select via SendPointerInput...");
    let environment3: ICoreWebView2Environment3 = environment.cast().expect("cast Environment3");
    send_drag_select_pointer(&comp_controller, &environment3, (25, 122), (500, 122));
    pump_messages();
    std::thread::sleep(Duration::from_millis(200));
    let selection2 = exec_and_read(&webview, "window.getSelection().toString()");
    println!("[spike]   selection (pointer) = {selection2:?}");

    // 3) Wheel at a deliberately non-origin point -- the exact case a real
    //    WebView2Feedback issue reports failing in some versions.
    println!("[spike] wheel at non-origin point...");
    send_wheel(&comp_controller, 120, 230, -240);
    pump_messages();
    std::thread::sleep(Duration::from_millis(300));
    let scroll_top = exec_and_read(&webview, "String(window.__scrollTop)");
    println!("[spike]   __scrollTop = {scroll_top}");

    println!();
    println!("=== VERDICT ===");
    let click_ok = clicked.trim() != "0" && !clicked.trim().is_empty();
    // `selection` is `ExecuteScript`'s own JSON-encoded return value -- an
    // empty selection round-trips as the literal 2-character JSON string
    // `""`, so anything else means real selected text came back.
    let selection_ok = selection.trim() != "\"\"" && !selection.trim().is_empty();
    let scroll_ok = scroll_top.trim() != "0" && !scroll_top.trim().is_empty();
    println!("click:      {}", if click_ok { "PASS" } else { "FAIL" });
    println!("drag-select:{}", if selection_ok { "PASS" } else { "FAIL" });
    println!("wheel:      {}", if scroll_ok { "PASS" } else { "FAIL" });

    Ok(())
}

fn send_click(comp_controller: &ICoreWebView2CompositionController, x: i32, y: i32) {
    unsafe {
        let _ = comp_controller.SendMouseInput(
            COREWEBVIEW2_MOUSE_EVENT_KIND_MOVE,
            COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_NONE,
            0,
            POINT { x, y },
        );
        let _ = comp_controller.SendMouseInput(
            COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_DOWN,
            COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_LEFT_BUTTON,
            0,
            POINT { x, y },
        );
        let _ = comp_controller.SendMouseInput(
            COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_UP,
            COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_NONE,
            0,
            POINT { x, y },
        );
    }
}

/// `mousedown` -> several `mousemove`s with the button still logically held
/// -> `mouseup` -- the same down/move/up shape a real text-selection drag
/// is made of, not a single jump from start to end. Generously spaced out
/// in real time (100ms/step, 20 steps) to rule out a timing/gesture-
/// recognition threshold as the explanation for an otherwise dead-on-target
/// drag producing no selection.
fn send_drag_select(
    comp_controller: &ICoreWebView2CompositionController,
    from: (i32, i32),
    to: (i32, i32),
) {
    unsafe {
        let _ = comp_controller.SendMouseInput(
            COREWEBVIEW2_MOUSE_EVENT_KIND_MOVE,
            COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_NONE,
            0,
            POINT { x: from.0, y: from.1 },
        );
        let _ = comp_controller.SendMouseInput(
            COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_DOWN,
            COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_LEFT_BUTTON,
            0,
            POINT { x: from.0, y: from.1 },
        );
        std::thread::sleep(Duration::from_millis(100));
        const STEPS: i32 = 20;
        for step in 1..=STEPS {
            let x = from.0 + (to.0 - from.0) * step / STEPS;
            let y = from.1 + (to.1 - from.1) * step / STEPS;
            let _ = comp_controller.SendMouseInput(
                COREWEBVIEW2_MOUSE_EVENT_KIND_MOVE,
                COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_LEFT_BUTTON,
                0,
                POINT { x, y },
            );
            pump_messages();
            std::thread::sleep(Duration::from_millis(100));
        }
        std::thread::sleep(Duration::from_millis(100));
        let _ = comp_controller.SendMouseInput(
            COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_UP,
            COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_NONE,
            0,
            POINT { x: to.0, y: to.1 },
        );
    }
}

/// Same down/move.../up drag as `send_drag_select`, but through
/// `SendPointerInput` -- a `PT_MOUSE`-kind `ICoreWebView2PointerInfo`, one
/// consistent `PointerId` across all three events (matching real Win32
/// pointer-message semantics), `PixelLocation` for the coordinate.
fn send_drag_select_pointer(
    comp_controller: &ICoreWebView2CompositionController,
    environment3: &ICoreWebView2Environment3,
    from: (i32, i32),
    to: (i32, i32),
) {
    const POINTER_ID: u32 = 1;
    let make_info = |flags: u32, point: (i32, i32)| unsafe {
        let info = environment3.CreateCoreWebView2PointerInfo().expect("create pointer info");
        info.SetPointerKind(PT_MOUSE.0 as u32).expect("set kind");
        info.SetPointerId(POINTER_ID).expect("set id");
        info.SetPointerFlags(flags).expect("set flags");
        info.SetPixelLocation(POINT { x: point.0, y: point.1 }).expect("set location");
        info
    };
    unsafe {
        let down = make_info(
            (POINTER_FLAG_NEW.0
                | POINTER_FLAG_INRANGE.0
                | POINTER_FLAG_INCONTACT.0
                | POINTER_FLAG_FIRSTBUTTON.0
                | POINTER_FLAG_PRIMARY.0
                | POINTER_FLAG_DOWN.0) as u32,
            from,
        );
        let _ = comp_controller.SendPointerInput(COREWEBVIEW2_POINTER_EVENT_KIND_DOWN, &down);
        std::thread::sleep(Duration::from_millis(100));

        const STEPS: i32 = 20;
        for step in 1..=STEPS {
            let x = from.0 + (to.0 - from.0) * step / STEPS;
            let y = from.1 + (to.1 - from.1) * step / STEPS;
            let update = make_info(
                (POINTER_FLAG_INRANGE.0
                    | POINTER_FLAG_INCONTACT.0
                    | POINTER_FLAG_FIRSTBUTTON.0
                    | POINTER_FLAG_PRIMARY.0
                    | POINTER_FLAG_UPDATE.0) as u32,
                (x, y),
            );
            let _ = comp_controller.SendPointerInput(COREWEBVIEW2_POINTER_EVENT_KIND_UPDATE, &update);
            pump_messages();
            std::thread::sleep(Duration::from_millis(100));
        }
        std::thread::sleep(Duration::from_millis(100));

        let up = make_info((POINTER_FLAG_INRANGE.0 | POINTER_FLAG_UP.0) as u32, to);
        let _ = comp_controller.SendPointerInput(COREWEBVIEW2_POINTER_EVENT_KIND_UP, &up);
    }
}

fn send_wheel(comp_controller: &ICoreWebView2CompositionController, x: i32, y: i32, delta: i32) {
    unsafe {
        let _ = comp_controller.SendMouseInput(
            COREWEBVIEW2_MOUSE_EVENT_KIND_MOVE,
            COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_NONE,
            0,
            POINT { x, y },
        );
        // `mousedata` carries the wheel delta (matches Win32's `WHEEL_DELTA`
        // scale, same as the real `WM_MOUSEWHEEL` HIWORD(wParam)).
        let _ = comp_controller.SendMouseInput(
            COREWEBVIEW2_MOUSE_EVENT_KIND_WHEEL,
            COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_NONE,
            delta as u32,
            POINT { x, y },
        );
    }
}
