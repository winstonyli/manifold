//! `gate kill-check`: the property the kill-on-close jobs exist for (`manifold_view_protocol::job`). For each view kind
//! (`VIEWS`, default `webview2`; CEF needs its host built) and each victim, it starts a headless instance with a live
//! tab, records the process tree, freezes everything but the victim (a live host exits when its socket drops and a live
//! engine child when its parent goes, which would hide a missing job), kills the victim and checks that nothing
//! recorded outlives it:
//!
//! - `manifold`: the instance is force-killed; its own job must take the host and the host's subprocesses
//!   (msedgewebview2, CEF renderer and GPU processes) with it. The host's own job cannot fail this case alone: manifold's
//!   job already ends a direct child, and Windows ends whatever sat in that child's tree.
//! - `host`: only the host is force-killed and manifold stays up; its subprocesses can only die through the host's own
//!   job (`Job::assign_self`).
//!
//! Process ids are reused, so a process is its id and its creation time: a child belongs to a parent only if it was
//! created no earlier, and a recorded process counts as alive only if the same id still has the same creation time.
//! A run of the former PowerShell script that walked parents by id alone swept unrelated processes into its tree and
//! killed them. Cleanup here touches only recorded processes named `manifold*` or `msedgewebview2`.

use std::io;
use std::time::Duration;

use manifold_hosts::CEF_RUNTIME_DIR;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_SUSPEND_RESUME, PROCESS_TERMINATE};

use crate::check::check;
use crate::instance::{poll, seed_settings, Instance, STEP_TIMEOUT};
use crate::probes::views;
use crate::procs::{creation_time, snapshot, Proc};
use crate::windows_main::Ctx;

/// How long the dead processes get to disappear after the kill.
const DEATH_WAIT: Duration = Duration::from_secs(15);

/// `root` and every process below it: a child is one whose parent id is a member's and which was created no earlier
/// than that member, so an id reused after its parent died does not join.
fn tree(root: &Proc, all: &[Proc]) -> Vec<Proc> {
    let mut members = vec![root.clone()];
    let mut next = 0;
    while next < members.len() {
        let parent = members[next].clone();
        for p in all {
            if p.ppid == parent.pid && p.created >= parent.created && !members.iter().any(|m| m.pid == p.pid) {
                members.push(p.clone());
            }
        }
        next += 1;
    }
    members
}

/// Whether the process `p` recorded earlier still runs in `now`: same id and same creation time.
fn alive(p: &Proc, now: &[Proc]) -> bool {
    now.iter().any(|q| q.pid == p.pid && q.created == p.created)
}

/// Whether cleanup may kill a leftover: only the app and the engine processes this stage starts.
fn ours(p: &Proc) -> bool {
    p.name.starts_with("manifold") || p.name.starts_with("msedgewebview2")
}

/// Force-kills `p` if it is still the process recorded (same creation time).
fn terminate(p: &Proc) {
    if creation_time(p.pid) != Some(p.created) {
        return;
    }
    // SAFETY: plain Win32 calls; the handle is closed before returning.
    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE, 0, p.pid);
        if !handle.is_null() {
            TerminateProcess(handle, 1);
            CloseHandle(handle);
        }
    }
}

#[link(name = "ntdll")]
extern "system" {
    fn NtSuspendProcess(process: HANDLE) -> i32;
}

/// Freezes `p` if it is still the process recorded (same creation time); a frozen process still dies with its job.
fn suspend(p: &Proc) {
    if creation_time(p.pid) != Some(p.created) {
        return;
    }
    // SAFETY: plain calls; the handle is closed before returning.
    unsafe {
        let handle = OpenProcess(PROCESS_SUSPEND_RESUME, 0, p.pid);
        if !handle.is_null() {
            NtSuspendProcess(handle);
            CloseHandle(handle);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Victim {
    Manifold,
    Host,
}

impl Victim {
    fn name(self) -> &'static str {
        match self {
            Victim::Manifold => "manifold",
            Victim::Host => "host",
        }
    }
}

/// The host (a direct child of the instance named `manifold-view-*`) once it has at least one subprocess.
fn host_with_engine(root: &Proc, all: &[Proc]) -> Option<Proc> {
    let host =
        all.iter().find(|p| p.ppid == root.pid && p.created >= root.created && p.name.starts_with("manifold-view-"))?;
    (tree(host, all).len() >= 2).then(|| host.clone())
}

fn case(ctx: &Ctx, kind: &str, victim: Victim) -> io::Result<()> {
    let label = format!("{kind}/{}", victim.name());
    let profile = ctx.work.join(format!("{kind}-{}", victim.name()));
    let more = if kind == "cef" {
        let dir = ctx.exe.with_file_name(CEF_RUNTIME_DIR).display().to_string().replace('\\', "/");
        format!(", \"cef_directory\": \"{dir}\"")
    } else {
        String::new()
    };
    seed_settings(&profile, kind, &more)?;
    let mut inst = Instance::launch(&ctx.exe, &profile, &ctx.work, &format!("{kind}-{}.log", victim.name()))?;
    for line in ["WAITTABS", "NEWTAB", "WAITTABS"] {
        inst.send(line)?;
    }
    let pid = inst.pid();
    let mut recorded = None;
    poll(&format!("{label}: a host with an engine process under instance {pid}"), STEP_TIMEOUT, || {
        let all = snapshot();
        let Some(root) = all.iter().find(|p| p.pid == pid) else { return false };
        let Some(host) = host_with_engine(root, &all) else { return false };
        recorded = Some((root.clone(), host, all));
        true
    })?;
    let (root, host, all) = recorded.expect("poll returned Ok only after recording");
    let doomed = match victim {
        Victim::Manifold => tree(&root, &all),
        Victim::Host => tree(&host, &all),
    };
    let mut names: Vec<&str> = doomed.iter().map(|p| p.name.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    println!("{label}: tree of {} processes before the kill: {}", doomed.len(), names.join(", "));
    // Freeze everything that is to die with the victim, so only the jobs can end it: a live host sees its socket to
    // manifold drop and exits, and a live Chromium or CEF child sees its parent's pipe close and exits.
    let victim_pid = if victim == Victim::Manifold { root.pid } else { host.pid };
    doomed.iter().filter(|p| p.pid != victim_pid).for_each(suspend);
    match victim {
        Victim::Manifold => inst.kill(),
        Victim::Host => terminate(&host),
    }
    let mut left = Vec::new();
    let gone = poll(&format!("{label}: the killed processes to end"), DEATH_WAIT, || {
        let now = snapshot();
        left = doomed.iter().filter(|p| alive(p, &now)).cloned().collect();
        left.is_empty()
    });
    check(&format!("{label}: no killed process or subprocess survived"), gone.is_ok());
    if gone.is_err() {
        let survivors: Vec<String> = left.iter().map(|p| format!("{} {}", p.pid, p.name)).collect();
        println!("{label}: survivors after {} s: {}", DEATH_WAIT.as_secs(), survivors.join(", "));
        left.iter().filter(|p| ours(p)).for_each(terminate);
    }
    Ok(())
}

pub(crate) fn kill_check(ctx: &mut Ctx) -> io::Result<()> {
    for kind in views() {
        for victim in [Victim::Manifold, Victim::Host] {
            if let Err(e) = case(ctx, &kind, victim) {
                check(&format!("{kind}/{}: {e}", victim.name()), false);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, ppid: u32, created: u64, name: &str) -> Proc {
        Proc { pid, ppid, created, name: name.into() }
    }

    #[test]
    fn a_tree_follows_children_created_no_earlier_than_their_parent() {
        let all =
            [p(1, 0, 10, "manifold.exe"), p(2, 1, 11, "manifold-view-cef.exe"), p(3, 2, 12, "manifold-view-cef.exe")];
        let pids: Vec<u32> = tree(&all[0], &all).iter().map(|m| m.pid).collect();
        assert_eq!(pids, [1, 2, 3]);
    }

    #[test]
    fn a_child_older_than_its_parent_is_an_id_reused_and_stays_out() {
        // pid 5 was a child of an earlier process that had pid 2; the parent id was reused later, by this host.
        let all = [p(1, 0, 10, "manifold.exe"), p(2, 1, 20, "manifold-view-cef.exe"), p(5, 2, 15, "vctip.exe")];
        let pids: Vec<u32> = tree(&all[0], &all).iter().map(|m| m.pid).collect();
        assert_eq!(pids, [1, 2]);
    }

    #[test]
    fn a_reused_id_is_not_the_recorded_process() {
        let recorded = p(7, 1, 100, "msedgewebview2.exe");
        assert!(alive(&recorded, &[p(7, 1, 100, "msedgewebview2.exe")]));
        assert!(!alive(&recorded, &[p(7, 3, 200, "notepad.exe")]));
        assert!(!alive(&recorded, &[]));
    }

    #[test]
    fn cleanup_touches_only_the_app_and_its_engines() {
        assert!(ours(&p(1, 0, 1, "manifold-view-webview2.exe")));
        assert!(ours(&p(1, 0, 1, "msedgewebview2.exe")));
        assert!(!ours(&p(1, 0, 1, "powershell.exe")));
        assert!(!ours(&p(1, 0, 1, "vctip.exe")));
    }

    #[test]
    fn this_test_process_is_in_a_live_snapshot() {
        let me = std::process::id();
        let all = snapshot();
        let mine = all.iter().find(|q| q.pid == me).expect("the running test is listed");
        assert!(alive(mine, &all));
    }
}
