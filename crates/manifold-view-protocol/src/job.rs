//! A Windows Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`: when its last handle closes, Windows ends every
//! process in it, however the holder ended (a clean exit, a crash, a force-kill), since the OS closes a dying
//! process's handles. manifold and the gate put each child they spawn in one (`assign_child`), so a view host or a
//! gate instance dies with its parent; the CEF and WebView2 hosts put themselves in one (`assign_self`), so the
//! browser processes their engine starts, which join the job as they are created, die with the host. `assign_self`
//! keeps its job open until the process ends, so the host's own exit code survives.

use std::os::windows::io::AsRawHandle;
use std::process::Child;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

/// An open job handle. Dropping it closes the handle; if it was the job's last, every process still in the job ends.
/// `label` starts every line it writes on stderr ("engine job", "gate job", "view host").
pub struct Job {
    handle: HANDLE,
    label: String,
}

// Safety: a job `HANDLE` is a kernel object reference, valid from any thread, and `Job` only passes it to Win32
// calls that are safe to make from any thread.
unsafe impl Send for Job {}
unsafe impl Sync for Job {}

/// Creates a job whose processes die when its last handle closes. `None` (said on stderr) when the job can't be
/// created. A job whose kill-on-close limit can't be set is said once and returned anyway: this is a crash-safety
/// net, not something a spawn should fail over.
pub fn kill_on_close(label: &str) -> Option<Job> {
    // SAFETY: null attributes and a null name ask for an unnamed job with default security.
    let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if handle.is_null() {
        eprintln!("{label}: not created: {}", std::io::Error::last_os_error());
        return None;
    }
    let job = Job { handle, label: label.to_owned() };
    // SAFETY: the struct is plain integers; all zeros is "no limits".
    let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    // SAFETY: `job.handle` is a live job handle, and `info` is the struct this information class names, with its size.
    let set = unsafe {
        SetInformationJobObject(
            job.handle,
            JobObjectExtendedLimitInformation,
            std::ptr::from_ref(&info).cast(),
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    };
    if set == 0 {
        // The job's processes then outlive it: say so once.
        eprintln!("{label}: kill-on-close not set: {}", std::io::Error::last_os_error());
    }
    Some(job)
}

impl Job {
    /// Puts a just-spawned child in the job. A failure (say the child already exited) is said on stderr and nothing
    /// else changes: that child just won't die with the job.
    pub fn assign_child(&self, child: &Child) {
        let who = format!("child {}", child.id());
        self.assign(child.as_raw_handle() as HANDLE, &who);
    }

    /// Puts this process in the job, and with it every process it starts from then on, and keeps the job open until
    /// this process ends: the OS closes the handle then, which ends whatever is still in the job. The handle is leaked
    /// on purpose: dropping it earlier (at the end of `main`, or while a panic unwinds) would end this process at once
    /// with exit code 0 and lose how it really ended. A failure is said on stderr.
    pub fn assign_self(self) {
        // SAFETY: returns the current process's pseudo-handle, which needs no closing.
        let process = unsafe { GetCurrentProcess() };
        self.assign(process, "this process");
        std::mem::forget(self);
    }

    fn assign(&self, process: HANDLE, who: &str) {
        // SAFETY: both handles are live for the duration of the call.
        if unsafe { AssignProcessToJobObject(self.handle, process) } == 0 {
            eprintln!("{}: {who} not assigned: {}", self.label, std::io::Error::last_os_error());
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: `handle` is this job's own open handle, closed only here.
        unsafe { CloseHandle(self.handle) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    #[test]
    fn a_job_is_created() {
        assert!(kill_on_close("test job").is_some());
    }

    /// The child half of `a_job_holder_keeps_its_exit_code`: only when the parent asks for it, put this process in a
    /// job and fail, as a host does when it panics. Run on its own it does nothing.
    #[test]
    fn panic_in_a_job_holder() {
        if std::env::var_os("JOB_TEST_PANIC").is_none() {
            return;
        }
        let job = kill_on_close("test job").expect("job created");
        job.assign_self();
        panic!("a host panics");
    }

    /// A host that holds its own job must still report how it ended: a panic is exit code 101, not the 0 that
    /// Windows gives a process its job's closing ended.
    #[test]
    fn a_job_holder_keeps_its_exit_code() {
        let status = Command::new(std::env::current_exe().expect("test exe"))
            .args(["--exact", "job::tests::panic_in_a_job_holder", "--nocapture"])
            .env("JOB_TEST_PANIC", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("run the test exe");
        assert_eq!(status.code(), Some(101), "the panic's exit code was lost");
    }

    /// The property the job exists for. No test calls `assign_self`: it would put the test runner in the job.
    #[test]
    fn dropping_the_job_ends_its_children() {
        let job = kill_on_close("test job").expect("job created");
        let mut child = Command::new("ping")
            .args(["-n", "60", "127.0.0.1"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn ping");
        job.assign_child(&child);
        drop(job);
        let deadline = Instant::now() + Duration::from_secs(10);
        let exited = loop {
            if let Some(status) = child.try_wait().expect("try_wait") {
                break Some(status);
            }
            if Instant::now() >= deadline {
                break None;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        if exited.is_none() {
            // Leave no `ping` behind on a failure.
            let _ = child.kill();
            let _ = child.wait();
        }
        assert!(exited.is_some(), "the child outlived its job");
    }
}
