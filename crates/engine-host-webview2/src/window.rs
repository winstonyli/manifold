//! A minimal Win32 window WebView2's windowed hosting can attach to.
//!
//! Genuinely invisible windows (`WS_VISIBLE` never set) are a known trap on
//! Windows for GPU-composited content -- `browser-ui`'s own headless mode
//! hit this exact issue (see its `resumed()` doc comment: DWM permanently
//! treats a truly-invisible window's swapchain as occluded, so it never
//! successfully renders a frame). The proven fix used there, replicated
//! here: a real, shown window, undecorated and positioned far off-screen,
//! so it never overlaps anything the user can see or click while still
//! being something the compositor treats as a normal paintable window.

use windows::core::{w, Result, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::HBRUSH;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, PeekMessageW, RegisterClassW, SetWindowPos,
    ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW, MSG, PM_REMOVE, SW_SHOWNOACTIVATE,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER, WINDOW_EX_STYLE, WNDCLASSW, WS_OVERLAPPEDWINDOW,
};

const CLASS_NAME: PCWSTR = w!("MultiengineBrowserWebView2Host");

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // WebView2's own window-subclassing handles everything it cares about;
    // this process never needs custom message handling of its own.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Creates the host window: off-screen positioned (never overlaps the
/// user's real desktop). Deliberately a plain `WS_OVERLAPPEDWINDOW`, not
/// `WS_POPUP`/`WS_EX_TOOLWINDOW` as originally written -- confirmed live
/// (`examples/capture_spike.rs`) that `Windows.Graphics.Capture`'s
/// `CreateForWindow` rejects both with `E_INVALIDARG` ("Could not capture
/// the given window") -- a real, undocumented-enough-to-be-worth-recording
/// restriction, not a guess. Being a normal top-level window means it could
/// technically show up in alt-tab/taskbar, but since it's permanently
/// parked off-screen the user would only ever see a blank rectangle out in
/// space if they somehow switched to it -- an acceptable tradeoff for
/// actually being capturable at all.
pub fn create_hidden_window() -> Result<HWND> {
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
        // A stale registration from a prior process is fine to ignore --
        // each engine-host is a fresh process, but RegisterClassW can still
        // race with itself across rapid respawns of the same binary if the
        // OS hasn't fully unregistered the class yet.
        let _ = RegisterClassW(&class);

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS_NAME,
            w!("multiengine-browser-webview2-host"),
            WS_OVERLAPPEDWINDOW,
            -32000,
            -32000,
            1024,
            768,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        Ok(hwnd)
    }
}

/// Resizes the actual host window's outer size to match the requested
/// content size -- capture (`capture.rs`) grabs the whole window's client
/// area, so the window itself must track `HostToEngine::Resize`, not just
/// the WebView2 controller's own bounds within it (otherwise the captured
/// frame would stay at the window's original fixed size regardless of what
/// `SetBounds` was told).
pub fn resize(hwnd: HWND, width: u32, height: u32) {
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            width as i32,
            height as i32,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

/// Non-blocking drain of this thread's Win32 message queue -- WebView2's
/// windowed hosting (and its own internal multi-process message routing)
/// needs this pumped regularly; `PM_REMOVE` (not a blocking `GetMessageW`)
/// keeps this composing with the tick loop's own fixed-interval sleep
/// rather than blocking the whole engine-host on window messages.
pub fn pump_messages() {
    unsafe {
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
