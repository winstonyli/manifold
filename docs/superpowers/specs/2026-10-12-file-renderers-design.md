# File renderers as separate processes (design)

Status: draft for approval, 2026-10-12. Supersedes the review item D-R9 ("PDF rendering in a child process"), which this design folds in.

## Problem

PDFium is native code that runs on a thread inside `manifold.exe` (`pdf_render.rs`). A malformed PDF that crashes it takes down the whole app and every open tab. The engines already solved this shape: each runs in its own host crate and process, and a crash there is a failed card, not a dead app. File renderers should work the same way.

## Goals

- A crash, hang or abort in a native renderer never reaches `manifold.exe`; the node shows an error card with a reason.
- A renderer is its own crate and binary, built and placed like an engine host, so native dependencies (PDFium and its download) leave `manifold`.
- Adding a format later is a new crate plus one registry entry, not a change to the node, card or session code.

## Non-goals

- Moving image decode (`ImageDecoder`) out of process. The `image` crate is memory-safe Rust, so a bad file is an `Err`, not a native crash. See "Parked".
- Changing what a PDF node shows (first page, 1000 px cap) or adding page navigation.
- Reusing the engine control channel. Engines are long-lived, interactive and stream frames; a renderer is a one-shot function from a file to a bitmap.

## Design

### Contract: one process per render

`manifold-render-pdf <input-path>` renders and exits. The result goes to stdout; diagnostics to stderr.

stdout, on success, is one frame:

| field | type |
|---|---|
| magic `MRND` | 4 bytes |
| `RENDER_PROTOCOL_VERSION` | u32 LE |
| width, height | u32 LE each |
| page count | u32 LE |
| RGBA bytes | `width * height * 4` |

On failure it writes nothing to stdout, one line to stderr (the user-facing reason), and exits 1. A crash is any other exit (a signal or `STATUS_ACCESS_VIOLATION`), reported as "PDF renderer crashed".

The frame header, its encode/decode and the version constant live in `ipc-protocol` (module `render`), beside the engine messages, so both sides share one definition and one wire-shape test. The version has its own constant: bumping it never forces an engine rebuild.

A format is identified by `ContentKind` (`content.rs`), so the registry is a function `renderer_for(ContentKind) -> Option<RendererSpec { crate_binary, timeout }>` with one entry (`Pdf`).

### Host side: `PdfRenderer` keeps its API

`request(id, path)`, `cancel(id)` and `tick()` stay as they are, so `event_loop.rs`, `fetched.rs`, `node_store.rs` and `session_sync.rs` do not change. Inside:

- Two worker threads (as `ImageDecoder`) take requests from one queue. Each spawns the renderer found by `sibling_binary(name)` (the beside-`manifold.exe` lookup in `process_manager/availability.rs` that `sibling_host` is built on; `sibling_host` itself is keyed by `EngineKind`, so it is not reused), assigns it to the existing kill-on-close job object so a dead manifold never leaves an orphan, and reads stdout on the worker thread until EOF.
- `cancel(id)` for a render already running kills that child (the running children sit in a shared `id -> Child` map the workers fill and clear); one still queued is skipped, as today. A result that arrives despite the cancel is dropped by `tick_fetchers`, which already ignores ids with no PDF node.
- The run function, `run_renderer(command, timeout) -> Result<Frame, String>`, takes the `Command`. Tests drive it with stand-in commands.
- `assign_to_engine_job` and `sibling_binary` are `pub(super)` to `process_manager`; the spawn-and-read function lives in a new `process_manager/renderer.rs` beside them (it needs no new visibility), and `pdf_render.rs` shrinks to the queue, cancel set and `tick`.
- Windows: `CREATE_NO_WINDOW` through `std::os::windows::process::CommandExt`, so no console flashes (no new `windows-sys` features).

### Failure mapping (one place, `run_renderer`)

| Situation | Reason shown |
|---|---|
| exit 1 with a stderr line | that line (e.g. the existing one-line PDFium error) |
| any other non-zero exit or signal | "PDF renderer crashed" |
| no exit within the timeout, child killed | "PDF renderer timed out" |
| binary not found next to `manifold.exe` | "PDF renderer is not installed" plus the expected path |
| stdout is not a valid frame (bad magic, short read, size mismatch) | "PDF renderer returned a bad frame" |
| version mismatch | "render protocol mismatch: rebuild manifold-render-pdf" |

All are `Err(String)`, which `tick_fetchers` already turns into `PdfPayload.outcome = Some(Err(..))` and a toast. A failed render is retried by the existing card click.

### Timeout

One constant, 120 s, covering the first-run PDFium download (`pdfium-auto`, a one-time network fetch) as well as the render. A hung PDF therefore costs a worker for up to two minutes; the second worker keeps other PDFs moving. The download is rare and the hang case is the one the process boundary exists to survive.

### Crate and build

- New `crates/manifold-render-pdf` (binary only). It takes over `pdfium-render`, `pdfium-auto` and `render_first_page`, `bind_once` and `pdfium_error` from `pdf_render.rs`. `manifold` drops both dependencies.
- Workspace member, built with `-p manifold-render-pdf`. Added to the documented build line (`CLAUDE.md` "Gate" and the `scripts/gate.sh` header; the script itself does not build) and to the clippy package list in `CLAUDE.md` and `.githooks/pre-commit`.
- Runtime placement: like `manifold-engine-webview2`, it needs nothing beyond the exe (PDFium's DLL is fetched into the user cache at first use, as now).

## Stress test of this draft (round 1)

1. *Pipe deadlock*: a child blocks writing a 4 MB frame if the parent waits for exit first. The worker reads to EOF before `wait`, and the timeout kills from a watchdog thread. Covered.
2. *First-run download inside the timeout*: a slow connection could exceed 120 s and be reported as a hang. Accepted: the retry via card click resumes where the cache left off. Recorded as a known limit, not hidden.
3. *Smoke coverage is thin*: `gate smoke` adds a fake `%PDF-1.4\n` file, which exercises only the error path. The success path needs a real one-page PDF. Plan: a tiny valid PDF committed as a test fixture, used by one `manifold` test that is skipped (with a printed reason) when the renderer binary is not built, and by one gate smoke step that is not skippable because the gate builds it.
4. *Orphans*: killed on cancel, killed by the job object at exit, killed by the watchdog on timeout. Test: after a timeout, the stand-in child is gone.
5. *Stale binary*: the frame carries a version, so an old renderer next to a new manifold yields the mismatch reason, not a misread bitmap.
6. *Packaging*: nothing in the repo packages releases today beyond the build outputs, so there is no installer to update; the README's runtime-placement section gets one line.

## Stress test (round 2: claims checked against the code)

Four corrections folded in above: the host lookup is keyed by `EngineKind` (use `sibling_binary`); `assign_to_engine_job` is private to `process_manager` (spawn code goes there); `gate.sh` documents the build line but does not run it; the engine wire-shape fingerprint does not cover a new header. Round 3 re-read the draft against these and added only the running-child map under `cancel`.

## Testing

- `ipc-protocol`: frame header round trip, each malformed frame, and a layout test pinning the field order and sizes (the existing `wire_shape_tests` fingerprint covers only the engine messages).
- `run_renderer` with stand-in commands: success frame, exit 1 with a message, crash exit, timeout (short, injected), garbage output, short output.
- `manifold-render-pdf`: renders the fixture to the expected size; a garbage file exits 1 with one line.
- Gate: `light` tier (PDF smoke) plus the new real-PDF smoke step.

## Parked, with why

- **Image decode out of process:** memory-safe Rust, so no native crash to isolate. Separate smaller issue found while reading `ImageDecoder`: a panic inside a decode kills that worker thread and later requests could queue forever. Fix there with `catch_unwind` or by respawning; it does not need a process.
- **A long-lived renderer per format:** faster for many PDFs, but a bad file takes down the next render too and needs restart logic. Revisit only if a per-render spawn is measured to matter (expect tens of ms against a render of hundreds).
- **Other formats (Office, EPUB, video posters):** the registry makes them additive; none are requested.

## Open decisions for the user

1. Per-render process (recommended) or one long-lived renderer per format.
2. Crate name: `manifold-render-pdf` (recommended), alongside the `manifold-engine-*` hosts.
3. Timeout 120 s with one constant, or a longer first-run allowance.
