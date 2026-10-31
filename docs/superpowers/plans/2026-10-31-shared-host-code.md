---
status: active
---

# Shared Host Code Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The four kill-on-close job objects (cef host, webview2 host, manifold, gate) become one `manifold_view_protocol::job` module, and the three `NavState` copies with their flush loops and the two shared-cell aliases (cef, webview2, servo) become one `manifold_view_protocol::nav_state` module (cleanup items CODE5 and CODE6).

**Architecture:**
- `manifold_view_protocol::job` (`#[cfg(windows)]`, `windows-sys` 0.59, target-gated so the protocol crate still builds on Linux for the WebKitGTK host): `kill_on_close(label) -> Option<Job>`, `Job::assign_child(&Child)`, `Job::assign_self()`, `Drop` closes the handle. Every failure is said on stderr with the label and the job is still used where it exists.
- manifold and gate keep their thin wrappers (`assign_to_view_job`, `job::assign`) over a `static OnceLock<Option<Job>>`; the two hosts call `job::kill_on_close("view host").inspect(Job::assign_self)` once in `main`.
- `manifold_view_protocol::nav_state` (std only): one `NavState` with `new(url, dirty)` and `take_message() -> Option<ViewToHost>`, plus `SharedNavState`, `SharedJsonBody`, `SharedRedirectContentType` and their constructors. The three flush loops become `if let Some(m) = nav.borrow_mut().take_message() { let _ = control.send(&m); }`.
- `had_recent_input` moves from webview2's `NavState` to `NavLabel::had_recent_input` beside `GESTURE_WINDOW`.
- Then `Cargo.toml` features only the deleted copies used are dropped, each re-grepped first.

**Tech Stack:** Rust (nightly-2026-09-23 per `rust-toolchain.toml`, edition 2021), `windows-sys` 0.59 (workspace), `windows` 0.62 (webview2, cef), cef-rs, webview2-com, Servo. The gate harness (`target/debug/gate.exe tier`, `all`, `sync-runtime-deps`), `~/.claude/runs/run.ps1`.

**Spec:** `docs/superpowers/specs/2026-10-31-shared-host-code-design.md` (status draft, reviewed; decisions f1, a1 "d2", e1 by the user). The cleanup items are CODE5 and CODE6 in `docs/superpowers/plans/2026-10-30-cleanup.md`.

**Base commit:** 7e6010d5 ("cleanup plan: mark GATE1/2/4/15 and CODE5/6 own plan, ..."). Every line number below was read at 7e6010d5. Another session may commit in this tree (the gate stage-skeleton spec of the same day also edits `crates/gate/src/instance.rs`), so find each edit by the code quoted with it, not by its line number. Task 1 Step 1 records the commit the work actually starts from, as B.

## Global Constraints

- **Scope.** Change nothing beyond what a task says; report every deviation in the task's commit message and to the reviewer.
- **Python.** Never run `python` reading stdin (`python -`, a heredoc into python). If a script is needed, write it to a file and run `python -P f.py < /dev/null`. No step here needs python.
- **Edits** with the Edit tool. Line endings are LF (`.gitattributes`).
- **Format and lints before every commit:** `cargo fmt --all`, then the repo clippy line, which must pass:
  `cargo clippy -j 8 -p manifold -p manifold-hosts -p manifold-view-protocol -p manifold-pages -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings`.
  The pre-commit hook (`.githooks/pre-commit`) checks both again. Never `--no-verify`. Don't silence a lint; fix it, or `#[allow(..., reason = "...")]` it saying why.
- **Edit loop:** the quickest targeted check (`cargo test -j 8 -p <crate> <filter>`); the repo clippy line once per task, before its commit; the gate once, at the end (Task 8), never per edit.
- **Tests never touch the real `%APPDATA%\manifold`.** None of the new tests reads or writes a file; the Task 8 kill check uses `--profile-dir` under its run folder.
- **Commits:** small, one per task, after `git diff --stat` shows only the task's files. `Cargo.lock` goes in the commit of any task that changes a dependency (CI builds with `--locked`). Every commit message ends with the line
  `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`.
- **Expected `git diff --stat` in Tasks 1-7** lists the task's files plus, ignored in every such check, `docs/runs.md` (`run.ps1` edits it at each recorded run; it is committed in Task 8 Step 8, never in a task commit). Stage task files by name, never `git add -A`.
- **Never push, tag or bump.**
- **One cargo per target dir.** If cargo says "Blocking waiting for file lock", another session is building: wait, don't start a second build.
- **Disk (precondition).** C: had only about 3.4 GB free on 2026-10-31, under the 5 GB threshold. Run `df -h /c` before starting and again at the start of Tasks 5, 6, 7 and 8 (the CEF and Servo steps are the heavy ones; Tasks 7 and 8 rebuild CEF and the gate's binaries). If under ~5 GB free, the executor stops and reports to the user; it deletes nothing (no `target*` directories, no caches) without asking. Tasks 1-4 are small incremental builds and may proceed, but check `df` before each.
- **What builds here** (Windows, this machine):
  - Builds and tests normally: `manifold-view-protocol`, `manifold`, `gate`, `manifold-view-webview2`.
  - `manifold-view-cef`: builds here (its CEF download is already in `target/debug/build/cef-dll-sys`, and `target/debug/manifold-view-cef.exe` was built 2026-10-30); it is outside the repo clippy line, so Task 5 runs clippy on it separately. Its Linux build is not checked here (verified by reading: the job use stays behind `#[cfg(windows)]`).
  - `manifold-view-servo`: only in its own target dir, `CARGO_TARGET_DIR=target-servo` (README "Building Servo on its own": about 33 min cold). Task 6 builds it incrementally; if that turns into a cold build, it may be stopped and the crate marked verified by reading.
  - `manifold-view-webkitgtk`: not touched.
  - The protocol crate on Linux: checked once through WSL in Task 2 (CI's Linux job runs `cargo test -p manifold-view-protocol`).
- **Long builds** run at BelowNormal with `-j 8` (LONG_RUNS.md); the gate run goes through `~/.claude/runs/run.ps1 -Kind gate -Detach` (RUNS.md).

## Review Focus

Choices this plan makes that the spec does not spell out. Each is marked **(P#)** where it is applied; check them before approving.

- **P1. `Job` carries its label.** The spec shows `pub struct Job(HANDLE)` but also requires the assign failure to name the label (and, for gate, the pid). So `Job` is `{ handle: HANDLE, label: String }`. Messages: `{label}: not created: {err}`, `{label}: kill-on-close not set: {err}`, `{label}: child {pid} not assigned: {err}`, `{label}: this process not assigned: {err}`. Gate's old wording was "instance {pid} not assigned"; nothing greps it (checked: only `instance.rs` and old plans mention it).
- **P2. A host ends itself when `_job` drops.** A host is inside its own job (`assign_self`), so closing the job's last handle at the end of `main` terminates the host there and then, with its subprocesses. Consequences, checked against the code:
  - Everything `main` does before returning still runs (CEF's `cef::shutdown()`, WebView2's `controller.Close()`); only drops of locals declared before `_job` are skipped, and the process is exiting anyway.
  - The host's exit code is then the job kill's, not `main`'s. manifold reads a host's exit status only in `connect_host` (`process_manager.rs`, the "view-host exited {how} before connecting" message), before the host connects. Before connecting, the hosts leave through `process::exit` (skips drops, unchanged) or a panic. A panic unwinds through `main`, so `_job` drops and the code in "view-host exited with code N before connecting" is no longer 101; the panic message is already in the log tail (the hook prints before unwinding), so the reason is still shown.
  - stdout is line-buffered, and the hosts log with `eprintln!` (unbuffered), so no complete line is lost.
  - It is the same kill the OS did at process exit before: the host assigned itself to the job first, so msedgewebview2 and the CEF children were already in it and died when the handle closed. Only the timing within `main`'s tail changes (the handle closes when `main` returns, not at the OS's teardown a moment later).
  - Verification: the host tail is checked by reading (Task 4 Step 4, Task 5 Step 4): `controller.Close()` (WebView2) and `cef::shutdown()` (CEF) run before `main` returns, and the job local is declared first so it drops last. No gate stage tests that engine profile writes survive the kill (`gate jar` runs InPrivate tabs, and its sign-in reaches the jar file through the app's own pull, not the engine profile), so the residual risk, that a profile write still in flight at exit is cut short, is not tested and is the owner's to accept.
  If any of this is unwanted, the fallback is `std::mem::forget(job)` on the hosts' `Job` (the old behaviour: the OS closes the handle at exit); that is a spec change for the owner.
- **P3. Where `_job` is declared.** The spec asks that `_job` be the last local dropped in `main`. WebView2: `_job` moves to the first statement of `main` (it is now declared after `_dispatcher_queue_controller`), so it is literally last. CEF: it cannot move above the subprocess branch (renderer subprocesses would each make a job of their own), so the six locals declared before it (`args`, `cmd`, `type_switch`, `cef_app`, `ret`, `is_browser_process`) are never dropped; all of CEF's real cleanup (`cef::shutdown()`) runs before the `return`.
- **P4. gate gains a dependency on `manifold-view-protocol`.** gate depended only on `manifold-hosts`; the shared job needs the protocol crate (and with it `interprocess`, `memmap2`, `postcard`, `manifold-pages`, all already built by the gate's build line for manifold).
- **P5. Features dropped beyond the spec's list, each because its only user goes:** `Win32_Security` from gate's `windows-sys` (its remaining Security use, `jar.rs` DPAPI, comes from `Win32_Security_Cryptography`, which enables `Win32_Security` itself: windows-sys 0.59 `Cargo.toml:148` `Win32_Security_Cryptography = ["Win32_Security"]`) and from webview2's `windows` (no `Security` use in `src/`, checked; if `cargo check` disagrees, the line is restored and the commit says so).
- **P6. webview2's module keeps its name.** `crates/manifold-view-webview2/src/nav_state.rs` keeps the event handlers and keeps its file name (the spec lists it under that name); the struct and aliases it held now come from `manifold_view_protocol::nav_state`. No name clash: main.rs imports items, not the protocol module.
- **P7. Doc comments that lose their home.** The engine-specific notes on `label` (CEF's `user_gesture` only for a new document; WebView2's untrustworthy `IsUserInitiated`, runtime tested 2026-09-26) move into the shared field's doc; the cef `client.rs` notes on the two aliases become one engine-neutral doc each.
- **P8. Out of scope stays unparked.** The spec's "Out of scope" (WebKitGTK's tuple comparison, the cef-only aliases, other duplicated helpers) is left alone by decision; Task 8 adds no PARKED entry for it.

## Answers to the spec's open questions

Both read at 7e6010d5:
- **Job setup call sites.** Each host calls it exactly once, in `main` (cef `main.rs`, webview2 `main.rs`; `git grep -n setup_kill_on_exit_job -- crates/`), never on another thread or before `main`'s own setup. The return value is only bound to `_job` and never used, so the move from `windows::core::Result<HANDLE>` to `Option<Job>` touches no other code.
- **`windows` uses the grep could miss.** cef has a `build.rs` and `src/input.rs`; neither uses the `windows` crate (`build.rs` has a comment saying "Windows"; `input.rs` has only the identifiers `windows_key_code` and `windows_vk`). So the cef `windows` dependency can go in Task 7. Task 5 Step 7 and Task 7 Step 1 grep both anyway.

## File map

| File | Change | Task |
|---|---|---|
| `crates/manifold-view-protocol/Cargo.toml` | target-gated `windows-sys` | 1 |
| `crates/manifold-view-protocol/src/lib.rs` | `#[cfg(windows)] pub mod job;`, `pub mod nav_state;` | 1, 2 |
| `crates/manifold-view-protocol/src/job.rs` | new: `Job`, `kill_on_close`, tests | 1 |
| `crates/manifold-view-protocol/src/nav_state.rs` | new: `NavState`, `take_message`, aliases, tests | 2 |
| `crates/manifold-view-protocol/src/nav_label.rs` | `had_recent_input`, test, module doc | 2 |
| `crates/manifold/src/process_manager/job.rs` | onto `job::kill_on_close` | 3 |
| `crates/gate/Cargo.toml`, `crates/gate/src/instance.rs` | protocol dependency; `mod job` onto the shared job | 3 |
| `crates/manifold-view-webview2/src/{main.rs,nav_state.rs,popup.rs}` | shared job and `NavState` | 4 |
| `crates/manifold-view-cef/src/{main.rs,client.rs,nav_handler.rs,life_span_handler.rs}` | shared job and `NavState` | 5 |
| `crates/manifold-view-servo/src/main.rs` | shared `NavState` | 6 |
| `crates/{manifold,gate,manifold-view-webview2,manifold-view-cef}/Cargo.toml` | drop job-only features / the cef `windows` dependency | 7 |
| `docs/superpowers/plans/2026-10-30-cleanup.md`, the spec, this plan | outcomes, status | 8 |

---

### Task 1: `manifold_view_protocol::job` (TDD)

**Files:**
- Modify: `crates/manifold-view-protocol/Cargo.toml`
- Modify: `crates/manifold-view-protocol/src/lib.rs`
- Create: `crates/manifold-view-protocol/src/job.rs`
- Modify: `Cargo.lock` (by cargo)

**Interfaces:**
- Consumes: `windows_sys::Win32::Foundation::{CloseHandle, HANDLE}`; `windows_sys::Win32::System::JobObjects::{AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE}`; `windows_sys::Win32::System::Threading::GetCurrentProcess` (signatures in windows-sys 0.59: `CreateJobObjectW(*const SECURITY_ATTRIBUTES, PCWSTR) -> HANDLE`, `SetInformationJobObject(HANDLE, JOBOBJECTINFOCLASS, *const c_void, u32) -> BOOL`, `AssignProcessToJobObject(HANDLE, HANDLE) -> BOOL`, `GetCurrentProcess() -> HANDLE`, `CloseHandle(HANDLE) -> BOOL`).
- Produces (all `#[cfg(windows)]`):
  ```rust
  pub struct Job { /* handle: HANDLE, label: String */ }   // Send + Sync; Drop closes the handle
  pub fn kill_on_close(label: &str) -> Option<Job>;
  impl Job {
      pub fn assign_child(&self, child: &std::process::Child);
      pub fn assign_self(&self);
  }
  ```

- [ ] **Step 1: Record the base.**

Run: `git -C "C:/Users/Winston Li/Documents/GitHub/manifold" status --short; git -C "C:/Users/Winston Li/Documents/GitHub/manifold" rev-parse --short HEAD`
First action: if any of these is untracked or modified, commit them as they stand, docs only: `docs/superpowers/plans/2026-10-31-shared-host-code.md` and `docs/superpowers/plans/2026-10-31-gate-stage-skeleton.md` (both `??` at 7e6010d5) and `docs/superpowers/specs/2026-10-31-gate-stage-skeleton-design.md` (` M` at 7e6010d5). `git add` them by name and commit "docs: shared host code and gate stage skeleton plans, skeleton spec as drafted" (Co-Authored-By line as in Global Constraints). Add no other file.
Then re-run the status line. Expected: a clean tree, no modified or staged tracked files, no untracked files you did not just commit. If anything else is modified or staged, stop and report what is uncommitted. Then take the short hash as B (the base of the work: the commit before any status flip or code). Write it into this plan's header under **Base commit** as `Started at <hash> (<date>).`, and set `status: active` in the front matter of BOTH this plan and the spec `docs/superpowers/specs/2026-10-31-shared-host-code-design.md` (Task 8 Step 6 sets both to `done`). Commit those two edits on their own ("docs: shared host code plan and spec active"; `git add` the two files by name), so `git log <B>..HEAD` includes it. Then require a clean tree again before Step 2.

- [ ] **Step 2: Add the dependency.**

In `crates/manifold-view-protocol/Cargo.toml`, after the `[dependencies]` block (after the line `manifold-pages.workspace = true`) and before `[[bin]]`, insert:

```toml

# The kill-on-close job (`job.rs`). Windows only: this crate is also built on Linux for the WebKitGTK host.
# `JOBOBJECT_EXTENDED_LIMIT_INFORMATION` embeds an `IO_COUNTERS` (Threading) and `CreateJobObjectW` takes a
# `SECURITY_ATTRIBUTES` pointer (Security); windows-sys gates each namespace behind its own feature.
[target.'cfg(windows)'.dependencies]
windows-sys = { workspace = true, features = [
    "Win32_Foundation",
    "Win32_Security",
    "Win32_System_JobObjects",
    "Win32_System_Threading",
] }
```

- [ ] **Step 3: Write the failing tests.**

In `crates/manifold-view-protocol/src/lib.rs`, replace

```rust
pub mod host_env;
pub mod messages;
```

with

```rust
pub mod host_env;
#[cfg(windows)]
pub mod job;
pub mod messages;
```

Create `crates/manifold-view-protocol/src/job.rs` with only the tests for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    #[test]
    fn a_job_is_created() {
        assert!(kill_on_close("test job").is_some());
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
```

- [ ] **Step 4: Run them to see them fail.**

Run: `cargo test -j 8 -p manifold-view-protocol job::`
Expected: a compile error, `cannot find function `kill_on_close` in this scope` (and the same for the method).

- [ ] **Step 5: Implement.**

Put this above the `#[cfg(test)]` module in `crates/manifold-view-protocol/src/job.rs`:

```rust
//! A Windows Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`: when its last handle closes, Windows ends every
//! process in it, however the holder ended (a clean exit, a crash, a force-kill), since the OS closes a dying
//! process's handles. manifold and the gate put each child they spawn in one (`assign_child`), so a view host or a
//! gate instance dies with its parent; the CEF and WebView2 hosts put themselves in one (`assign_self`), so the
//! browser processes their engine starts, which join the job as they are created, die with the host. A process in
//! its own job also ends when it closes the job's last handle (a host's `_job` dropping at the end of `main`).

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

    /// Puts this process in the job, and with it every process it starts from then on. A failure is said on stderr.
    pub fn assign_self(&self) {
        // SAFETY: returns the current process's pseudo-handle, which needs no closing.
        let process = unsafe { GetCurrentProcess() };
        self.assign(process, "this process");
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
```

- [ ] **Step 6: Run the tests.**

Run: `cargo test -j 8 -p manifold-view-protocol job::`
Expected: `test job::tests::a_job_is_created ... ok`, `test job::tests::dropping_the_job_ends_its_children ... ok`, `test result: ok. 2 passed`. Then `tasklist /FI "IMAGENAME eq PING.EXE"` prints "INFO: No tasks are running which match the specified criteria." (no `ping` left).

- [ ] **Step 7: Format, lint, commit.**

Run: `cargo fmt --all` then the repo clippy line (Global Constraints).
Expected: clippy ends with `Finished` and no warning.
Run: `git diff --stat`
Expected: `Cargo.lock`, `crates/manifold-view-protocol/Cargo.toml`, `crates/manifold-view-protocol/src/job.rs`, `crates/manifold-view-protocol/src/lib.rs` only.

```bash
cd "/c/Users/Winston Li/Documents/GitHub/manifold"
git add Cargo.lock crates/manifold-view-protocol/Cargo.toml crates/manifold-view-protocol/src/job.rs crates/manifold-view-protocol/src/lib.rs
git commit -F - <<'EOF'
protocol: one kill-on-close job module (CODE5)

manifold_view_protocol::job: kill_on_close(label) -> Option<Job>, Job::assign_child, Job::assign_self; Drop closes
the handle. windows-sys is a cfg(windows) dependency, so the crate still builds on Linux. Callers move onto it in the
next commits. Plan 2026-10-31-shared-host-code.md Task 1.

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>
EOF
```

---

### Task 2: `manifold_view_protocol::nav_state` and `NavLabel::had_recent_input` (TDD)

**Files:**
- Modify: `crates/manifold-view-protocol/src/lib.rs`
- Create: `crates/manifold-view-protocol/src/nav_state.rs`
- Modify: `crates/manifold-view-protocol/src/nav_label.rs`

**Interfaces:**
- Consumes: `crate::nav_label::{NavLabel, GESTURE_WINDOW}` (`NavLabel::created_by_host(now: Instant) -> NavLabel`, fields `user_started: Option<bool>`, `host_started: Option<(bool, Instant)>`, `last_input: Option<Instant>`, `finished: bool`); `crate::ViewToHost::NavigationStateChanged { url: String, title: String, can_go_back: bool, can_go_forward: bool, loading: bool, user_started: Option<bool> }` (`ViewToHost` derives `Debug, Clone`, not `PartialEq`).
- Produces:
  ```rust
  #[derive(Clone, Default)]
  pub struct NavState { pub url: String, pub title: String, pub can_go_back: bool, pub can_go_forward: bool,
                        pub loading: bool, pub label: NavLabel, pub dirty: bool }
  impl NavState {
      pub fn new(url: String, dirty: bool) -> Self;
      pub fn take_message(&mut self) -> Option<ViewToHost>;
  }
  pub type SharedNavState = Rc<RefCell<NavState>>;
  pub fn new_shared_nav_state(url: String, dirty: bool) -> SharedNavState;
  pub type SharedJsonBody = Rc<RefCell<Option<String>>>;
  pub fn new_shared_json_body() -> SharedJsonBody;
  pub type SharedRedirectContentType = Rc<RefCell<Option<String>>>;
  pub fn new_shared_redirect_content_type() -> SharedRedirectContentType;
  impl NavLabel { pub fn had_recent_input(&self) -> bool; }
  ```

- [ ] **Step 1: Write the failing tests.**

In `crates/manifold-view-protocol/src/lib.rs`, replace

```rust
pub mod nav_label;
pub mod render;
```

with

```rust
pub mod nav_label;
pub mod nav_state;
pub mod render;
```

Create `crates/manifold-view-protocol/src/nav_state.rs` with only the tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// The message's fields in order, so a test compares them in one assert (`ViewToHost` has no `PartialEq`).
    fn fields(m: ViewToHost) -> (String, String, bool, bool, bool, Option<bool>) {
        match m {
            ViewToHost::NavigationStateChanged { url, title, can_go_back, can_go_forward, loading, user_started } => {
                (url, title, can_go_back, can_go_forward, loading, user_started)
            }
            other => panic!("not a NavigationStateChanged: {other:?}"),
        }
    }

    #[test]
    fn a_clean_state_sends_nothing() {
        assert!(NavState::default().take_message().is_none());
    }

    #[test]
    fn a_dirty_state_sends_once_then_is_clean() {
        let mut s = NavState {
            url: String::from("https://a.test/"),
            title: String::from("A"),
            can_go_back: true,
            loading: true,
            dirty: true,
            ..NavState::default()
        };
        let m = s.take_message().expect("a dirty state sends");
        assert_eq!(fields(m), (String::from("https://a.test/"), String::from("A"), true, false, true, None));
        assert!(!s.dirty);
        assert!(s.take_message().is_none());
    }

    #[test]
    fn a_finished_navigation_clears_its_label_after_sending_it() {
        let mut s = NavState { dirty: true, ..NavState::default() };
        s.label.user_started = Some(true);
        s.label.finished = true;
        assert_eq!(fields(s.take_message().unwrap()).5, Some(true), "the load's end still carries the label");
        assert_eq!((s.label.user_started, s.label.finished), (None, false));
    }

    #[test]
    fn an_unfinished_navigation_keeps_its_label() {
        let mut s = NavState { dirty: true, ..NavState::default() };
        s.label.user_started = Some(false);
        assert_eq!(fields(s.take_message().unwrap()).5, Some(false));
        assert_eq!(s.label.user_started, Some(false));
    }

    #[test]
    fn a_host_created_state_labels_its_first_navigation_the_hosts() {
        let s = NavState::new(String::from("https://a.test/"), true);
        assert_eq!(s.url, "https://a.test/");
        assert!(s.dirty);
        assert!(s.label.host_started.is_some_and(|(back_forward, _)| !back_forward));
        assert!(!NavState::new(String::new(), false).dirty);
    }
}
```

In `crates/manifold-view-protocol/src/nav_label.rs`, add this test at the end of `mod tests` (after `a_new_browser_counts_as_the_hosts_navigation`):

```rust

    #[test]
    fn recent_input_is_seen_without_being_used_up() {
        let now = Instant::now();
        let l = NavLabel { last_input: Some(now), ..NavLabel::default() };
        assert!(l.had_recent_input());
        assert!(l.last_input.is_some(), "asking doesn't use it up");
        let old = now.checked_sub(GESTURE_WINDOW).expect("the clock has run longer than GESTURE_WINDOW");
        assert!(!NavLabel { last_input: Some(old), ..NavLabel::default() }.had_recent_input());
        assert!(!NavLabel::default().had_recent_input());
    }
```

- [ ] **Step 2: Run them to see them fail.**

Run: `cargo test -j 8 -p manifold-view-protocol nav_`
Expected: compile errors: `cannot find type `NavState` in this scope`, `cannot find type `ViewToHost``, and `no method named `had_recent_input` found for struct `NavLabel``.

- [ ] **Step 3: Implement `had_recent_input`.**

In `crates/manifold-view-protocol/src/nav_label.rs`, replace

```rust
    /// A navigation is started and not yet finished.
    pub fn under_way(&self) -> bool {
        self.user_started.is_some() && !self.finished
    }
```

with

```rust
    /// A navigation is started and not yet finished.
    pub fn under_way(&self) -> bool {
        self.user_started.is_some() && !self.finished
    }

    /// The page got a click or key press within `GESTURE_WINDOW`. Unlike `label_new`, it doesn't use it up
    /// (WebView2's `popup.rs` asks it for each `NewWindowRequested`).
    pub fn had_recent_input(&self) -> bool {
        self.last_input.is_some_and(|at| at.elapsed() < GESTURE_WINDOW)
    }
```

And replace the module doc

```rust
//! Whether a navigation was started by the user or by the host (manifold
//! itself), shared by the CEF and WebView2 view hosts. Only user-started
//! navigations are shared with the tab's other instances.
```

with

```rust
//! Whether a navigation was started by the user or by the host (manifold
//! itself), set by the CEF and WebView2 view hosts and left at its default
//! by Servo's, all through `nav_state::NavState::label`. Only user-started
//! navigations are shared with the tab's other instances.
```

- [ ] **Step 4: Implement `nav_state`.**

Put this above the `#[cfg(test)]` module in `crates/manifold-view-protocol/src/nav_state.rs`:

```rust
//! The navigation state a view host reports, used by the CEF, WebView2 and Servo hosts. The engine's callbacks (CEF's
//! `DisplayHandler`/`LoadHandler`/`RequestHandler`, WebView2's navigation events, Servo's `WebViewDelegate`) each
//! change one part of it as it arrives and set `dirty`; the host's main loop sends one
//! `ViewToHost::NavigationStateChanged` per tick for all of them (`take_message`), not one per callback.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use crate::nav_label::NavLabel;
use crate::ViewToHost;

#[derive(Clone, Default)]
pub struct NavState {
    pub url: String,
    pub title: String,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub loading: bool,
    /// Who started the current navigation. CEF reports Chromium's own `user_gesture` only for a new document
    /// (`on_before_browse`); a same-document url change (`history.pushState`, a new `#fragment`) falls back to
    /// `last_input`, set from the host's `main.rs`. WebView2 has no gesture flag to trust: `IsUserInitiated` was true
    /// even for a page's `setTimeout` navigation in a tab that never got any input (runtime tested 2026-09-26), so it
    /// rests on `last_input` alone, which a new document uses up as it starts. Servo leaves it at the default, so its
    /// messages say `user_started: None`.
    pub label: NavLabel,
    /// At least one part changed since the main loop last sent the state.
    pub dirty: bool,
}

impl NavState {
    /// The state of a browser the host created at `url`: that first navigation is the host's
    /// (`NavLabel::created_by_host`). `dirty` says whether it is sent on the first tick: CEF sends it at once, WebView2
    /// waits for its first navigation event.
    pub fn new(url: String, dirty: bool) -> Self {
        Self { url, label: NavLabel::created_by_host(Instant::now()), dirty, ..Self::default() }
    }

    /// If dirty: clears it and returns the message, then applies the label's reset (a finished navigation's label is
    /// sent with its load's end and cleared after, so a later change nothing started isn't labelled).
    pub fn take_message(&mut self) -> Option<ViewToHost> {
        if !std::mem::take(&mut self.dirty) {
            return None;
        }
        let message = ViewToHost::NavigationStateChanged {
            url: self.url.clone(),
            title: self.title.clone(),
            can_go_back: self.can_go_back,
            can_go_forward: self.can_go_forward,
            loading: self.loading,
            user_started: self.label.user_started,
        };
        if std::mem::take(&mut self.label.finished) {
            self.label.user_started = None;
        }
        Some(message)
    }
}

/// A `NavState` the engine's callbacks and the main loop share.
pub type SharedNavState = Rc<RefCell<NavState>>;

pub fn new_shared_nav_state(url: String, dirty: bool) -> SharedNavState {
    Rc::new(RefCell::new(NavState::new(url, dirty)))
}

/// A page's raw JSON body (`ViewToHost::JsonBody`): filled once per navigation by the engine's page-kind check when
/// the page is JSON, drained by the main loop.
pub type SharedJsonBody = Rc<RefCell<Option<String>>>;

pub fn new_shared_json_body() -> SharedJsonBody {
    Rc::new(RefCell::new(None))
}

/// The content type of a page manifold may show in its own viewer (`ViewToHost::RedirectableContent`): the other
/// answer of the same page-kind check, drained the same way.
pub type SharedRedirectContentType = Rc<RefCell<Option<String>>>;

pub fn new_shared_redirect_content_type() -> SharedRedirectContentType {
    Rc::new(RefCell::new(None))
}
```

- [ ] **Step 5: Run the tests.**

Run: `cargo test -j 8 -p manifold-view-protocol nav_`
Expected: the five `nav_state::tests::*` and the five `nav_label::tests::*` tests pass, `test result: ok. 10 passed` (0 failed).

- [ ] **Step 6: The protocol crate on Linux** (CI's cfg-leak check; the job module must vanish there).

Guards first, in order:
1. `df -h /c | tail -1`. A cold `target-linux` build needs several GB; if under ~5 GB free and `target-linux` does not already exist, skip this step (fallback below) rather than build, and delete nothing without asking.
2. `wsl.exe -l -q` (its output is UTF-16, so read it through `tr -d '\0'`). If no line equals `Ubuntu`, do not use `-d Ubuntu`: skip this step (fallback below), or use another listed distro only if the user names it.

Run (from bash): `wsl.exe -d Ubuntu -- bash -c 'cd "/mnt/c/Users/Winston Li/Documents/GitHub/manifold" && CARGO_TARGET_DIR=target-linux cargo test -j 8 -p manifold-view-protocol'`
Expected: `test result: ok.` with no `job::tests` in the list (they are Windows only) and the `nav_state::tests` present. This builds in `target-linux` (minutes, incremental).
Fallback (guard fails, or WSL is unavailable or errors): say "Linux check skipped: <reason>" in the commit message and in the closing report, and rely on CI's Linux job (`cargo test -p manifold-view-protocol`) plus a read of `lib.rs` (`job` is behind `#[cfg(windows)]`, and `windows-sys` is a `target.'cfg(windows)'` dependency). Never fall back silently.

- [ ] **Step 7: Format, lint, commit.**

Run: `cargo fmt --all`, the repo clippy line, `git diff --stat`.
Expected: clippy clean; diff lists only `crates/manifold-view-protocol/src/{lib.rs,nav_label.rs,nav_state.rs}`.

```bash
cd "/c/Users/Winston Li/Documents/GitHub/manifold"
git add crates/manifold-view-protocol/src/lib.rs crates/manifold-view-protocol/src/nav_label.rs crates/manifold-view-protocol/src/nav_state.rs
git commit -F - <<'EOF'
protocol: one NavState with take_message, and NavLabel::had_recent_input (CODE6)

manifold_view_protocol::nav_state: NavState::new(url, dirty), take_message() (the cef/webview2 flush: the message,
then the finished label's reset), SharedNavState, SharedJsonBody, SharedRedirectContentType and their constructors.
had_recent_input moves beside GESTURE_WINDOW. The hosts move onto them in the next commits. Plan
2026-10-31-shared-host-code.md Task 2.

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>
EOF
```

---

### Task 3: manifold and gate onto the shared job

**Files:**
- Modify: `crates/manifold/src/process_manager/job.rs` (whole file replaced; read at 7e6010d5, 118 lines)
- Modify: `crates/gate/Cargo.toml`
- Modify: `crates/gate/src/instance.rs:33-92` (`pub(crate) mod job`)
- Modify: `Cargo.lock` (by cargo)

**Interfaces:**
- Consumes: `manifold_view_protocol::job::{kill_on_close, Job}` (Task 1).
- Produces (unchanged signatures, callers untouched): `pub(super) fn assign_to_view_job(child: &Child)` in manifold (callers in `process_manager.rs` and `process_manager/webkit.rs`; find them with `git grep -n assign_to_view_job -- crates/manifold`); `pub fn assign(child: &Child)` in gate's `instance::job` (callers in `all.rs`, `instance.rs`, `launch_forms.rs`; `git grep -n "job::assign" -- crates/gate`).

- [ ] **Step 1: Replace manifold's `process_manager/job.rs`.**

Replace the whole file `crates/manifold/src/process_manager/job.rs` with (the long doc comment is the old one, kept word for word except its first lines, which named the deleted `ViewJobHandle`):

```rust
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
```

- [ ] **Step 2: Add the protocol dependency to gate (P4).**

In `crates/gate/Cargo.toml`, replace

```toml
[dependencies]
manifold-hosts.workspace = true
```

with

```toml
[dependencies]
manifold-hosts.workspace = true
# The kill-on-close job every instance joins (`instance::job`).
manifold-view-protocol.workspace = true
```

- [ ] **Step 3: Replace gate's `mod job`.**

In `crates/gate/src/instance.rs`, replace the whole block from

```rust
/// The Job Object every instance is assigned to; same construction as crates/manifold `process_manager/job.rs`.
pub(crate) mod job {
```

down to and including its closing `}` (the line after `eprintln!("gate job: instance {} not assigned: {}", child.id(), std::io::Error::last_os_error());` and the two closing braces of `assign`'s `if` and fn, i.e. through the `}` that ends the module, just before `fn timed_out`), with:

```rust
/// The Job Object every instance is assigned to (`manifold_view_protocol::job`, as crates/manifold's view-host job).
pub(crate) mod job {
    use std::process::Child;
    use std::sync::OnceLock;

    use manifold_view_protocol::job::{kill_on_close, Job};

    /// Created at the first instance, with kill-on-close: Windows kills every process in it when `gate` exits for any
    /// reason. `None` when it could not be created (said on stderr); a static, so it is never closed early.
    static JOB: OnceLock<Option<Job>> = OnceLock::new();

    /// Adds a just-spawned instance to the job; a failure (say the child already exited) is said on stderr and the
    /// run goes on with `Drop` as the only cleanup.
    pub fn assign(child: &Child) {
        if let Some(job) = JOB.get_or_init(|| kill_on_close("gate job")) {
            job.assign_child(child);
        }
    }
}
```

After the edit, `git diff crates/gate/src/instance.rs` must show only that block changed, and `fn timed_out(what: String) -> io::Error {` must directly follow the new module (one blank line between).

- [ ] **Step 4: Targeted checks.**

Run: `cargo test -j 8 -p manifold process_manager::`
Expected: `test result: ok.` (no failures; the count is whatever it was before, none of these tests is new).
Run: `cargo test -j 8 -p gate instance::`
Expected: `test result: ok.`
Run: `git grep -n -E "ViewJobHandle|JobHandle|fn view_job|fn job\(\)" -- crates/`
Expected: no output.

- [ ] **Step 5: Format, lint, commit.**

Run: `cargo fmt --all`, the repo clippy line, `git diff --stat`.
Expected: clippy clean; diff lists `Cargo.lock`, `crates/gate/Cargo.toml`, `crates/gate/src/instance.rs`, `crates/manifold/src/process_manager/job.rs` only.

```bash
cd "/c/Users/Winston Li/Documents/GitHub/manifold"
git add Cargo.lock crates/gate/Cargo.toml crates/gate/src/instance.rs crates/manifold/src/process_manager/job.rs
git commit -F - <<'EOF'
app, gate: the view-host and instance jobs use manifold_view_protocol::job (CODE5)

manifold's VIEW_JOB and gate's JOB are OnceLock<Option<Job>> over kill_on_close("engine job" / "gate job");
ViewJobHandle, JobHandle and their unsafe Send/Sync impls go. A failed assignment in manifold is now said on stderr
(it was silent). gate's message reads "child N not assigned" (was "instance N"). gate now depends on
manifold-view-protocol. Plan 2026-10-31-shared-host-code.md Task 3.

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>
EOF
```

---

### Task 4: the WebView2 host onto the shared job and `NavState`

**Files:**
- Modify: `crates/manifold-view-webview2/src/main.rs` (lines 1-11 doc, 46, 96-130, 203-205, 425-441, 526-548 at 7e6010d5)
- Modify: `crates/manifold-view-webview2/src/nav_state.rs` (lines 1-77)
- Modify: `crates/manifold-view-webview2/src/popup.rs` (lines 22, 37-41, 54)

**Interfaces:**
- Consumes: `manifold_view_protocol::job::{self, Job}` (Task 1); `manifold_view_protocol::nav_state::{new_shared_json_body, new_shared_nav_state, new_shared_redirect_content_type, SharedJsonBody, SharedNavState, SharedRedirectContentType}`, `NavState::take_message`, `NavLabel::had_recent_input` (Task 2).
- Produces: nothing new; `nav_state::register_handlers(webview: &ICoreWebView2, state: SharedNavState, json_body: SharedJsonBody, redirect_content_type: SharedRedirectContentType)` and `popup::register(webview: &ICoreWebView2, nav_state: SharedNavState) -> PendingPopup` keep their signatures with the protocol's types.

- [ ] **Step 1: `nav_state.rs` loses its definitions.**

In `crates/manifold-view-webview2/src/nav_state.rs`, replace everything from the first line down to and including

```rust
pub fn new_shared_redirect_content_type() -> SharedRedirectContentType {
    Rc::new(RefCell::new(None))
}
```

(lines 1-77: the module doc, the imports, `NavState`, its `impl`, the three aliases and their constructors) with:

```rust
//! The WebView2 events that keep the host's `NavState` (`manifold_view_protocol::nav_state`) current: the real
//! native events (`NavigationStarting`/`NavigationCompleted`/`SourceChanged`/`DocumentTitleChanged`/
//! `HistoryChanged`) play the role CEF's `DisplayHandler`/`LoadHandler` callbacks do, each just mutating the shared
//! cell and setting `dirty`, coalesced into one `NavigationStateChanged` message per tick by `main.rs`'s loop
//! (`NavState::take_message`) rather than one message per event.

use std::time::Instant;

use manifold_view_protocol::nav_state::{SharedJsonBody, SharedNavState, SharedRedirectContentType};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2NavigationStartingEventArgs, ICoreWebView2SourceChangedEventArgs,
};
use webview2_com::{
    take_pwstr, DocumentTitleChangedEventHandler, ExecuteScriptCompletedHandler, HistoryChangedEventHandler,
    NavigationCompletedEventHandler, NavigationStartingEventHandler, SourceChangedEventHandler,
};
use windows::core::{BOOL, HSTRING, PWSTR};
```

The rest of the file (from `/// The raw COM binding's `CanGoBack`/`CanGoForward` are `[out, retval]` on) is unchanged.

- [ ] **Step 2: `popup.rs` reads the label.**

In `crates/manifold-view-webview2/src/popup.rs`, replace `use crate::nav_state::SharedNavState;` with `use manifold_view_protocol::nav_state::SharedNavState;` (rustfmt moves it into the external-crate group).

Replace

```rust
/// window (`NavState::had_recent_input`): `IsUserInitiated` on navigations
/// was true even for a `setTimeout` navigation in a tab that never got
/// input (see `NavState::label`), so it isn't trusted alone.
```

with

```rust
/// window (`NavLabel::had_recent_input`): `IsUserInitiated` on navigations
/// was true even for a `setTimeout` navigation in a tab that never got
/// input (see `manifold_view_protocol::nav_state::NavState::label`), so it isn't trusted alone.
```

Replace

```rust
            let user_gesture = user_initiated && nav_state.try_borrow().is_ok_and(|s| s.had_recent_input());
```

with

```rust
            let user_gesture = user_initiated && nav_state.try_borrow().is_ok_and(|s| s.label.had_recent_input());
```

- [ ] **Step 3: `main.rs` imports and doc.**

In `crates/manifold-view-webview2/src/main.rs`, replace

```rust
//! `manifold-view-cef/src/main.rs`'s shape (fixed-tick poll loop,
//! `SharedNavState` + `dirty` flag) since CEF is the closest existing
```

with

```rust
//! `manifold-view-cef/src/main.rs`'s shape (fixed-tick poll loop, the
//! shared `manifold_view_protocol::nav_state` state) since CEF is the closest existing
```

Replace

```rust
use nav_state::new_shared_nav_state;
```

with

```rust
use manifold_view_protocol::job::{self, Job};
use manifold_view_protocol::nav_state::{new_shared_json_body, new_shared_nav_state, new_shared_redirect_content_type};
```

- [ ] **Step 4: `main.rs` job: one call, first in `main` (P3).**

Delete the old call and its comment, i.e. remove this whole block (lines 114-130):

```rust
    // WebView2 spawns its own Chromium-like multi-process tree
    // (msedgewebview2.exe browser/renderer/GPU processes) that are NOT
    // direct children of this process in the Windows sense (they're
    // children of WebView2's own browser-process, a grandchild of us) --
    // a hard-killed manifold-view-webview2.exe (e.g. an
    // external `timeout` cutting the whole tree, or a plain force-kill)
    // leaves those grandchildren running indefinitely, invisible to any
    // cleanup this process's own `Shutdown` handler could do (it never
    // gets to run). A Job Object with `KILL_ON_JOB_CLOSE`, assigned to this
    // process at startup, fixes this generally: Windows terminates every
    // process still in the job the moment this process's own job handle is
    // closed, which happens automatically on ANY exit path (graceful,
    // crashed, or force-killed) since the OS closes all of a dying
    // process's handles regardless of how it died. `_job`'s only purpose
    // is to stay alive for the whole process lifetime; never used again
    // after this.
    let _job = setup_kill_on_exit_job();

```

and insert at the top of `main`, directly after `fn main() {` and before `    // Must happen before any window exists (`create_hidden_window` below is`:

```rust
    // WebView2 spawns its own Chromium-like multi-process tree
    // (msedgewebview2.exe browser/renderer/GPU processes) that are NOT
    // direct children of this process in the Windows sense (they're
    // children of WebView2's own browser-process, a grandchild of us) --
    // a hard-killed manifold-view-webview2.exe (e.g. an
    // external `timeout` cutting the whole tree, or a plain force-kill)
    // leaves those grandchildren running indefinitely, invisible to any
    // cleanup this process's own `Shutdown` handler could do (it never
    // gets to run). A Job Object with `KILL_ON_JOB_CLOSE`, assigned to this
    // process at startup, fixes this generally: Windows terminates every
    // process still in the job the moment this process's own job handle is
    // closed, which happens automatically on ANY exit path (graceful,
    // crashed, or force-killed) since the OS closes all of a dying
    // process's handles regardless of how it died. `_job` is the first
    // local so it is the last one `main` drops: closing it ends this
    // process and everything still in the job, after the rest of `main`'s
    // cleanup has run. A failure is said on stderr (`manifold_view_protocol::job`).
    let _job = job::kill_on_close("view host").inspect(Job::assign_self);

```

Replace (in the `DispatcherQueue` comment)

```rust
    // is to stay alive for the whole process lifetime, same as `_job` below.
```

with

```rust
    // is to stay alive for the whole process lifetime, same as `_job` above.
```

Delete the old function and its doc (lines 526-548), from

```rust
/// See the call site's doc comment. Returns the job handle purely to keep
/// it alive for the process lifetime -- never queried or signaled again.
fn setup_kill_on_exit_job() -> windows::core::Result<windows::Win32::Foundation::HANDLE> {
```

through its closing `}` (the one after `        Ok(job)\n    }`), together with the blank line that follows it, so `/// The installed WebView2 runtime's version, as the loader finds it (honouring` follows the end of `main` after one blank line.

- [ ] **Step 5: `main.rs` state and flush.**

Replace

```rust
    let nav_state = new_shared_nav_state(initial_url.clone());
    let pending_json_body = nav_state::new_shared_json_body();
    let pending_redirect_content_type = nav_state::new_shared_redirect_content_type();
```

with

```rust
    // Not dirty: the first navigation event reports it (`main.rs` navigates to `initial_url` below).
    let nav_state = new_shared_nav_state(initial_url.clone(), false);
    let pending_json_body = new_shared_json_body();
    let pending_redirect_content_type = new_shared_redirect_content_type();
```

Replace

```rust
        {
            let mut state = nav_state.borrow_mut();
            if state.dirty {
                state.dirty = false;
                let _ = control.send(&ViewToHost::NavigationStateChanged {
                    url: state.url.clone(),
                    title: state.title.clone(),
                    can_go_back: state.can_go_back,
                    can_go_forward: state.can_go_forward,
                    loading: state.loading,
                    user_started: state.label.user_started,
                });
                if std::mem::take(&mut state.label.finished) {
                    state.label.user_started = None;
                }
            }
        }
```

with

```rust
        if let Some(m) = nav_state.borrow_mut().take_message() {
            let _ = control.send(&m);
        }
```

- [ ] **Step 6: Targeted checks.**

Run: `cargo test -j 8 -p manifold-view-webview2`
Expected: compiles; `test result: ok.` (the crate's existing tests; none new).
Run: `git grep -n -E "setup_kill_on_exit_job|had_recent_input\(\)|nav_state::new_shared|struct NavState" -- crates/manifold-view-webview2`
Expected: one line, `crates/manifold-view-webview2/src/popup.rs:...: ... s.label.had_recent_input());`.

- [ ] **Step 7: Format, lint, commit.**

Run: `cargo fmt --all`, the repo clippy line, `git diff --stat`.
Expected: clippy clean; diff lists `crates/manifold-view-webview2/src/{main.rs,nav_state.rs,popup.rs}` only.

```bash
cd "/c/Users/Winston Li/Documents/GitHub/manifold"
git add crates/manifold-view-webview2/src/main.rs crates/manifold-view-webview2/src/nav_state.rs crates/manifold-view-webview2/src/popup.rs
git commit -F - <<'EOF'
webview2: shared job and NavState from manifold_view_protocol (CODE5, CODE6)

setup_kill_on_exit_job goes: _job = job::kill_on_close("view host").inspect(Job::assign_self), now the first local
of main so it is dropped last; a failure is printed (it was dropped silently), and a failed limit still joins the job.
NavState, its aliases and had_recent_input come from the protocol crate; the flush is take_message(). Plan
2026-10-31-shared-host-code.md Task 4.

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>
EOF
```

---

### Task 5: the CEF host onto the shared job and `NavState`

**Files:**
- Modify: `crates/manifold-view-cef/src/main.rs` (lines 18-29, 42-65, 121-135, 179-187, 427-443 at 7e6010d5)
- Modify: `crates/manifold-view-cef/src/client.rs` (lines 20-23, 29-48)
- Modify: `crates/manifold-view-cef/src/nav_handler.rs` (lines 1-46)
- Modify: `crates/manifold-view-cef/src/life_span_handler.rs` (line 8)

**Interfaces:**
- Consumes: `manifold_view_protocol::job::{self, Job}` (`#[cfg(windows)]`); `manifold_view_protocol::nav_state::{new_shared_json_body, new_shared_nav_state, new_shared_redirect_content_type, SharedJsonBody, SharedNavState, SharedRedirectContentType}`.
- Produces: nothing new. `ViewDisplayHandler::new(state: SharedNavState)`, `ViewLoadHandler::new(state: SharedNavState, muted: SharedMuted)`, `ViewRequestHandler::new(state: SharedNavState)` and `ClientBuilder::build(..)` keep their signatures with the protocol's types. `SharedMuted`/`new_shared_muted` stay in `nav_handler.rs`; `SharedAudioPlaying`, `SharedPageState`, `SharedPageText` and the other cef-only aliases stay in `client.rs`.

- [ ] **Step 1: Disk and build state.**

Run: `df -h /c | tail -1; ls -d "/c/Users/Winston Li/Documents/GitHub/manifold/target/debug/build/cef-dll-sys"`
Expected: at least ~5 GB available, and the directory exists. Otherwise stop and report to the user (Global Constraints "Disk"; the CEF build is one of the heavy steps); delete nothing without asking.

- [ ] **Step 2: `nav_handler.rs` loses `NavState`.**

In `crates/manifold-view-cef/src/nav_handler.rs`, replace everything from the first line down to and including

```rust
pub fn new_shared_nav_state(initial_url: String) -> SharedNavState {
    std::rc::Rc::new(RefCell::new(NavState::new(initial_url)))
}
```

(lines 1-46) with:

```rust
use std::time::Instant;

use cef::{rc::Rc, *};
use manifold_view_protocol::nav_state::SharedNavState;
```

The rest (from `/// The last `HostToView::SetMuted` value requested for this tab` on) is unchanged.

- [ ] **Step 3: `client.rs` loses the two aliases.**

In `crates/manifold-view-cef/src/client.rs`, replace

```rust
use crate::nav_handler::{
    DisplayHandlerBuilder, LoadHandlerBuilder, RequestHandlerBuilder, SharedMuted, SharedNavState, ViewDisplayHandler,
    ViewLoadHandler, ViewRequestHandler,
};
```

with

```rust
use crate::nav_handler::{
    DisplayHandlerBuilder, LoadHandlerBuilder, RequestHandlerBuilder, SharedMuted, ViewDisplayHandler,
    ViewLoadHandler, ViewRequestHandler,
};
```

and add, in the external-crate `use` group (after `use cef::{rc::Rc as CefRc, *};`):

```rust
use manifold_view_protocol::nav_state::{SharedJsonBody, SharedNavState, SharedRedirectContentType};
```

Delete these lines (29-48), together with the blank line after them:

```rust
/// The renderer subprocess's own answer to a `check-json-body`
/// `ProcessMessage` (see `render_process_handler.rs`) -- `Client::
/// on_process_message_received` below is where a reply *arrives* in the
/// browser process; `main.rs`'s own poll loop drains it into
/// `ViewToHost::JsonBody`, same "shared-RefCell-drained-by-the-main-loop"
/// shape `find_handler.rs`/`download_handler.rs` already use.
pub type SharedJsonBody = Rc<RefCell<Option<String>>>;

pub fn new_shared_json_body() -> SharedJsonBody {
    Rc::new(RefCell::new(None))
}

/// Same shape as `SharedJsonBody`, for the `redirect-content-result` reply
/// -- see `manifold_view_protocol::ViewToHost::RedirectableContent`'s own doc
/// comment.
pub type SharedRedirectContentType = Rc<RefCell<Option<String>>>;

pub fn new_shared_redirect_content_type() -> SharedRedirectContentType {
    Rc::new(RefCell::new(None))
}
```

so the file goes from the `use crate::render_process_handler::{...};` block straight to `/// The renderer's latest `check-audio-playing` reply -- unlike`. (`Rc` and `RefCell` stay imported: `SharedAudioPlaying` and the other cef aliases use them.)

- [ ] **Step 4: `main.rs` imports and the job (P3).**

In `crates/manifold-view-cef/src/main.rs`, replace

```rust
use manifold_view_protocol::{HostToView, InputEvent, TabState, ViewToHost};
```

with

```rust
#[cfg(windows)]
use manifold_view_protocol::job::{self, Job};
use manifold_view_protocol::nav_state::{new_shared_json_body, new_shared_nav_state, new_shared_redirect_content_type};
use manifold_view_protocol::{HostToView, InputEvent, TabState, ViewToHost};
```

Replace

```rust
use client::{
    new_shared_audio_playing, new_shared_json_body, new_shared_page_state, new_shared_page_text,
    new_shared_redirect_content_type, ClientBuilder,
};
```

with

```rust
use client::{new_shared_audio_playing, new_shared_page_state, new_shared_page_text, ClientBuilder};
```

Replace `use nav_handler::{new_shared_muted, new_shared_nav_state};` with `use nav_handler::new_shared_muted;`.

Delete the old function (lines 42-65) and the blank line after it:

```rust
/// Same job-object setup `manifold-view-webview2` already uses for its own
/// multi-process tree -- see this function's own call site for why.
#[cfg(windows)]
fn setup_kill_on_exit_job() -> windows::core::Result<windows::Win32::Foundation::HANDLE> {
```

through its closing `}` (after `        Ok(job)\n    }`), so `/// Whether libcef answered `api_hash` with a hash.` follows `const AUDIO_PLAYING_POLL_INTERVAL: Duration = Duration::from_millis(1000);` after one blank line.

Replace

```rust
    // automatically. Same fix `manifold-view-webview2` already applies for
    // its own equivalent multi-process tree; `_job`'s only purpose is to
    // stay alive for the whole process lifetime. Elsewhere there is no job;
    // on Linux CEF's subprocesses exited by themselves within 5 s of a
    // `kill -9` of this process (WSL2, 2026-10-02; the engine-install spec's
    // Platforms section).
    #[cfg(windows)]
    let _job = setup_kill_on_exit_job();
```

with

```rust
    // automatically. Same fix `manifold-view-webview2` already applies for
    // its own equivalent multi-process tree (`manifold_view_protocol::job`;
    // a failure is said on stderr). `_job` stays alive for the whole process
    // lifetime and closes when `main` returns, after `cef::shutdown()`;
    // closing it ends this process and its subprocesses (the locals declared
    // above it are never dropped, which costs nothing at exit). Elsewhere
    // there is no job; on Linux CEF's subprocesses exited by themselves
    // within 5 s of a `kill -9` of this process (WSL2, 2026-10-02; the
    // engine-install spec's Platforms section).
    #[cfg(windows)]
    let _job = job::kill_on_close("view host").inspect(Job::assign_self);
```

- [ ] **Step 5: `main.rs` state and flush.**

Replace

```rust
    let nav_state = new_shared_nav_state(initial_url.clone());
```

with

```rust
    // Dirty: the browser is created at `initial_url`, and that state is sent on the first tick.
    let nav_state = new_shared_nav_state(initial_url.clone(), true);
```

Replace

```rust
        {
            let mut nav = nav_state.borrow_mut();
            if nav.dirty {
                nav.dirty = false;
                let _ = control.send(&ViewToHost::NavigationStateChanged {
                    url: nav.url.clone(),
                    title: nav.title.clone(),
                    can_go_back: nav.can_go_back,
                    can_go_forward: nav.can_go_forward,
                    loading: nav.loading,
                    user_started: nav.label.user_started,
                });
                if std::mem::take(&mut nav.label.finished) {
                    nav.label.user_started = None;
                }
            }
        }
```

with

```rust
        if let Some(m) = nav_state.borrow_mut().take_message() {
            let _ = control.send(&m);
        }
```

- [ ] **Step 6: `life_span_handler.rs` comment.**

In `crates/manifold-view-cef/src/life_span_handler.rs`, replace

```rust
/// the main loop each tick, same pattern as `nav_handler`'s `NavState`. A
```

with

```rust
/// the main loop each tick, same pattern as `manifold_view_protocol::nav_state::NavState`. A
```

- [ ] **Step 7: Check the CEF host (Windows).**

Run: `cargo clippy -j 8 -p manifold-view-cef --all-targets -- -D warnings`
Expected: `Finished` with no warning. The `windows` crate is still a dependency here (Task 7 drops it), so no unused-import noise. If cef-dll-sys's build script starts a download or fails for want of the CEF archive, stop: mark the crate "verified by reading" in the commit message and continue with Task 6 (the gate in Task 8 then cannot exercise it either; say so in the closing report).
Run: `git grep -n -E "setup_kill_on_exit_job|struct NavState|fn new_shared_nav_state|type SharedJsonBody|type SharedRedirectContentType|windows::" -- crates/manifold-view-cef/src crates/manifold-view-cef/build.rs`
Expected: no output. (`input.rs` mentions only the identifiers `windows_key_code` and `windows_vk`, which `windows::` does not match; `build.rs` has only a comment containing the word "Windows".)

(The crate's unit tests, `main.rs` `tests`, are untouched by this task and are not run: the test exe links libcef and needs its runtime beside it.)

- [ ] **Step 8: Format, lint, commit.**

Run: `cargo fmt --all`, the repo clippy line, `git diff --stat`.
Expected: clippy clean; diff lists `crates/manifold-view-cef/src/{client.rs,life_span_handler.rs,main.rs,nav_handler.rs}` only.

```bash
cd "/c/Users/Winston Li/Documents/GitHub/manifold"
git add crates/manifold-view-cef/src/client.rs crates/manifold-view-cef/src/life_span_handler.rs crates/manifold-view-cef/src/main.rs crates/manifold-view-cef/src/nav_handler.rs
git commit -F - <<'EOF'
cef: shared job and NavState from manifold_view_protocol (CODE5, CODE6)

setup_kill_on_exit_job goes: #[cfg(windows)] _job = job::kill_on_close("view host").inspect(Job::assign_self); a
failure is printed (it was dropped silently), and the handle now closes when main returns, after cef::shutdown().
NavState, SharedJsonBody and SharedRedirectContentType come from the protocol crate (dirty at start, as before);
the flush is take_message(). Checked with cargo clippy -p manifold-view-cef on Windows; the Linux build by reading.
Plan 2026-10-31-shared-host-code.md Task 5.

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>
EOF
```

---

### Task 6: the Servo host onto the shared `NavState`

**Files:**
- Modify: `crates/manifold-view-servo/src/main.rs` (lines 6-8, 36-44, 336-350 at 7e6010d5)

**Interfaces:**
- Consumes: `manifold_view_protocol::nav_state::NavState` (`Default`, pub fields, `take_message`).
- Produces: nothing. `Delegate::nav: RefCell<NavState>` keeps its type name, now the protocol's; the construction `NavState { url: initial_url.clone(), ..Default::default() }` (line 188) is unchanged.

- [ ] **Step 1: Replace the struct and the flush.**

In `crates/manifold-view-servo/src/main.rs`, replace

```rust
use manifold_view_protocol::{
    modifiers as host_modifiers, HostToView, InputEvent as HostInputEvent, KeyCode as HostKeyCode, ViewToHost,
};
```

with

```rust
use manifold_view_protocol::nav_state::NavState;
use manifold_view_protocol::{
    modifiers as host_modifiers, HostToView, InputEvent as HostInputEvent, KeyCode as HostKeyCode, ViewToHost,
};
```

Delete these lines and the blank line after them:

```rust
#[derive(Default)]
struct NavState {
    url: String,
    title: String,
    can_go_back: bool,
    can_go_forward: bool,
    loading: bool,
    dirty: bool,
}
```

Replace

```rust
        {
            let mut nav = delegate.nav.borrow_mut();
            if nav.dirty {
                nav.dirty = false;
                let _ = control.send(&ViewToHost::NavigationStateChanged {
                    url: nav.url.clone(),
                    title: nav.title.clone(),
                    can_go_back: nav.can_go_back,
                    can_go_forward: nav.can_go_forward,
                    loading: nav.loading,
                    // Servo's `NavigationRequest` carries only the url.
                    user_started: None,
                });
            }
        }
```

with

```rust
        // Servo's `NavigationRequest` carries only the url, so `nav.label` stays at its default and every message
        // says `user_started: None`.
        if let Some(m) = delegate.nav.borrow_mut().take_message() {
            let _ = control.send(&m);
        }
```

`ViewToHost` stays imported (the loop sends other messages). No other line changes: the delegate callbacks set `url`, `title`, `loading`, `can_go_back`, `can_go_forward` and `dirty` by field, which the shared struct has, all `pub`.

- [ ] **Step 2: Read-check.**

Run: `git grep -n -E "struct NavState|user_started|label" -- crates/manifold-view-servo/src/main.rs`
Expected: only the new comment line (`... says `user_started: None`.`) and the `nav.label` mention in it; no other `label` use (Servo never writes the label, so `take_message` always leaves `user_started` `None` and `finished` false, as the old flush did).

- [ ] **Step 3: Build it, if it stays incremental.**

Run: `df -h /c | tail -1`. Under ~5 GB free: stop and report to the user (Servo is the heaviest build here); delete nothing without asking. With at least ~5 GB free, launch it through the wrapper (RUNS.md), detached, with `CARGO_TARGET_DIR` set in the launcher's environment so the child inherits it (no `set X=y &&` in the command, no unquoted path with a space):
```bash
cd "/c/Users/Winston Li/Documents/GitHub/manifold"
CARGO_TARGET_DIR=target-servo powershell -NoProfile -File "$USERPROFILE/.claude/runs/run.ps1" -Kind probe -Name servo-build -Detach -Command 'cargo build -j 8 -p manifold-view-servo'
```
It prints the run id and returns; the run's output is `%LOCALAPPDATA%\claude-runs\manifold\<run id>\out.log` and `err.log` (cargo writes to `err.log`). Right after launching, arm a watcher (Bash `run_in_background`) that exits when `run.json`'s `"end"` is no longer `null`, with a deadline of 45 min:
```bash
RJ="$LOCALAPPDATA/claude-runs/manifold/<run id>/run.json"
end=$((SECONDS+2700)); until ! grep -q '"end": *null' "$RJ" || [ $SECONDS -ge $end ]; do sleep 20; done; grep -E '"(end|exit)"' "$RJ"
```
Watch `err.log` meanwhile. Expected: `Compiling manifold-view-protocol`, `Compiling manifold-view-servo`, `Finished` within a few minutes (target-servo holds a 2026-10-29 build). If `err.log` shows Servo's dependency tree compiling (`Compiling mozjs_sys`, `Compiling style`, dozens of crates), it is a cold build of about 33 min (README "Building Servo on its own"): let it finish only if disk stays above ~5 GB; otherwise stop it (find the `cargo.exe` with `Get-CimInstance Win32_Process -Filter "Name='cargo.exe'"` whose command line names `manifold-view-servo`, and stop that one only) and record "Servo verified by reading" in the commit message. The raw run folder stays (RUNS.md); record the headline in the row. Clippy is not run on Servo (it would recompile the tree in check mode); the change only deletes code and calls `take_message`.

- [ ] **Step 4: Format, lint, commit.**

Run: `cargo fmt --all`, the repo clippy line (Servo is not in it; this proves the rest still builds), `git diff --stat`.
Expected: clippy clean; diff lists `crates/manifold-view-servo/src/main.rs` only.

```bash
cd "/c/Users/Winston Li/Documents/GitHub/manifold"
git add crates/manifold-view-servo/src/main.rs
git commit -F - <<'EOF'
servo: the shared NavState and take_message (CODE6)

Servo's own NavState goes; its default NavLabel keeps user_started None and finished false, so its messages are
unchanged. <Built incrementally in target-servo | Verified by reading: the build was cold>. Plan
2026-10-31-shared-host-code.md Task 6.

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>
EOF
```

(Keep exactly one of the two alternatives in the angle brackets, without the brackets.)

---

### Task 7: drop the features only the deleted copies used

**Files:**
- Modify: `crates/manifold/Cargo.toml` (lines 62-79), `crates/gate/Cargo.toml` (lines 18-28), `crates/manifold-view-webview2/Cargo.toml` (lines 20-48), `crates/manifold-view-cef/Cargo.toml` (lines 16-20)
- Modify: `Cargo.lock` (by cargo)

**Interfaces:**
- Consumes: nothing.
- Produces: nothing; builds unchanged.

- [ ] **Step 0: Disk.** Run `df -h /c | tail -1`. Under ~5 GB free, stop and report to the user (the cef clippy in Step 6 and the cef feature change relink CEF); delete nothing without asking.

- [ ] **Step 1: Re-grep every feature before deleting it.**

Run feature-specific greps (from bash, repo root). A bare `windows::` pattern is too broad: `std::os::windows`, `winit`, `windows::core::w!` and the like legitimately match, so grep each feature by its own namespace:
```bash
git grep -n "JobObjects" -- 'crates/**/*.rs'
git grep -n "JobObjects" -- 'crates/**/Cargo.toml'
git grep -n -E "Win32::Security\b" -- crates/manifold/src crates/gate/src crates/manifold-view-webview2/src
git grep -n "System::Threading" -- crates/manifold/src crates/gate/src crates/manifold-view-webview2/src
git grep -n "windows::Win32" -- crates/manifold-view-cef/src
git grep -n -E "windows::|windows_sys" -- crates/manifold-view-cef/build.rs
```
Expected: no `.rs` hit for `JobObjects` outside `crates/manifold-view-protocol/src/job.rs`; the `Cargo.toml` grep prints exactly the `Win32_System_JobObjects` feature lines that Steps 2-5 delete (`crates/manifold/Cargo.toml`, `crates/gate/Cargo.toml`, `crates/manifold-view-webview2/Cargo.toml`, `crates/manifold-view-cef/Cargo.toml`) plus the protocol crate's own (keep that one); no `windows::` hit in cef `src`, and none in cef `build.rs`. For the other namespaces, the hits are exactly the ones below and nothing else (re-grep gives the line numbers; they are not pinned here):
- gate: `all.rs` uses `windows_sys::Win32::System::Threading::GetSystemTimes`; `jar.rs` uses `windows_sys::Win32::Security::Cryptography::{..}` (so gate keeps `Win32_Security_Cryptography`, which enables `Win32_Security`).
- manifold: `jar.rs` uses `windows::Win32::Security::Cryptography` (the `windows` crate's `Win32_Security_Cryptography`, which stays; this is not a windows-sys `Security` user) and `windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE}`; `telemetry.rs` uses `windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes}`.
- webview2: no `Win32::Security` and no `System::Threading` hit.

So: no `JobObjects` user left anywhere outside the protocol crate; no windows-sys `Security` user in manifold; gate's only Security user is Cryptography; no `Threading`, `Security` or `JobObjects` user in webview2; no `windows::` in cef `src`. If the output differs, stop and report before deleting anything.

- [ ] **Step 2: manifold.**

In `crates/manifold/Cargo.toml`, replace

```toml
    "Win32_UI_WindowsAndMessaging",
    # For the view-host Job Object (see `process_manager::job`'s own
    # doc comment) -- `Win32_Security`/`Win32_System_Threading` aren't used
    # directly, but `JOBOBJECT_EXTENDED_LIMIT_INFORMATION` embeds types from
    # both (a `SECURITY_ATTRIBUTES` pointer parameter, an `IO_COUNTERS`
    # field), and windows-sys gates each Win32 namespace behind its own
    # feature with no automatic cross-feature pulling.
    "Win32_System_JobObjects",
    "Win32_Security",
    "Win32_System_Threading",
] }
```

with

```toml
    "Win32_UI_WindowsAndMessaging",
    # `OpenProcess`/`WaitForSingleObject` (`jar.rs`) and `GetProcessTimes` (`telemetry.rs`). The view-host job is
    # `manifold_view_protocol::job`.
    "Win32_System_Threading",
] }
```

- [ ] **Step 3: gate (P5).**

In `crates/gate/Cargo.toml`, replace

```toml
# Windows-only: the Job Object that kills every launched instance if `gate` itself dies. Same crate, version and
# feature set as crates/manifold's view-host job (process_manager/job.rs).
[target.'cfg(windows)'.dependencies]
windows-sys = { workspace = true, features = [
    "Win32_Foundation",
    "Win32_System_JobObjects",
    "Win32_Security",
    "Win32_Security_Cryptography",
    "Win32_System_Threading",
    "Win32_System_SystemInformation",
] }
```

with

```toml
# Windows-only: DPAPI for the jar probe (`jar.rs`; `Win32_Security_Cryptography` enables `Win32_Security` itself),
# and the CPU-time sample and local time (`all.rs`). The instance job is `manifold_view_protocol::job`.
[target.'cfg(windows)'.dependencies]
windows-sys = { workspace = true, features = [
    "Win32_Foundation",
    "Win32_Security_Cryptography",
    "Win32_System_Threading",
    "Win32_System_SystemInformation",
] }
```

- [ ] **Step 4: webview2 (P5).**

In `crates/manifold-view-webview2/Cargo.toml`, delete these three lines from the `windows` feature list:

```toml
    "Win32_System_JobObjects",
    "Win32_System_Threading",
    "Win32_Security",
```

- [ ] **Step 5: cef.**

In `crates/manifold-view-cef/Cargo.toml`, delete these lines and the blank line before the next comment:

```toml
[target.'cfg(windows)'.dependencies]
# Only for the browser-process Job Object setup in main.rs (kills orphaned
# CEF renderer/GPU subprocesses on exit) -- same crate/features
# manifold-view-webview2 already uses for the identical purpose.
windows = { workspace = true, features = ["Win32_Foundation", "Win32_Security", "Win32_System_JobObjects", "Win32_System_Threading"] }
```

so `manifold-view-protocol.workspace = true` is followed by one blank line and `# The exe keeps the name released builds, release.yml and the scripts know; it changes with a release that`.

- [ ] **Step 6: Check each crate.**

Run: `cargo check -j 8 -p manifold -p gate -p manifold-view-webview2 --all-targets`
Expected: `Finished`, no error. If `cargo check` names a removed feature for any crate (a missing `Win32::Security`, `Threading` or `JobObjects` item, or an unresolved `windows` import), restore exactly that feature line in that crate's Cargo.toml, re-run, and say so in the commit message (P5).
Run: `cargo clippy -j 8 -p manifold-view-cef --all-targets -- -D warnings` (skip if Task 5 Step 7 found CEF unbuildable here, and say so).
Expected: `Finished`, no warning.

- [ ] **Step 7: Format, lint, commit.**

Run: `cargo fmt --all`, the repo clippy line, `git diff --stat`.
Expected: clippy clean; diff lists `Cargo.lock` and the four `Cargo.toml` files only.

```bash
cd "/c/Users/Winston Li/Documents/GitHub/manifold"
git add Cargo.lock crates/manifold/Cargo.toml crates/gate/Cargo.toml crates/manifold-view-webview2/Cargo.toml crates/manifold-view-cef/Cargo.toml
git commit -F - <<'EOF'
deps: drop the job-only Windows features (CODE5)

By grep: Win32_System_JobObjects goes from manifold, gate and webview2; Win32_Security from manifold (it was for the
job struct), gate (implied by Win32_Security_Cryptography) and webview2; Win32_System_Threading from webview2;
cef's windows dependency goes (its only use was the job). manifold and gate keep Threading (jar, telemetry, all.rs).
Plan 2026-10-31-shared-host-code.md Task 7.

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>
EOF
```

---

### Task 8: docs, the gate and the kill check

**Files:**
- Modify: `docs/superpowers/plans/2026-10-30-cleanup.md` (lines 20, 42, 44)
- Modify: `docs/superpowers/specs/2026-10-31-shared-host-code-design.md` (front matter)
- Modify: `docs/superpowers/plans/2026-10-31-shared-host-code.md` (this file: front matter, "Started at")
- Modify: `docs/runs.md` (rows written by `run.ps1`; headline edits)
- README.md, CLAUDE.md, PARKED.md: the spec lists no edits to them; Step 7 checks that none went stale.

**Interfaces:** none.

- [ ] **Step 1: Build everything the gate and the kill check run.**

Run: `df -h /c | tail -1` (at least ~5 GB free; otherwise stop and report to the user, deleting nothing without asking; the CEF build and the gate's run are the heavy steps).
Run: `cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate -p manifold-view-cef` (CEF left out if Task 5 found it unbuildable here).
Expected: `Finished`.
Run: `target/debug/gate.exe sync-runtime-deps`
Expected: it copies the new `manifold-view-cef.exe` (and CEF's runtime if needed) into `target/debug/cef-runtime/`, and Servo's DLLs beside `target-servo/debug/manifold-view-servo.exe`, with no error. (Without it the gate's CEF stages refuse a cef-runtime host older than the built one, as in run 2026-10-30-download-broken-cef-probe.)

- [ ] **Step 2: The tier.**

Run: `target/debug/gate.exe tier <B>` (B from Task 1 Step 1).
Expected: one line per changed path, strictest first, ending `TIER full`: `crates/manifold/src/process_manager/job.rs` is core (`process_manager` is in `CORE_MODULES`, `crates/gate/src/tier.rs`). Use the tier it prints.

- [ ] **Step 3: The gate, once (detached, BelowNormal, recorded).**

Check first: `Get-Process python -EA 0` (no leftovers of yours) and no row in `docs/runs.md` without an `end` whose wrapper pid is alive (RUNS.md "Finding and closing").
`WITH_ENGINES` is supported (`crates/gate/src/all.rs` module doc: `WITH_ENGINES="servo webkit cef"` adds an engine's steps; `cef` adds `download-cef` and needs the CEF host built and synced, which Step 1 did). Set `WITH_ENGINES=cef` in the launcher's environment so the CEF host's download path runs under the new job and `NavState` code; do not add `servo` (needs `target-servo` with its DLLs) or `webkit`. Skip `WITH_ENGINES` only if CEF was unbuildable in Task 5, and say so.
Run (from bash, repo root; `TIER` is the tier word Step 2's `gate tier` printed on its `TIER` line, `full`, `light` or `ui`):
```bash
TIER=full   # replace with the word from `target/debug/gate.exe tier <B>`
WITH_ENGINES=cef powershell -NoProfile -File "$USERPROFILE/.claude/runs/run.ps1" -Kind gate -Name shared-host-code-gate -Detach -Command "cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate && target\\debug\\gate.exe all \"%RUN_DIR%\\gate-log.txt\" $TIER"
```
(The command is double-quoted so bash expands `$TIER`; the log path keeps plain `"` around it, as CLAUDE.md "Gate" requires. If the quoting reaches the gate wrong, the error is "mode must be full, light or ui": fall back to the single-quoted form with the tier typed in.)
Expected: it prints the run id and returns. Arm a Monitor (an until-loop with a deadline of 40 min, checking that `run.json` in `%LOCALAPPDATA%\claude-runs\manifold\<run id>\` has an `end`) rather than polling by hand; the last full gate took about 10 min (run 2026-10-17-full). Expected result: exit 0 and the log's last lines report every stage passed. Then set the row's headline in `docs/runs.md` (and `run.json`) from the last lines of `gate-log.txt`, with the CPU load and Defender state the wrapper recorded. A failing stage: read its log tail, decide whether it is this change (fix in a new commit and rerun the gate) or contention (say who was on the machine), and report it; never retry silently.

The closing report must say plainly that the Servo host's flush (`take_message` in `manifold-view-servo`) was only compiled (or only read, if Task 6's build was skipped), never run: the gate does not include Servo here. Say it again next to the Servo line of the cleanup-plan outcome.

- [ ] **Step 3b: The jar stage (hosts and the jar pull still work) and the exit-message wording (P1).**

`gate jar` is not a step of `gate all` (`crates/gate/src/main.rs` lists it as its own stage), so run it separately. It does NOT verify P2: it runs InPrivate tabs and asserts that a reused profile has no cookies (`crates/gate/src/jar.rs`, `login_runs`); the sign-in reaches the jar file through the app's own pull, not through the engine profile. It only shows that the jar pull and both hosts (cef, webview2) still work under the new job and `NavState` code. Run it detached through the wrapper (from bash, repo root; default `RUNS` is all), and arm the same `run.json` watcher as Task 6 Step 3 (deadline 15 min):
```bash
powershell -NoProfile -File "$USERPROFILE/.claude/runs/run.ps1" -Kind probe -Name shared-host-code-jar -Detach -Command 'target\debug\gate.exe jar'
```
Expected: it prints the run id; then exit 0 and the `login`, `fresh`, `cef`, `webview2` cases pass. A failure is a finding: read the log tail, decide whether it is this change, and report (if the new job tail is the cause, the fallback is `std::mem::forget(job)` in both hosts (P2), a spec change for the owner).

P2 tail check (by reading, since no stage tests it): `git grep -n -E "controller\.Close\(\)|_job|job::" -- crates/manifold-view-webview2/src/main.rs` and `git grep -n -E "cef::shutdown|_job|job::" -- crates/manifold-view-cef/src/main.rs`. Expected: in WebView2 the job local is the first statement of `main` and `controller.Close()` comes after it; in CEF `cef::shutdown()` runs before the `return` that ends `main`, after the job was created. Say in the closing report that the engine-profile-write residual risk is untested and the owner's to accept (P2).

Wording check, as P1 did for "not assigned": `git grep -n -E "exited with code|before connecting|not assigned" -- crates/gate/src` must print nothing after Task 3 (at 7e6010d5 its only hit was the old "instance N not assigned" line in `instance.rs`, which Task 3 replaced; the new text lives in the protocol crate). At 7e6010d5 no gate stage asserted "exited with code" or "before connecting": the only asserts are unit tests in `crates/manifold/src/process_manager/tests.rs` that use fake hosts (Task 3 Step 4 ran them). If the grep finds a gate stage that asserts either phrase against a real host, run that stage on its own now (its `gate <stage>` command, through `run.ps1 -Kind probe`) and report the result.

- [ ] **Step 4: The kill check (the property the job exists for, spec "Tests").**

Write this script to `killcheck.ps1` in the session's scratchpad directory (call that folder SCRATCH; its 8.3 path has no space):

```powershell
# Kill manifold (headless) with a live view tab and see that no host or engine process survives it.
$ErrorActionPreference = 'Stop'
$exe = 'C:\Users\Winston Li\Documents\GitHub\manifold\target\debug\manifold.exe'
$base = Join-Path $env:RUN_DIR 'killcheck'
function Get-Tree([int]$root) {
    $all = @(Get-CimInstance Win32_Process)
    $ids = [System.Collections.Generic.List[int]]::new(); $ids.Add($root)
    for ($i = 0; $i -lt $ids.Count; $i++) {
        foreach ($p in $all) { if ($p.ParentProcessId -eq $ids[$i] -and -not $ids.Contains([int]$p.ProcessId)) { $ids.Add([int]$p.ProcessId) } }
    }
    $ids
}
$failed = $false
# The CEF cases need the runtime the gate itself passes (jar.rs, shots.rs): forward slashes inside the JSON.
$cefDir = 'C:/Users/Winston Li/Documents/GitHub/manifold/target/debug/cef-runtime'
foreach ($case in ('webview2', 'manifold'), ('cef', 'manifold'), ('webview2', 'host'), ('cef', 'host')) {
  $app = $null; $tree = @(); $doomed = @(); $alive = @()
  try {
    $kind = $case[0]; $victim = $case[1]
    $label = "$kind/$victim"
    $profile = Join-Path $base "$kind-$victim"
    New-Item -ItemType Directory -Force $profile | Out-Null
    $more = ''
    if ($kind -eq 'cef') { $more = ', "cef_directory": "' + $cefDir + '"' }
    # No BOM: serde_json rejects one.
    [IO.File]::WriteAllText((Join-Path $profile 'settings.json'), '{"has_shown_first_launch_demo": true, "default_starting_view_kind": "' + $kind + '"' + $more + '}')
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $exe
    $psi.Arguments = '--headless --profile-dir "' + $profile + '"'
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.RedirectStandardInput = $true
    $app = [System.Diagnostics.Process]::Start($psi)
    $app.PriorityClass = 'BelowNormal'
    foreach ($line in 'WAITTABS', 'NEWTAB', 'WAITTABS') { $app.StandardInput.WriteLine($line) }
    $app.StandardInput.Flush()
    # Wait for manifold -> host -> at least one engine process (msedgewebview2.exe or a CEF subprocess).
    $deadline = (Get-Date).AddSeconds(120)
    do {
        $tree = @(Get-Tree $app.Id)
        $hosts = @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$($app.Id)" | Where-Object Name -like 'manifold-view-*')
        if ($hosts.Count -gt 0 -and $tree.Count -ge 3) { break }
        Start-Sleep -Milliseconds 500
    } while ((Get-Date) -lt $deadline)
    if ($hosts.Count -eq 0 -or $tree.Count -lt 3) {
        "${label}: FAIL no host with an engine process within 120 s (tree: $($tree -join ','))"
        $failed = $true; continue
    }
    $names = $tree | ForEach-Object { (Get-Process -Id $_ -EA 0).ProcessName } | Sort-Object -Unique
    "${label}: tree of $($tree.Count) processes before the kill: $($names -join ', ')"
    if ($victim -eq 'manifold') {
        # Everything must die. This case alone cannot fail because of `assign_self`: manifold's own job kills the
        # host (a direct child) and, with it, whatever the host started.
        $doomed = $tree
        Stop-Process -Id $app.Id -Force -EA 0
    } else {
        # Force-kill only the HOST: its subprocesses (msedgewebview2, CEF renderer/GPU) must die with it. Only the
        # host's own job (`assign_self`) can do that; manifold stays up (it may start a replacement host, which is
        # not in `$doomed`).
        $hostId = [int]$hosts[0].ProcessId
        $doomed = @(Get-Tree $hostId)
        if ($doomed.Count -lt 2) { "${label}: FAIL the host has no subprocess to check"; $failed = $true; continue }
        Stop-Process -Id $hostId -Force -EA 0
    }
    $deadline = (Get-Date).AddSeconds(15)
    do {
        $alive = @($doomed | Where-Object { Get-Process -Id $_ -EA 0 })
        if ($alive.Count -eq 0) { break }
        Start-Sleep -Milliseconds 250
    } while ((Get-Date) -lt $deadline)
    if ($alive.Count -eq 0) { "${label}: PASS no killed process or subprocess survived" }
    else {
        "${label}: FAIL survivors after 15 s: " + (($alive | ForEach-Object { "$_ $((Get-Process -Id $_ -EA 0).ProcessName)" }) -join ', ')
        $alive | ForEach-Object { Stop-Process -Id $_ -Force -EA 0 }; $failed = $true
    }
  } finally {
    # Whatever happened above (a throw, a `continue`, a FAIL), leave nothing this case started: the app, and every
    # process of its tree and of the doomed set that is still alive. Only processes found under $app are touched.
    if ($app) {
        $left = @(@($tree) + @($doomed) + @(Get-Tree $app.Id) | Sort-Object -Unique)
        Stop-Process -Id $app.Id -Force -ErrorAction SilentlyContinue
        foreach ($id in $left) { Stop-Process -Id $id -Force -ErrorAction SilentlyContinue }
    }
  }
}
if ($failed) { exit 1 }
```

Run (from bash; replace SCRATCH with the scratchpad path):
```bash
powershell -NoProfile -File "$USERPROFILE/.claude/runs/run.ps1" -Kind probe -Name shared-host-code-killcheck -Detach -Command 'powershell -NoProfile -ExecutionPolicy Bypass -File SCRATCH\killcheck.ps1'
```
It prints the run id and returns. Arm the same `run.json` watcher as Task 6 Step 3 (`RJ` = this run's `run.json`, deadline 15 min), then read `out.log` in the run folder. Expected (about 2-4 min): for each of `webview2/manifold`, `cef/manifold`, `webview2/host`, `cef/host`, a line `<case>: tree of N processes before the kill: manifold, manifold-view-webview2, msedgewebview2` (`manifold, manifold-view-cef` for cef) then `<case>: PASS no killed process or subprocess survived`, exit 0; the wrapper records the row.

Read the cases for what they prove. The `/manifold` cases (force-kill manifold) alone cannot fail because of `assign_self`: manifold's own job kills the host, and Windows then ends whatever sat in the host's tree whether or not the host joined a job of its own. The `/host` cases (force-kill the HOST process only, manifold stays up) are the ones that exercise the hosts' `assign_self`: the host's subprocesses (msedgewebview2, CEF children) can only die there through the host's job. A FAIL is a finding: stop and report it with the survivors' names (it means a job layer did not hold). If CEF was not buildable here, drop the two `'cef'` cases from the loop and say so.

- [ ] **Step 5: Outcomes in the cleanup plan.**

Let C1 be the Task 1 commit and C7 the Task 7 commit (`git log --oneline <B>..HEAD`). In `docs/superpowers/plans/2026-10-30-cleanup.md`:

After the CODE5 paragraph (the line starting `**CODE5. Kill-on-exit job-object setup copied**`), add the line

```markdown
  **Outcome:** Done 2026-10-31 (<C1>..<C7>): one `manifold_view_protocol::job` (`kill_on_close`, `Job::assign_child`, `Job::assign_self`) for the cef and webview2 hosts, manifold and gate; job-only features dropped. Gate run <gate run id> <exit/headline>; kill check <killcheck run id>: no host or engine process outlived a force-killed manifold, and no engine subprocess outlived a force-killed host (webview2, cef).
```

After the CODE6 paragraph (the line starting `**CODE6. `NavState` struct, flush loop and shared-cell aliases copied**`), add the line

```markdown
  **Outcome:** Done 2026-10-31 (<C1>..<C7>): one `manifold_view_protocol::nav_state` (`NavState::new`, `take_message`, `SharedNavState`, `SharedJsonBody`, `SharedRedirectContentType`) for the cef, webview2 and servo hosts; `had_recent_input` is `NavLabel::had_recent_input`. Servo <built in target-servo | verified by reading>.
```

In decision c1 (line 20), replace

```markdown
- **c1** Shared host code: CODE5 and CODE6 move into a shared home. **f1** (user): the home is `manifold-view-protocol`, with a `#[cfg(windows)]` module and a `windows` dependency.
```

with

```markdown
- **c1** Shared host code: CODE5 and CODE6 move into a shared home. **f1** (user): the home is `manifold-view-protocol`, with a `#[cfg(windows)]` module and a `windows` dependency; **a1** (user, spec 2026-10-31): that dependency is `windows-sys`.
```

Fill in every `<...>` with the real value (the run ids from `docs/runs.md`, the commit ids, one of the Servo alternatives).

- [ ] **Step 6: Statuses.**

In `docs/superpowers/specs/2026-10-31-shared-host-code-design.md`, change the front matter `status: draft` to `status: done`. In this plan, change `status: draft` to `status: done` (leftovers, if any, go to `PARKED.md` first, per VERSIONING.md; this plan expects none, P8).

- [ ] **Step 7: Docs check (old wording).**

Run (from bash, repo root):
```bash
git grep -n -E "setup_kill_on_exit_job|ViewJobHandle|JobHandle|view_job\(\)|nav_handler'?s? .?NavState|NavState::had_recent_input|SharedNavState. \+ .dirty|Win32_System_JobObjects" -- README.md CLAUDE.md PARKED.md ROADMAP.md docs/runs.md crates/
```
Expected: only README dev-log lines (dated history, describing the code as it was then), which stay as history. Any hit in `crates/` (a comment) or in a present-tense README section is stale: fix it in this task. Older plans and specs under `docs/superpowers/` are history and are not edited (the cleanup plan is the one live doc, Step 5).
Run: `git grep -n -E "kill-on-close|Job Object" -- README.md`
Expected: every hit read and judged: dated dev-log lines stay as history; a present-tense section that still describes the deleted per-crate setup is stale and fixed here. Say in the closing report which hits you judged and how.

- [ ] **Step 8: Commit.**

Run: `git diff --stat` and `git status --short`
Expected: this plan and the spec were committed in Task 1 Step 1 (the spec in 2ea11006), so they appear in the diff stat as modified (the `status: done` flips and the ticked boxes). The diff stat lists `docs/runs.md` (the run rows' headlines), `docs/superpowers/plans/2026-10-30-cleanup.md`, `docs/superpowers/plans/2026-10-31-shared-host-code.md`, `docs/superpowers/specs/2026-10-31-shared-host-code-design.md`, plus every `crates/` file whose stale comment Step 7 fixed (list each one in the `git add` below). Any other path: stop and find out why.

```bash
cd "/c/Users/Winston Li/Documents/GitHub/manifold"
git add docs/runs.md docs/superpowers/plans/2026-10-30-cleanup.md docs/superpowers/plans/2026-10-31-shared-host-code.md docs/superpowers/specs/2026-10-31-shared-host-code-design.md <each crates/ file fixed in Step 7>
git commit -F - <<'EOF'
docs: CODE5 and CODE6 done; shared host code spec and plan done

Cleanup plan outcomes with the commit range, the gate run and the kill check; decision c1 names windows-sys (a1).
Plan 2026-10-31-shared-host-code.md Task 8.

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>
EOF
```

- [ ] **Step 9: Leftovers.**

Run: `Get-Process python, cargo, manifold, manifold-view-*, msedgewebview2, PING -EA 0 | Select Id, ProcessName, StartTime`
Expected: nothing this plan started (another session's processes are theirs: leave them). Delete `SCRATCH\killcheck.ps1`. No push, tag or bump.
