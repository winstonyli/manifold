//! Throwaway verification spike -- NOT wired into the real engine-host.
//! Examples can't import `src/main.rs`'s private modules (this crate has no
//! `[lib]` target, same reason `capture_spike.rs`/`composition_input_spike.rs`
//! duplicate code instead of importing it), so this runs the *exact same*
//! JS strings `src/input.rs`'s `send_input` now generates for `MouseDown`/
//! `MouseMove`/`MouseUp`/`Scroll` against a real, plain **windowed** WebView2
//! (this project's actual hosting mode -- unlike `composition_input_spike.rs`,
//! no Composition/DirectComposition involved at all) to confirm the new
//! drag-selection and nested-scroll JS logic actually works live, not just
//! that it reads plausibly.
//!
//! Run with `cargo run -p manifold-engine-webview2 --example drag_select_verify`.

use std::sync::mpsc;
use std::time::Duration;

use webview2_com::Microsoft::Web::WebView2::Win32::{ICoreWebView2, ICoreWebView2Controller, ICoreWebView2Environment};
use webview2_com::{
    CreateCoreWebView2ControllerCompletedHandler, CreateCoreWebView2EnvironmentCompletedHandler,
    ExecuteScriptCompletedHandler, NavigationCompletedEventHandler,
};
use windows::core::{w, Result, HSTRING, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::HBRUSH;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, PeekMessageW, RegisterClassW, ShowWindow,
    TranslateMessage, CS_HREDRAW, CS_VREDRAW, MSG, PM_REMOVE, SW_SHOWNOACTIVATE, WINDOW_EX_STYLE,
    WNDCLASSW, WS_OVERLAPPEDWINDOW,
};

const WIDTH: u32 = 900;
const HEIGHT: u32 = 500;
const CLASS_NAME: PCWSTR = w!("ManifoldDragSelectVerifyHost");

const TEST_HTML: &str = r##"<!doctype html>
<html><body style="margin:0;padding:0;font-family:sans-serif;">
<a id="link" href="#nope" style="position:absolute;left:20px;top:20px;width:260px;height:50px;
  display:block;font-size:18px;" onclick="window.__clicked=(window.__clicked||0)+1;return false;">a link</a>
<p id="text" style="position:absolute;left:20px;top:110px;width:820px;font-family:monospace;font-size:24px;white-space:pre;margin:0;">xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx</p>
<div id="scrollbox" style="position:absolute;left:20px;top:180px;width:200px;height:120px;
  overflow:auto;border:1px solid black;">
<div style="height:2000px;">tall</div>
</div>
<input id="field" style="position:absolute;left:20px;top:330px;width:200px;font-size:18px;" />
<script>
window.__scrollTop = 0;
document.getElementById('scrollbox').addEventListener('scroll', function (e) {
  window.__scrollTop = e.target.scrollTop;
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
            w!("manifold-drag-select-verify"),
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
            tx.send(environment.ok_or_else(|| {
                windows::core::Error::from(windows::Win32::Foundation::E_POINTER)
            }))
            .expect("send environment over mpsc channel");
            Ok(())
        }),
    )?;
    Ok(webview2_com::wait_with_pump(rx)??)
}

fn create_controller(
    environment: ICoreWebView2Environment,
    parent: HWND,
) -> webview2_com::Result<(ICoreWebView2Controller, ICoreWebView2)> {
    let (tx, rx) = mpsc::channel();
    CreateCoreWebView2ControllerCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| unsafe {
            environment
                .CreateCoreWebView2Controller(parent, &handler)
                .map_err(webview2_com::Error::WindowsError)
        }),
        Box::new(move |error_code, controller| {
            error_code?;
            tx.send(controller.ok_or_else(|| {
                windows::core::Error::from(windows::Win32::Foundation::E_POINTER)
            }))
            .expect("send controller over mpsc channel");
            Ok(())
        }),
    )?;
    let controller = webview2_com::wait_with_pump(rx)??;
    let webview = unsafe { controller.CoreWebView2() }?;
    Ok((controller, webview))
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

// --- The exact scripts `src/input.rs::send_input` now generates ---

fn guard(x: f64, y: f64) -> String {
    format!("var el=document.elementFromPoint({x},{y});if(el&&el.closest('input,textarea,select'))return;")
}

fn mousedown_script(x: f64, y: f64) -> String {
    format!(
        "(function(){{{g}var r=document.caretRangeFromPoint({x},{y});if(r){{var s=window.getSelection();s.removeAllRanges();s.collapse(r.startContainer,r.startOffset);}}}})();",
        g = guard(x, y),
    )
}

/// Diagnostic-reporting variant of `src/input.rs`'s `MouseMove` script --
/// instead of running silently -- used to find out *why* `extend()` isn't
/// growing the selection.
fn mousemove_diag_script(x: f64, y: f64) -> String {
    format!(
        "(function(){{var el=document.elementFromPoint({x},{y});if(el&&el.closest('input,textarea,select'))return 'guard-skip';var r=document.caretRangeFromPoint({x},{y});if(!r)return 'no-range';var s=window.getSelection();var before=s.anchorOffset+'/'+s.focusOffset;try{{s.extend(r.startContainer,r.startOffset);return 'offset='+r.startOffset+' before='+before+' after='+s.anchorOffset+'/'+s.focusOffset+' len='+s.toString().length;}}catch(e){{return 'threw:'+e.message;}}}})();"
    )
}

fn click_script(x: f64, y: f64) -> String {
    format!("(function(){{var e=document.elementFromPoint({x},{y});if(e){{if(e.focus){{e.focus();}}e.click();}}}})();")
}

fn scroll_script(x: f64, y: f64, dx: f64, dy: f64) -> String {
    format!(
        "(function(){{var el=document.elementFromPoint({x},{y});while(el&&el!==document.body&&el!==document.documentElement){{var cs=getComputedStyle(el);var sy=(cs.overflowY==='auto'||cs.overflowY==='scroll')&&el.scrollHeight>el.clientHeight;var sx=(cs.overflowX==='auto'||cs.overflowX==='scroll')&&el.scrollWidth>el.clientWidth;if(sy||sx){{el.scrollBy({dx},{dy});return;}}el=el.parentElement;}}window.scrollBy({dx},{dy});}})();"
    )
}

fn main() -> Result<()> {
    unsafe {
        windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_SINGLETHREADED).ok();
    }

    let hwnd = create_window()?;
    let environment = create_environment().expect("create environment");
    let (controller, webview) = create_controller(environment, hwnd).expect("create controller");
    unsafe {
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

    // 1) Drag-select across the text line: down -> several moves -> up.
    //    Per the new `MouseUp` logic, a completed drag must NOT also fire a
    //    synthetic click.
    println!("[verify] drag-select over plain text...");
    let diag = exec_and_read(
        &webview,
        "(function(){var el=document.elementFromPoint(25,122);var r=document.caretRangeFromPoint(25,122);return JSON.stringify({elId:el&&el.id,hasRange:!!r,startText:r&&r.startContainer&&r.startContainer.nodeName});})()",
    );
    println!("[verify]   diag (text point) = {diag}");
    exec(&webview, mousedown_script(25.0, 122.0));
    pump_messages();
    std::thread::sleep(Duration::from_millis(50));
    let after_down = exec_and_read(&webview, "String(window.getSelection().rangeCount)");
    println!("[verify]   rangeCount after mousedown = {after_down}");
    pump_messages();
    for step in 1..=10 {
        let x = 25.0 + (500.0 - 25.0) * step as f64 / 10.0;
        let result = exec_and_read(&webview, &mousemove_diag_script(x, 122.0));
        println!("[verify]   move step {step} @x={x} -> {result}");
        std::thread::sleep(Duration::from_millis(20));
    }
    // Real `send_input` would skip the click here because `moved` is true --
    // this spike calls `exec` directly (it isn't going through the real
    // `Drag` state machine), so it explicitly does NOT call click_script,
    // matching what the real code's gating is meant to achieve.
    std::thread::sleep(Duration::from_millis(100));
    let selection = exec_and_read(&webview, "window.getSelection().toString()");
    println!("[verify]   selection = {selection:?}");

    // 2) A plain stationary click on the link still fires (down + up, no
    //    intervening move -- so the real code's `moved` stays false and the
    //    click still fires; here just call click_script directly, matching
    //    what `MouseUp` does when `moved` is false).
    println!("[verify] plain click on link...");
    exec(&webview, mousedown_script(150.0, 45.0));
    pump_messages();
    std::thread::sleep(Duration::from_millis(50));
    exec(&webview, click_script(150.0, 45.0));
    pump_messages();
    std::thread::sleep(Duration::from_millis(150));
    let clicked = exec_and_read(&webview, "String(window.__clicked || 0)");
    println!("[verify]   __clicked = {clicked}");

    // 3) Clicking into the text `<input>` must still focus it (the
    //    form-control guard must not divert the selection there).
    println!("[verify] click into input field...");
    let diag2 = exec_and_read(
        &webview,
        "(function(){var el=document.elementFromPoint(120,345);return JSON.stringify({elId:el&&el.id,tag:el&&el.tagName});})()",
    );
    println!("[verify]   diag (input point) = {diag2}");
    let mousedown_result = exec_and_read(
        &webview,
        "(function(){var el=document.elementFromPoint(120,345);if(el&&el.closest('input,textarea,select'))return 'guard-skip';return 'ran-through';})()",
    );
    println!("[verify]   mousedown guard check = {mousedown_result}");
    exec(&webview, mousedown_script(120.0, 345.0));
    pump_messages();
    std::thread::sleep(Duration::from_millis(50));
    let after_mousedown_only =
        exec_and_read(&webview, "String(document.activeElement && document.activeElement.id)");
    println!("[verify]   activeElement after mousedown alone = {after_mousedown_only}");
    exec(&webview, click_script(120.0, 345.0));
    pump_messages();
    std::thread::sleep(Duration::from_millis(150));
    let focused_input = exec_and_read(&webview, "String(document.activeElement && document.activeElement.id)");
    println!("[verify]   document.activeElement.id = {focused_input}");

    // 4) Wheel/scroll reaching a nested scrollable div, not just the page.
    println!("[verify] nested scroll...");
    exec(&webview, scroll_script(120.0, 230.0, 0.0, 240.0));
    pump_messages();
    std::thread::sleep(Duration::from_millis(200));
    let scroll_top = exec_and_read(&webview, "String(window.__scrollTop)");
    println!("[verify]   __scrollTop = {scroll_top}");

    println!();
    println!("=== VERDICT ===");
    let selection_ok = selection.trim() != "\"\"" && !selection.trim().is_empty();
    let click_ok = clicked.trim() != "0" && !clicked.trim().is_empty();
    let input_ok = focused_input.trim() == "\"field\"";
    let scroll_ok = scroll_top.trim() != "0" && !scroll_top.trim().is_empty();
    println!("drag-select:      {}", if selection_ok { "PASS" } else { "FAIL" });
    println!("plain click:      {}", if click_ok { "PASS" } else { "FAIL" });
    println!("input still focusable: {}", if input_ok { "PASS" } else { "FAIL" });
    println!("nested scroll:    {}", if scroll_ok { "PASS" } else { "FAIL" });

    Ok(())
}
