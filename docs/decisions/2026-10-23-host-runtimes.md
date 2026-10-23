# Host runtimes: Os, Runtime and HostLink (2026-10-23)

Plan: `docs/superpowers/plans/2026-10-23-host-runtimes.md`. Code: `crates/manifold/src/process_manager/runtime.rs`.

## Decision

An engine row (`ViewDesc`) states two facts instead of a spawn mode and an allow-list of OS strings:

- `host_os: Option<Os>`, the OS its host is built for (`None`: whichever OS manifold runs on, as for Servo and CEF).
- `link: HostLink`, how the host reaches manifold: `PipesShm` (named pipes and a file-backed frame) or `Tcp`.

`Runtime::between(host, here)` says how a host built for one OS runs on another: `Native` on the same OS, `Wsl` for a
Linux host on Windows, `None` otherwise. Availability, the "needs Windows or Linux" texts, the spawn refusal, the
host-pin platform id and the reaping rule all read these; none keeps its own OS table.

## Rulings

- **A. Typed `Os` on the row**, with the pin id (`windows-x64`, `linux-x64`, `macos-x64`) derived. `std::env::consts::OS`
  strings and `cfg!(windows)` no longer decide anything; any OS that is not Windows, Linux or macOS is `Os::Other`, which no host is built for, so only Servo and CEF run there.
- **B. README matrix hand-written**, with unit tests over every (OS, engine) pair (`runtime::tests`) so it cannot drift
  from the code unnoticed.
- **C. Transport is the host's property, not the runtime's.** The plan first said "per host-and-runtime pair". Only a
  TCP host exists that crosses the WSL boundary, and nothing crosses a VM by shared memory, so `Wsl` implies `Tcp` (a
  test asserts it) and the same TCP host runs natively over loopback. A host that speaks both would need a second link
  per pair; there is none.
- **D. A module of its own** (`process_manager/runtime.rs`), not more of the 1,100-line `process_manager.rs`.
- **Any TCP host is reaped gracefully** (`Drop for ViewTab`, `reap_host`): it gets time to exit on its own, because
  force-killing the wrapper (`wsl.exe`) skips its exit trap and orphans the tree. A native host has no wrapper and
  takes the same graceful path harmlessly.
- **No `xvfb-run` fallback for a native Linux host.** It was written for a session without a display, but manifold's
  own window (winit) panics without one, so the host always inherits a display. Deleted as unreachable (run with
  `DISPLAY` unset, 2026-10-23: manifold panicked before any tab). Only the WSL launch still uses `xvfb-run`.

## Decided against

- **`target-lexicon` crate.** It models target triples; the need here is three OS values and our own pin ids. It is not
  in the Windows dependency graph.
- **Wine as a `Runtime`.** It adds nothing: the only Windows-only engine is WebView2, and CEF covers Blink natively.
  WebView2's frame capture (`Windows.Graphics.Capture`) under Wine is unverified. If ever wanted, it is one `Runtime`
  variant and one arm of `Runtime::between`.

- **Reading FreeBSD and other Unixes as Linux.** First ruling (2026-10-23), reversed the same day for `Os::Other`:
  WebKit showed as available there although its Linux ELF would not run. `Other` costs one arm in `name` and
  `pin_platform`.

## Verified

2026-10-23, WSL Ubuntu: the Linux build ran headless with WebKit as the default engine; the host started directly
(`DISPLAY` set), connected over loopback TCP, loaded `manifold://start` and a `data:` page, and exited with no host left
behind. Windows and Linux unit tests pass.

## Not verified

A force-killed manifold with a native tab was not tried.
