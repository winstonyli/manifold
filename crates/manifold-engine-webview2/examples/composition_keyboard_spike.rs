//! Throwaway spike -- NOT wired into the real engine-host. Answers a
//! question left open by discovering `ICoreWebView2CompositionController`
//! has no `SendKeyInput` at all (confirmed: absent from this project's
//! pinned webview2-com-sys 0.39.1 bindings, and confirmed a real API gap,
//! not a missing binding, by a Microsoft team member's own comment on
//! WebView2Feedback#1054: "the parent window... receives keyboard input
//! directly from Windows... once focused, the OS automatically routes
//! keyboard events to WebView2" -- no injection API needed *for real
//! hardware input*.
//!
//! The open question: does that same automatic routing also pick up
//! `PostMessage`-injected `WM_KEYDOWN`/`WM_CHAR`/`WM_KEYUP` sent to the
//! *top-level Composition host window* (not WebView2's own internal
//! `Chrome_RenderWidgetHostHWND` child, which is what this project's round
//! 1 tested for *mouse* and found ignored -- a different window, and mouse,
//! not keyboard, so that result doesn't automatically generalize here), and
//! whether it requires the window to hold *real* Win32 keyboard focus
//! (`SetFocus`) as opposed to just WebView2's own internal `MoveFocus`.
//! That distinction matters a lot: if real `SetFocus` is required, native
//! keyboard delivery hits the same "needs to be the real focused window"
//! wall that ruled out `SendInput` for mouse, which would be a genuine
//! architectural blocker for a hidden, never-foregrounded, multi-tab host
//! window -- if `MoveFocus` alone is enough, it's viable.
//!
//! Run with `cargo run -p manifold-engine-webview2 --example composition_keyboard_spike`.

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
use windows::Win32::UI::Input::KeyboardAndMouse::{SetFocus, VK_RETURN};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, PeekMessageW, PostMessageW, RegisterClassW, ShowWindow,
    TranslateMessage, CS_HREDRAW, CS_VREDRAW, MSG, PM_REMOVE, SW_SHOWNOACTIVATE, WINDOW_EX_STYLE, WM_CHAR,
    WM_KEYDOWN, WM_KEYUP, WNDCLASSW, WS_OVERLAPPEDWINDOW,
};
use windows::UI::Composition::Compositor;
use windows_numerics::Vector2;

const WIDTH: u32 = 500;
const HEIGHT: u32 = 300;
const CLASS_NAME: PCWSTR = w!("ManifoldCompositionKeyboardSpikeHost");

const TEST_HTML: &str = r##"<!doctype html>
<html><body style="margin:0;padding:0;font-family:sans-serif;">
<input id="field" autofocus style="position:absolute;left:20px;top:20px;width:400px;font-size:24px;" />
<script>
window.__keydowns = [];
document.getElementById('field').addEventListener('keydown', function (e) {
  window.__keydowns.push(e.key + '/trusted=' + e.isTrusted);
});
</script>
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
            w!("manifold-composition-keyboard-spike"),
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

/// `wparam`/`lparam` shaped like a real `Enter` keypress -- `VK_RETURN`,
/// scancode 0x1c in bits 16-23, matching what a genuine `WM_KEYDOWN` for
/// Enter looks like (not just a bare virtual-key code with a zeroed lParam).
fn post_enter_key(hwnd: HWND) {
    unsafe {
        let lparam_down = LPARAM(0x1c0001);
        let _ = PostMessageW(Some(hwnd), WM_KEYDOWN, WPARAM(VK_RETURN.0 as usize), lparam_down);
        let _ = PostMessageW(Some(hwnd), WM_CHAR, WPARAM(0x0D), lparam_down);
        let lparam_up = LPARAM(0xc01c0001u32 as isize);
        let _ = PostMessageW(Some(hwnd), WM_KEYUP, WPARAM(VK_RETURN.0 as usize), lparam_up);
    }
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

    // Click into the field first (mouse already confirmed working) so it
    // has focus for keydown to land on, then MoveFocus for good measure.
    println!("[spike] click into field + MoveFocus...");
    send_click(&comp_controller, 100, 35);
    pump_messages();
    std::thread::sleep(Duration::from_millis(100));
    unsafe {
        controller.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC)?;
    }
    pump_messages();
    std::thread::sleep(Duration::from_millis(200));
    let active = exec_and_read(&webview, "String(document.activeElement && document.activeElement.id)");
    println!("[spike]   document.activeElement.id = {active}");

    // Phase 1: PostMessage WM_KEYDOWN/WM_CHAR/WM_KEYUP to the top-level host
    // window WITHOUT giving it real Win32 focus (matches this project's
    // actual architecture: the host window is never the real foreground/
    // focused OS window).
    println!("[spike] Phase 1: PostMessage Enter WITHOUT real Win32 SetFocus...");
    post_enter_key(hwnd);
    pump_messages();
    std::thread::sleep(Duration::from_millis(200));
    let keydowns_1 = exec_and_read(&webview, "window.__keydowns.join(' | ')");
    println!("[spike]   __keydowns after phase 1 = {keydowns_1:?}");

    // Phase 2: same, but WITH real Win32 SetFocus on the host window first,
    // to isolate whether that's the missing ingredient.
    println!("[spike] Phase 2: PostMessage Enter WITH real Win32 SetFocus...");
    unsafe {
        let _ = SetFocus(Some(hwnd));
    }
    pump_messages();
    std::thread::sleep(Duration::from_millis(100));
    post_enter_key(hwnd);
    pump_messages();
    std::thread::sleep(Duration::from_millis(200));
    let keydowns_2 = exec_and_read(&webview, "window.__keydowns.join(' | ')");
    println!("[spike]   __keydowns after phase 2 = {keydowns_2:?}");

    println!();
    println!("=== VERDICT ===");
    let phase1_worked = !keydowns_1.trim().is_empty() && keydowns_1.trim() != "\"\"";
    let phase2_only = !phase1_worked && !keydowns_2.trim().is_empty() && keydowns_2.trim() != "\"\"";
    if phase1_worked {
        println!("PASS: PostMessage'd WM_KEYDOWN reached the page WITHOUT real Win32 focus -- native keyboard delivery is architecturally viable for a never-foregrounded host window.");
    } else if phase2_only {
        println!("PARTIAL: only worked WITH real Win32 SetFocus -- native keyboard delivery would require making the host window the real focused window, the same architectural wall that ruled out SendInput for mouse.");
    } else {
        println!("FAIL: PostMessage'd keyboard input did not reach the page in either phase -- native keyboard delivery via PostMessage to the top-level host window does not work; CDP dispatchKeyEvent remains necessary.");
    }

    Ok(())
}
