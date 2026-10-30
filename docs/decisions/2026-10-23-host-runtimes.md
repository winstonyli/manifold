# Host runtimes: Os, HostRuntime and HostLink (2026-10-23)

Plan: `docs/superpowers/plans/2026-10-23-host-runtimes.md`. Code: `crates/manifold/src/process_manager/runtime.rs`; `Os`
has lived in `crates/manifold-hosts/src/lib.rs` since the host table (2026-10-29, spec
`docs/superpowers/specs/2026-10-29-host-table-design.md`, H11).

## Decision

An engine row (`ViewDesc`) states two facts instead of a spawn mode and an allow-list of OS strings:

- `host_os: Option<Os>`, the OS its host is built for (`None`: whichever OS manifold runs on, as for Servo and CEF).
  Amended 2026-10-29 (host table spec, H2): this fact is now the host's row, `Host.os` in `crates/manifold-hosts`,
  which `ViewDesc.host` points to; `Host::built_for` resolves `None`.
- `link: HostLink`, how the host reaches manifold: `PipesShm` (named pipes and a file-backed frame) or `Tcp`.

`HostRuntime::between(host, here)` says how a host built for one OS runs on another: `Native` on the same OS, `Wsl` for a
Linux host on Windows, `None` otherwise. Availability, the "needs Windows or Linux" texts, the spawn refusal, the
host-pin platform id and the reaping rule all read these; none keeps its own OS table.

## Rulings

- **A. Typed `Os` on the row**, with the pin id (`windows-x64`, `linux-x64`, `macos-x64`) derived. `std::env::consts::OS`
  strings and `cfg!(windows)` no longer decide anything; any OS that is not Windows, Linux or macOS is `Os::Other`, which no host is built for, so only Servo and CEF run there.
  Amended 2026-10-29 (host table spec, H11): `Os` moved, with its tests, to `crates/manifold-hosts`; the row that holds
  it is the host's (`Host.os`).
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
- **Wine as a `HostRuntime`.** It adds nothing: the only Windows-only engine is WebView2, and CEF covers Blink natively.
  WebView2's frame capture (`Windows.Graphics.Capture`) under Wine is unverified. If ever wanted, it is one `HostRuntime`
  variant and one arm of `HostRuntime::between`.

- **Reading FreeBSD and other Unixes as Linux.** First ruling (2026-10-23), reversed the same day for `Os::Other`:
  WebKit showed as available there although its Linux ELF would not run. `Other` costs one arm in `name` and
  `pin_platform`.

## Verified

2026-10-23, WSL Ubuntu: the Linux build ran headless with WebKit as the default engine; the host started directly
(`DISPLAY` set), connected over loopback TCP, loaded `manifold://start` and a `data:` page, and exited with no host left
behind. Windows and Linux unit tests pass.

## Not verified

A force-killed manifold with a native tab was not tried.

## Host tags (2026-10-23, same day)

The engine/host split is carried by tags, not a second kind. A host row (`ViewDesc`, keyed by `ViewKind`, which stays
the user's choice and the stored key) has `tags: TagSet` in place of the four bools `bgra`, `jar`, `devtools` and `find`:
`Bgra`, `Jars`, `DevTools`, `Find` and exactly one rendering-engine family (`Blink`, `WebKit`, `Servo`). A live tab
inherits its host's tags in `App::card_kind` (a sleeping tab has none), so a capability row can require them like it
requires `Loaded`. CEF and WebView2 share `Blink` while staying two hosts. The WebKit host crate and binary are
`manifold-view-webkitgtk`; the engine stays "WebKit" (`ViewKind::Webkit`, the stored key `webkit`), so a macOS WKWebView
host can be added beside it.

- **Hosts are a runtime registry, not stored cards.** They differ per machine; a stored card would sync and need a
  `FORMAT_VERSION` bump.
- **Tags carry classification only.** `binary`, `prepare`, `source`, `link`, `host_os`, `runtime_dir`, `color`, copy stay
  plain host data. No family "card" above the host. Amended 2026-10-29 (host table spec, H2): `binary`, `source`,
  `host_os` and `runtime_dir` are still plain data, in a table of their own: `ViewDesc.host` points to the host's row
  (`manifold_hosts::Host`: `exe`, `delivery`, `os`, `runtime`); `prepare`, `link`, `color` and copy stay on `ViewDesc`.
- **No capability rows yet.** `Caps` is card-shell abilities (select, drag, menu); the four readers hold a `ViewKind`, not
  a card, and read `ViewKind::has(tag)`. A row requiring `Find` or `DevTools` would be added with a consumer.
- **`TagSet` is `u32`** (was `u16`): 9 tags now.
- **Parked (macOS trigger):** resolving "WebKit" to a host per OS by a family query, and storing the query instead of the
  host key. See PARKED.md.
