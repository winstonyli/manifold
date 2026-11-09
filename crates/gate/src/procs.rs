//! Process queries the gate's stages share: a snapshot of this user's processes, each with its creation time (an id
//! alone is reused), and a process's CPU time.

use std::mem::{size_of, zeroed};
use std::time::Duration;

use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

/// A process as one snapshot saw it. `created` (FILETIME ticks) tells it from a later process that reused its id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Proc {
    pub(crate) pid: u32,
    pub(crate) ppid: u32,
    pub(crate) created: u64,
    /// The image name in lower case, as `manifold-view-cef.exe`.
    pub(crate) name: String,
}

/// The creation time and the CPU time (kernel plus user) of `pid`, in FILETIME ticks of 100 ns.
fn process_times(pid: u32) -> Option<(u64, u64)> {
    let ticks = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
    // SAFETY: plain Win32 calls; the handle is closed before returning and the out parameters are locals.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let (mut created, mut exited, mut kernel, mut user): (FILETIME, FILETIME, FILETIME, FILETIME) = zeroed();
        let ok = GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user);
        CloseHandle(handle);
        (ok != 0).then(|| (ticks(created), ticks(kernel) + ticks(user)))
    }
}

pub(crate) fn creation_time(pid: u32) -> Option<u64> {
    process_times(pid).map(|(created, _)| created)
}

/// The CPU time `pid` has used so far, kernel plus user.
pub(crate) fn cpu_time(pid: u32) -> Option<Duration> {
    process_times(pid).map(|(_, cpu)| Duration::from_nanos(cpu * 100))
}

/// Every process of this user the snapshot lists and whose creation time can be read (the others cannot be ours).
pub(crate) fn snapshot() -> Vec<Proc> {
    let mut out = Vec::new();
    // SAFETY: the snapshot handle is closed before returning; `entry` is a local with `dwSize` set as the API needs.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return out;
        }
        let mut entry: PROCESSENTRY32W = zeroed();
        entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut more = Process32FirstW(snap, &mut entry);
        while more != 0 {
            if let Some(created) = creation_time(entry.th32ProcessID) {
                let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
                let name = String::from_utf16_lossy(&entry.szExeFile[..len]).to_lowercase();
                out.push(Proc { pid: entry.th32ProcessID, ppid: entry.th32ParentProcessID, created, name });
            }
            more = Process32NextW(snap, &mut entry);
        }
        CloseHandle(snap);
    }
    out
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    #[test]
    fn cpu_time_of_this_process_rises() {
        let me = std::process::id();
        let before = cpu_time(me).expect("this process's CPU time");
        let start = Instant::now();
        let mut x = 0u64;
        while start.elapsed() < Duration::from_millis(200) {
            x = std::hint::black_box(x.wrapping_mul(31).wrapping_add(7));
        }
        let after = cpu_time(me).expect("this process's CPU time");
        assert!(after >= before + Duration::from_millis(15), "{before:?} -> {after:?}");
    }
}
