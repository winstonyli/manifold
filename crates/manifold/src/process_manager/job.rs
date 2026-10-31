//! The Windows Job Object every view-host child is assigned to, so the hosts die with manifold.exe.

use super::*;

/// The Windows Job Object every view-host child (`spawn_tab`/
/// `spawn_webkit_tab`, both call `assign_to_view_job` right after
/// spawning) gets assigned to, configured with `JOB_OBJECT_LIMIT_KILL_ON_
/// JOB_CLOSE` (`manifold_view_protocol::job`) -- Windows itself then guarantees every process in the job
/// dies the instant this job's last handle closes, which happens
/// automatically when *this* manifold.exe process terminates for *any*
/// reason (a clean exit, a crash, a Task Manager "End Task," anything
/// short of the whole OS going down) since Windows closes all of a
/// terminated process's own handles as part of teardown. `None` when the job could not be created
/// (said once on stderr). A static is never dropped, so the handle stays open until the process ends.
///
/// Exists because engine-hosts becoming orphaned was already a known, if
/// only ever partially worked around, problem: `spawn_tab`'s own comment
/// above already notes "a prior run that crashed... can leave a stale
/// manifold-view-*.exe still holding tab_id's named pipe indefinitely," with
/// a fallback-socket-id workaround for *that* one symptom. Found live to
/// be worse than a stale pipe, though -- orphaned `manifold-view-
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
static VIEW_JOB: std::sync::OnceLock<Option<manifold_view_protocol::job::Job>> = std::sync::OnceLock::new();

/// Adds a just-spawned view-host child to `VIEW_JOB`, creating the job at the first call -- called right
/// after every successful `Command::spawn()` in this module. A job that failed to create or configure, or an
/// assignment that fails (e.g. the child already exited), is said on stderr and the spawn goes on: the worst case is
/// falling back to this app's prior behavior (no automatic cleanup), not a spawn failure over a crash-safety extra.
#[cfg(target_os = "windows")]
pub(super) fn assign_to_view_job(child: &Child) {
    if let Some(job) = VIEW_JOB.get_or_init(|| manifold_view_protocol::job::kill_on_close("engine job")) {
        job.assign_child(child);
    }
}
/// No kill-on-exit job off Windows: a view host sees the socket drop when manifold dies and shuts itself down
/// (`manifold_view_protocol::transport::spawn_host_reader`). Measured on Linux (WSL2, 2026-10-02): `kill -9` of manifold with a
/// live CEF tab left no CEF process after 1 s, so `PR_SET_PDEATHSIG` or a process group is not needed there. Said once
/// on stderr rather than silently, since other hosts (WebKit, through WSL) are not covered by that measurement.
#[cfg(not(target_os = "windows"))]
pub(super) fn assign_to_view_job(_child: &Child) {
    static WARNED: std::sync::Once = std::sync::Once::new();
    WARNED.call_once(|| {
        eprintln!("manifold: no kill-on-exit job on this platform; view hosts exit when their socket to manifold drops")
    });
}
