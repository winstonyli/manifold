//! `WakeEvent`: what a host's loop waits on between turns, instead of a clock. One wait ends on whichever comes first:
//! a wake (`set`, from the reader thread after each message, or from an engine callback), a window message for this
//! thread, or the deadline. Each part is there for a reason:
//!
//! - No dispatch before waiting: a message the caller's last turn did not see would be handled with nobody looking at
//!   its result, and the wait would then block on an empty queue. `MWMO_INPUTAVAILABLE` returns at once for anything
//!   already queued, and the dispatch after the wait runs it.
//! - The dispatch after waking runs the engine's callbacks (WGC `FrameArrived`, WebView2's events) before the caller
//!   looks at their results. WebView2's message routing and its `DispatcherQueue` need the thread's messages
//!   dispatched too.
//! - The deadline is a high-resolution waitable timer in the handle list, because the wait's own timeout argument
//!   rounds up to the 15.6 ms system tick (run `2026-11-09-timer-quantization`).
//! - `WAIT_BACKSTOP` turns a forgotten deadline into at most a second of latency.

use std::io;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_FAILED};
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateWaitableTimerExW, SetEvent, SetWaitableTimer, CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, INFINITE,
    TIMER_ALL_ACCESS,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MsgWaitForMultipleObjectsEx, PeekMessageW, TranslateMessage, MSG, MWMO_INPUTAVAILABLE, PM_REMOVE,
    QS_ALLINPUT,
};

/// The longest one `wait` blocks, whatever its deadline.
const WAIT_BACKSTOP: Duration = Duration::from_secs(1);

/// A kernel handle, closed on drop.
struct Owned(HANDLE);

impl Owned {
    fn new(handle: HANDLE) -> io::Result<Self> {
        if handle.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(handle))
        }
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: the handle is live and owned here alone.
        unsafe { CloseHandle(self.0) };
    }
}

/// An auto-reset wake event and a high-resolution timer for the deadline. Hosts share it as `Arc<WakeEvent>`.
pub struct WakeEvent {
    event: Owned,
    timer: Owned,
}

// SAFETY: kernel event and timer handles may be used from any thread, and `WakeEvent` only passes them to Win32 calls
// that are safe from any thread.
unsafe impl Send for WakeEvent {}
unsafe impl Sync for WakeEvent {}

impl WakeEvent {
    pub fn new() -> io::Result<Self> {
        // SAFETY: null attributes and a null name ask for an unnamed object with default security; auto-reset,
        // initially unsignaled.
        let event = Owned::new(unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) })?;
        // SAFETY: as above; a synchronization (auto-reset) timer with high resolution.
        let timer = Owned::new(unsafe {
            CreateWaitableTimerExW(
                std::ptr::null(),
                std::ptr::null(),
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                TIMER_ALL_ACCESS,
            )
        })?;
        Ok(Self { event, timer })
    }

    /// Ends the current or next `wait`. Any thread; wakes before a wait are kept as one.
    pub fn set(&self) {
        // SAFETY: the event handle is live while `self` is.
        unsafe { SetEvent(self.event.0) };
    }

    /// Blocks until a wake, a window message for this thread, or `deadline` (at most `WAIT_BACKSTOP` from now), then
    /// dispatches every queued message of this thread. Call it only on the thread whose window messages it should
    /// dispatch: the host's loop thread.
    pub fn wait(&self, deadline: Instant) {
        let now = Instant::now();
        let left = deadline.min(now + WAIT_BACKSTOP).saturating_duration_since(now);
        if !left.is_zero() {
            // Negative is relative, in 100 ns units, rounded up so the wait never ends early. Arming also resets a
            // signal an earlier arm left behind.
            let due = -(left.as_nanos().div_ceil(100) as i64);
            // SAFETY: the timer handle is live; no period and no completion routine.
            let armed = unsafe { SetWaitableTimer(self.timer.0, &due, 0, None, std::ptr::null(), 0) };
            if armed == 0 {
                panic!("WakeEvent: arming the timer failed: {}", io::Error::last_os_error());
            }
            let handles = [self.event.0, self.timer.0];
            // SAFETY: both handles are live for the call.
            let woke =
                unsafe { MsgWaitForMultipleObjectsEx(2, handles.as_ptr(), INFINITE, QS_ALLINPUT, MWMO_INPUTAVAILABLE) };
            if woke == WAIT_FAILED {
                panic!("WakeEvent: the wait failed: {}", io::Error::last_os_error());
            }
        }
        dispatch_queued();
    }
}

/// Runs every message queued for this thread.
fn dispatch_queued() {
    // SAFETY: `MSG` is plain data; all zeros is a valid buffer for `PeekMessageW` to fill.
    let mut msg: MSG = unsafe { std::mem::zeroed() };
    // SAFETY: a null hwnd takes every message of this thread; `msg` is filled before it is translated or dispatched.
    while unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) } != 0 {
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, PeekMessageW, PostMessageW, PostThreadMessageW, RegisterClassW, HWND_MESSAGE,
        MSG, PM_NOREMOVE, WM_USER, WNDCLASSW,
    };

    use super::{WakeEvent, WAIT_BACKSTOP};

    /// Whether this thread's queue holds a message, left in place.
    fn queued() -> bool {
        // SAFETY: `MSG` is plain data, and a null hwnd peeks every message of this thread.
        let mut msg: MSG = unsafe { std::mem::zeroed() };
        unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_NOREMOVE) != 0 }
    }

    #[test]
    fn a_wake_from_another_thread_ends_the_wait() {
        let ev = Arc::new(WakeEvent::new().unwrap());
        let e = ev.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            e.set();
        });
        let start = Instant::now();
        ev.wait(start + Duration::from_secs(5));
        assert!(start.elapsed() < Duration::from_millis(500));
        t.join().unwrap();
    }

    #[test]
    fn many_wakes_before_the_wait_end_one_wait() {
        let ev = WakeEvent::new().unwrap();
        for _ in 0..100 {
            ev.set();
        }
        let start = Instant::now();
        ev.wait(start + Duration::from_secs(5)); // the wakes are kept
        assert!(start.elapsed() < Duration::from_millis(500));
        let start = Instant::now();
        ev.wait(start + Duration::from_millis(50));
        assert!(start.elapsed() >= Duration::from_millis(40));
    }

    #[test]
    fn the_deadline_is_not_rounded_to_the_system_tick() {
        let ev = WakeEvent::new().unwrap();
        // Best of 5, so load cannot fail it; the tick would give at least ~15 ms every time. A regression guard only
        // while no process has raised the system timer rate (timeBeginPeriod); `gate hostrate` is the end-to-end check.
        let best = (0..5)
            .map(|_| {
                let s = Instant::now();
                ev.wait(s + Duration::from_millis(4));
                s.elapsed()
            })
            .min()
            .unwrap();
        assert!(best >= Duration::from_micros(3500) && best < Duration::from_millis(10), "{best:?}");
    }

    #[test]
    fn a_stale_timer_does_not_end_the_next_wait() {
        let ev = WakeEvent::new().unwrap();
        ev.set();
        ev.wait(Instant::now() + Duration::from_millis(5)); // ends on the event; the timer stays armed
        std::thread::sleep(Duration::from_millis(20)); // and fires with nobody waiting
        let start = Instant::now();
        ev.wait(start + Duration::from_millis(50));
        assert!(start.elapsed() >= Duration::from_millis(40), "{:?}", start.elapsed());
    }

    #[test]
    fn a_past_deadline_returns_at_once() {
        let ev = WakeEvent::new().unwrap();
        let start = Instant::now();
        ev.wait(start - Duration::from_millis(1)); // a no-panic guard (deadline in the past) more than a timing one
        assert!(start.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn a_far_deadline_waits_only_the_backstop() {
        let ev = WakeEvent::new().unwrap();
        let start = Instant::now();
        ev.wait(start + Duration::from_secs(60));
        let took = start.elapsed();
        assert!(
            took >= WAIT_BACKSTOP - Duration::from_millis(5) && took < WAIT_BACKSTOP + Duration::from_millis(500),
            "{took:?}"
        );
    }

    static POSTED_SEEN: AtomicBool = AtomicBool::new(false);

    unsafe extern "system" fn note_wm_user(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if msg == WM_USER {
            POSTED_SEEN.store(true, Ordering::SeqCst);
            return 0;
        }
        // SAFETY: the arguments are the ones Windows passed this window procedure.
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    #[test]
    fn a_posted_message_ends_the_wait_and_is_dispatched() {
        // A window message, not a thread message: `DispatchMessageW` calls no window procedure for a thread message,
        // so a drain that skipped dispatching would pass with one.
        let class: Vec<u16> = "manifold-wake-test\0".encode_utf16().collect();
        // SAFETY: a null name asks for this executable's module handle.
        let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
        // SAFETY: `WNDCLASSW` is plain data; all zeros is no icon, cursor, brush or menu.
        let mut wc: WNDCLASSW = unsafe { std::mem::zeroed() };
        wc.lpfnWndProc = Some(note_wm_user);
        wc.hInstance = instance;
        wc.lpszClassName = class.as_ptr();
        // SAFETY: `wc` and the class name it points at outlive the call.
        assert_ne!(unsafe { RegisterClassW(&wc) }, 0, "{}", std::io::Error::last_os_error());
        // SAFETY: the class was registered above; `HWND_MESSAGE` makes a message-only window with no size or menu.
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                std::ptr::null(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            )
        };
        assert!(!hwnd.is_null(), "{}", std::io::Error::last_os_error());
        let ev = WakeEvent::new().unwrap();
        let target = hwnd as isize; // an `HWND` is not `Send`
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            // SAFETY: the window lives until the test ends, after this thread is joined.
            assert_ne!(unsafe { PostMessageW(target as HWND, WM_USER, 0, 0) }, 0);
        });
        let start = Instant::now();
        ev.wait(start + Duration::from_secs(5));
        assert!(start.elapsed() < Duration::from_millis(500), "{:?}", start.elapsed());
        t.join().unwrap();
        assert!(POSTED_SEEN.load(Ordering::SeqCst), "the window procedure did not see WM_USER");
        assert!(!queued(), "the wait left a message queued");
    }

    #[test]
    fn a_message_queued_before_the_wait_ends_it() {
        queued(); // creates this thread's message queue
                  // SAFETY: posts a message with no pointers in it to this thread's own queue.
        assert_ne!(unsafe { PostThreadMessageW(GetCurrentThreadId(), WM_USER, 0, 0) }, 0);
        // Seen but left queued, as a nested pump inside a turn would leave it.
        // SAFETY: as in `queued`.
        let mut msg: MSG = unsafe { std::mem::zeroed() };
        assert_ne!(unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), WM_USER, WM_USER, PM_NOREMOVE) }, 0);
        let ev = WakeEvent::new().unwrap();
        let start = Instant::now();
        ev.wait(start + Duration::from_secs(5));
        assert!(start.elapsed() < Duration::from_millis(500), "{:?}", start.elapsed());
        assert!(!queued(), "the wait left a message queued");
    }
}
