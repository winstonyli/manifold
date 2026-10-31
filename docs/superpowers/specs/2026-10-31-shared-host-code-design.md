---
status: done
---

# Shared host code

Cleanup items CODE5 (job-object setup) and CODE6 (`NavState`, flush loop, shared aliases) of
`docs/superpowers/plans/2026-10-30-cleanup.md`. Decisions (user): the shared home is `manifold-view-protocol`
(f1); the job object is one `windows_sys` module that all four copies call (a1, "d2"); `NavState` is one struct with
`take_message()` and the shared aliases, used by the cef, webview2 and servo hosts (e1).

## Problem

**Job object.** Four copies create a job with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`:

| Copy | Binding | Shape |
|---|---|---|
| `manifold-view-cef/src/main.rs:44-65` (`setup_kill_on_exit_job`) | `windows` | creates, assigns the host's own process |
| `manifold-view-webview2/src/main.rs:528-548` | `windows` | the same body as cef's (cef's is `#[cfg(windows)]`, webview2's crate is Windows-only) |
| `manifold/src/process_manager/job.rs:42-107` | `windows_sys` | `OnceLock` job, `assign_to_view_job(child)`, logs "engine job: ..." |
| `gate/src/instance.rs:33-92` | `windows_sys` | the same as manifold's, logs "gate job: ..." |

The create-and-limit step is the same in all four; they differ in whom they assign and in the log prefix.

**NavState.** `manifold-view-cef/src/nav_handler.rs:13-46` and `manifold-view-webview2/src/nav_state.rs:23-59` define
the same struct (`url`, `title`, `can_go_back`, `can_go_forward`, `loading`, `label: NavLabel`, `dirty`), differing in
the initial `dirty` (cef `true`, webview2 `false`) and a webview2-only `had_recent_input`. `manifold-view-servo/src/main.rs:36-44`
has the same fields without `label`. The flush that turns a dirty `NavState` into `ViewToHost::NavigationStateChanged`
is written three times (cef `main.rs:427-443`, webview2 `main.rs:425-441`, servo `main.rs:336-350`); the cef and
webview2 loops are identical, servo's sends `user_started: None` and skips the `label.finished` reset.
`SharedJsonBody` and `SharedRedirectContentType` (with constructors) are defined in cef `client.rs:35-46` and in
webview2 `nav_state.rs:64-75`. `manifold-view-webkitgtk` (`main.rs:495-514`) compares a tuple instead and is left alone.

## Design

### Job object: `manifold_view_protocol::job` (`#[cfg(windows)]`)

```rust
pub struct Job(HANDLE);                       // `unsafe impl Send + Sync` (a kernel handle); Drop closes it
pub fn kill_on_close(label: &str) -> Option<Job>;
impl Job {
    pub fn assign_child(&self, child: &Child);
    pub fn assign_self(&self);
}
```

`kill_on_close(label)` creates the job and sets `KILL_ON_JOB_CLOSE`. A creation failure returns `None` after
`eprintln!("{label}: not created: {err}")`; a failed limit setup is said once (`{label}: kill-on-close not set: {err}`)
and the job is returned anyway, as manifold and gate do today ("a crash-safety net, not a feature a spawn should fail
over"). `assign_child` and `assign_self` say a failed assignment on stderr and carry on (manifold ignored it silently;
gate said it; the shared version says it).

- cef and webview2 hosts: `let _job = job::kill_on_close("view host").inspect(Job::assign_self);` replaces their
  `setup_kill_on_exit_job` and its call; cef's call keeps its `#[cfg(windows)]` (the cef host also builds on Linux).
  The old code dropped the `Err` silently; the new one prints it. Two deliberate differences: when the limit step
  fails the old code returned before assigning, the new one still joins the job (without kill-on-close); and the old
  host never closed the handle, while `Job` now closes it on drop, so `_job` must be the last local dropped in `main`
  (the host's own subprocesses die with it, after its other cleanup). `process::exit` paths skip drops, and the OS
  closes the handle then, as before.
- manifold: `static VIEW_JOB: OnceLock<Option<Job>>` over `job::kill_on_close("engine job")` (the prefix stays, it is what users see in logs); `ViewJobHandle` and its
  `unsafe impl Send/Sync` go (`Job` carries them once); `assign_to_view_job` calls `job.assign_child`. The non-Windows
  stub and its Linux comment stay.
- gate: the same `OnceLock<Option<Job>>` in `instance.rs` over `kill_on_close("gate job")`, behind its existing thin
  `assign(&child)` (call sites `all.rs:145`, `instance.rs:216`, `launch_forms.rs:50` stay); its own `JobHandle` goes.
  The assign failure message names the label and the child's pid, as gate's does now.
- `manifold-view-protocol/Cargo.toml`: `[target.'cfg(windows)'.dependencies] windows-sys` (workspace version 0.59) with
  `Win32_Foundation`, `Win32_Security`, `Win32_System_JobObjects`, `Win32_System_Threading`. It must be target-gated:
  the protocol crate is also built on Linux for the webkitgtk host. `lib.rs` gets `#[cfg(windows)] pub mod job;`.
- Features, by grep: `JobObjects` is used only by these four copies, so it is dropped from manifold's and gate's
  `windows-sys` features, from webview2's `windows` features, and cef's `windows` dependency goes entirely (its
  `windows::` use is only the job, `main.rs:45-50`). `Win32_Security` in manifold was added for the job struct (its
  Cargo.toml comment) and is dropped with that comment; `Win32_System_Threading` stays in manifold (`jar.rs:183`,
  `telemetry.rs:64`) and gate (`all.rs:39`); webview2 uses `Threading` only in the job fn, so it drops that too. The
  plan re-greps each before deleting.

### NavState: `manifold_view_protocol::nav_state` (std only, no cfg gate)

```rust
#[derive(Clone, Default)]
pub struct NavState { pub url: String, pub title: String, pub can_go_back: bool, pub can_go_forward: bool,
                      pub loading: bool, pub label: NavLabel, pub dirty: bool }
impl NavState {
    pub fn new(url: String, dirty: bool) -> Self;
    /// If dirty: clear it, and return the message with the label's reset applied.
    pub fn take_message(&mut self) -> Option<ViewToHost>;
}
pub type SharedNavState = Rc<RefCell<NavState>>;
pub fn new_shared_nav_state(url: String, dirty: bool) -> SharedNavState;
pub type SharedJsonBody = Rc<RefCell<Option<String>>>;           // + new_shared_json_body
pub type SharedRedirectContentType = Rc<RefCell<Option<String>>>; // + constructor
```

`take_message` returns `NavigationStateChanged { url, title, can_go_back, can_go_forward, loading, user_started:
label.user_started }` and then, if `std::mem::take(&mut label.finished)`, sets `label.user_started = None` (the
existing cef/webview2 behaviour). The constructor takes `dirty` so no host changes its start-up: cef passes `true`,
webview2 `false`. The three flush loops become `if let Some(m) = nav.borrow_mut().take_message() { let _ = control.send(&m); }`.
Servo keeps its `RefCell<NavState>` (no `Rc`) but uses the shared struct; its default `NavLabel` gives
`user_started: None` and `finished: false`, which is what its flush does today, so its behaviour is unchanged.
`had_recent_input` (webview2's, `nav_state.rs:42`, one caller `popup.rs:54`) becomes `NavLabel::had_recent_input(&self)` next to
`GESTURE_WINDOW` in `nav_label.rs`; the caller reads `s.label.had_recent_input()`. Servo builds its state with
`NavState { url, ..Default::default() }` as it does now (`dirty` false, default label); `NavState::new` is the
cef/webview2 constructor and sets the label as the host-created default those hosts use today. The cef-only aliases (`SharedPageState`, `SharedPageText`, and the like) stay in cef.
`NavLabel` already lives in the protocol crate (`nav_label.rs`); `take_message` needs no new dependency.
Files that change: cef `client.rs`, `main.rs`, `nav_handler.rs` (which lose their own definitions and import the
shared ones); webview2 `main.rs`, `popup.rs`, `nav_state.rs` (the same); servo `main.rs`. Stale comments edited with
them: webview2 `nav_state.rs:1-7` and `main.rs:3`, cef `life_span_handler.rs:8`, `nav_label.rs:1-3` (now also Servo's).

## Tests

- `take_message` (in `nav_state.rs`'s own test module; `ViewToHost` is compared field by field or the test derives
  what it needs): a clean state returns `None`; a dirty one returns the message and is then clean; with
  `label.finished` set, `user_started` is `None` afterwards; with `finished` clear, `user_started` is kept. (Unit test
  beside `nav_label.rs`'s.)
- `job` (Windows only): spawn a long `ping`, `assign_child`, drop the `Job`, and the child exits within a deadline
  (`try_wait` in a loop against an `Instant` deadline, killing the child if the deadline passes so no `ping` is left
  behind). A second test asserts `kill_on_close` returns `Some`; none calls `assign_self` (it would put the test
  runner in the job).
- Behavioural, once at the end of the change: build the hosts and run the gate tier that launches them (`ui`); kill
  manifold with a live view tab and see that no host survives (the property the job exists for).

## Out of scope

`manifold-view-webkitgtk` (tuple comparison), the cef-only aliases, and the host crates' other duplicated helpers.

## Open questions for the plan

- Whether any host calls the job setup on a non-main thread or before `main` runs its own setup that expects the old
  `windows::core::Result` (the plan greps the call sites at `cef/main.rs:134-135` and `webview2/main.rs:130`).
- Whether cef and webview2 use `windows` for anything the grep misses (build scripts, `cfg(test)`).
