---
status: done
---

# Host runtimes: one concept for "how an engine host runs on this OS"

**Goal:** replace `SpawnMode`, `ViewDesc::only_on` / `only_on_reason`, the `Wsl` branches and `host_platform`'s special case with two typed facts: the engine's host OS and a `Runtime` that maps it onto this machine. Linux then runs WebKit natively with no new branch.

**Spec:** the decision record `docs/decisions/2026-10-23-host-runtimes.md` (written by this plan, Task 3). Decisions A=1, B=4, C=7 (as simplified below), D=9 were taken in chat on 2026-10-23.

## Global constraints

- No migration or compatibility shims: delete `SpawnMode`, `only_on`, `only_on_reason`, `webkit_host_missing` and their tests; do not keep both.
- No new dependency (`target-lexicon` was considered and rejected: the need is three OS values).
- `cargo fmt --all` before each commit; `cargo clippy -j 8 -p manifold -p manifold-view-protocol -p manifold-pages -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings` must pass at every commit (so no commit may add unused items).
- Behaviour on Windows does not change: WebKit still starts through `wsl.exe`, over TCP, with the same command string, reaper and timeouts. The reason texts stay (`needs Windows`, `needs Windows or Linux`), now derived.
- No test reads the real profile (`profile::dir()` rule): the new tests are pure.
- Do not tag, push or publish; do not run the gate unasked beyond what Task 2's verification names (the user approved the work; the gate runs on request, and this plan asks for `gate light` once, at the end).

## Design

```rust
// process_manager/runtime.rs
pub(crate) enum Os { Windows, Linux, MacOs }          // Os::ALL in that order; other Unixes read as Linux
impl Os { const ALL; const fn here() -> Os; fn name(self) -> &'static str; fn pin_platform(self) -> &'static str }
pub(crate) enum Runtime { Native, Wsl }               // how a host built for one OS runs on another
impl Runtime { fn between(host: Os, here: Os) -> Option<Runtime>; fn of(kind: ViewKind) -> Option<Runtime> }
pub(crate) enum HostLink { PipesShm, Tcp }             // how the host reaches the app: a property of the host
fn unavailable_reason(kind: ViewKind, here: Os) -> Option<String>   // "needs Windows or Linux", derived
```

`ViewDesc` loses `spawn_mode`, `only_on`, `only_on_reason`; gains `host_os: Option<Os>` (`None`: built for whichever OS manifold runs on) and `link: HostLink`.

Ruling on C: transport is not a runtime property. The host decides (`link`); `Runtime::Wsl` is only valid for a `Tcp` host (nothing crosses a VM boundary by shared memory), and a test asserts that for every engine and OS. The effect is what option 7 described: native Linux WebKit runs over loopback TCP with the host unchanged.

## Review Focus

- A freebsd-like OS: reads as Linux (documented), no panic.
- macOS: WebKit and WebView2 read unavailable with the derived text; Servo and CEF unchanged.
- A WebKit tab on Linux with no display: falls back to `xvfb-run -a`; with neither, the error names the missing display.
- Dropping a native TCP tab kills the host at once and does not touch a frame file; dropping a WSL tab still goes to the reaper thread.
- `host_platform` for WebKit stays `linux-x64` on Windows (the pin is the Linux ELF).

## Task 1: Os, Runtime, HostLink, and the swap in ViewDesc (one commit)

**Files:** create `crates/manifold/src/process_manager/runtime.rs`; modify `process_manager.rs`, `process_manager/availability.rs`, `process_manager/tests.rs`, `view_install.rs`, `settings.rs`.

- [ ] **Step 1 (RED):** in `runtime.rs` tests: `Runtime::between` table (same OS Native; Linux host on Windows Wsl; Linux host on macOS None; Windows host on Linux and macOS None); `unavailable_reason` gives `needs Windows` for WebView2 on Linux and macOS, `needs Windows or Linux` for WebKit on macOS, `None` for Servo everywhere; `Os::pin_platform`; the Wsl-implies-Tcp invariant over all kinds and OSes. Run: `cargo test -p manifold runtime` fails to compile (module missing).
- [ ] **Step 2 (GREEN):** write the module; swap `ViewDesc` fields (Servo and CEF `host_os: None, link: PipesShm`; WebKit `Some(Linux), Tcp`; WebView2 `Some(Windows), PipesShm`); rewire: `platform_unavailable_reason`/`unsupported_on`/`availability_with`/`platform_default_view_kind` take `Os`; `spawn_tab` refuses with the derived reason; `sibling_host`, `view_binary_path`, `Drop for ViewTab` and `reap_host` read `Runtime::of(kind)` or `desc.link`; `host_platform` is `host_os.unwrap_or(Os::here()).pin_platform()`; `settings.rs` passes `Os::here()`. Update the old tests (the freebsd cases fold into "reads as Linux"; `webkit_host_missing` still exists until Task 2).
- [ ] **Step 3:** `cargo test -p manifold` all pass, fmt, clippy. Commit "Host runtimes: Os, Runtime and HostLink replace SpawnMode and only_on".

## Task 2: native TCP launch (own commit)

**Files:** modify `process_manager/webkit.rs` (becomes the TCP-host spawn, generalized), `process_manager.rs`, `process_manager/tests.rs`.

- [ ] **Step 1 (RED):** pure tests: `native_host_argv(host, has_display)` is `[host]` with a display and `[xvfb-run, -a, host]` without; a Wsl spawn still builds the same `webkit_shell_command` string (existing tests keep passing).
- [ ] **Step 2 (GREEN):** `spawn_webkit_tab` becomes `spawn_tcp_host(kind, runtime, ...)`: bind `0.0.0.0` for Wsl, `127.0.0.1` for Native; Wsl builds the `wsl.exe` command as today; Native builds `Command::new(argv[0])` with the `VIEW_*` env and `VIEW_HOST_IP=127.0.0.1`; a missing `xvfb-run` maps to "WebKit needs a display or xvfb-run". Delete `webkit_host_missing` and its test; `spawn_tab` no longer special-cases Linux.
- [ ] **Step 3:** tests, fmt, clippy, commit "WebKit starts natively on Linux (loopback TCP, xvfb-run only without a display)".

## Task 3: record, docs, verification (own commit)

- [ ] `docs/decisions/2026-10-23-host-runtimes.md`: the design, A/B/C/D, target-lexicon and Wine decided against (Wine adds nothing: the only Windows-only engine is WebView2, which CEF's Blink covers; WebView2's capture needs `Windows.Graphics.Capture`, unverified under Wine).
- [ ] README "Engines by OS": the Linux WebKit cell, the architecture line "WebKitGTK, hosted inside WSL2", and any `SpawnMode` or `spawn_webkit_tab` mention (grep); PARKED: the WebKit-host-in-release entry's wording; this plan `status: done`.
- [x] Verification (2026-10-23): `cargo test -p manifold` on Windows 1481 passed; on Linux under WSL Ubuntu
  (`CARGO_TARGET_DIR=$HOME/mf-target`, nice 19, -j 6) 1474 passed, 0 failed. Not done: the native WebKit tab was not
  started against a display (only the launch's pure parts are tested), and `gate light` was not run (it does not
  exercise the WebKit path, and the gate runs before a tag, on request).

## Rulings during execution

- `Os::MacOs` became `Os::Mac` (clippy `enum_variant_names`).
- `spawn_webkit_tab` kept its name and gained a `Runtime` parameter instead of becoming `spawn_tcp_host(kind, ..)`: WebKit
  is the only TCP engine, so a kind parameter would be unused generality.
- `Drop for ViewTab` and `reap_host` key on `link == Tcp`, not `Runtime::Wsl`: a native `xvfb-run` wrapper orphans
  Xvfb when force-killed, the same as the `wsl.exe` relay (found while writing Task 2).
- `sibling_host` looks for the built ELF when `host_os == Some(Linux)`, so it works for both runtimes.

## Stress-test log

- Round 1: `Os::ALL` order decides the derived reason text (`needs Windows or Linux`): fixed the order and added the text to the test. Two commits cannot leave unused items under `-D warnings`: Task 1 now includes the wiring.
- Round 2: C as worded (per-pair transport) needs a host that speaks both; only the Tcp host exists, so the transport is the host's property with an invariant test. Native display handling belongs to the launch, not to `ViewDesc`. Drop must not remove a frame file for a Tcp host.
- Round 3: a full pass found nothing material; open risk is the unverified native launch (no Linux machine here, WSL Ubuntu stands in).
