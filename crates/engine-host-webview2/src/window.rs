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

use windows::core::{w, GUID, Result, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::HBRUSH;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Shell::ITaskbarList;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, PeekMessageW, RegisterClassW, SetWindowPos,
    ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW, MSG, PM_REMOVE, SW_SHOWNOACTIVATE,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER, WINDOW_EX_STYLE, WNDCLASSW, WS_OVERLAPPEDWINDOW,
};

const CLASS_NAME: PCWSTR = w!("MultiengineBrowserWebView2Host");
// CLSID_TaskbarList -- {56FDF344-FD6D-11D0-958A-006097C9A090}. Not
// generated as a coclass helper by windows-rs here, so constructed
// directly from the well-known GUID rather than guessed.
const CLSID_TASKBAR_LIST: GUID = GUID::from_u128(0x56FDF344_FD6D_11D0_958A_006097C9A090);

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
/// `WS_POPUP`/`WS_EX_TOOLWINDOW` -- confirmed live (`examples/
/// capture_spike.rs`) that `Windows.Graphics.Capture`'s `CreateForWindow`
/// rejects both with `E_INVALIDARG` ("Could not capture the given
/// window"). **Also confirmed live (same error) that `CreateForWindow`
/// rejects an *owned* window just as categorically** -- an attempt to hide
/// this window from the taskbar by giving it an owner (any owner, not just
/// a specially-crafted one) hit the identical `E_INVALIDARG` the moment
/// `Capture::new` ran, crashing the engine-host in a respawn loop. So
/// nothing about *this* window's own identity (style bits or ownership)
/// can change before capture starts -- taskbar visibility has to be fixed
/// up afterward instead, see `hide_from_taskbar`.
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

/// Drops this window's taskbar entry via `ITaskbarList::DeleteTab` --
/// Explorer's own documented API for exactly this ("app has a top-level
/// window that must stay a normal window for other reasons, but shouldn't
/// show up in the taskbar"), not a window-style trick at all.
///
/// Three window-style-based approaches were tried first and all made
/// things *worse*, not better -- kept here so a future session doesn't
/// retry them:
/// - `WS_EX_TOOLWINDOW`/`WS_POPUP` at window-creation time: `Windows.
///   Graphics.Capture`'s `CreateForWindow` rejects both outright with
///   `E_INVALIDARG` (this is *why* `create_hidden_window` uses plain
///   `WS_OVERLAPPEDWINDOW` at all).
/// - Giving the window an owner (any owner, including a purpose-built
///   invisible one): confirmed live, `CreateForWindow` rejects an *owned*
///   window with the identical `E_INVALIDARG` -- ownership status is
///   checked just as strictly as the style bits.
/// - Adding `WS_EX_TOOLWINDOW` via `SetWindowLongPtrW` *after* capture
///   already started (this doesn't trip the `CreateForWindow` check, since
///   that only validates once): confirmed live, this reliably corrupts the
///   already-running capture -- with `SWP_FRAMECHANGED` forcing an
///   immediate non-client recalculation (`WS_EX_TOOLWINDOW` windows get a
///   shorter caption), content came back blank white with a
///   garbage-colored seam at the old caption boundary; the bare style bit
///   with no recalculation trigger still came back blank (no seam, but no
///   content either); adding an explicit hide/show cycle (the normally-
///   correct way to force Explorer to notice a style change) made capture
///   go solid black instead, i.e. stopped delivering frames entirely.
///   `ITaskbarList::DeleteTab` touches none of this -- no style bits, no
///   visibility transition, no NC-area recalculation -- so it doesn't
///   share any of these failure modes.
///
/// Runs on a **detached background thread**, not inline -- confirmed live
/// that `ITaskbarList::DeleteTab` can hang indefinitely in an environment
/// with no responsive Explorer/taskbar shell (observed in this project's
/// own sandboxed test session: `engine-host-webview2.exe` stayed alive but
/// completely unresponsive, never even reaching the tick loop, until
/// force-killed). This project already went to real effort (the whole
/// off-screen-window + `--headless` design) to avoid depending on a real
/// interactive desktop, so this call must never be allowed to block
/// startup even in a shell-less/CI-like context -- best-effort only, same
/// shape as `favicon::register`'s graceful degradation. `HWND` isn't
/// `Send` (wraps a raw pointer Rust can't prove is thread-safe, even
/// though Win32 handles are designed to cross threads freely) -- carried
/// across as a plain `isize` and reconstructed on the other side instead
/// of fighting the type system.
pub fn hide_from_taskbar(hwnd: HWND) {
    let hwnd_value = hwnd.0 as isize;
    std::thread::spawn(move || unsafe {
        let hwnd = HWND(hwnd_value as *mut core::ffi::c_void);
        let _ = windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
        );
        let Ok(taskbar_list) =
            CoCreateInstance::<_, ITaskbarList>(&CLSID_TASKBAR_LIST, None, CLSCTX_INPROC_SERVER)
        else {
            return;
        };
        let _ = taskbar_list.HrInit();
        let _ = taskbar_list.DeleteTab(hwnd);
    });
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
