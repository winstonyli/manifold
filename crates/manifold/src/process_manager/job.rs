//! The Windows Job Object every view-host child is assigned to, so the hosts die with manifold.exe.

use super::*;

/// A Windows Job Object every view-host child (`spawn_tab`/
/// `spawn_webkit_tab`, both call `assign_to_view_job` right after
/// spawning) gets assigned to, configured with `JOB_OBJECT_LIMIT_KILL_ON_
/// JOB_CLOSE` -- Windows itself then guarantees every process in the job
/// dies the instant this job's last handle closes, which happens
/// automatically when *this* manifold.exe process terminates for *any*
/// reason (a clean exit, a crash, a Task Manager "End Task," anything
/// short of the whole OS going down) since Windows closes all of a
/// terminated process's own handles as part of teardown.
///
/// Exists because engine-hosts becoming orphaned was already a known, if
/// only ever partially worked around, problem: `spawn_tab`'s own comment
/// above already notes "a prior run that crashed... can leave a stale
/// view-host-*.exe still holding tab_id's named pipe indefinitely," with
/// a fallback-socket-id workaround for *that* one symptom. Found live to
/// be worse than a stale pipe, though -- orphaned `manifold-engine-
/// webview2.exe` processes accumulating across several crashed/force-
/// killed sessions were confirmed to silently starve *new* engine-hosts'
/// own frame capture (`Windows.Graphics.Capture`, which every WebView2
/// view-host's off-screen hosting window uses -- see that crate's own
/// `window.rs`), reproducing as tab content stuck on its loading spinner
/// forever, not a speed problem. Diagnosed by instrumenting the frame-
/// receive path directly: zero `FrameReady` messages arrived for over a
/// minute with stale processes present, then several hundred arrived
/// within seconds of killing them by hand.
///
/// A Job Object rather than a "scan for and kill anything that looks
/// orphaned" pass at startup on purpose -- the latter has no reliable way
/// to distinguish a genuine orphan from a second manifold.exe instance's
/// own perfectly healthy children by image name alone, and getting that
/// distinction wrong means one running copy of this app occasionally
/// kills another's tabs out from under it. A Job Object needs no such
/// heuristic: each manifold.exe process gets its own job, so there is
/// nothing to misidentify. It only prevents *future* orphaning, though --
/// it has no way to reach back and clean up engine-hosts already orphaned
/// by a session that ran before this existed; those still need a one-time
/// manual Task Manager cleanup.
#[cfg(target_os = "windows")]
struct ViewJobHandle(windows_sys::Win32::Foundation::HANDLE);
// Safety: a Win32 `HANDLE` is a plain kernel object reference, valid to
// use from any thread -- Rust's blanket `!Send`/`!Sync` on raw pointers is
// only being overly cautious here, not flagging a real hazard.
#[cfg(target_os = "windows")]
unsafe impl Send for ViewJobHandle {}
#[cfg(target_os = "windows")]
unsafe impl Sync for ViewJobHandle {}

#[cfg(target_os = "windows")]
static VIEW_JOB: std::sync::OnceLock<ViewJobHandle> = std::sync::OnceLock::new();

/// Creates (once) and returns the Job Object every view-host child gets
/// assigned to -- see `ViewJobHandle`'s own doc comment. A null handle
/// (creation failed) is a silent no-op everywhere it's used, and a job whose
/// kill-on-close setup failed is logged once and used anyway: this is a
/// crash-safety net, not a feature any spawn should fail over losing.
#[cfg(target_os = "windows")]
fn view_job() -> windows_sys::Win32::Foundation::HANDLE {
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::JobObjects::{
        JobObjectExtendedLimitInformation, SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    VIEW_JOB
        .get_or_init(|| unsafe {
            let job = windows_sys::Win32::System::JobObjects::CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if !job.is_null() {
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let set = SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if set == 0 {
                    // Engines then outlive a crashed manifold: say so once.
                    eprintln!("engine job: kill-on-close not set: {}", std::io::Error::last_os_error());
                }
            }
            ViewJobHandle(job as HANDLE)
        })
        .0
}

/// Adds a just-spawned view-host child to `view_job()` -- called right
/// after every successful `Command::spawn()` in this module. A no-op (not
/// an error) if the job itself failed to create/configure, or if this one
/// assignment fails (e.g. the child already exited) -- either way, the
/// worst case is falling back to this app's prior behavior (no automatic
/// cleanup), not a spawn failure over a crash-safety extra.
#[cfg(target_os = "windows")]
pub(super) fn assign_to_view_job(child: &Child) {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
    let job = view_job();
    if job.is_null() {
        return;
    }
    let process_handle = child.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE;
    unsafe {
        let _ = AssignProcessToJobObject(job, process_handle);
    }
}
/// No kill-on-exit job off Windows: an view-host sees the socket drop when manifold dies and shuts itself down
/// (`ipc_protocol::transport::spawn_host_reader`). Measured on Linux (WSL2, 2026-10-02): `kill -9` of manifold with a
/// live CEF tab left no CEF process after 1 s, so `PR_SET_PDEATHSIG` or a process group is not needed there. Said once
/// on stderr rather than silently, since other hosts (WebKit in WSL) are not covered by that measurement.
#[cfg(not(target_os = "windows"))]
pub(super) fn assign_to_view_job(_child: &Child) {
    static WARNED: std::sync::Once = std::sync::Once::new();
    WARNED.call_once(|| {
        eprintln!(
            "manifold: no kill-on-exit job on this platform; engine hosts exit when their socket to manifold drops"
        )
    });
}
