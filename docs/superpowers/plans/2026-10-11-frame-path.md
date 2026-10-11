# Frame path (survey E2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (Tasks 2-6 are sequential and share one mechanism, so subagent-driven development buys little; if it is used, implementers and reviewers are Sonnet). Steps use checkbox (`- [ ]`) syntax for tracking. Task 1 is done (Results below): Tasks 2-5 go, Task 7 waits on Decision 3.

**Goal:** An engine frame costs only what changed. **Reframed after Task 1 (2026-10-11):** WebView2 and CEF already send nothing for a static page, so deduplication is not the win. The win is damage: a small change (a spinner) sends and uploads a whole 1080p frame today, 8.5 ms of app-thread CPU per frame. Task 2's sink exists to produce the damaged rows, Task 4 (band upload, 8.5 to 0.6 ms) is the main payoff, and the removed intermediate copy is worth 40-60 ms/s of one core while a page animates. Dedup stays as a free by-product (and WebKit/Servo may need it; Servo is unmeasured).
- An engine that produced the same pixels as last time sends nothing: no copy into the frame file, no `FrameReady`, no upload, and no redraw. A static page goes idle (already true for WebView2 and CEF; the sink keeps it true for every engine).
- An engine copies a frame once, from wherever its pixels already are (the WebView2 staging map, the CEF paint buffer) straight into the shared frame file. No intermediate `Vec`.
- An engine reports which rows changed (`FrameReady::damage`), and the app uploads only those rows to the GPU.
- The tab thumbnail is re-rendered only when it is drawn, inside the frame's own encoder, not with its own submit after every upload.

**Architecture:** one mechanism, shared by every engine and driven by one signal (the rows that changed):
- `ipc_protocol::FrameSink` (new, `crates/ipc-protocol/src/frame_sink.rs`) owns an engine's frame store: the shared frame file, or an inline buffer for WebKit over TCP. `offer(src, pitch, w, h)` compares each row against the store, copies only rows that differ, and records the span. `show()` marks the next send whole. `take()` returns the `FrameReady` to send, or `None` when nothing changed. The per-engine pre-create, recreate-on-resize, retry-on-failure and dedup code (four copies today) is deleted in favor of it.
- The app's reader thread unions the damage of frames that pile up in the tab's one-frame slot. `GpuState::upload_frame` writes only that band, unless the texture is new or marked stale.
- Thumbnails become dirty flags, flushed by `GpuState::render` for the thumbnails egui actually draws that frame.

**Tech Stack:** Rust (edition 2021, nightly-2026-09-23), wgpu 30.0.1, memmap2 0.9.11, windows 0.62.2, CEF (`cef` crate), Servo, WebKitGTK over WSL. No new dependencies.

**Source:**
- `.superpowers/survey/engines.md` E2 (lines 30-50); also E3 (52-65, done), E4 (67-94), E8 (143-163), E9 (165-174), E23 and E25 where they touch frame files.
- `docs/superpowers/plans/2026-10-10-wave-3b-engine-io.md`: wave 3b gave each tab a one-frame slot and parked E2 ("the copy and upload path is unchanged", line 166).
- The code, read 2026-10-11 at `674d6af`. Every survey claim below was re-checked against it.

**Global constraints (every task):**
- Python: never run `python` reading stdin (`python -`, heredocs). Write the script to a file and run `python -P f.py < /dev/null`.
- `cargo fmt --all` before each commit. Clippy: `cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 -p gate --all-targets -- -D warnings`. CEF and Servo get their own `cargo check` (Task 3 says how). Never `--no-verify`.
- No test reads or writes the real `%APPDATA%\manifold` (`profile::dir()` is `None` under `cfg(test)`). Frame files live in `%TEMP%`, which is fine.
- One cargo per target dir. Builds use `-j 8` (16-core machine; leave a quarter free).
- Timing runs follow `~/.claude/LONG_RUNS.md`: check GPU leases and who is on the eGPU, take an `exclusive` lease (at most 15 min) for GPU timings, require `\Processor(_Total)` under ~20%, record `(Get-MpComputerStatus).RealTimeProtectionEnabled`, report best-of-N if anything was shared.
- No fixed sleeps in scripts or tests; wait on an observable condition with a deadline.
- Headless only: `scripts/drive.sh` sessions. A visible-window run needs the user's go-ahead (Decision 5).
- Delete what the new mechanism supersedes. No compatibility shims.
- Implementers report every deviation from this plan.

---

## Evidence

All line numbers are at `674d6af`.

### The path today, per frame

| Step | Where | What it costs |
|---|---|---|
| WebView2: capture | `crates/manifold-engine-webview2/src/capture.rs:138-142` | `CopyResource` into a staging texture, then `Map(D3D11_MAP_READ)` right after it, which waits for the GPU copy to finish |
| WebView2: copy 1 | `capture.rs:148-157` | `vec![0u8; w*h*4]` (a fresh ~8 MB zeroed allocation at 1080p) and a row copy into it |
| WebView2: copy 2 | `crates/manifold-engine-webview2/src/main.rs:516` | `buf.write_frame(&pixels)` into the shared frame file |
| WebView2: rate | `main.rs:50-55`, `:495` | at most one frame per `FRAME_INTERVAL` (33 ms), only while visible |
| CEF: copy 1 | `crates/manifold-engine-cef/src/render_handler.rs:79` | `on_paint` copies the paint buffer with `to_vec()` into `PaintedFrame`, visible or not |
| CEF: copy 2 | `crates/manifold-engine-cef/src/main.rs:536` | `write_frame` into the frame file, only while visible |
| CEF: rate | `main.rs:204` | `windowless_frame_rate: 60`; nothing calls `was_hidden`, so a hidden animated tab keeps painting and copying (copy 1) |
| Servo | `crates/manifold-engine-servo/src/main.rs:366-382` | `read_to_image` (one GL readback into an image) then `write_frame` |
| WebKit | `crates/manifold-engine-webkit/src/main.rs:525-563`, `:574-600` | snapshot pixbuf, `tightly_packed_rgba` into a `Vec`, and the whole frame inline over TCP; **already skips unchanged frames** (`SentFrame`/`frame_due`, `:17-33`) |
| App: slot | `crates/manifold/src/process_manager.rs:615-618` | the reader puts `{width, height, pixels}` in the slot; `frame_id` and `damage` are dropped |
| App: map | `crates/manifold/src/event_loop.rs:1322-1324` | `SharedFrameBuffer::open_existing` per frame: open the file, map it, and fault its pages in again |
| App: upload | `crates/manifold/src/gpu.rs:530`, `:723-735` | `write_full_texture`: the whole frame, every time |
| App: thumbnail | `gpu.rs:601`, `:666-715` | `render_thumbnail` after every upload: a bind group, an encoder, a 16-tap downsample pass and its own `queue.submit` |
| App: pacing | `event_loop.rs:612`, `:650-659` | 6 frames within one second (`BUSY_FRAMES`) mark the loop active, which then redraws every vsync (`about_to_wait`, `:214-263`) |

### Survey E2's claims, checked

- **Confirmed:** the double copy in WebView2 (`capture.rs:148` and `main.rs:516`) and CEF (`render_handler.rs:79` and `main.rs:536`); the host's per-frame `open_existing` (now `event_loop.rs:1322`); no tear protection; damage is always `None` (every engine sends `damage: None`) and the host ignores both `damage` and `frame_id` (`engine_io.rs:30-31` says so); the per-upload thumbnail submit.
- **Wrong:** the cost model of "five tabs at 30 fps". Only the focused tab and a split partner are `SetVisible(true)` (`tabs.rs:514-559`, `view_modes.rs:186`), and WebView2, CEF and Servo send frames only while visible. The steady state is one or two streams.
- **Stale:** WebKit's "unbounded queue" (wave 3b replaced it with the slot); the host's line numbers; the frame-file removal is now `process_manager.rs:226`.
- **Missed by the survey:**
  1. WebKit already deduplicates whole frames (`frame_due`), so the mechanism here generalizes something one engine already does.
  2. The pacing coupling: a page that sends 6 or more frames a second keeps the whole app redrawing at vsync, egui included. If WebView2's capture delivers frames for a page that isn't changing, a static WebView2 tab costs a redraw every vsync plus ~250 MB/s of uploads over USB4 (8.3 MB x 30). This is the largest possible win and it is **unverified**; Task 1's M1 measures it.
  3. Hidden CEF tabs keep painting and copying (no `was_hidden`).
  4. WebView2's capture device is created with `D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, ...)` (`capture.rs:190-206`), the system default adapter, while the app asks wgpu for `HighPerformance` (`gpu.rs:232`), the eGPU. The two may differ, which would put a frame across USB4 twice. M1 logs both.
- **Tearing** (survey's seqlock idea): an engine can overwrite the file while the app reads it, but every write is followed by a `FrameReady` whose damage covers it, so a torn texture is replaced within one frame. Parked, not planned (Out of scope).

### Baselines

None of the per-frame costs above has been measured on this machine; the survey's figures are estimates. A 1920x1080 BGRA frame is 8,294,400 bytes. Expected orders of magnitude, to be replaced by Task 1's numbers:
- an 8 MB `memcpy`: ~0.5-1.5 ms; a fresh zeroed 8 MB `Vec` adds page faults and a zeroing pass, ~1-3 ms;
- `open_existing` plus first touch of 8 MB of a new mapping: ~0.5-2 ms of soft faults;
- an 8 MB `write_texture` to the eGPU: the CPU copy into wgpu's staging belt plus a transfer over USB4 (a ~4-lane PCIe tunnel); a few ms of transfer per frame.

The single-copy change removes only the intermediate copy: WebView2's allocation, zeroing and row copy (~1-3 ms per frame, 3-9% of one core at its 30 fps cap) and CEF's `to_vec` (~0.5-1.5 ms, 3-9% at 60 fps). The remaining copy into the file stays, except for unchanged rows. That is modest. The case for the work rests on M1 (does a static page send frames?) and M2b (is a whole-frame upload expensive enough that a band matters?). Task 1 decides.

---

## Design options

### A. Where to cut the copy and detect changes

1. **`FrameSink` in `ipc-protocol`, compare-then-copy row by row into the store (recommended).** One mechanism for all four engines. The store is the comparison baseline, so no second copy of the previous frame is kept. Detecting unchanged rows costs a read of the store row (a `memcmp`), which is cheaper than the write it replaces, and gives damage for free.
   - Pro: deletes four engines' copies of create/recreate/retry/dedup logic; WebKit's `frame_due` becomes a special case it subsumes.
   - Con: a fully changed frame (video, animation) costs a compare on top of the copy, so up to ~one extra 8 MB read; M2a measures it.
2. **Use each engine's own damage signal** (CEF `on_paint`'s `dirty_rects`, WGC `DirtyRegions`, Servo nothing, WebKit nothing).
   - Pro: no compare cost.
   - Con: per-engine special cases, and two of four engines have no signal, so the compare is needed anyway. Rejected as the mechanism; WGC's `DirtyRegions` stays a parked refinement (Out of scope).
3. **Hash each frame and skip unchanged ones.** Whole-frame only, no rows, and the hash reads as much as the compare. Rejected.

### B. How the app reads the frame file

1. **Keep `open_existing` per frame (recommended unless M2a says otherwise).** Today's code; with deduplication, it runs only for frames that changed.
2. **Cache the mapping per tab.** Not possible safely today: on a resize the engine truncates and rewrites the same file name, and a cached read-write mapping of it is exactly what makes the engine's `create` fail with error 1224 (`frame_buffer.rs:113-147`). It needs generation-named files, which is a protocol change (`FrameReady` gains a generation, `PROTOCOL_VERSION` 3 to 4, fingerprint re-recorded). That also removes `create`'s 20x10 ms retry and covers survey E23 and E25. Gated: Task 7, only if M2a shows `open_existing` + first touch at 0.5 ms or more per frame.

### C. What the app uploads

1. **The damaged band, as one `write_texture` of whole rows (recommended, gated on M2b).** Rows are contiguous in the file, so a band is one copy with `bytes_per_row = width*4` and an offset. No per-rect work.
2. **Rectangles.** Needs per-column damage the engines don't have (option A.1 yields rows). Rejected.
3. **Keep the GPU texture as a persistent mapped staging buffer and copy on the GPU.** More code for the same transfer over USB4; the bytes still have to cross. Rejected.

### D. Thumbnails

1. **Dirty flag, flushed in `render` for the thumbnails egui draws that frame (recommended, gated on M2b).** The signal is the frame's own paint list (egui meshes name their texture), so no view-mode predicate has to stay in sync with which screens show thumbnails (cards in Graph view, group cells, the hover peek, the transition backdrop all count automatically). Several thumbnails in one encoder need the downsample shader to stop reading one shared uniform; it derives the box size from `dpdx`/`dpdy` of the UV instead, which deletes the uniform buffer and binding 2.
2. **Keep one downsample per upload but record it into the next frame's encoder.** Saves the submit, still downsamples frames nobody sees. Rejected in favor of D.1.
3. **Refresh thumbnails on a timer.** A per-case knob; D.1's signal is exact. Rejected.

### E. The WebView2 `Map` stall

Two staging textures used alternately, mapping the previous frame's (one frame of latency), or `D3D11_MAP_FLAG_DO_NOT_WAIT` with a retry next tick. Gated: only if M1 shows the `Map` wait at 2 ms or more. Not a task in this plan; recorded under Out of scope with its trigger.

---

## Task 1: Measure (decides everything after it)

**Prerequisite:** `.superpowers/rep/out/done` exists (another session's timed run owns the machine until then). Check `Get-Process cargo,rustc,manifold*` and the GPU leases before starting.

**Expected duration:** 1.5-2.5 hours, most of it builds: a cold debug build of manifold, WebView2 and CEF in the M1 worktree (start it first, in the background, and write M2a/M2b meanwhile), and release test builds of `ipc-protocol` and `manifold`. The runs themselves are minutes.

**Files:**
- Modify (committed): `crates/ipc-protocol/src/frame_buffer.rs` (tests module: `#[ignore]` `handoff_timing`).
- Modify (committed): `crates/manifold/src/gpu.rs` (new `#[cfg(test)] mod upload_timing` beside `frame_tests`, and the downsample pipeline moved into its own function, Step 9).
- Throwaway worktree only (never committed): `crates/manifold-engine-webview2/src/capture.rs`, `crates/manifold-engine-webview2/src/main.rs`, `crates/manifold-engine-cef/src/{render_handler.rs,main.rs}`.
- Modify (committed): this plan's "Results" section. Modify (local only, `.superpowers/` is git-ignored): `.superpowers/survey/engines.md` E2 with the numbers.

### M1: what the engines actually send (throwaway instrumentation)

- [ ] **Step 1: Worktree.** `git worktree add ../manifold-m1 main`. All M1 edits go there; the worktree is removed in Step 6.
- [ ] **Step 2: Instrument WebView2.** In the worktree's `capture.rs`:
  - in `create_d3d11_device`, after creating the device, log the adapter: `device.cast::<IDXGIDevice>()?.GetAdapter()?.GetDesc()?` and print `Description` (UTF-16) once as `[m1] capture adapter: <name>`;
  - in `try_get_latest_frame`, time the `Map` call (`Instant` around `:142`) and the `vec!` + row copy (`:148-157`) separately.
  In the worktree's `main.rs` frame block (`:495-521`): time `write_frame`; before writing, compare `buf.as_slice() == &pixels[..]` to count unchanged frames; every 2 s print one line `[m1] frames=<n>/2s unchanged=<k> map_ms=<mean,max> alloc_copy_ms=<mean> write_ms=<mean>`.
- [ ] **Step 3: Instrument CEF the same way.** In the worktree's `render_handler.rs`, count `on_paint` calls in a `Cell<u32>` on the handler (hidden tabs paint too, and their frames are never taken, so counting takes would miss them). In `crates/manifold-engine-cef/src/main.rs`, count unchanged frames against the frame file and time `write_frame`. Print the same 2 s line, with the paint count and whether the tab is visible. CEF's `to_vec` cost is timed by M2a instead.
- [ ] **Step 4: Pages.** Write two files to `%TEMP%` (short path, no spaces; `suggestions::looks_like_url` treats a space as a search):
  - `m1-static.html`: text and an image, no animation, no caret.
  - `m1-anim.html`: a CSS animation that changes every frame across the full viewport (`@keyframes` on `background-color` of `body`), and a small one (a 20x20 spinning square).
- [ ] **Step 5: Run.** Build in the worktree with a short target dir and a junction, so `scripts/drive.sh` (which runs `target/debug/manifold.exe`, `drive.sh:50`) finds it and CEF's C++ build stays under MAX_PATH: `CARGO_TARGET_DIR="C:/Users/Winston Li/AppData/Local/Temp/m1t"`, `cmd //c mklink /J target <that dir>` in the worktree, `depot_tools` stripped from `PATH` (README:46), then `cargo build -j 8 -p manifold -p manifold-engine-webview2 -p manifold-engine-cef` and the runtime-deps sync README "CEF" requires after a CEF build. This is a cold build (no shared `target/`); expect it to dominate Task 1's time. Check free disk first (`df -h /c`); delete the target dir in Step 6. For each engine in `webview2`, `cef` and each page, a `scripts/drive.sh` session with `SEED=<dir>` holding a `settings.json` for that engine (`seed_settings PROFILE ENGINE` in `scripts/lib.sh` writes one) that opens the page with `ADDRESSBAR file:///<short temp path>` + `ENTER`, `WAITIDLE`, then a `!` line without `&` (it blocks the command writer, `drive.sh:30-35`) that polls the newest `%TEMP%\manifold-*.log` written after the session started until it holds 10 `[m1]` lines or 60 s pass, then `QUIT`. For CEF also run a hidden-tab case: open the animated page, `NEWTAB` (the first tab is hidden), and collect 10 lines.
  Record, per engine and page: frames per second sent, the unchanged fraction, mean and max `Map` wait (WebView2), the alloc+copy and write times, and the capture adapter next to the app's adapter (the app does not log its adapter today; read it from `Get-CimInstance Win32_VideoController` plus the M2b test's adapter line, and note which display the window is on).
  Note: headless sessions run the same frame path; only whether the window is on screen differs. If WGC delivers nothing at all headless (zero frames on the animated page), stop and ask for Decision 5 (a visible run, `VISIBLE=1 scripts/drive.sh`).
- [ ] **Step 6: Remove the worktree** (`git worktree remove ../manifold-m1 --force`, then `git worktree prune`), delete the short target dir, and check `Get-Process manifold*,msedgewebview2,cef*` for leftovers.

### M2a: copy and map costs (committed `#[ignore]` bench)

- [ ] **Step 7: Write `handoff_timing`** in `crates/ipc-protocol/src/frame_buffer.rs`'s tests module. Use a local `best_median` (ipc-protocol cannot see `manifold::bench`; five lines). At 1920x1080, `pitch = width*4 + 256` (a padded D3D-style source), N = 200 iterations each, print best and median for:
  1. today's WebView2 path: `vec![0u8; w*h*4]` + row copy from the pitched source + the copy `write_frame` does;
  2. today's CEF path: `src.to_vec()` (tight source) + the copy `write_frame` does;
  (Write that copy as `buf.mmap[..].copy_from_slice(..)`: the tests module sees the private field, and the bench then survives Task 3's deletion of `write_frame`.)
  3. a direct row copy from the pitched source into the mapping (what `FrameSink` does when every row changed, minus the compare);
  4. compare-then-copy, every row changed (the sink's worst case);
  5. compare-then-copy, no row changed (the sink's best case);
  6. the app today: `SharedFrameBuffer::open_existing` + read every byte (`as_slice().iter().fold` with `black_box`) + drop;
  7. a persistent mapping: read every byte of an already open buffer.
  Use a socket id like `format!("handoff-timing-{}", std::process::id())` and remove the file at the end. Items 4 and 5 need `copy_changed_rows`, which Task 2 adds; write it inline in the test for now (Task 2 replaces it with the real function and keeps the bench).
- [ ] **Step 8: Run** `cargo test --release -j 8 -p ipc-protocol handoff_timing -- --ignored --nocapture`, three times, with CPU load and Defender state noted.

### M2b: upload costs (committed `#[ignore]` bench)

- [ ] **Step 9: Write `frame_upload_timing`** in a new `#[cfg(test)] mod upload_timing` in `crates/manifold/src/gpu.rs`. It creates its own instance and adapter exactly as `GpuState::new` does (`HighPerformance`, PRIMARY backends, no surface), prints the adapter name and backend first, and times with `crate::bench::timed`/`report`:
  1. `queue.write_texture` of a whole 1920x1080 frame, CPU time only;
  2. whole frame: `write_texture` + `queue.submit([])` + `device.poll(wgpu::PollType::wait_indefinitely())`;
  3. a 64-row band: same as 2;
  4. an empty submit + poll (the fixed cost of a submit);
  5. the current thumbnail pass: bind group + encoder + downsample pass + submit + poll, from a 1920x1080 source into a 256x144 target. To build the pipeline without a window, move its creation out of `GpuState::new` (`gpu.rs:371-420`, the bind group layout, pipeline layout and pipeline) into `fn downsample_pipeline(device: &wgpu::Device, shader: &wgpu::ShaderModule) -> (wgpu::BindGroupLayout, wgpu::RenderPipeline)`, called by both. That move is the only production change in Task 1.
  Hold an `exclusive` lease while it runs (LONG_RUNS.md), and list who else is on the eGPU before and after.
- [ ] **Step 10: Run** `cargo test --release -j 8 -p manifold --bins frame_upload_timing -- --ignored --nocapture`, three times; report best-of-N if the eGPU or CPU was shared.

### Decide

- [ ] **Step 11: Apply the stop conditions** and write the numbers and verdicts into "Results" below and into survey E2:

| Go | Condition |
|---|---|
| Task 2 + 3 (sink, engines) | M1: a static page sends 6 or more frames a second on any engine, **or** M2a: (item 1 or 2) minus (item 4) times the engine's measured frame rate is 30 ms/s or more |
| Task 4 (band upload) | Task 2 goes, **and** M2b: item 2 is 2 ms or more and item 3 is at most a quarter of it |
| Task 5 (thumbnails) | M2b: item 5 is 0.2 ms or more per upload (independent of Tasks 2-4) |
| Task 7 (generation names) | M2a: item 6 minus item 7 is 0.5 ms or more per frame |
| WebView2 `Map` (Out of scope E) | M1: mean `Map` wait 2 ms or more |
| Capture adapter fix (Out of scope) | M1: the capture adapter differs from the app's |

If nothing goes, record the negative results in survey E2, in this plan's Results and in README's frame-path notes, commit the two benches (they are the evidence), and stop.

- [ ] **Step 12: Commit** the two benches and the Results: `git add crates/ipc-protocol/src/frame_buffer.rs crates/manifold/src/gpu.rs docs/superpowers/plans/2026-10-11-frame-path.md`, message `bench: frame handoff and upload timings (frame path Task 1)`. `.superpowers/` is git-ignored (`.gitignore:6`), so the survey edit stays local by design.

### Results

Task 1 ran 2026-10-11 at `69dc59c` (branch `frame-measure`). Machine: Ryzen 7 7840U, shared with other sessions. Headless `drive.sh` sessions with `RESIZE 1920 1080` (WebView2 captured 1920x1042), debug builds. Windows Defender real-time protection: off (`RealTimeProtectionEnabled` False) throughout. `\Processor(_Total)` was 15-65% from other sessions during the runs (per-run values below); no one else was on the eGPU during M2b and no other GPU lease was live.

**M1: what the engines send** (per-2 s lines; WebView2 means skip the first two lines after load, CEF figures are read off the 10 lines)

| Engine | Page | Frames sent/s | Unchanged | Other |
|---|---|---|---|---|
| WebView2 | static | 0 after load (3-5 frames in the first 2-4 s) | n/a | no frames at all once loaded |
| WebView2 | full-viewport animation | 25.8 | 0% | `Map` wait mean 0.93 ms, max 5.3 ms; alloc + row copy 2.1 ms; `write_frame` 0.48 ms |
| WebView2 | 20x20 spinner | 26.0 | 0% | `Map` mean 0.93 ms, max 5.9 ms (one 12 ms); alloc + copy 2.0 ms; write 0.48 ms |
| WebView2 | full-viewport animation, default 800x562 | 27.6 | 0% | `Map` mean 0.62 ms; alloc + copy 0.54 ms; write 0.10 ms |
| CEF | static | 0 after load (2 paints, 2 frames, in the first 2 s) | n/a | |
| CEF | full-viewport animation | 60 paints/s, 53 taken/s | 0% | `write_frame` 0.45 ms |
| CEF | 20x20 spinner | 60 paints/s, 52 taken/s | 0% | `write_frame` 0.45 ms |
| CEF | animation, tab hidden | 60 paints/s, 0 taken | n/a | a hidden tab keeps painting (Decision 4's premise confirmed) |

- WGC (WebView2) and CEF both deliver nothing for a page that is not changing. The plan's largest suspected win (a static page sending 6 or more frames a second, keeping the app redrawing every vsync) does not exist for these two engines. Servo and WebKit were outside M1 (WebKit already deduplicates; Servo is untested).
- The spinner page sends full frames at the full rate: nothing in the capture path reports that only 20x20 pixels changed, so the damage the sink would find is the only way to shrink the upload.
- The frame loop takes about 53 of CEF's 60 paints a second (8 ms poll sleep); the rest are overwritten in the one-frame slot.
- **Capture adapter: AMD Radeon(TM) 780M** (the iGPU; `D3D11CreateDevice(None, ...)`). **The app's adapter (`HighPerformance`): AMD Radeon RX 9060 XT (Vulkan)**, the eGPU (logged by M2b). They differ.
- Plan error found: after `NEWTAB`, focus stays on the first tab and the new tab is the hidden one. The hidden CEF case needs `NEWTAB`, `WAITTABS`, `NEXTTAB` (the first run measured the hidden new-tab page: 3 paints, then 0).

**M2a: copy and map costs** (`handoff_timing`, release, 1920x1080, pitch 7936, N = 200; best / median, three runs; CPU load at the start of each run 65%, 54%, 27%)

| Item | Run 1 | Run 2 | Run 3 |
|---|---|---|---|
| 1 WebView2 today (vec + pitched row copy + file copy) | 1.8 / 2.2 ms | 1.8 / 2.3 ms | 1.7 / 1.9 ms |
| 2 CEF today (`to_vec` + file copy) | 1.4 / 1.9 ms | 1.4 / 1.6 ms | 1.3 / 1.4 ms |
| 3 direct pitched row copy | 279 / 378 us | 391 / 694 us | 260 / 312 us |
| 4 compare-then-copy, every row changed | 335 / 447 us | 418 / 540 us | 244 / 324 us |
| 5 compare-then-copy, nothing changed | 279 / 350 us | 309 / 349 us | 183 / 274 us |
| 6 app: `open_existing` + read all + drop | 2.2 / 2.7 ms | 2.0 / 2.5 ms | 1.6 / 2.3 ms |
| 7 persistent mapping: read all | 113 / 142 us | 69 / 104 us | 86 / 108 us |

The compare costs little: item 4 is within 0.1 ms of item 3 (a changed frame pays about 50 us extra), so Risks' "skip the compare when the previous frame changed every row" is not needed.

**M2b: upload costs** (`frame_upload_timing`, release, adapter AMD Radeon RX 9060 XT (Vulkan), N = 100; best / median; three runs; no other eGPU user or lease before or after each run; CPU 15-45% before, 20-29% after)

| Item | Run 1 | Run 2 | Run 3 |
|---|---|---|---|
| 1 `write_texture`, whole frame, CPU time | 8.2 / 8.3 ms | 8.2 / 8.3 ms | 8.1 / 8.2 ms |
| 2 whole frame: write + submit + poll | 8.7 / 8.8 ms | 8.6 / 8.7 ms | 8.5 / 8.7 ms |
| 3 64-row band: write + submit + poll | 613 / 718 us | 633 / 723 us | 593 / 692 us |
| 4 empty submit + poll | 4.7 / 5.2 us | 4.6 / 5.1 us | 6.6 / 7.2 us |
| 5 thumbnail pass + submit + poll | 280 / 353 us | 252 / 307 us | 232 / 261 us |

A whole-frame upload costs 8.5 ms of CPU time on the thread that calls `write_texture` (the transfer to the eGPU is paid inside the call), about 22% of that thread at 26 frames a second. That is 4-5 times the whole engine-side copy cost, so the app side is where the time goes.

**Verdicts** (Decide step; best-of-three numbers, medians in parentheses)

| Gate | Computation | Verdict |
|---|---|---|
| Task 2 + 3 (sink, engines) | M1 static: 0 frames/s on both engines, so that arm fails. M2a arm: WebView2 (item 1 - item 4) x 25.8 fps = (1.7 - 0.24) ms x 25.8 = 38 ms/s ((2.2 - 0.45) x 25.8 = 45); CEF (item 2 - item 4) x 53 taken/s = (1.3 - 0.24) x 53 = 56 ms/s ((1.6 - 0.45) x 53 = 61), both at or above 30 ms/s | **Go**, on the M2a arm only, and only while content animates |
| Task 4 (band upload) | Task 2 goes; item 2 = 8.5 ms (at least 2 ms); item 3 / item 2 = 0.59 / 8.5 = 7% (at most 25%) | **Go** |
| Task 5 (thumbnails) | item 5 = 0.23-0.28 ms best (0.26-0.35 median), at least 0.2 ms | **Go**, marginally |
| Task 7 (generation names) | item 6 - item 7 = 1.6 - 0.09 = 1.5 ms best (2.0 ms in runs 1 and 2), at least 0.5 ms | **Go** (Decision 3 still the user's) |
| WebView2 `Map` stall | mean `Map` wait 0.93 ms, below 2 ms (max 5-12 ms, rare) | No |
| Capture adapter fix | capture on the 780M, app on the RX 9060 XT | **Go** (Out of scope item; its own plan) |

Reading of the verdicts (not part of the table): the plan's central premise, that a static page keeps sending frames, is false for WebView2 and CEF. The dedup half of Task 2 therefore buys nothing for them. What the measurements do support is the damage half: Task 2's sink supplies the damaged rows, Task 4 turns them into a band upload (8.5 ms to 0.6 ms for a 64-row change, on pages that animate in a small region), and the intermediate-copy removal is worth about 40-60 ms/s of one core while a page animates. The unmeasured cross-adapter copy (the capture runs on the iGPU, the app on the eGPU) is not in these numbers.

---

## Task 2: `FrameSink` and `copy_changed_rows`

**Files:**
- Create: `crates/ipc-protocol/src/frame_sink.rs`
- Modify: `crates/ipc-protocol/src/lib.rs` (module and re-export), `crates/ipc-protocol/src/frame_buffer.rs` (add `as_mut_slice`; the bench switches to the real `copy_changed_rows`), `crates/ipc-protocol/src/messages.rs` (doc of `FrameReady`'s `damage`).

**Interfaces:**

```rust
/// Copies `src` (`dst.len() / row_bytes` rows, `pitch` bytes apart) over `dst` (the same rows, tightly packed),
/// writing only the rows that differ. Returns the span of rows that differed, or `None` if none did.
pub fn copy_changed_rows(dst: &mut [u8], src: &[u8], row_bytes: usize, pitch: usize) -> Option<Range<u32>>;

/// An engine's frame store and what it has not yet reported (survey E2). Every engine offers each frame it gets
/// and sends what `take` returns; unchanged frames cost a compare and nothing else.
pub struct FrameSink { /* store, size, changed rows, whole flag, frame_id, a frame the store couldn't take yet */ }

impl FrameSink {
    /// A sink writing the shared frame file of `socket_id` (`frame_path`), created at the first offered size.
    pub fn shared(socket_id: &str) -> Self;
    /// A sink keeping frames in memory and sending them inline (WebKit over TCP).
    pub fn inline() -> Self;
    /// Offers a frame of `width` x `height` BGRA/RGBA rows `pitch` bytes apart. Returns false (and keeps nothing)
    /// for a malformed frame: zero size, `pitch < width * 4`, or `src` shorter than `pitch * (height - 1) + width * 4`.
    pub fn offer(&mut self, src: &[u8], pitch: usize, width: u32, height: u32) -> bool;
    /// The tab was shown: the next `take` reports the whole stored frame, changed or not.
    pub fn show(&mut self);
    /// The `FrameReady` to send now, if anything changed since the last one.
    pub fn take(&mut self) -> Option<EngineToHost>;
}
```

Behavior that the tests pin:
- A new or recreated store (first frame, or a size change) is reported whole (`damage: None`): it was compared against zeros, so its changed span means nothing.
- Otherwise `damage` is `Some((0, first_row, width, row_count))` covering the union of the rows changed since the last `take`.
- `take` after `show()` with a stored frame returns a whole report even if nothing changed; with no stored frame it returns `None` and the whole flag waits for the first frame.
- `take` with nothing changed returns `None`.
- `frame_id` increments per report.
- Shared store creation failing (error 1224 while the app still maps the old file, `frame_buffer.rs:113-147`): `offer` keeps a copy of that one frame (tightly packed) and `take` retries the create first; while it fails, `take` returns `None`. This replaces CEF's `recreate_failing`/put-back dance and Servo's `frame_ready` retry. Log the first failure of a run, as CEF does today.
- Inline: the store is a `Vec`; `take` returns `pixels: Some(store.clone())` (the TCP message needs an owned buffer; one copy, as today).

- [ ] **Step 1: Failing tests first** (in `frame_sink.rs`, `#[cfg(test)] mod tests`):
  - `copy_changed_rows_reports_the_changed_span`: identical buffers give `None`; changing rows 3 and 7 of 10 gives `Some(3..8)` and makes `dst == src`; a pitched `src` (pitch > row_bytes) copies only `row_bytes` per row.
  - `an_inline_sink_reports_whole_then_bands_then_nothing`: first offer then take gives `damage: None` with the pixels; the same frame again gives `None`; one changed row gives `Some((0, y, w, 1))`; a new size gives `damage: None`; `show()` then take with no new offer gives `damage: None`.
  - `malformed_frames_are_refused`: zero width, short `src`, `pitch < width * 4`.
  - `a_shared_sink_round_trips_through_the_frame_file`: `shared("sink-test-<pid>")`, offer, take, then `SharedFrameBuffer::open_existing(frame_path(..), w, h)` reads the same bytes; remove the file at the end.
  - `#[cfg(windows)] a_shared_sink_retries_a_failed_create`: hold an `open_existing` mapping of the file, offer a frame of a new size (create fails with 1224 after its retries), take gives `None`; drop the mapping; take gives the whole frame at the new size.
  Run `cargo test -j 8 -p ipc-protocol frame_sink`; expect compile failure.
- [ ] **Step 2: Implement.** Add `pub fn as_mut_slice(&mut self) -> &mut [u8]` to `SharedFrameBuffer`. Implement `copy_changed_rows` (`chunks_exact_mut(row_bytes)` over `dst`, slice compare, `copy_from_slice` on mismatch) and `FrameSink` (an enum store `Shared(SharedFrameBuffer) | Inline(Vec<u8>)`). Drop the old shared store before creating the new one (Windows refuses to truncate a mapped file; the comment at CEF `main.rs:513` explains it). Re-export both from `lib.rs`.
- [ ] **Step 3: Switch the M2a bench** to the real `copy_changed_rows` (delete its inline copy).
- [ ] **Step 4: Document `damage`** on `FrameReady` in `messages.rs:385-391`: "`None`: the whole frame changed. `Some((x, y, w, h))`: only that rectangle changed since this tab's previous `FrameReady`; engines report whole rows (`x == 0`, `w == width`)." Doc comments are not part of the wire format: no `PROTOCOL_VERSION` bump. Run the fingerprint test (`cargo test -j 8 -p ipc-protocol fingerprint`) to prove it.
- [ ] **Step 5: Verify** `cargo test -j 8 -p ipc-protocol`, then fmt and clippy. **Commit:** `ipc-protocol: FrameSink, one frame store that sends only changed rows (frame path Task 2)`.

---

## Task 3: Engines adopt the sink

**Files:** `crates/manifold-engine-webview2/src/{capture.rs,main.rs}`, `crates/manifold-engine-cef/src/{render_handler.rs,main.rs}`, `crates/manifold-engine-servo/src/main.rs`, `crates/manifold-engine-webkit/src/main.rs`, `crates/ipc-protocol/src/frame_buffer.rs` (delete `write_frame` once no engine uses it, and switch its tests to `as_mut_slice`).

The rule for every engine: offer each frame it gets; call `show()` on `SetVisible(true)` after hidden; send what `take()` returns, only while visible, at the engine's existing pacing. Delete each engine's pre-create, recreate-on-resize, retry state, `frame_id` counter and (WebKit) `SentFrame`/`frame_due`.

- [ ] **Step 1: WebView2.**
  - `capture.rs`: replace `try_get_latest_frame() -> Option<(Vec<u8>, u32, u32)>` with `with_latest_frame<R>(&mut self, f: impl FnOnce(&[u8], usize, u32, u32) -> R) -> Option<R>`, which calls `f(mapped bytes, RowPitch, width, height)` between `Map` and `Unmap`. The mapped slice is `RowPitch * (height - 1) + width * 4` bytes from `pData`. Keep `frame.Close()` on every path (the comment at `:97-109` explains the leak it prevents). Delete the `vec!` and row copy.
  - `main.rs`: one `FrameSink::shared(&socket_id)` replaces `frame_buffer`, `frame_id` and the resize block (`:273-274`, `:495-521`). Per tick while visible and `FRAME_INTERVAL` elapsed: `capture.with_latest_frame(|src, pitch, w, h| sink.offer(src, pitch, w, h))`, then `if let Some(msg) = sink.take() { control.send(&msg) }`. Keep logging a size change once (the sink can expose `size()` for that, or the engine compares before offering).
  - `SetVisible(true)` after hidden (`:336-338`): `sink.show()`.
  - Build check: `cargo build -j 8 -p manifold-engine-webview2`.
- [ ] **Step 2: CEF.**
  - `render_handler.rs`: the handler holds `Rc<RefCell<FrameSink>>` instead of `SharedLatestFrame`; `on_paint` calls `sink.borrow_mut().offer(slice, width * 4, w, h)` on the CEF buffer directly. Delete `PaintedFrame`, `SharedLatestFrame` and the `to_vec`. (`on_paint` runs inside the main loop's CEF pump on the same thread, which is why `RefCell` works today; the loop must not hold the sink's borrow across that pump, or `on_paint` panics on a second borrow. Borrow, `take()`, release, then send.)
  - Behavior change to note in the commit: today the first frame-file create at startup is fatal (`expect`); with the sink it is retried like any other create.
  - `main.rs`: delete `buffer_size`, `recreate_failing`, the pre-created `frame_buffer` and `frame_id` (`:243-251`) and the frame block's body (`:507-549`); the loop sends `sink.take()` while visible. `SetVisible(true)` after hidden calls `show()`.
  - Decision 4: also call the browser host's `was_hidden` (hidden = `!v`; exact binding per the `cef` crate in use) on `SetVisible`, which stops hidden tabs painting. If adopted, verify live that a re-shown tab repaints (M1's hidden case, re-run: frames while hidden drop to zero; after `SETVISIBLE` the tab shows content).
  - Build check: CEF's own build per README "CEF" (`cargo build -j 8 -p manifold-engine-cef`; needs the CEF download already in place, which `target/debug` has).
- [ ] **Step 3: Servo.** `FrameSink::shared` replaces the pre-create (`:211-212`), the Resize recreate (`:259-266`, keep the `rendering_context`/`webview` resize) and the frame block's buffer handling (`:366-382`): after `read_to_image`, `sink.offer(image.as_raw(), width as usize * 4, width, height)` then send `take()`. The `frame_ready` flag stays (it says Servo painted), but it no longer has to stay set to retry a failed create, since the sink retries. `SetVisible(true)` keeps forcing a paint and also calls `show()`.
  - Build check: `CARGO_TARGET_DIR=target-servo cargo check -j 8 -p manifold-engine-servo` (README:427; a full build is 33 min, `check` reuses `target-servo`'s dependencies). If it fails for reasons unrelated to this change, stop and report (Decision 1).
- [ ] **Step 4: WebKit.** `FrameSink::inline()` replaces `SentFrame`, `frame_due`, its test and `last_sent` (`:17-33`, `:281-287`). The loop snapshots the pixbuf every 16 ms today and copies it with `tightly_packed_rgba` before comparing; instead, offer the pixbuf's own pixels with its rowstride as the pitch when it has 4 channels (GdkPixbuf leaves the last row unpadded, which `offer`'s length rule, `pitch * (height - 1) + width * 4`, accepts). For a 3-channel pixbuf keep `tightly_packed_rgba` and offer its output with `pitch = width * 4`. The send stays paced by `FRAME_INTERVAL` through `sent_at`: `take()` only once it has elapsed. `SetVisible(true)`: `show()` (replaces `last_sent = None` at `:352-356`, which already handles the re-show case).
  - Build check, in WSL: `wsl.exe -d Ubuntu -- bash -c 'cd /mnt/c/Users/Winston\ Li/Documents/GitHub/manifold && CARGO_TARGET_DIR=target-linux cargo build -j 8 -p manifold-engine-webkit'`, then `cargo test` the same way for that crate.
- [ ] **Step 5: Delete `SharedFrameBuffer::write_frame`** (`frame_buffer.rs:212-215`) after `grep -rn write_frame crates` shows only its tests; switch those tests to `as_mut_slice().copy_from_slice(..)`.
- [ ] **Step 6: Verify.**
  - `cargo test -j 8 -p ipc-protocol`; clippy (Global constraints); fmt.
  - Live, headless (after `cargo build -j 8 -p manifold -p manifold-engine-webview2` and CEF): for WebView2 and CEF, a `drive.sh` session opens `m1-static.html`, `WAITIDLE`, `SCREENSHOT a.png`, `NEWTAB`, `WAITIDLE`, `PREVTAB` (the static tab is shown again, so `show()` must produce its frame), `WAITIDLE`, `SCREENSHOT b.png`, then `TOGGLEVIEW` (Graph; the transition hides the tab) and `SCREENSHOT c.png`. Pass: `a.png` and `b.png` show the page, identical in the content area; `c.png`'s card for that tab shows the page.
  - Live, animated page: content keeps animating (two screenshots taken with a `WAITIDLE` between them differ in the animated region).
  - Frame counts (static: none after load) are measured in Task 6 Step 3 with M1's instrumentation; not here.
- [ ] **Step 7: Commit** `engines: one FrameSink for WebView2, CEF, Servo and WebKit; unchanged frames are not sent (frame path Task 3)`.

---

## Task 4: The app uploads only changed rows (gated on Task 1)

**Files:** `crates/manifold/src/process_manager/engine_io.rs`, `crates/manifold/src/process_manager.rs` (reader arm `:615-618`), `crates/manifold/src/process_manager/tests.rs`, `crates/manifold/src/event_loop.rs` (`FrameUpdate` `:621-634`, `drain_engine_ipc` `:1144-`, `upload_frames` `:1299-1333`), `crates/manifold/src/gpu.rs` (`TabTexture` `:135-141`, `upload_frame` `:519-602`, `write_full_texture` `:723-735`, `GpuState::new` `:212-260`).

- [ ] **Step 1: Failing tests** in `engine_io.rs` (pure functions):
  - `frame_rows(damage: Option<(u32, u32, u32, u32)>, height: u32) -> Option<Range<u32>>`: `None` stays `None` (whole); `Some((0, 3, w, 2))` gives `3..5`; a rectangle reaching past `height`, or `h == 0`, gives `None` (an engine's bad damage degrades to a whole upload, never to a panic in wgpu).
  - `merge_rows(prev: Option<Range<u32>>, next: Option<Range<u32>>) -> Option<Range<u32>>`: either `None` gives `None`; otherwise the covering span (`2..4` and `7..9` give `2..9`).
  - In `process_manager/tests.rs`, extend `frames_never_queue_on_the_events_channel`'s helper or add `piled_up_frames_union_their_rows`: feed three `FrameReady` with damage rows `1..2`, `5..6`, and a different size; the slot holds one frame of the last size with `rows: None`. Feed two same-size frames: the slot's `rows` covers both.
- [ ] **Step 2: Implement.** `Frame` gains `rows: Option<Range<u32>>` (`None` = whole). The reader arm computes `frame_rows(damage, height)`, takes the slot's previous frame if any, and stores `merge_rows(prev.rows, rows)` when the sizes match, `None` otherwise. `FrameUpdate` carries `rows`.
- [ ] **Step 3: GPU band upload.** `upload_frame(tab_id, pixels, width, height, format, rows: Option<Range<u32>>)`:
  - a newly created texture, a texture whose `stale` flag is set, or `rows == None`: write the whole frame and clear `stale`;
  - otherwise one `write_texture` at `origin.y = rows.start`, `offset = rows.start * width * 4`, `bytes_per_row = width * 4`, extent `width x rows.len()`. `write_full_texture` becomes this one function (`write_rows`); delete the old one.
  - `TabTexture` gains `stale: bool`. `GpuState::mark_stale(tab_id)` sets it. Callers: `upload_frames` when `open_existing` fails (the frame was skipped, so rows it changed never reached the texture), and `upload_frame` itself when it drops an invalid frame (`:522-524`).
  - Log the adapter once in `GpuState::new` (`eprintln!("gpu: {} ({:?})", info.name, info.backend)`), since every frame-path measurement needs it.
- [ ] **Step 4: Verify.**
  - `cargo test -j 8 -p manifold --bins engine_io process_manager`; clippy; fmt.
  - Parity: an animation's phase can't be pinned between two runs, so the parity page changes once: `m4-change.html` shows several paragraphs and, after a 1 s `setTimeout`, rewrites one paragraph in the middle. For WebView2 and CEF, a headless session loads it, waits until two consecutive `SCREENSHOT`s taken after `WAITIDLE` are identical (deadline 10 s), and keeps the last. Do the same with a reference build from a worktree where `upload_frames` passes `rows = None` (whole uploads). Pass: the two final screenshots are byte-identical in the content area. Also check the band path ran: a temporary `eprintln!` of `rows` per upload shows a band, not `None`, for the change.
  - Perf: re-run M2b (unchanged) and the M1 counters on the small-spinner page with a temporary log of `rows.len()` per upload: uploads cover the spinner's rows, not the frame.
- [ ] **Step 5: Commit** `app: upload only the rows an engine changed (frame path Task 4)`.

---

## Task 5: Thumbnails refresh when drawn (gated on Task 1)

**Files:** `crates/manifold/src/gpu.rs` (shader `:69-102`, `upload_frame`'s tail `:556-601`, `render_thumbnail` `:666-715`, `render` `:977-`, `free_tab_thumbnail` `:753`, `free_tab_texture` `:771`, `GpuState::new` for the params buffer and binding 2).

- [ ] **Step 1: Shader.** `fs_downsample` computes `let box_size = abs(vec2<f32>(dpdx(in.uv).x, dpdy(in.uv).y));` before the loop (derivatives in uniform control flow) and the uniform `DownsampleParams`, its binding 2, `downsample_params_buffer` and the bind group layout entry are deleted. (One output texel spans `dpdx(uv).x` of source UV horizontally, which equals `1 / thumb_width` for the full-screen triangle; that is what the uniform held.) Update the shader's comment.
- [ ] **Step 2: Dirty set.** `GpuState` gains `thumbs_dirty: HashSet<String>`. `upload_frame` creates or resizes the thumbnail texture as today but inserts the tab into `thumbs_dirty` instead of calling `render_thumbnail`. `render_thumbnail` takes an encoder (`&mut wgpu::CommandEncoder`) instead of submitting.
- [ ] **Step 3: Flush in `render`.** After tessellation, collect the `egui::TextureId`s of this frame's meshes (`ClippedPrimitive { primitive: Primitive::Mesh(m), .. }` gives `m.texture_id`), map them back to tab ids through `thumbnails` (a reverse lookup over at most a few hundred entries; or keep a `HashMap<TextureId, String>` beside `thumbnails`), and for each one in `thumbs_dirty`, record its downsample into the frame's encoder before the egui pass, then remove it from the set. `panes` (`:1056`) holds shared borrows of `tab_textures`; the flush only reads `tab_textures` and `thumbnails` and mutates `thumbs_dirty`, so write it with field-level borrows (not a `&mut self` method) or run it before `panes` is built, by tessellating earlier. Note the screenshot path tessellates the same primitives: one flush covers both.
- [ ] **Step 4: Free paths.** `free_tab_texture` flushes the tab's thumbnail first if it is dirty (its own encoder and submit, as today's `render_thumbnail` did; this covers `hide_focused_tab`, `tabs.rs:514-526`, and `close_split`, `tabs.rs:606-613`, where the tab's last frame must reach its card). `free_tab_thumbnail` removes the tab from `thumbs_dirty`. The close/sleep paths call `free_tab_thumbnail` before `free_tab_texture` (`node_store.rs:138-139`, `tabs.rs:670-671`, `tabs/sleep.rs:25-26`), so they never flush a thumbnail about to be freed.
- [ ] **Step 5: Tests.** A unit test for the dirty-set bookkeeping if it lives in a pure helper; otherwise the proof is live:
  - Graph parity: headless session with three tabs on static pages, `TOGGLEVIEW`, `SCREENSHOT` in Graph view, before and after this task (same seed, same pages). Pass: identical, or off by at most one level in a few thumbnail texels (the derivative is computed per 2x2 quad and may round differently from `1.0 / thumb_width`); anything larger is a bug to explain.
  - Draw flush: open `m4-change.html`, wait for its change, `TOGGLEVIEW` (the ToGraph transition hides the tab but keeps its texture, `view_modes.rs:186`), screenshot: the card shows the changed paragraph.
  - Free flush: open `m4-change.html`, wait for its change, `NEWTAB` (focusing the new tab runs `hide_focused_tab`, which frees the old texture, `tabs.rs:514-526`), `WAITIDLE`, `TOGGLEVIEW`, screenshot: the first tab's card shows the changed paragraph.
  - Perf: M2b's item 5 no longer happens per upload; count downsample passes per second with a temporary counter while one animated tab is focused in Item view with no card or peek on screen: zero.
- [ ] **Step 6: Commit** `gpu: thumbnails refresh in the frame's encoder when drawn (frame path Task 5)`.

---

## Task 6: Docs, survey and gate

**Files:** `README.md` (frame-path notes; the engine table's "shared memory" wording), `CHANGELOG.md`, this plan's Results, and (local, git-ignored) `.superpowers/survey/engines.md`: E2 status, done or the parts parked, with Task 1's numbers; E8's FrameSink idea is what Task 2 built, so mark it.

- [ ] **Step 1: Update** the docs above. Say what was deleted (`write_frame`, `PaintedFrame`, `SharedLatestFrame`, `SentFrame`/`frame_due`, the per-engine recreate/retry state, `write_full_texture`, the downsample uniform) and why.
- [ ] **Step 2: Gate.** `cargo build -j 8 -p manifold -p manifold-engine-webview2`, then `scripts/gate.sh LOG full` (engine changes), detached at BelowNormal per README "scripts/gate.sh". Report CPU load and Defender state with the result.
- [ ] **Step 3: Perf proof.** Re-run M1's counters (worktree instrumentation) and M2a/M2b. Report, next to Task 1's numbers: frames sent per second on the static page (expected: zero after load), engine copy time per frame, upload time per frame on the small-spinner page.
- [ ] **Step 4: Commit** `docs: frame path (survey E2)`.

---

## Task 7 (gated): generation-named frame files

Only if Task 1's M2a shows the app's per-frame map cost (item 6 minus item 7) at 0.5 ms or more. Sketch, to be planned in full then: `FrameSink::shared` creates `manifold-frame-{socket_id}-{gen}.bin` with a new generation on each size change and deletes the previous one once the app has acknowledged it (or on the next generation); `FrameReady` gains `gen: u32`, `PROTOCOL_VERSION` 3 to 4 and the fingerprint is re-recorded; the app caches one read-only mapping per tab and re-opens only on a new generation. This deletes `create`'s 20x10 ms retry and the sink's failed-create path, and gives E23 (Unix frame names) and E25 (graceful exit) their unique names.

---

## Proofs, summarized

- **Parity:** sink unit tests (whole/band/nothing/show/retry/malformed); `piled_up_frames_union_their_rows`; band-vs-whole screenshot diff (Task 4); Graph screenshot parity and the hide-path card (Task 5); re-shown static tabs render (Task 3); `scripts/gate.sh full`.
- **Perf:** M1 counters before and after (frames sent per second, static and animated), M2a/M2b benches before and after, a downsample-pass count in Item view. Each with CPU load, Defender state and the eGPU's other users.

## Risks

- **eGPU / USB4.** Every byte uploaded crosses a ~4-lane PCIe tunnel. This plan reduces bytes (no upload for unchanged frames, bands for small changes) and submits (thumbnails join the frame's encoder). It adds no dispatches and no readbacks; data already stays resident in the tab texture. Task 1's M2b measures the transfer on this machine instead of assuming it.
- **Capture adapter.** If WebView2 captures on the iGPU while the app renders on the eGPU, the copies this plan removes are not the biggest cost; M1 says so before any work starts.
- **Compare cost on changing content.** A video or full-page animation pays a compare plus a copy. M2a item 4 vs item 3 bounds it; if the compare costs more than the dedup saves on the measured content, Task 2 can skip the compare when the previous frame changed every row (one flag, no per-engine case), and the plan records that.
- **Stuck content.** Deduplication means no frame arrives for a static page; anything that relied on a stream of frames breaks. Checked: the only consumers are `note_engine_frames` (pacing; fewer frames is the point) and `tab_view.rs:624` (`has_tab_texture`, satisfied by `show()`'s whole report). A live tab whose texture is freed must also be hidden, so that showing it again goes through `show()`: checked for all three such paths (`hide_focused_tab` `tabs.rs:514-526`, `close_split` `tabs.rs:606-613`; close and sleep end the tab or its engine). The implementer re-greps `free_tab_texture` callers in Task 3 in case one was added.
- **Tearing.** Unchanged: the engine can still write the file while the app reads it, and the next `FrameReady`'s damage covers whatever was torn, as today. With band uploads a torn band is repaired by the next band over the same rows, which the engine's next report includes because those rows changed again.
- **Servo build.** `CLAUDE.md` says Servo does not build on Windows; README:427 documents a Windows build into `target-servo` (2026-09-30), and `target-servo/debug/manifold-engine-servo.exe` exists. Task 3 uses `cargo check` there. If it doesn't build, Decision 1.
- **CEF `was_hidden`.** Can change when CEF fires `on_paint` after a show; verified live in Task 3.

## Out of scope

- A seqlock or double buffer against tearing (self-healing today; see Risks).
- Zero-copy GPU sharing (D3D11 shared textures opened in wgpu, or DMA-BUF on Linux): L/XL, needs wgpu-hal interop per backend, and changes the engine contract.
- WGC `DirtyRegions`/`SetDirtyRegionMode` and `SetMinUpdateInterval` (windows 0.62.2 has them): a refinement of the damage the compare already finds; consider only if M1 shows the compare matters.
- The WebView2 `Map` stall (option E): triggered by M1 (mean wait 2 ms or more).
- Creating the WebView2 capture device on the app's adapter (by LUID): triggered by M1 showing different adapters.
- Sending only the damaged rows inline for WebKit (TCP payload): WebKit runs over WSL for development only.
- Changing which tabs are visible, or frame rates (`FRAME_INTERVAL`, `windowless_frame_rate`).

## Decisions for the user

1. **Migrate all four engines now, or WebView2 and CEF only.** Recommend all four: otherwise `write_frame` and the per-engine logic stay alive for two engines, and the mechanism isn't the one mechanism. Costs a Servo `cargo check` in `target-servo` and a WebKit WSL build.
2. **Keep `FrameReady::frame_id`.** The app never reads it. Recommend keeping it until a protocol bump happens anyway (Task 7), since removing it alone forces one.
3. **Generation-named frame files (Task 7, protocol bump).** Recommend only if M2a shows 0.5 ms or more per frame.
4. **CEF `was_hidden` on `SetVisible`.** Recommend yes, with Task 3's live check: hidden CEF tabs stop painting.
5. **A visible-window run.** Only if M1 headless shows WebView2 delivering no frames at all; it would put a window on screen.
6. **CLAUDE.md says Servo doesn't build on Windows; README says it does (`target-servo`).** Recommend correcting CLAUDE.md's clippy note to say Servo is left out of clippy for build time, not because it can't build (a separate, one-line doc change; not part of this plan's commits unless asked).

## Stress test

Five adversarial passes over the plan, each re-reading the code it cites. Design-level catches made while drafting (before round 1) are listed first because they shaped the design.

- **Drafting.**
  - A re-shown static tab would stay blank under deduplication (its texture was freed on hide and no new frame comes): `show()` added.
  - A shared downsample uniform breaks with several thumbnails in one encoder (`write_buffer` lands before the whole submit): box size from `dpdx`/`dpdy`, uniform deleted.
  - A new store's changed span is measured against zeros: new and recreated stores report whole.
  - Frames of different sizes piling up in the slot can't union rows: a size change merges to whole.
  - A frame the app skipped (`open_existing` failed mid-resize, or an invalid frame) leaves rows never uploaded: `stale` flag forces the next upload whole.
  - Hidden CEF tabs paint and copy: counted in M1, `was_hidden` as Decision 4.
- **Round 1 (draft against the code).**
  - WebKit's `sent_at` is its 30 fps pacing, not dedup state; the draft deleted it. Kept.
  - Counting CEF frames at `take` misses hidden paints: counted in `on_paint`.
  - M2b can't build the downsample pipeline without a `GpuState` (needs a window): pipeline creation moves into a function.
  - `.superpowers/` is git-ignored: survey edits are local, not committed.
  - Task 3's live check named no command to refocus a tab: `PREVTAB`; Graph via `TOGGLEVIEW`.
  - Task 4's parity check was incoherent (an animation's phase can't be matched across runs): a page that changes once, compared against a whole-upload reference build.
  - `dpdy` sign and derivative placement: `abs`, computed before the loop.
  - Task 5 was gated as if it needed Task 2: it is independent.
- **Round 2 (paths that free or hide).**
  - `TOGGLEVIEW` hides through the transition without freeing the texture (`view_modes.rs:186`); `hide_focused_tab` frees it. The thumbnail test split into a draw-flush case and a free-flush case.
  - `close_split` frees a live tab's texture: checked that it hides first (`tabs.rs:608-613`).
  - CEF's `on_paint` re-enters the sink during the CEF pump: the loop must not hold the `RefCell` borrow across it.
  - CEF and Servo treat the first frame-file create as fatal today; the sink retries it. Recorded as a behavior change.
  - `render`'s `panes` borrow `tab_textures` while the flush needs `thumbs_dirty`: field-level borrows or earlier tessellation.
- **Round 3 (numbers and run mechanics).**
  - Expected gain overstated: only the intermediate copy is removable; the copy into the file stays. Corrected to ~1-3 ms (WebView2) and ~0.5-1.5 ms (CEF) per frame.
  - "60 Hz redraw" assumed the display rate: "every vsync".
  - The M1 worktree: `drive.sh` runs `target/debug/manifold.exe`, and CEF's C++ build breaks on long worktree paths. Short target dir plus a junction; a cold build, so Task 1's estimate rose from one hour to 1.5-2.5 hours.
  - How a headless session waits for log lines: a blocking `!` line in the command file (`drive.sh`'s documented mechanism), with a deadline.
- **Round 4 (deletions against later tasks).**
  - The M2a bench used `write_frame`, which Task 3 deletes: it writes through the private mapping instead.
  - `was_hidden`'s exact binding in the `cef` crate wasn't checked: hedged, implementer confirms.
  - "Zero downsample passes in Item view" ignored any card on screen: qualified.
- **Round 5 (full pass).** Checked: the reader/drain race (a frame read while the next is written is covered by the next report's rows), the inline path (WebKit pixels are always whole, so a merged band reads from the newest full buffer), engine respawn (a new sink reports whole), and the Unix create path (rename, no 1224). Nothing material found.

## Outcome (2026-10-11)

Tasks 2-5 landed on `main`: `FrameSink` (7826f5a), the four engines on it (8abca96), band upload (ef32fdb), thumbnails flushed when drawn (0812a11). Task 7 (generation-named frame files) is parked on Decision 3.

- **Gate:** `scripts/gate.sh full` at 0812a11 plus docs: `gate two` 66/66 three times (58-62 s), `gate smoke` 54/54 (63 s), launch-forms 7/7, `gate link` 30/30. 06:15-06:20, CPU 37.9% at start from other sessions, Defender real-time protection off. Total about 5 minutes.
- **Live checks (headless, WebView2 and CEF):** a static tab shown again is byte-identical to its first screenshot; Graph cards show the page; a page that changes once ends byte-identical under band and whole uploads; the changed paragraph reaches the card after a Graph toggle and after a tab switch (the free-flush path); no downsample passes in Item view with one animated tab focused. Graph parity before/after: three static pages, identical (WebView2 only).
- **Band sizes (800x562 spinner page):** uploads were 28-37 rows (5-7% of the frame); loads and resizes were whole.
- **Not re-measured:** per-upload CPU time and frames sent per second after the change (Task 6 Step 3's counters were not re-run; only rows per upload were logged). M2b's whole-frame figure (8.5 ms) and band figure (0.6 ms) from Task 1 stand as the expected effect.
- **Not run live:** Servo and WebKit (both build; Servo by `cargo check` in `target-servo`, WebKit in WSL). The stale-texture path (`mark_stale`) has no test since it needs a GPU.
- **Open:** the capture adapter differs from the app's (iGPU vs eGPU); the cross-adapter copy is unmeasured. CEF screenshots render at a smaller scale than WebView2's, probably old, not investigated.
