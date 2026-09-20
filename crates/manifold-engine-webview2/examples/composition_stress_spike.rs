//! Throwaway spike -- NOT wired into the real engine-host. Answers the
//! bug-applicability question left open after finding two real, open,
//! unresolved WebView2Feedback issues in Composition Mode's input routing:
//! [#5712](https://github.com/MicrosoftEdge/WebView2Feedback/issues/5712)
//! and [#5713](https://github.com/MicrosoftEdge/WebView2Feedback/issues/5713),
//! both describing input going totally unresponsive after repeatedly
//! pressing inside the WebView, dragging *outside* the host window, and
//! releasing there. Both reports describe a **real physical mouse**
//! leaving a **real OS window** -- this project's whole input model is
//! synthetic (`SendMouseInput` replaying already-captured events), so the
//! mouse never actually leaves anything at the OS level. This spike checks
//! whether the same degradation shows up anyway under *purely synthetic*
//! repeated out-of-bounds drags, or whether that specific trigger condition
//! genuinely doesn't apply here.
//!
//! Method: establish a working baseline (click + drag-select, same checks
//! `composition_input_spike.rs` already uses), run many repeated
//! press-inside/drag-past-the-visual-edge/release-outside cycles purely via
//! `SendMouseInput`, then re-run the *identical* baseline checks and compare.
//!
//! Run with `cargo run -p manifold-engine-webview2 --example composition_stress_spike`.

use std::sync::mpsc;
use std::time::Duration;

use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2CompositionController, ICoreWebView2Controller, ICoreWebView2Environment,
    ICoreWebView2Environment3, COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_DOWN,
    COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_UP, COREWEBVIEW2_MOUSE_EVENT_KIND_MOVE,
    COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_LEFT_BUTTON, COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_NONE,
    COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC,
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
    CreateDispatcherQueueController, RoInitialize, DQTAT_COM_NONE, DQTYPE_THREAD_CURRENT, RO_INIT_SINGLETHREADED,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, PeekMessageW, RegisterClassW, ShowWindow,
    TranslateMessage, CS_HREDRAW, CS_VREDRAW, MSG, PM_REMOVE, SW_SHOWNOACTIVATE, WINDOW_EX_STYLE,
    WNDCLASSW, WS_OVERLAPPEDWINDOW,
};
use windows::UI::Composition::Compositor;
use windows_numerics::Vector2;

const WIDTH: u32 = 700;
const HEIGHT: u32 = 400;
const CLASS_NAME: PCWSTR = w!("ManifoldCompositionStressSpikeHost");
const STRESS_CYCLES: u32 = 150;

const TEST_HTML: &str = r##"<!doctype html>
<html><body style="margin:0;padding:0;font-family:sans-serif;">
<button id="btn" style="position:absolute;left:20px;top:20px;width:200px;height:50px;font-size:18px;"
  onclick="window.__clickCount=(window.__clickCount||0)+1;">click me</button>
<p id="text" style="position:absolute;left:20px;top:100px;width:600px;font-family:monospace;font-size:24px;white-space:pre;margin:0;">xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx</p>
</body></html>"##;

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
            w!("manifold-composition-stress-spike"),
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
            tx.send(environment.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_POINTER)))
                .expect("send environment");
            Ok(())
        }),
    )?;
    Ok(webview2_com::wait_with_pump(rx)??)
}

fn create_composition_controller(
    environment: &ICoreWebView2Environment,
    parent: HWND,
) -> webview2_com::Result<(ICoreWebView2CompositionController, ICoreWebView2Controller, ICoreWebView2)> {
    let environment3: ICoreWebView2Environment3 = environment.cast().map_err(webview2_com::Error::WindowsError)?;
    let (tx, rx) = mpsc::channel();
    CreateCoreWebView2CompositionControllerCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| unsafe {
            environment3
                .CreateCoreWebView2CompositionController(parent, &handler)
                .map_err(webview2_com::Error::WindowsError)
        }),
        Box::new(move |error_code, controller| {
            error_code?;
            tx.send(controller.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_POINTER)))
                .expect("send composition controller");
            Ok(())
        }),
    )?;
    let comp_controller = webview2_com::wait_with_pump(rx)??;
    let controller: ICoreWebView2Controller = comp_controller.cast().map_err(webview2_com::Error::WindowsError)?;
    let webview = unsafe { controller.CoreWebView2() }?;
    Ok((comp_controller, controller, webview))
}

fn exec(webview: &ICoreWebView2, script: String) {
    let handler = ExecuteScriptCompletedHandler::create(Box::new(|_error, _result| Ok(())));
    unsafe {
        let _ = webview.ExecuteScript(&HSTRING::from(script), &handler);
    }
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

fn send_drag_select(comp_controller: &ICoreWebView2CompositionController, from: (i32, i32), to: (i32, i32)) {
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
        const STEPS: i32 = 10;
        for step in 1..=STEPS {
            let x = from.0 + (to.0 - from.0) * step / STEPS;
            let y = from.1 + (to.1 - from.1) * step / STEPS;
            let _ = comp_controller.SendMouseInput(
                COREWEBVIEW2_MOUSE_EVENT_KIND_MOVE,
                COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_LEFT_BUTTON,
                0,
                POINT { x, y },
            );
        }
        let _ = comp_controller.SendMouseInput(
            COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_UP,
            COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_NONE,
            0,
            POINT { x: to.0, y: to.1 },
        );
    }
}

/// The exact shape the two real WebView2Feedback bug reports describe:
/// press *inside* the visual, drag *past its edge* (negative/oversized
/// coordinates -- there's no real window here for the pointer to "leave",
/// so this is the closest synthetic equivalent), release *outside*. No
/// `MOVE` back inside before `UP`, matching "drag outside and release
/// there" rather than "drag out and back."
fn send_drag_outside_and_release(comp_controller: &ICoreWebView2CompositionController, inside: (i32, i32)) {
    unsafe {
        let _ = comp_controller.SendMouseInput(
            COREWEBVIEW2_MOUSE_EVENT_KIND_MOVE,
            COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_NONE,
            0,
            POINT { x: inside.0, y: inside.1 },
        );
        let _ = comp_controller.SendMouseInput(
            COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_DOWN,
            COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_LEFT_BUTTON,
            0,
            POINT { x: inside.0, y: inside.1 },
        );
        // Step out past the visual's own bounds in both axes.
        const STEPS: i32 = 6;
        let outside = (-200, -200);
        for step in 1..=STEPS {
            let x = inside.0 + (outside.0 - inside.0) * step / STEPS;
            let y = inside.1 + (outside.1 - inside.1) * step / STEPS;
            let _ = comp_controller.SendMouseInput(
                COREWEBVIEW2_MOUSE_EVENT_KIND_MOVE,
                COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_LEFT_BUTTON,
                0,
                POINT { x, y },
            );
        }
        // Release well outside the visual's bounds.
        let _ = comp_controller.SendMouseInput(
            COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_UP,
            COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_NONE,
            0,
            POINT { x: outside.0, y: outside.1 },
        );
    }
}

fn run_baseline_check(
    label: &str,
    comp_controller: &ICoreWebView2CompositionController,
    webview: &ICoreWebView2,
) -> (bool, bool) {
    let _ = exec_and_read(webview, "window.__clickCount=0;window.getSelection().removeAllRanges();'1'");
    pump_messages();

    send_click(comp_controller, 100, 45);
    pump_messages();
    std::thread::sleep(Duration::from_millis(100));
    let clicked = exec_and_read(webview, "String(window.__clickCount||0)");
    // `exec_and_read` returns `ExecuteScript`'s own JSON-encoded return
    // value -- a JS string "1" round-trips as the literal 3-character JSON
    // text `"1"`, not bare `1` (same gotcha as `drag_select_verify.rs` hit).
    let click_ok = clicked.trim() == "\"1\"";

    send_drag_select(comp_controller, (25, 122), (400, 122));
    pump_messages();
    std::thread::sleep(Duration::from_millis(100));
    let selection = exec_and_read(webview, "window.getSelection().toString()");
    let selection_ok = selection.trim() != "\"\"" && !selection.trim().is_empty();

    println!("[spike] {label}: click__clicked={clicked} (ok={click_ok})  selection={selection:?} (ok={selection_ok})");
    (click_ok, selection_ok)
}

fn main() -> Result<()> {
    unsafe {
        RoInitialize(RO_INIT_SINGLETHREADED).ok();
    }
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

    println!("[spike] environment + composition controller...");
    let environment = create_environment().expect("create environment");
    let (comp_controller, controller, webview) =
        create_composition_controller(&environment, hwnd).expect("create composition controller");

    unsafe {
        comp_controller.SetRootVisualTarget(&root)?;
        controller.SetBounds(RECT { left: 0, top: 0, right: WIDTH as i32, bottom: HEIGHT as i32 })?;
        controller.SetIsVisible(true)?;
    }

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
    assert!(navigated.get(), "navigation never completed");

    unsafe {
        controller.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC)?;
    }
    pump_messages();
    std::thread::sleep(Duration::from_millis(200));

    println!("[spike] === BASELINE (before stress) ===");
    let (base_click, base_selection) = run_baseline_check("baseline", &comp_controller, &webview);

    println!("[spike] === STRESS: {STRESS_CYCLES} press-inside/drag-past-edge/release-outside cycles ===");
    for i in 0..STRESS_CYCLES {
        send_drag_outside_and_release(&comp_controller, (100, 45));
        pump_messages();
        std::thread::sleep(Duration::from_millis(20));
        if i % 10 == 9 {
            println!("[spike]   ...{} cycles done", i + 1);
        }
    }
    // Let any pending input processing settle.
    for _ in 0..10 {
        pump_messages();
        std::thread::sleep(Duration::from_millis(50));
    }

    println!("[spike] === AFTER STRESS ===");
    let (after_click, after_selection) = run_baseline_check("after-stress", &comp_controller, &webview);

    println!();
    println!("=== VERDICT ===");
    println!("baseline:     click={base_click} selection={base_selection}");
    println!("after stress: click={after_click} selection={after_selection}");
    if base_click && base_selection && after_click && after_selection {
        println!("RESULT: input survived {STRESS_CYCLES} synthetic drag-past-edge/release-outside cycles with no degradation.");
        println!("The #5712/#5713 trigger condition (real mouse leaving a real window) does not appear to reproduce under purely synthetic SendMouseInput.");
    } else if base_click && base_selection && !(after_click && after_selection) {
        println!("RESULT: input DEGRADED after stress cycles -- the bug (or something like it) DOES reproduce under synthetic input too. Real risk, not just a real-mouse-specific issue.");
    } else {
        println!("RESULT: baseline itself was not clean -- rerun/investigate before trusting the after-stress comparison.");
    }

    Ok(())
}
