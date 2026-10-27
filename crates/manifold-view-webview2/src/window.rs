//! A minimal Win32 window to anchor WebView2's Composition visual tree to.
//!
//! Nothing renders into this window's own client area any more: WebView2 is
//! hosted via `ICoreWebView2CompositionController` against a
//! `Windows.UI.Composition` visual (`composition.rs`), and frames are
//! captured from that visual (`capture.rs`). But Composition hosting still
//! needs a real top-level `HWND` -- `CreateDesktopWindowTarget` takes one,
//! and `CreateCoreWebView2CompositionController` takes one as its parent --
//! which is all this window is for.
//!
//! Genuinely invisible windows (`WS_VISIBLE` never set) are a known trap on
//! Windows for GPU-composited content -- `manifold`'s own headless mode
//! hit this exact issue (see its `resumed()` doc comment: DWM permanently
//! treats a truly-invisible window's swapchain as occluded, so it never
//! successfully renders a frame). The proven fix used there, replicated
//! here: a real, shown window, undecorated and positioned far off-screen,
//! so it never overlaps anything the user can see or click while still
//! being something the compositor treats as a normal paintable window.

use windows::core::{w, Result, GUID, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::HBRUSH;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Shell::ITaskbarList;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, PeekMessageW, RegisterClassW, SetWindowPos, ShowWindow,
    TranslateMessage, CS_HREDRAW, CS_VREDRAW, MSG, PM_REMOVE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER,
    SW_SHOWNOACTIVATE, WNDCLASSW, WS_EX_NOACTIVATE, WS_OVERLAPPEDWINDOW,
};

const CLASS_NAME: PCWSTR = w!("ManifoldWebView2Host");
// CLSID_TaskbarList -- {56FDF344-FD6D-11D0-958A-006097C9A090}. Not
// generated as a coclass helper by windows-rs here, so constructed
// directly from the well-known GUID rather than guessed.
const CLSID_TASKBAR_LIST: GUID = GUID::from_u128(0x56FDF344_FD6D_11D0_958A_006097C9A090);

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // WebView2's own window-subclassing handles everything it cares about;
    // this process never needs custom message handling of its own.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Creates the host window: off-screen positioned (never overlaps the
/// user's real desktop), a plain `WS_OVERLAPPEDWINDOW`.
///
/// Capture reads the Composition visual (`CreateFromVisual`), which doesn't
/// inspect the window, so the style and ownership are not load-bearing.
/// `WS_OVERLAPPEDWINDOW` is kept because it is what has been verified to
/// work end to end; `hide_from_taskbar` handles the taskbar entry it
/// produces (see its doc comment for why that mechanism was picked, and
/// what changing it would need re-verifying).
///
/// `WS_EX_NOACTIVATE`: the window still activates inside this process's own
/// thread (its first `ShowWindow`, despite `SW_SHOWNOACTIVATE`, and
/// `MoveFocus`'s `SetFocus` in `main` both did), which keeps the WebView's
/// logical focus, but it never becomes the OS foreground window. Without
/// it, whenever this process had foreground rights (inherited from a
/// `manifold` that had the foreground, or the foreground lock open), that
/// activation took the foreground from the user's own app, e.g. out of a
/// fullscreen game (`scripts/foreground-watch.ps1 -Permissive`, 2026-10-07).
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
        // each view-host is a fresh process, but RegisterClassW can still
        // race with itself across rapid respawns of the same binary if the
        // OS hasn't fully unregistered the class yet.
        let _ = RegisterClassW(&class);

        let hwnd = CreateWindowExW(
            WS_EX_NOACTIVATE,
            CLASS_NAME,
            w!("manifold-webview2-host"),
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
/// things *worse*, not better. **All three failed against the old
/// `CreateForWindow`-based capture, which no longer exists** (capture reads
/// the Composition visual now, and doesn't look at the window), so none of
/// these is known to be a problem any more -- but none has been re-tested
/// either. Kept here so a future session that wants to simplify this away
/// (e.g. `WS_EX_TOOLWINDOW` at creation time, which would make this whole
/// function unnecessary) knows exactly what to re-verify rather than
/// assuming it always failed for reasons that still hold:
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
/// Runs on a **detached background thread**, not inline. An earlier round
/// of live testing produced a zero-screenshot, unresponsive-looking
/// `manifold-view-webview2.exe` right after adding this call and was first
/// read as `DeleteTab` hanging -- **re-tested since, with a hard-timeout-
/// wrapped synchronous call and diagnostic prints bracketing it, and it
/// completed normally in well under the 2s retry budget below**. The
/// original observation was actually this session's own well-documented
/// coprocess/test-harness flakiness (a disabled-feature control run
/// reproduced the identical symptom), not a real hang -- correcting the
/// record here since an earlier commit message overstated this as
/// "confirmed live." The real reason to background it: the retry loop
/// below is a synchronous sleep totalling up to 2s, and blocking
/// view-host startup for that long before the first frame is captured
/// would be a real, avoidable regression, hang or not. Kept backgrounded
/// on that basis, with the same best-effort shape as `favicon::register`.
/// `HWND` isn't `Send` (wraps a raw pointer Rust can't prove is
/// thread-safe, even though Win32 handles are designed to cross threads
/// freely) -- carried across as a plain `isize` and reconstructed on the
/// other side instead of fighting the type system.
pub fn hide_from_taskbar(hwnd: HWND) {
    let hwnd_value = hwnd.0 as isize;
    std::thread::spawn(move || unsafe {
        let hwnd = HWND(hwnd_value as *mut core::ffi::c_void);
        let _ =
            windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
        let Ok(taskbar_list) = CoCreateInstance::<_, ITaskbarList>(&CLSID_TASKBAR_LIST, None, CLSCTX_INPROC_SERVER)
        else {
            return;
        };
        let _ = taskbar_list.HrInit();
        // `ShowWindow` returning doesn't guarantee Explorer's shell hook has
        // already registered this window as a taskbar tab -- a `DeleteTab`
        // that races ahead of that registration deletes nothing, and the
        // entry can still appear once Explorer catches up. `DeleteTab` is
        // idempotent, so retrying a few times over a short window is cheap,
        // reliable insurance against that race rather than trusting a single
        // fire-and-forget call to always land after registration.
        for _ in 0..10 {
            let _ = taskbar_list.DeleteTab(hwnd);
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    });
}

/// Resizes the host window to match the requested content size.
///
/// Capture no longer depends on this (it reads the Composition visual,
/// which `composition::resize_root_visual` sizes directly), but WebView2
/// still lays out against its parent window: leaving the window at its
/// original fixed size while `SetBounds` grew past it is a mismatch with no
/// upside, so the window tracks `HostToView::Resize` too.
pub fn resize(hwnd: HWND, width: u32, height: u32) {
    unsafe {
        let _ = SetWindowPos(hwnd, None, 0, 0, width as i32, height as i32, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

/// Non-blocking drain of this thread's Win32 message queue -- WebView2's
/// own internal multi-process message routing (and the `DispatcherQueue`
/// Composition hosting runs on, see `main`) needs this pumped regularly;
/// `PM_REMOVE` (not a blocking `GetMessageW`)
/// keeps this composing with the tick loop's own fixed-interval sleep
/// rather than blocking the whole view-host on window messages.
pub fn pump_messages() {
    unsafe {
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
