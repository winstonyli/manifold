# Review Scan Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the verified findings of the five-part review scan of `dev` @ 4b833af (2026-10-04). Crashers, data loss and security come first, then the remaining bugs, then robustness and performance, and last the behaviour-preserving refactors.

**Architecture:**
- Each task is one coherent set of files and ends with a test (or, where the logic needs a live GPU, engine or window, a build plus a named manual check) and a commit.
- Shared helpers are introduced once and reused by later tasks:
  - `crates/manifold/src/rgba.rs`: image fit and limited decode (Task 1).
  - `crates/manifold/src/store/atomic.rs`: the one "write temp, fsync, rename" helper (Task 7).
  - `ipc_protocol::PROTOCOL_VERSION` with its handshake (Task 4).
  - `crates/manifold/src/popup_gate.rs`: popup URL and rate policy (Task 5).
  - `crates/manifold/src/net.rs`: the one ureq agent (Task 22).
  - `ipc_protocol::downloads`: download destination naming (Task 19).
- Protocol changes (Tasks 5 and 18) bump `PROTOCOL_VERSION`, so a stale engine host fails fast with "rebuild the engine host" instead of misreading messages.

**Tech Stack:**
- Rust 2021 workspace.
- `manifold` (bin) with egui/epaint 0.36.2 and wgpu 30.0.1; image 0.25; ureq 3.
- `ipc-protocol` (postcard 1.1).
- Engine hosts: `manifold-engine-cef`, `-webview2`, `-webkit`, `-servo`.

**Spec:** self-contained. The findings come from `target/review-scan/A.md`–`E.md`, which are git-ignored and will not travel with this plan. Every finding this plan acts on is restated inside its task: id, file:line at 4b833af, failure, chosen fix. Finding ids are `<report>-<id>`; for example A-B1 is report A, bug 1. Each was re-verified against the code before it was planned. The ones that turned out false, or already handled, are listed under "Dropped findings" at the end.

## Global Constraints

- Work on `dev`, never `main`.
- Run `cargo fmt --all` before each commit.
- `cargo fmt --all --check` and `cargo clippy -j 8 -p manifold -p ipc-protocol --all-targets -- -D warnings` must pass.
- Never use --no-verify.
- Never silence a lint without a why-comment.
- Tests: `cargo test -j 8 -p manifold --bins` (bin-only crate) plus `cargo test -j 8 -p ipc-protocol`.
- Tests must never touch the real %APPDATA%. Use `store::testutil::temp_dir`. `profile::dir()` is None under cfg(test).
- Cap CPU at <=12 busy threads (-j 8). Kill only PIDs you started. GUI jobs run at BelowNormal.
- C: is about 96% full (41G free measured), so use the existing target dir and create no new worktrees.
- The repo is LF.
- Python edits run as `python -P script < /dev/null`, reading and writing UTF-8 with newline=''.
- Cargo.toml keeps its BOM.
- Never log cookie values or dump the real jar.
- Commit trailer for opus implementers: `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Engine crates (cef/webview2/webkit/servo) may not fully build here. Note per task what `cargo check -p <crate>` can verify and what only reading can.

### Engine build facts (for the per-task notes)

- **`manifold-engine-cef`:** `cargo check -j 8 -p manifold-engine-cef` works on Windows (the CEF binaries are already in `target/debug`).
- **`manifold-engine-webview2`:** `cargo check -j 8 -p manifold-engine-webview2` works on Windows.
- **`manifold-engine-webkit`:** builds only in WSL. If WSL Ubuntu is available, run `wsl.exe -d Ubuntu -- bash -c 'cd "/mnt/c/Users/Winston Li/Documents/GitHub/manifold" && CARGO_TARGET_DIR=target-linux cargo check -j 8 -p manifold-engine-webkit'`. Otherwise it is verified by reading only, and the task report says so.
- **`manifold-engine-servo`:** does not build on Windows. Verified by reading only.
- **Clippy:** run it on an engine crate only where a task says so: `cargo clippy -j 8 -p <crate> -- -D warnings`. Pre-existing warnings in an engine crate are not this plan's job; report them and don't fix them.

### Conventions for every task

- **`App` fields:** `App` (`crates/manifold/src/main.rs:785`) has a hand-written `Default` (around `main.rs:1378-1545`). A new field needs an entry there as well as in the struct.
- **Test-only filter:** run one test with `cargo test -j 8 -p manifold --bins <name_substring>`.
- **Full gate before each commit:**
  ```bash
  cargo fmt --all
  cargo fmt --all --check
  cargo clippy -j 8 -p manifold -p ipc-protocol --all-targets -- -D warnings
  cargo test -j 8 -p manifold --bins
  cargo test -j 8 -p ipc-protocol
  ```
- **Commit message:** `<Area>: <what changed>`, matching `git log` style ("Popups: ...", "Names: ..."), ending with the trailer line.
- **Line numbers** are at 4b833af and drift as tasks land. Find code by the function names given.

---

## File map

| File | Tasks |
|---|---|
| `crates/manifold/src/rgba.rs` (new) | 1 |
| `crates/manifold/src/gpu.rs` | 1, 2, 27 |
| `crates/manifold/src/fetched.rs` | 1, 9, 17, 26 |
| `crates/manifold/src/favicon_fetch.rs` | 1, 22, 26 |
| `crates/ipc-protocol/src/frame_buffer.rs` | 3 |
| `crates/manifold/src/process_manager.rs` | 3, 4, 23 |
| `crates/ipc-protocol/src/{lib,channel,transport}.rs` | 4, 26, 28 |
| `crates/ipc-protocol/src/messages.rs` | 5, 18, 26 |
| `crates/manifold/src/popup_gate.rs` (new) | 5 |
| engine `life_span_handler.rs` / `popup.rs` / webkit `main.rs` | 5 |
| `crates/manifold/src/local_state.rs`, `session_sync.rs` | 6, 12, 24, 29 |
| `crates/manifold/src/store/atomic.rs` (new), `store/{local,journal}.rs`, `jar.rs`, `redirect_fetch.rs` | 7 |
| `crates/manifold/src/session.rs`, `main.rs`, `settings_page.rs`, `popups.rs` | 8 |
| `crates/manifold/src/event_loop.rs` | 2, 5, 9, 10, 13, 18, 29 |
| `crates/manifold/src/tabs.rs` | 9, 14, 26 |
| `crates/manifold/src/debug_control.rs` | 10, 29 |
| `crates/manifold/src/engine_install.rs` | 11 |
| `crates/manifold/src/{closed,store/model,store/json,cli}.rs` | 12 |
| `crates/manifold/src/{graph_ui,settings_page,popups,tab_view,suggestions,shortcuts}.rs` | 13 |
| `crates/manifold/src/{card,graph_ui,level}.rs` | 14, 15, 25, 30 |
| `crates/manifold/src/{graph_layout,node}.rs` | 15, 30 |
| `crates/manifold/src/{pdf_fetch,image_view}.rs` | 16, 27 |
| `crates/manifold/src/{json_level,json_level_ui,json_view}.rs` | 17 |
| engine `input.rs` files | 18 |
| `crates/ipc-protocol/src/downloads.rs` (new), engine download code | 19 |
| `crates/manifold/src/{store/journal,store/crc32c,jar}.rs` | 20 |
| `crates/manifold/src/{profile,settings,nav_history,jar_defs,main}.rs`, `store/mod.rs` (testutil) | 21 |
| `crates/manifold/src/{net (new),redirect_fetch,favicon_fetch,engine_install,fetched,event_loop,session_sync}.rs` | 22 |
| engine `main.rs` files, webview2 `capture.rs`, cef `jar_cookies.rs`/`app.rs`/`Cargo.toml`, `process_manager.rs`, `README.md` | 23 |
| `crates/manifold/src/{main,event_loop,session_sync,fetched,crash_report,settings_page}.rs` | 24 |
| `crates/manifold/src/{card,popups,graph_ui,main}.rs` | 25 |
| docs and dead code across `manifold`, `ipc-protocol`, cef `app.rs`; `.githooks/pre-commit`, `CLAUDE.md` | 26 |
| `crates/manifold/src/gpu.rs`, `pdf_fetch.rs` → `pdf_render.rs` and its users, `README.md` | 27 |
| `crates/ipc-protocol/src/{nav_label (new),transport,messages}.rs`, engine download/nav/input code | 28 |
| `crates/manifold/src/{event_loop,debug_control,shortcuts,session_sync}.rs`, `session_sync/tests.rs` (new) | 29 |
| `crates/manifold/src/{card,node,image_view,main,tab_view}.rs` | 30 |

---

## Phase A: crashers, data loss, security

### Task 1: Oversize images and favicons can't crash the GPU path

**Findings:**
- **D-B1 (high).**
  - `gpu.rs` `upload_favicon` (~752) and `upload_image` (~804) call `create_texture_with_data` with no size check.
  - The device uses `wgpu::Limits::default()` (`gpu.rs:243`, max 2D side 8192), and no `on_uncaptured_error` handler is installed. wgpu 30's default handler `panic!`s.
  - Ways in:
    - a picked or dropped image (`fetched.rs` `insert_image_node_with_id`, ~481-497);
    - a web image converted to a node (up to 100 MB);
    - any site's `/favicon.ico` (up to 2 MB, e.g. a PNG declaring 9000x1).
  - A 100 MP photo (11648x8736) kills the app. The node is saved, so every later launch crashes on restore.
- **D-B5 (low).** `upload_image` registers a new egui texture and `insert`s it over an existing entry, leaking the old registration. This is reachable via PDF Refresh before the first render lands.
- **D-R2 (med).**
  - `favicon_fetch.rs:131` decodes with `image::load_from_memory` (no dimension cap, ~512 MiB alloc cap).
  - It then re-encodes the full-size image into the cache and uploads it.
- **D-R1, part (high, partly deferred).** `fetched.rs:494` does `img.to_rgba8()`, a full extra copy even when the image is already RGBA8. Use `into_rgba8()`. Moving the decode off the UI thread is deferred (see Deferred).
- **D-F2a (low).** `upload_favicon` and `upload_image` duplicate about 25 lines of texture creation. Fold them into one `register_rgba`.
- **D-F3 (low), partly.** A shared "decode with limits, fit to a max side" helper replaces the per-site decode dance.

**Files:**
- Create: `crates/manifold/src/rgba.rs`
- Modify: `crates/manifold/src/main.rs` (add `mod rgba;` in the module list, alphabetical, after `mod redirect_fetch;`)
- Modify: `crates/manifold/src/gpu.rs` (`upload_favicon`, `upload_image`, new private `register_rgba`)
- Modify: `crates/manifold/src/fetched.rs` (`insert_image_node_with_id`, ~470-500)
- Modify: `crates/manifold/src/favicon_fetch.rs` (`fetch_favicon` ~124-134, `read_png` ~165)
- Test: `crates/manifold/src/rgba.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces:
  - `pub(crate) const MAX_TEXTURE_SIDE: u32 = 8192;`
  - `pub(crate) const FAVICON_SIDE: u32 = 64;`
  - `pub(crate) fn fit_within(width: u32, height: u32, max: u32) -> (u32, u32)`
  - `pub(crate) fn fit_image(img: image::RgbaImage, max: u32) -> image::RgbaImage`
  - `pub(crate) fn decode_limited(bytes: &[u8], max_side: u32) -> image::ImageResult<image::RgbaImage>`

All are in `rgba.rs`.

- [ ] **Step 1: Write the failing tests** in the new `crates/manifold/src/rgba.rs`, with the module skeleton (empty fns returning wrong values are fine so it compiles; or write the tests first and let the compile error be the failure):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_size_within_the_limit_is_kept() {
        assert_eq!(fit_within(100, 50, 8192), (100, 50));
        assert_eq!(fit_within(8192, 8192, 8192), (8192, 8192));
    }

    #[test]
    fn an_oversize_image_is_scaled_to_the_limit_keeping_its_aspect() {
        assert_eq!(fit_within(11648, 8736, 8192), (8192, 6144));
        assert_eq!(fit_within(9000, 1, 8192), (8192, 1), "a side never drops to 0");
        assert_eq!(fit_within(1, 9000, 8192), (1, 8192));
    }

    #[test]
    fn the_limit_matches_the_device_limits_gpu_requests() {
        // GpuState::new requests `wgpu::Limits::default()`; if that ever changes, this must too.
        assert_eq!(wgpu::Limits::default().max_texture_dimension_2d, MAX_TEXTURE_SIDE);
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::RgbaImage::new(width, height).write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        bytes.into_inner()
    }

    #[test]
    fn a_decode_past_the_side_limit_is_refused() {
        assert!(decode_limited(&png(600, 1), 512).is_err());
        assert_eq!(decode_limited(&png(100, 40), 512).unwrap().dimensions(), (100, 40));
    }

    #[test]
    fn fit_image_downscales_only_when_needed() {
        assert_eq!(fit_image(image::RgbaImage::new(100, 100), FAVICON_SIDE).dimensions(), (64, 64));
        assert_eq!(fit_image(image::RgbaImage::new(16, 16), FAVICON_SIDE).dimensions(), (16, 16));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -j 8 -p manifold --bins rgba::`
Expected: FAIL. The first time, it fails as a compile error for the missing functions, or with assertion failures if you stubbed them.

- [ ] **Step 3: Implement `rgba.rs`** (above the test module):

```rust
//! Size rules for pixels headed to a GPU texture: the device's 2D side limit,
//! and decoding with a cap so a hostile file can't ask for gigabytes.

/// `wgpu::Limits::default().max_texture_dimension_2d`, which `GpuState::new`
/// requests. A texture past it is a wgpu validation error (a panic under the
/// default handler).
pub(crate) const MAX_TEXTURE_SIDE: u32 = 8192;

/// Favicons are drawn at most ~32 logical px; 64 covers a 2x display.
pub(crate) const FAVICON_SIDE: u32 = 64;

/// `(width, height)` scaled down, keeping the aspect ratio, so neither side
/// exceeds `max`. Never returns a 0 side.
pub(crate) fn fit_within(width: u32, height: u32, max: u32) -> (u32, u32) {
    if width <= max && height <= max {
        return (width, height);
    }
    let scale = f64::from(max) / f64::from(width.max(height));
    let side = |v: u32| ((f64::from(v) * scale).round() as u32).clamp(1, max);
    (side(width), side(height))
}

/// `img`, downscaled to `fit_within(.., max)` if it's larger.
pub(crate) fn fit_image(img: image::RgbaImage, max: u32) -> image::RgbaImage {
    let (w, h) = img.dimensions();
    let (fw, fh) = fit_within(w, h, max);
    if (fw, fh) == (w, h) {
        return img;
    }
    image::imageops::resize(&img, fw, fh, image::imageops::FilterType::Triangle)
}

/// Decodes `bytes` (format guessed from content), refusing an image wider or
/// taller than `max_side` before its pixels are allocated.
pub(crate) fn decode_limited(bytes: &[u8], max_side: u32) -> image::ImageResult<image::RgbaImage> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(max_side);
    limits.max_image_height = Some(max_side);
    reader.limits(limits);
    Ok(reader.decode()?.into_rgba8())
}
```

Check `fit_within(11648, 8736, 8192)`: the scale is 8192/11648, and 8736 × that is exactly 6144.0. With `.round()` this gives `(8192, 6144)`. Add `mod rgba;` to `main.rs`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -j 8 -p manifold --bins rgba::`
Expected: PASS (5 tests).

- [ ] **Step 5: Route the GPU uploads through one guarded helper** in `gpu.rs`.

Add a private method on `GpuState`, next to `upload_favicon`:

```rust
    /// One texture for tightly packed RGBA8, or `None` (logged) when it can't
    /// be one: a wrong byte count, or a side past the device limit. Callers
    /// downscale with `rgba::fit_image` first; this is the last guard, since
    /// wgpu's default error handler panics on an oversize texture.
    fn register_rgba(&mut self, label: &str, rgba: &[u8], width: u32, height: u32) -> Option<(wgpu::Texture, wgpu::TextureView)> {
        if !frame_is_valid(rgba.len(), width, height, self.device.limits().max_texture_dimension_2d) {
            eprintln!("{label}: refusing a {width}x{height} texture ({} bytes)", rgba.len());
            return None;
        }
        let texture = self.device.create_texture_with_data(
            &self.queue,
            &wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            rgba,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Some((texture, view))
    }
```

Rewrite `upload_favicon`'s first half as `let Some((texture, view)) = self.register_rgba("favicon texture", rgba, width, height) else { return };`. Keep the rest (update in place or register, plus FIFO eviction) unchanged.

Rewrite `upload_image` as follows. This fixes D-B5: the old registration is freed before a new one replaces it.

```rust
    pub fn upload_image(&mut self, key: &str, rgba: &[u8], width: u32, height: u32) {
        let Some((texture, view)) = self.register_rgba("image node texture", rgba, width, height) else { return };
        // A second upload for one key (a PDF re-rendered before its first
        // render landed) must not leak the first registration.
        self.free_image(key);
        let egui_id = self.egui_renderer.register_native_texture(&self.device, &view, wgpu::FilterMode::Linear);
        self.images.insert(key.to_string(), ThumbTexture { texture, width, height, egui_id });
    }
```

Update `upload_image`'s doc to drop "after a Refresh (which frees the old one first)" and say that a replace frees the old one itself.

- [ ] **Step 6: Downscale at the sources.**

In `fetched.rs` `insert_image_node_with_id`, replace

```rust
        let rgba = img.to_rgba8();
        let (width, height) = rgba.dimensions();
```

with

```rust
        // `into_rgba8` moves when the image is already RGBA8; `to_rgba8` copied it.
        let rgba = crate::rgba::fit_image(img.into_rgba8(), crate::rgba::MAX_TEXTURE_SIDE);
        let (width, height) = rgba.dimensions();
```

Make the binding `let img = match ...` usable by value; it already is.

In `favicon_fetch.rs` `fetch_favicon`, replace the decode line with

```rust
    let image = crate::rgba::decode_limited(&bytes, 512).ok()?;
    let image = crate::rgba::fit_image(image, crate::rgba::FAVICON_SIDE);
```

In `read_png`, make it `let image = crate::rgba::fit_image(image::open(path).ok()?.into_rgba8(), crate::rgba::FAVICON_SIDE);`. A cache file written before this fix may be full size.

- [ ] **Step 7: Run the full gate** (Conventions). Expected: all pass.

- [ ] **Step 8: Commit**

```bash
git add crates/manifold/src/rgba.rs crates/manifold/src/main.rs crates/manifold/src/gpu.rs crates/manifold/src/fetched.rs crates/manifold/src/favicon_fetch.rs
git commit -m "GPU: fit images and favicons to the texture limit; one guarded texture helper; free before replace

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: A lost or outdated surface recovers; GPU errors log instead of panicking

**Findings:**
- **D-B2 (med).** `gpu.rs` `render` (~1035) maps every non-success `get_current_texture` result to `surface_ok: false`, and the caller (`event_loop.rs:689`) just retries next frame. Nothing calls `surface.configure` except `resize` (`gpu.rs:451`). After `Outdated` or `Lost` with no resize event, the window stays frozen with no error. Examples are a display mode change, a driver reset, or this machine's USB4 eGPU being replugged. There is no `set_device_lost_callback`.
- **D-B1, second half.** No `on_uncaptured_error` handler is installed, so any wgpu validation error panics the app.

**Files:**
- Modify: `crates/manifold/src/gpu.rs`:
  - `GpuState` struct (~160), adding fields;
  - `GpuState::new` (~205-450);
  - `render`, around `get_current_texture` (~1035).
- Modify: `crates/manifold/src/event_loop.rs` (after `gpu.render(..)` returns, ~689)

**Interfaces:**
- Produces:
  - `GpuState::take_device_lost(&self) -> Option<String>`: returns the loss message once, then `None`.

- [ ] **Step 1: Store what surface recreation needs.**

Add these fields to `GpuState`:

```rust
    /// Kept to recreate `surface` after `CurrentSurfaceTexture::Lost`.
    instance: wgpu::Instance,
    window: std::sync::Arc<winit::window::Window>,
    /// The next `render` reconfigures before acquiring (set after `Suboptimal`).
    reconfigure: bool,
    /// Set by the device-lost callback; read once by `take_device_lost`.
    device_lost: std::sync::Arc<std::sync::Mutex<Option<String>>>,
```

In `new`, keep `instance` and `window.clone()` in the struct, and set `reconfigure: false`. Check the `window` parameter type at the top of `new` (`Arc<Window>`) and match it.

- [ ] **Step 2: Install the error handlers** in `new`, right after `request_device`:

```rust
        // wgpu's default handler panics. An error here (an oversize texture
        // that slipped past `register_rgba`, a driver hiccup) should cost a
        // frame, not the app and its unsaved state.
        device.on_uncaptured_error(std::sync::Arc::new(|err: wgpu::Error| eprintln!("wgpu error: {err}")));
        let device_lost = std::sync::Arc::new(std::sync::Mutex::new(None));
        let lost = device_lost.clone();
        device.set_device_lost_callback(move |reason, message| {
            eprintln!("wgpu device lost ({reason:?}): {message}");
            *lost.lock().unwrap_or_else(|e| e.into_inner()) = Some(format!("{message} ({reason:?})"));
        });
```

`DeviceLostReason::Destroyed` also fires when the app drops its own device at exit. Ignore that reason: `if matches!(reason, wgpu::DeviceLostReason::Destroyed) { return; }` before storing. Check the variant name in `wgpu-types-30.0.1/src/lib.rs` (`pub enum DeviceLostReason`).

```rust
    /// The device-lost message, once: the caller toasts it.
    pub fn take_device_lost(&self) -> Option<String> {
        self.device_lost.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
```

- [ ] **Step 3: Recover in `render`.** Replace the `let frame = match self.surface.get_current_texture() { .. }` block with:

```rust
        if std::mem::take(&mut self.reconfigure) {
            self.surface.configure(&self.device, &self.config);
        }
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) => t,
            // Presentable now, but wgpu asks for a reconfigure; done before the next acquire.
            wgpu::CurrentSurfaceTexture::Suboptimal(t) => {
                self.reconfigure = true;
                t
            }
            // The swapchain no longer matches the window (mode change, eGPU replug
            // with no resize event): without a reconfigure every later acquire fails too.
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.config);
                return RenderOutcome { surface_ok: false, screenshot_result: None };
            }
            // The surface itself is gone: make a new one for the same window.
            wgpu::CurrentSurfaceTexture::Lost => {
                match self.instance.create_surface(self.window.clone()) {
                    Ok(surface) => {
                        self.surface = surface;
                        self.surface.configure(&self.device, &self.config);
                    }
                    Err(err) => eprintln!("wgpu: couldn't recreate the lost surface: {err}"),
                }
                return RenderOutcome { surface_ok: false, screenshot_result: None };
            }
            // Timeout/Occluded/Validation: skip this frame and retry next redraw (keep the
            // existing comment about the pending screenshot and texture deltas here).
            _ => return RenderOutcome { surface_ok: false, screenshot_result: None },
        };
```

Keep the existing explanatory comment, moved onto the last arm. If `CurrentSurfaceTexture` has more variants in 30.0.1 (`wgpu-30.0.1/src/api/surface_texture.rs:48`), the `_` arm covers them.

- [ ] **Step 4: Toast a device loss once.** In `event_loop.rs`, right after the `let outcome = match egui_output { .. };` block inside `if let Some(gpu) = &mut self.gpu`, collect the message and toast it after the borrow ends:

```rust
            let lost = gpu.take_device_lost();
            // ... existing outcome handling unchanged ...
```

After the `if let Some(gpu)` block ends:

```rust
        if let Some(message) = lost {
            self.queue_toast(format!("The graphics device was lost ({message}). Restart manifold to draw again."));
        }
```

Declare `let mut lost = None;` before the `if let` and assign it inside, so it lives past the block.

- [ ] **Step 5: Verify.** This path needs a live surface and device, so it has no unit test. Run the full gate. Then run the app once, built with `cargo build -j 8 -p manifold` and launched at BelowNormal:

```powershell
cmd //c start "" //b //belownormal target\debug\manifold.exe
```

Confirm it draws normally, resize the window, then close it. Kill only the PID you started.

- [ ] **Step 6: Commit**

```bash
git add crates/manifold/src/gpu.rs crates/manifold/src/event_loop.rs
git commit -m "GPU: reconfigure an outdated surface, recreate a lost one, log errors and toast device loss

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Frame buffer files, engine teardown and per-tab logs clean up after themselves

**Findings:**
- **E-B1 (high).**
  - Each CEF, Servo or WebView2 host creates `%TEMP%\manifold-frame-<socket_id>.bin` (`ipc-protocol/src/frame_buffer.rs:28-30`). It is 8 MB at 1080p and 33 MB at 4K.
  - Nothing ever deletes it. `process_manager.rs` `Drop for EngineTab` (~264-308) kills the host and removes only `cache_dir`.
  - Measured: 526 files, 1.5 GB in `%LOCALAPPDATA%\Temp`.
- **B-R1 (med).** `Drop for EngineTab` polls a WebKit child for up to 3 s with `thread::sleep` on the UI thread. Replacing a session with N WebKit tabs freezes the window for up to 3N s.
- **B-R6 (low).** `per_tab_log_files` (`process_manager.rs:890`) uses `File::create(temp/manifold-{tab_id}.log)`, which truncates. A second instance or a respawn wipes the log needed to debug a crash. The name itself is relied on by README and `scripts/jar-probe.sh`, so keep it and append instead.

**Files:**
- Modify: `crates/ipc-protocol/src/frame_buffer.rs`: add `remove_frame_file` and `sweep_stale_frames_in`, plus tests.
- Modify: `crates/ipc-protocol/src/lib.rs`: re-export.
- Modify: `crates/manifold/src/process_manager.rs`:
  - `EngineTab.child` becomes `Option<Child>`;
  - `Drop` hands the child to a reaper thread;
  - `per_tab_log_files`;
  - add `sweep_frame_files`.
- Modify: `crates/manifold/src/main.rs` (~2710, next to `std::thread::spawn(process_manager::sweep_cef_cache);`)

**Interfaces:**
- Produces:
  - `ipc_protocol::frame_buffer::remove_frame_file(name: &str)`
  - `ipc_protocol::frame_buffer::sweep_stale_frames_in(dir: &Path, older_than: Duration) -> usize`, returning the number of files removed.
  - `process_manager::sweep_frame_files()`

- [ ] **Step 1: Write the failing tests** in `frame_buffer.rs`'s test module:

```rust
    fn sweep_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("manifold-frame-sweep-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_sweep_removes_only_old_frame_files() {
        let dir = sweep_dir("old");
        std::fs::write(dir.join("manifold-frame-a.bin"), [0u8; 8]).unwrap();
        std::fs::write(dir.join("manifold-store-x"), [0u8; 8]).unwrap();
        std::fs::write(dir.join("other.bin"), [0u8; 8]).unwrap();
        assert_eq!(sweep_stale_frames_in(&dir, Duration::from_secs(3600)), 0, "too new");
        assert_eq!(sweep_stale_frames_in(&dir, Duration::ZERO), 1);
        assert!(!dir.join("manifold-frame-a.bin").exists());
        assert!(dir.join("manifold-store-x").exists() && dir.join("other.bin").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A live engine keeps its frame file mapped; Windows refuses an exclusive
    /// open of a mapped file, which is how the sweep tells it apart.
    #[cfg(windows)]
    #[test]
    fn the_sweep_spares_a_mapped_frame_file() {
        let name = format!("test-sweep-live-{}", std::process::id());
        let live = SharedFrameBuffer::create(&name, 4, 4).unwrap();
        let dir = std::env::temp_dir();
        sweep_stale_frames_in(&dir, Duration::ZERO);
        assert!(frame_path(&name).exists(), "a mapped file survives");
        drop(live);
        remove_frame_file(&name);
        assert!(!frame_path(&name).exists());
    }
```

Add `use std::time::Duration;` and `use std::path::Path;` as needed.

**Note on the live test:** it sweeps the real temp dir with `older_than = 0`. That deletes other stale `manifold-frame-*.bin` files, which is the point of the feature, but a running manifold's frames are mapped, so they're spared.

- [ ] **Step 2: Run to verify failure.** Run `cargo test -j 8 -p ipc-protocol frame_buffer`. Expected: compile failure, because the functions are missing.

- [ ] **Step 3: Implement** in `frame_buffer.rs`:

```rust
/// Deletes `name`'s frame file. Only call once its engine-host has exited:
/// on Windows a file still mapped elsewhere can't be removed (the error is ignored).
pub fn remove_frame_file(name: &str) {
    let _ = std::fs::remove_file(frame_path(name));
}

/// Deletes `manifold-frame-*.bin` files in `dir` last modified more than
/// `older_than` ago and not in use. A live engine-host keeps its file
/// mapped. On Windows that makes an exclusive open fail, so a file that opens
/// with `share_mode(0)` belongs to no one. On Unix age alone decides, so
/// callers pass a generous age.
pub fn sweep_stale_frames_in(dir: &Path, older_than: std::time::Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !(name.starts_with("manifold-frame-") && name.ends_with(".bin")) {
            continue;
        }
        let path = entry.path();
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age >= older_than);
        if old && unused(&path) && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

#[cfg(windows)]
fn unused(path: &Path) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    // Dropped before the delete below; only the open's success matters.
    OpenOptions::new().read(true).share_mode(0).open(path).is_ok()
}

#[cfg(not(windows))]
fn unused(_path: &Path) -> bool {
    true
}
```

Change the `use std::path::PathBuf;` import to `use std::path::{Path, PathBuf};`. In `lib.rs`, extend the re-export to `pub use frame_buffer::{frame_path, remove_frame_file, sweep_stale_frames_in, SharedFrameBuffer};`.

- [ ] **Step 4: Run `cargo test -j 8 -p ipc-protocol frame_buffer`.** Expected: PASS. If `the_sweep_spares_a_mapped_frame_file` fails, the exclusive-open assumption is wrong on this Windows build. Stop and report; don't weaken the test. The fallback is an age-only sweep with `older_than` of one day.

- [ ] **Step 5: Reap engine hosts off the UI thread and remove their frame file.** In `process_manager.rs`:

1. Change `child: Child` to `child: Option<Child>` in `EngineTab`. At both constructors (~1126 local, ~1216 WebKit), write `child: Some(child)`.
2. Replace the body of `Drop for EngineTab` after the `Shutdown` send with a hand-off. This keeps the existing WebKit comment, moved into `reap_host`.

```rust
impl Drop for EngineTab {
    fn drop(&mut self) {
        self.to_engine.send(&HostToEngine::Shutdown);
        let Some(child) = self.child.take() else { return };
        let (kind, socket_id) = (self.kind, self.socket_id.clone());
        let (cache_dir, slot) = (self.cache_dir.take(), self.profile_slot.take());
        // Off the UI thread: a WebKit host gets up to 3 s to exit on its own,
        // and dropping a session of N of them used to freeze the window for 3N s.
        let reaper = std::thread::spawn(move || {
            reap_host(kind, child);
            // The host has exited, so its frame file is no longer mapped by it.
            if kind != EngineKind::Webkit {
                ipc_protocol::remove_frame_file(&socket_id);
            }
            if let Some(dir) = cache_dir {
                remove_cache_dir(dir, slot);
            }
        });
        CACHE_REMOVALS.lock().unwrap_or_else(|e| e.into_inner()).push(reaper);
    }
}

/// Waits for an engine host to exit after `Shutdown`, killing it if needed.
fn reap_host(kind: EngineKind, mut child: Child) {
    if kind == EngineKind::Webkit {
        // (existing WebKit comment, unchanged)
        for _ in 0..30 {
            if matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}
```

`remove_cache_dir` is called from the reaper thread and itself pushes another handle into `CACHE_REMOVALS`. `finish_cache_removals` must therefore loop until the list is empty, not take it once:

```rust
pub fn finish_cache_removals() {
    loop {
        let removals = std::mem::take(&mut *CACHE_REMOVALS.lock().unwrap_or_else(|e| e.into_inner()));
        if removals.is_empty() {
            return;
        }
        for removal in removals {
            let _ = removal.join();
        }
    }
}
```

Update `finish_cache_removals`' doc: it waits for every reap and cache removal. The WebKit wait can make exit take up to 3 s, the same as before, when Drop did it inline.

Grep for other uses of `.child` on `EngineTab` (`grep -n "\.child\b" crates/manifold/src/process_manager.rs`) and adapt each one to the `Option`.

- [ ] **Step 6: Append to per-tab logs.** In `per_tab_log_files`, replace `std::fs::File::create(..)` with:

```rust
    let mut stdout_file =
        std::fs::OpenOptions::new().create(true).append(true).open(per_tab_log_path(tab_id))?;
    // Appended, not truncated: a second manifold, or a respawn racing the old
    // host's last writes, used to wipe the log that explains a crash.
    use std::io::Write as _;
    let _ = writeln!(stdout_file, "---- manifold pid {} spawning a host for {tab_id} ----", std::process::id());
```

`log_tail` already reads only the last 16 KiB, so appending doesn't change the connect-error text.

- [ ] **Step 7: Sweep at startup.** In `process_manager.rs`:

```rust
/// Frame files left by hosts that never got reaped (manifold killed or
/// crashed): any `manifold-frame-*.bin` in temp that no running host has
/// mapped and that is over ten minutes old.
pub fn sweep_frame_files() {
    let removed = ipc_protocol::sweep_stale_frames_in(&std::env::temp_dir(), Duration::from_secs(600));
    if removed > 0 {
        eprintln!("removed {removed} stale frame buffer file(s)");
    }
}
```

In `main.rs` add `std::thread::spawn(process_manager::sweep_frame_files);` after the `sweep_webview2_profiles` spawn.

- [ ] **Step 8: Verify.**
  1. Run the full gate.
  2. Build with `cargo build -j 8 -p manifold`.
  3. Count the frame files before: `ls "$LOCALAPPDATA/Temp"/manifold-frame-*.bin | wc -l`.
  4. Start manifold once at BelowNormal and close it.
  5. Count again. Expected: the count drops to the files of any other running manifold, at most a handful.
  6. Close a tab while it runs. Expected: its `manifold-frame-<id>.bin` is gone within a second.

- [ ] **Step 9: Commit**

```bash
git add crates/ipc-protocol/src/frame_buffer.rs crates/ipc-protocol/src/lib.rs crates/manifold/src/process_manager.rs crates/manifold/src/main.rs
git commit -m "Engines: delete frame files when a host exits and sweep stale ones; reap hosts off the UI thread; append per-tab logs

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Protocol version handshake; `accept_remote` keeps its deadline; protocol tests

**Findings:**
- **E-R1 (med).** postcard encodes enum variants by position. A stale engine host decodes a newer host's messages as other messages and nobody notices; for example, an old WebKit host would read `SetCookies` as `GetCookies`. Stale hosts are realistic here: WebKit is built separately in WSL, and CEF runs from a copy.
- **E-B3 (med, security/robustness).** `transport.rs` `accept_remote` (~200-226) checks the deadline only on `WouldBlock`. A connection with a wrong token loops straight back to `accept`. Each bad peer costs up to 1 s on the UI thread, and the listener is on 0.0.0.0. Anyone who keeps connecting freezes manifold indefinitely.
- **E-F8 (low, test gap).** There are no tests for:
  - a truncated payload;
  - garbage postcard bytes;
  - `spawn_host_reader` turning a dropped peer into `Shutdown`.

**Design.**
- Each side writes `PROTOCOL_VERSION` (u32 LE, 4 raw bytes) as the very first bytes of the stream it sends on. The reader of that stream checks it before any postcard message.
  - **Local pair:**
    - The host writes on its sender in `accept_local_pair_while`.
    - The engine writes on its sender in `connect_local_pair`.
  - **Remote:**
    - The host writes after the token is accepted, in `accept_remote`.
    - The engine writes after the token, in `connect_remote`.
  - **Readers:**
    - The engine's `spawn_host_reader` reads it first. On a mismatch it logs and delivers `Shutdown`.
    - The host's `process_manager::spawn_reader_thread` reads it first. On a mismatch it delivers `Crashed { reason }` with a reason starting with `PROTOCOL_MISMATCH`, and Task 9 shows that as a Failed card without respawning.
- All four engines connect only through these functions (cef `main.rs:169-170`, servo `169-174`, webkit `60-66`, webview2 `146-147`), so no engine code changes.
- **Old-meets-new behaviour:** an old peer's first 4 bytes are a length prefix, which reads as a wrong version. Each direction fails fast.

**Files:**
- Modify: `crates/ipc-protocol/src/lib.rs`: add `PROTOCOL_VERSION` and `PROTOCOL_MISMATCH`.
- Modify: `crates/ipc-protocol/src/channel.rs`: add `send_version`/`recv_version`, plus tests.
- Modify: `crates/ipc-protocol/src/transport.rs`:
  - `accept_local_pair_while`, `connect_local_pair`, `accept_remote`, `connect_remote`, `spawn_host_reader`;
  - tests.
- Modify: `crates/manifold/src/process_manager.rs` (`spawn_reader_thread`, ~976)

**Interfaces:**
- Produces:
  - `pub const PROTOCOL_VERSION: u32 = 1;`
  - `pub const PROTOCOL_MISMATCH: &str = "protocol mismatch";`
  - `MessageChannel::send_version(&mut self) -> io::Result<()>`
  - `MessageChannel::recv_version(&mut self) -> io::Result<u32>`
  - `ipc_protocol::check_version(theirs: u32) -> Result<(), String>`: `Err` carries a message that starts with `PROTOCOL_MISMATCH`.
- Later protocol changes (Tasks 5, 18) bump `PROTOCOL_VERSION`.

- [ ] **Step 1: Write the failing tests.** In `channel.rs` tests:

```rust
    #[test]
    fn a_version_round_trips_ahead_of_messages() {
        let mut sender = MessageChannel::new(Cursor::new(Vec::new()));
        sender.send_version().unwrap();
        sender.send(&7u32).unwrap();
        let mut receiver = MessageChannel::new(Cursor::new(sender.into_inner().into_inner()));
        assert_eq!(receiver.recv_version().unwrap(), crate::PROTOCOL_VERSION);
        assert_eq!(receiver.recv::<u32>().unwrap(), 7);
    }

    #[test]
    fn a_truncated_payload_is_an_eof_not_a_hang() {
        let mut bytes = 10u32.to_le_bytes().to_vec();
        bytes.extend_from_slice(&[1, 2, 3]);
        let err = MessageChannel::new(Cursor::new(bytes)).recv::<u32>().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn garbage_payload_is_invalid_data_not_a_panic() {
        let mut bytes = 4u32.to_le_bytes().to_vec();
        bytes.extend_from_slice(&[0xff, 0xff, 0xff, 0xff]);
        let err = MessageChannel::new(Cursor::new(bytes)).recv::<crate::HostToEngine>().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }
```

In `lib.rs` tests:

```rust
    #[test]
    fn a_version_mismatch_names_both_versions() {
        assert!(check_version(PROTOCOL_VERSION).is_ok());
        let err = check_version(PROTOCOL_VERSION + 1).unwrap_err();
        assert!(err.starts_with(PROTOCOL_MISMATCH), "{err}");
        assert!(err.contains(&(PROTOCOL_VERSION + 1).to_string()));
    }
```

In `transport.rs` tests, add:

```rust
    #[test]
    fn a_dropped_host_becomes_shutdown() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let host = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.write_all(&crate::PROTOCOL_VERSION.to_le_bytes()).unwrap();
            // dropped: the engine's reader must turn this into Shutdown
        });
        let rx = spawn_host_reader(MessageChannel::new(TcpStream::connect(("127.0.0.1", port)).unwrap()));
        host.join().unwrap();
        assert!(matches!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), HostToEngine::Shutdown));
    }

    #[test]
    fn a_wrong_version_from_the_host_becomes_shutdown() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let host = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.write_all(&(crate::PROTOCOL_VERSION + 1).to_le_bytes()).unwrap();
            // A well-formed Navigate after it must never be delivered.
            MessageChannel::new(stream)
                .send(&HostToEngine::Navigate { url: "https://x".into(), referrer: None })
                .unwrap();
        });
        let rx = spawn_host_reader(MessageChannel::new(TcpStream::connect(("127.0.0.1", port)).unwrap()));
        host.join().unwrap();
        assert!(matches!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), HostToEngine::Shutdown));
    }

    /// An intruder that keeps reconnecting with a bad token must not keep
    /// `accept_remote` past its deadline.
    #[test]
    fn accept_remote_times_out_under_a_stream_of_bad_peers() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let intruder = {
            let stop = stop.clone();
            std::thread::spawn(move || {
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    if let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) {
                        let _ = s.write_all(&[0u8; TOKEN_LEN]);
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
            })
        };
        let start = Instant::now();
        let result = accept_remote(&listener, Duration::from_millis(300), &[7; TOKEN_LEN]);
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        intruder.join().unwrap();
        assert_eq!(result.err().map(|e| e.kind()), Some(io::ErrorKind::TimedOut));
        assert!(start.elapsed() < Duration::from_secs(2), "took {:?}", start.elapsed());
    }
```

Then update the existing tests that exchange messages so they still pass after Step 3 has made both sides write a version:
- `lib.rs` `local_transport_loopback_ping`
- `transport.rs` `only_a_peer_with_the_token_is_accepted`

No change should be needed in either, because both use the paired functions. Check this by running them.

- [ ] **Step 2: Run** `cargo test -j 8 -p ipc-protocol`. Expected: compile errors (missing functions).

- [ ] **Step 3: Implement.**

In `lib.rs`:

```rust
/// Written as the first 4 bytes (u32 LE) of every control stream, before any
/// postcard message. postcard encodes enum variants by position, so a host
/// and engine built from different `messages.rs` would silently misread each
/// other. Bump it on any change to `HostToEngine`, `EngineToHost` or a type
/// they carry.
pub const PROTOCOL_VERSION: u32 = 1;

/// Starts the `EngineToHost::Crashed` reason for a version mismatch, so the
/// host shows a failed card instead of respawning the same stale binary.
pub const PROTOCOL_MISMATCH: &str = "protocol mismatch";

/// `Ok` when the peer speaks `PROTOCOL_VERSION`.
pub fn check_version(theirs: u32) -> Result<(), String> {
    if theirs == PROTOCOL_VERSION {
        return Ok(());
    }
    Err(format!(
        "{PROTOCOL_MISMATCH}: the engine host speaks protocol {theirs}, manifold speaks {PROTOCOL_VERSION}; rebuild the engine host"
    ))
}
```

In `channel.rs` (inside `impl<S: Read + Write> MessageChannel<S>`):

```rust
    /// Writes `PROTOCOL_VERSION`; the first thing each side sends.
    pub fn send_version(&mut self) -> io::Result<()> {
        self.stream.write_all(&crate::PROTOCOL_VERSION.to_le_bytes())?;
        self.stream.flush()
    }

    /// Reads the peer's `send_version`.
    pub fn recv_version(&mut self) -> io::Result<u32> {
        let mut buf = [0u8; 4];
        self.stream.read_exact(&mut buf)?;
        Ok(u32::from_le_bytes(buf))
    }
```

In `transport.rs`:

- `accept_local_pair_while`: `let mut sender = ..; sender.send_version()?;` before `Ok((sender, receiver))`.
- `connect_local_pair`: `let mut sender = ..; sender.send_version()?;`.
- `connect_remote`: after `stream.write_all(token)?;`, `let mut channel = MessageChannel::new(stream); channel.send_version()?; Ok(channel)`.
- `accept_remote`, in the success branch: `let mut channel = MessageChannel::new(stream); channel.send_version()?; return Ok(channel);`.
- `accept_remote`, deadline fix. Replace the `Ok((mut stream, _addr))` arm with:

```rust
            Ok((mut stream, _addr)) => {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!("timed out after {timeout:?} waiting for engine-host to connect"),
                    ));
                }
                stream.set_nonblocking(false)?;
                stream.set_read_timeout(Some(left.min(Duration::from_secs(1))))?;
                let mut sent = [0u8; TOKEN_LEN];
                if stream.read_exact(&mut sent).is_ok() && &sent == token {
                    stream.set_read_timeout(None)?;
                    let mut channel = MessageChannel::new(stream);
                    channel.send_version()?;
                    return Ok(channel);
                }
            }
```

Also add the timeout-error construction as a small closure or fn, `timed_out(timeout)`, used by both arms.

- `spawn_host_reader`: check the version first:

```rust
    std::thread::spawn(move || {
        match from_host.recv_version().map(crate::check_version) {
            Ok(Ok(())) => {}
            Ok(Err(mismatch)) => {
                eprintln!("{mismatch}");
                let _ = tx.send(HostToEngine::Shutdown);
                return;
            }
            Err(_) => {
                let _ = tx.send(HostToEngine::Shutdown);
                return;
            }
        }
        loop {
            let msg = from_host.recv::<HostToEngine>().unwrap_or(HostToEngine::Shutdown);
            let is_shutdown = matches!(msg, HostToEngine::Shutdown);
            if tx.send(msg).is_err() || is_shutdown {
                break;
            }
        }
    });
```

Add to `spawn_host_reader`'s doc: "Reads the host's `PROTOCOL_VERSION` first; a mismatch is a `Shutdown`."

- [ ] **Step 4: Host reader.** In `process_manager.rs` `spawn_reader_thread`, before the loop:

```rust
        let version = match &mut from_engine {
            RemoteOrLocalReceiver::Local(t) => t.recv_version(),
            RemoteOrLocalReceiver::Remote(t) => t.recv_version(),
        };
        match version.map(ipc_protocol::check_version) {
            Ok(Ok(())) => {}
            Ok(Err(mismatch)) => {
                eprintln!("{mismatch}");
                let _ = tx.send(EngineToHost::Crashed { reason: mismatch });
                return;
            }
            Err(_) => {
                let _ = tx.send(EngineToHost::Crashed { reason: "engine-host connection lost".to_string() });
                return;
            }
        }
```

- [ ] **Step 5: Run** `cargo test -j 8 -p ipc-protocol` and `cargo test -j 8 -p manifold --bins process_manager`. Expected: PASS.

- [ ] **Step 6: Engine checks.** The engines' calls are unchanged in signature. Run `cargo check -j 8 -p manifold-engine-cef` and `cargo check -j 8 -p manifold-engine-webview2`; both are expected to pass. WebKit is checked in WSL if available. Servo is checked by reading only: it uses `connect_local_pair` and `spawn_host_reader` unchanged.

- [ ] **Step 7: Live check.** Rebuild the hosts first: `cargo build -j 8 -p manifold -p manifold-engine-webview2 -p manifold-engine-cef`. Then start manifold at BelowNormal and open a WebView2 tab and a CEF tab. Both must load a page. A tab that shows "protocol mismatch" means a stale host binary was used: the CEF host runs from a copy (`ensure_host_copy`), so check that the copy was refreshed.

- [ ] **Step 8: Run the full gate, then commit.**

```bash
git add crates/ipc-protocol/src crates/manifold/src/process_manager.rs
git commit -m "IPC: exchange a protocol version before any message; accept_remote honours its deadline under bad peers; protocol tests

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Page popups need a gesture, a safe scheme and a rate budget

**Findings:**
- **E-B2 (high, security/DoS).** Every engine queues every popup URL with no check:
  - CEF `life_span_handler.rs` `on_before_popup` (~48-72) ignores `_user_gesture`.
  - WebView2 `popup.rs` `register` never reads `IsUserInitiated`.
  - WebKit `main.rs` `connect_create` (~196) pushes the URI unconditionally.

  The host (`event_loop.rs:1653` → `tabs.rs` `spawn_tab_from`) spawns a full engine process for each one.
  - `setInterval(() => window.open('https://x'), 10)` exhausts the machine.
  - `window.open('file:///C:/...')` becomes a browser-initiated navigation of a new tab, which bypasses the renderer's file: block.
- **B-R3 (low-med).** The same issue seen from the host: no rate limit and no scheme filter.
- **E-B2 also:** `ContextMenuLink.href` reaches "Open in new tab" (`node_menu.rs:566-571`) with any scheme (`javascript:`, `file:`).

**Design:**
- `EngineToHost::OpenTabRequested { url, user_gesture: bool }`. Bump `PROTOCOL_VERSION` to 2.
- Pure host policy in a new `popup_gate.rs`:
  - `page_tab_url_allowed(url)` allows only `http:`, `https:`, `about:blank` and `manifold:` URLs (case-insensitive scheme).
  - `PopupGate` allows at most 5 page-opened tabs per source tab per 10 s, and drops gesture-less requests.
  - A drop is toasted at most once per source tab per 10 s.
- WebKit already blocks script popups by default (`javascript-can-open-windows-automatically` is false), but it sends the flag too.

**Files:**
- Create: `crates/manifold/src/popup_gate.rs` (plus `mod popup_gate;` in `main.rs`)
- Modify: `crates/ipc-protocol/src/messages.rs` (`OpenTabRequested`, ~414) and `lib.rs` (`PROTOCOL_VERSION = 2`)
- Modify: `crates/manifold/src/event_loop.rs` (the `OpenTabRequested` arm, ~1653)
- Modify: `crates/manifold/src/node_menu.rs` (~566, context-menu "Open in new tab")
- Modify: `crates/manifold/src/main.rs` (`App` field `popup_gate: popup_gate::PopupGate`, plus Default)
- Modify:
  - `crates/manifold-engine-cef/src/life_span_handler.rs` (`SharedPendingOpenTab` becomes `Rc<RefCell<Vec<(String, bool)>>>`, and `on_before_popup` pushes `(url, user_gesture != 0)`)
  - `crates/manifold-engine-cef/src/main.rs` (~439 drain)
- Modify:
  - `crates/manifold-engine-webview2/src/popup.rs` (`PendingPopup` is `Vec<(String, bool)>`, reading `args.IsUserInitiated`)
  - `crates/manifold-engine-webview2/src/main.rs` (~450 drain)
- Modify: `crates/manifold-engine-webkit/src/main.rs` (~189-200: push `(uri, nav_action.is_user_gesture())`; ~467 drain)

**Interfaces:**
- Produces:
  - `popup_gate::page_tab_url_allowed(url: &str) -> bool`
  - `popup_gate::PopupGate::admit(&mut self, source: &str, user_gesture: bool, now: Instant) -> Admit`
  - `enum Admit { Open, Drop { toast: bool } }`
- Consumes: `ipc_protocol::PROTOCOL_VERSION` (Task 4).

- [ ] **Step 1: Write the failing tests** in the new `popup_gate.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn only_web_and_internal_schemes_may_open_a_tab() {
        for ok in ["https://a.org/x", "HTTP://a.org", "about:blank", "manifold://welcome"] {
            assert!(page_tab_url_allowed(ok), "{ok}");
        }
        for bad in ["file:///C:/x", "javascript:alert(1)", "about:config", "ms-settings:", "data:text/html,x", "", "blank"] {
            assert!(!page_tab_url_allowed(bad), "{bad}");
        }
    }

    #[test]
    fn a_popup_without_a_gesture_is_dropped() {
        let mut gate = PopupGate::default();
        assert_eq!(gate.admit("t", false, Instant::now()), Admit::Drop { toast: true });
    }

    #[test]
    fn gestured_popups_are_capped_per_source_per_window() {
        let mut gate = PopupGate::default();
        let t0 = Instant::now();
        for _ in 0..POPUP_BURST {
            assert_eq!(gate.admit("t", true, t0), Admit::Open);
        }
        assert_eq!(gate.admit("t", true, t0), Admit::Drop { toast: true });
        assert_eq!(gate.admit("t", true, t0), Admit::Drop { toast: false }, "one toast per window");
        assert_eq!(gate.admit("other", true, t0), Admit::Open, "per source tab");
        assert_eq!(gate.admit("t", true, t0 + POPUP_WINDOW + Duration::from_millis(1)), Admit::Open);
    }
}
```

- [ ] **Step 2: Run** `cargo test -j 8 -p manifold --bins popup_gate`. Expected: compile failure.

- [ ] **Step 3: Implement `popup_gate.rs`:**

```rust
//! What a page may open as a new tab. A page-opened tab is a whole engine
//! process, and its URL becomes a browser-initiated navigation that skips the
//! renderer's own checks (a `file:` URL would open), so the host decides.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// At most this many page-opened tabs per source tab per `POPUP_WINDOW`.
pub(crate) const POPUP_BURST: usize = 5;
pub(crate) const POPUP_WINDOW: Duration = Duration::from_secs(10);

/// Whether a URL that came from page content (a popup, a context-menu link)
/// may become a new tab's first navigation.
pub(crate) fn page_tab_url_allowed(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    lower == "about:blank"
        || ["http://", "https://", "manifold://"].iter().any(|scheme| lower.starts_with(scheme))
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Admit {
    Open,
    /// `toast`: first drop for this source in the current window.
    Drop { toast: bool },
}

#[derive(Default)]
pub(crate) struct PopupGate {
    /// Per source tab: opens in the current window, and when the last drop was toasted.
    by_source: HashMap<String, (Vec<Instant>, Option<Instant>)>,
}

impl PopupGate {
    pub(crate) fn admit(&mut self, source: &str, user_gesture: bool, now: Instant) -> Admit {
        let (opens, toasted) = self.by_source.entry(source.to_string()).or_default();
        opens.retain(|&t| now.duration_since(t) <= POPUP_WINDOW);
        if user_gesture && opens.len() < POPUP_BURST {
            opens.push(now);
            return Admit::Open;
        }
        let toast = toasted.is_none_or(|t| now.duration_since(t) > POPUP_WINDOW);
        if toast {
            *toasted = Some(now);
        }
        Admit::Drop { toast }
    }

    /// Forget a closed tab.
    pub(crate) fn forget(&mut self, source: &str) {
        self.by_source.remove(source);
    }
}
```

`Option::is_none_or` is stable since Rust 1.82. If the toolchain is older, use `map_or(true, ..)`.

- [ ] **Step 4: Run** `cargo test -j 8 -p manifold --bins popup_gate`. Expected: PASS.

- [ ] **Step 5: Protocol change.**
  1. In `messages.rs`, change the variant to `OpenTabRequested { url: String, user_gesture: bool }`. Add a doc line: "`user_gesture`: the engine saw a user action (click, key) behind the request; the host drops requests without one."
  2. Set `PROTOCOL_VERSION = 2` in `lib.rs`.
  3. Fix the `messages.rs` docs of the variant that say Servo doesn't send it; unchanged.

- [ ] **Step 6: Host side.**

Add `popup_gate: popup_gate::PopupGate` to `App` (Default: `PopupGate::default()`).

In `event_loop.rs`, replace the arm with:

```rust
            EngineToHost::OpenTabRequested { url, user_gesture } => {
                if !crate::popup_gate::page_tab_url_allowed(&url) {
                    eprintln!("popup to a disallowed scheme dropped: {url}");
                } else {
                    match self.popup_gate.admit(tab_id, user_gesture, std::time::Instant::now()) {
                        crate::popup_gate::Admit::Open => {
                            self.spawn_tab_from(tab_id, &url);
                        }
                        crate::popup_gate::Admit::Drop { toast } => {
                            if toast {
                                self.queue_toast("Blocked pop-ups from this page".to_string());
                            }
                        }
                    }
                }
            }
```

Call `self.popup_gate.forget(id)` where a tab node is finally removed (`remove_tab` in `tabs.rs`; grep `fn remove_tab`).

In `node_menu.rs`, guard the context-menu open:

```rust
        if open_tab {
            if !crate::popup_gate::page_tab_url_allowed(&menu.href) {
                self.queue_toast(format!("Can't open {} in a new tab", menu.href));
            } else if let Some(opener) = self.focused.clone() {
                self.spawn_tab_from(&opener, &menu.href);
            } else {
                self.spawn_new_tab(None, None, &menu.href);
            }
        }
```

- [ ] **Step 7: Engine side.**
- **CEF:**
  - In `life_span_handler.rs`, set `pub type SharedPendingOpenTab = Rc<RefCell<Vec<(String, bool)>>>;`.
  - In `on_before_popup`, rename `_user_gesture` to `user_gesture` and push `(url.to_string(), user_gesture != 0)`.
  - In `main.rs` ~439, use `for (url, user_gesture) in pending_open_tab.borrow_mut().drain(..) { let _ = control.send(&EngineToHost::OpenTabRequested { url, user_gesture }); }`.
- **WebView2 `popup.rs`:** set `pub type PendingPopup = Rc<RefCell<Vec<(String, bool)>>>;`. In the handler:

```rust
            let mut uri = PWSTR::null();
            let mut user = windows::Win32::Foundation::BOOL::default();
            let user_gesture = unsafe { args.IsUserInitiated(&mut user) }.is_ok() && user.as_bool();
            if unsafe { args.Uri(&mut uri) }.is_ok() {
                pending_for_handler.borrow_mut().push((take_pwstr(uri), user_gesture));
            }
```

  The `BOOL` path can differ by `windows` crate version. Use whatever `ICoreWebView2NewWindowRequestedEventArgs::IsUserInitiated` takes in the version in `Cargo.lock`, then fix the drain in `main.rs` ~450.
- **WebKit:** in `main.rs` ~189, set the queue type to `Rc<RefCell<Vec<(String, bool)>>>`. In `connect_create`, use `pending_open_tab.borrow_mut().push((uri.to_string(), nav_action.is_user_gesture()));`. Here `nav_action` is `&NavigationAction`, which has `is_user_gesture()` in webkit2gtk-rs; it needs `mut` in some versions, so check the binding. Fix the drain at ~467.

- [ ] **Step 8: Verify.**
  1. Run the full gate.
  2. Run `cargo check -j 8 -p manifold-engine-cef` and `cargo check -j 8 -p manifold-engine-webview2`. Expected: pass.
  3. WebKit: run the WSL check if available; otherwise verify by reading.
  4. Rebuild the engines and manifold. On a page with `<a target=_blank href=https://example.org>`, a click opens a tab.
  5. Run `window.open('https://example.org')` from a `setTimeout` with no click. Expected: a "Blocked pop-ups" toast. To run it, use the CEF devtools, or put a `javascript:` URL in the address bar if the build allows it; if neither is available, note that this step was reasoned rather than run.

- [ ] **Step 9: Commit**

```bash
git add crates/manifold/src/popup_gate.rs crates/manifold/src/main.rs crates/manifold/src/event_loop.rs crates/manifold/src/node_menu.rs crates/manifold/src/tabs.rs crates/ipc-protocol/src crates/manifold-engine-cef/src crates/manifold-engine-webview2/src crates/manifold-engine-webkit/src
git commit -m "Popups: page-opened tabs need a user gesture, an http(s)/about:blank/manifold scheme and a per-tab budget

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: A non-finite position or camera value no longer wipes personal state

**Finding: A-B1 (high).**
- **Where:** `local_state.rs`:
  - `LocalNode.pos: [f32; 2]` (51);
  - `camera: Option<([f32; 4], Option<String>)>` (29);
  - `to_bytes`/`from_bytes` (116-123).
- **Writers:** `session_sync.rs:1068` (pos) and `:1109` (camera) don't filter.
- **Failure:**
  - serde_json writes NaN/inf as `null` and reading `null` into `f32` fails, so `from_bytes` returns `None` for the whole state.
  - On the next launch everything is gone: positions, titles, zooms, mutes, live URLs, the camera and **all of Recently Closed**. The next write makes the loss permanent.
  - The same bytes make `session::content_names` return `None` (content pruning stalls), and `load_named_session`'s `to_value` fails the same way.
- **Fix, once, in `LocalState`:**
  - Sanitize on write: drop a node entry with a non-finite `pos` (that node falls back to its shared position), and drop a non-finite `camera` or `json_outline_width`.
  - Read leniently: a `null` inside `pos` reads as 0, and a `null` inside `camera` reads as no camera. That rescues files already written with `null`.
  - `zoom: Option<f64>` already reads `null` as `None`.

**Files:**
- Modify: `crates/manifold/src/local_state.rs`
- Test: `crates/manifold/src/local_state.rs` (`mod tests`)

**Interfaces:**
- `LocalState::to_bytes(&self) -> Vec<u8>` and `from_bytes(&[u8]) -> Option<LocalState>` keep their signatures.

- [ ] **Step 1: Write the failing tests** in `local_state.rs` tests:

```rust
    #[test]
    fn a_non_finite_value_costs_only_itself() {
        let mut state = with_pos(f32::NAN);
        state.nodes.insert("b".into(), LocalNode { pos: [5.0, 6.0], title: "kept".into(), ..LocalNode::default() });
        state.camera = Some(([0.0, f32::INFINITY, 1.0, 1.0], None));
        state.json_outline_width = Some(f32::NAN);
        state.closed = vec![JsonDoc::default()];
        let back = LocalState::from_bytes(&state.to_bytes()).expect("still readable");
        assert_eq!(back.closed.len(), 1, "Recently Closed survives");
        assert!(!back.nodes.contains_key("a"), "the bad position is dropped");
        assert_eq!(back.nodes["b"].title, "kept");
        assert_eq!((back.camera, back.json_outline_width), (None, None));
    }

    /// Files written before the fix hold `null` where the value was NaN.
    #[test]
    fn nulls_written_by_older_builds_read_back() {
        let bytes = br#"{"camera":[[0.0,null,1.0,1.0],null],"nodes":{"a":{"pos":[null,2.0]}},"closed":[{"nodes":[]}]}"#;
        let state = LocalState::from_bytes(bytes).expect("lenient");
        assert_eq!(state.nodes["a"].pos, [0.0, 2.0]);
        assert_eq!(state.camera, None);
        assert_eq!(state.closed.len(), 1);
    }
```

- [ ] **Step 2: Run** `cargo test -j 8 -p manifold --bins local_state`. Expected: both new tests FAIL. The first panics at `expect("still readable")`; the second returns `None`.

- [ ] **Step 3: Implement.**

Add `#[serde(deserialize_with = "lenient_pos")]` on `LocalNode::pos`. On `camera`, change the attribute to `#[serde(default, deserialize_with = "lenient_camera")]`. Then:

```rust
/// A `null` coordinate (an older build wrote NaN that way) reads as 0.
fn lenient_pos<'de, D: serde::Deserializer<'de>>(d: D) -> Result<[f32; 2], D::Error> {
    let [x, y] = <[Option<f32>; 2]>::deserialize(d)?;
    Ok([x.unwrap_or(0.0), y.unwrap_or(0.0)])
}

/// A camera rect with a `null` in it reads as no saved camera.
#[allow(clippy::type_complexity)] // mirrors the field's own type
fn lenient_camera<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<([f32; 4], Option<String>)>, D::Error> {
    let raw = Option::<([Option<f32>; 4], Option<String>)>::deserialize(d)?;
    Ok(raw.and_then(|(r, level)| Some(([r[0]?, r[1]?, r[2]?, r[3]?], level))))
}
```

Drop the `#[allow]` if clippy doesn't fire `type_complexity` there.

In `impl LocalState`:

```rust
    fn all_finite(&self) -> bool {
        self.nodes.values().all(|n| n.pos.iter().all(|v| v.is_finite()))
            && self.camera.as_ref().is_none_or(|(r, _)| r.iter().all(|v| v.is_finite()))
            && self.json_outline_width.is_none_or(f32::is_finite)
    }

    /// serde_json writes NaN and infinity as `null`, which a plain `f32`
    /// can't read back: one bad value used to make the whole file unreadable,
    /// Recently Closed included. Each non-finite value is dropped alone.
    pub(crate) fn to_bytes(&self) -> Vec<u8> {
        if self.all_finite() {
            return serde_json::to_vec(self).expect("personal state serializes");
        }
        let mut clean = self.clone();
        clean.nodes.retain(|_, n| n.pos.iter().all(|v| v.is_finite()));
        if clean.camera.as_ref().is_some_and(|(r, _)| r.iter().any(|v| !v.is_finite())) {
            clean.camera = None;
        }
        clean.json_outline_width = clean.json_outline_width.filter(|w| w.is_finite());
        serde_json::to_vec(&clean).expect("personal state serializes")
    }
```

`load_named_session` (`main.rs` ~1900) runs `serde_json::to_value(l)` on a state that came from `from_bytes`. After this change that state is finite: lenient reads give 0 or `None`. So nothing changes there.

- [ ] **Step 4: Run** `cargo test -j 8 -p manifold --bins local_state`. Expected: PASS, including the existing `bytes_round_trip_and_garbage_is_a_fresh_start`.

- [ ] **Step 5: Run the full gate, then commit.**

```bash
git add crates/manifold/src/local_state.rs
git commit -m "Store: a non-finite position or camera value is dropped alone instead of making personal state unreadable

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: One atomic-replace helper: always fsynced, unique temp, cleaned on error

**Findings:**
- **A-R1 (med).** Four hand-rolled "write temp, rename" helpers differ in durability and cleanup:
  - `store/local.rs` `replace` (76-88) uses a per-pid temp, fsyncs, and cleans up.
  - `store/journal.rs` `write_compacted` (1273-1283) uses a fixed `.tmp`, fsyncs, and doesn't clean up.
  - `save_damaged` (1314-1321) fsyncs and doesn't clean up.
  - `jar.rs` `Jar::save` (411-421) uses a per-pid temp, doesn't fsync, and doesn't clean up.
- **A-B2 (med).** `Jar::save` → `write_private` is `std::fs::write` on Windows, with no `sync_all`. After a power loss, the renamed jar can come back zero-filled. `Jar::load` then moves it aside and starts empty, signing the user out of everything in that jar.
- **A-B3 (low-med).** A failed jar write or rename leaves `<file>.<pid>.tmp` behind: a full copy of the credentials outside `prune_asides`' cap.
- **A-Rb1 (med if it fires).** `Journal::open` (`journal.rs:772`) uses `if !path.exists()`. `exists()` is false on *any* metadata error, so a transient access-denied renames a fresh empty document over the user's session, with no `.prev`.
- **D-B3 (med), temp-name half.** `redirect_fetch.rs` `save` (~112-125) names the temp `<id>.<ext>.<pid>.tmp`, which is the same for two threads of one process. On Windows a second writer can still be writing after the first renamed. The result is a mixed file that was validated before it was written. The in-flight dedupe half is in Task 22.

**Design:**
- New `store/atomic.rs`. Temp names are `<path>.<pid>-<n>.tmp`, where `n` is a process-wide `AtomicU64`.
- `jar::remove_dead_tmps` and `jar::named_jar_of` parse that form as well as the old `<pid>.tmp`, through one parser in `atomic.rs`.

**Files:**
- Create: `crates/manifold/src/store/atomic.rs` (and add `pub mod atomic;` to `store/mod.rs`)
- Modify: `crates/manifold/src/store/local.rs`: delete `replace`, and call `atomic::replace` from `write` (~73).
- Modify: `crates/manifold/src/json_file.rs:75`: `crate::store::atomic::replace`.
- Modify: `crates/manifold/src/store/journal.rs`:
  - `Journal::open` (~769), `write_compacted` (~1273) and `save_damaged` (~1314);
  - the test near 1956 that checks for a `.tmp`.
- Modify: `crates/manifold/src/jar.rs`:
  - `Jar::save` (~411) and `write_private` (~305): `write_private` is deleted;
  - `remove_dead_tmps` (~224) and `named_jar_of` (~271).
- Modify: `crates/manifold/src/redirect_fetch.rs` `save` (~112)

**Interfaces:**
- Produces:
  - `store::atomic::replace(path: &Path, bytes: &[u8]) -> io::Result<()>`
  - `store::atomic::replace_private(path: &Path, bytes: &[u8]) -> io::Result<()>`: 0600 on Unix.
  - `store::atomic::replace_with(path: &Path, bytes: &[u8], private: bool, before_rename: impl FnOnce()) -> io::Result<()>`
  - `store::atomic::temp_path(path: &Path) -> PathBuf`
  - `store::atomic::temp_pid(suffix: &str) -> Option<u32>`: parses `<pid>.tmp` or `<pid>-<n>.tmp`.

- [ ] **Step 1: Write the failing tests** in the new `store/atomic.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::testutil::temp_dir;

    fn tmp_files(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect()
    }

    #[test]
    fn replace_writes_whole_and_leaves_no_temp() {
        let dir = temp_dir("atomic-ok");
        let path = dir.join("f.json");
        replace(&path, b"one").unwrap();
        replace(&path, b"two").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        assert!(tmp_files(&dir).is_empty());
    }

    #[test]
    fn a_failed_rename_removes_the_temp() {
        let dir = temp_dir("atomic-fail");
        let path = dir.join("target");
        std::fs::create_dir(&path).unwrap(); // a directory can't be replaced by a file
        assert!(replace(&path, b"x").is_err());
        assert!(tmp_files(&dir).is_empty(), "{:?}", tmp_files(&dir));
    }

    #[test]
    fn before_rename_runs_between_write_and_rename() {
        let dir = temp_dir("atomic-hook");
        let path = dir.join("f");
        std::fs::write(&path, b"old").unwrap();
        let mut seen = None;
        replace_with(&path, b"new", false, || seen = Some(std::fs::read(&path).unwrap())).unwrap();
        assert_eq!(seen.as_deref(), Some(&b"old"[..]), "the hook sees the old file");
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
    }

    #[test]
    fn temp_names_are_unique_per_call_and_carry_the_pid() {
        let path = Path::new("x/default.json");
        let (a, b) = (temp_path(path), temp_path(path));
        assert_ne!(a, b);
        let suffix = a.file_name().unwrap().to_str().unwrap().strip_prefix("default.json.").unwrap();
        assert_eq!(temp_pid(suffix), Some(std::process::id()));
        assert_eq!(temp_pid("1234.tmp"), Some(1234), "the pre-counter form");
        assert_eq!(temp_pid("12-3.tmp"), Some(12));
        assert_eq!(temp_pid("notapid.tmp"), None);
        assert_eq!(temp_pid("12-.tmp"), None);
        assert_eq!(temp_pid("12"), None);
    }
}
```

- [ ] **Step 2: Run** `cargo test -j 8 -p manifold --bins store::atomic`. Expected: compile failure.

- [ ] **Step 3: Implement `store/atomic.rs`:**

```rust
//! The one "write beside, fsync, rename over" helper. Every file manifold
//! replaces goes through it, so all of them get the same durability (the data
//! reaches the disk before the rename can) and the same cleanup (a failed
//! replace leaves no temp file, which for a jar is a copy of its credentials).

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::journal::with_suffix;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

/// `<path>.<pid>-<n>.tmp`, new on every call. Unique across threads (a
/// shared name let two fetches write into one file) and across instances
/// (the pid; `jar::remove_dead_tmps` reads it to find crashed saves).
pub(crate) fn temp_path(path: &Path) -> PathBuf {
    with_suffix(path, &format!(".{}-{}.tmp", std::process::id(), NEXT_TEMP.fetch_add(1, Ordering::Relaxed)))
}

/// The pid in a temp suffix written by `temp_path` (`<pid>-<n>.tmp`) or by
/// builds before it (`<pid>.tmp`).
pub(crate) fn temp_pid(suffix: &str) -> Option<u32> {
    let stem = suffix.strip_suffix(".tmp")?;
    let (pid, n) = stem.split_once('-').unwrap_or((stem, "0"));
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    (digits(pid) && digits(n)).then(|| pid.parse().ok()).flatten()
}

pub(crate) fn replace(path: &Path, bytes: &[u8]) -> io::Result<()> {
    replace_with(path, bytes, false, || {})
}

/// `replace` for a file only the current user may read (0600 on Unix; on
/// Windows it inherits the profile folder's ACL).
pub(crate) fn replace_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    replace_with(path, bytes, true, || {})
}

/// Writes `bytes` to a fresh temp beside `path` and fsyncs it, runs
/// `before_rename` (the journal keeps `.prev` there), then renames over
/// `path`. On any error `path` is as it was and the temp is gone.
pub(crate) fn replace_with(path: &Path, bytes: &[u8], private: bool, before_rename: impl FnOnce()) -> io::Result<()> {
    let tmp = temp_path(path);
    let result = write_new(&tmp, bytes, private).and_then(|()| {
        before_rename();
        std::fs::rename(&tmp, path)
    });
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn write_new(path: &Path, bytes: &[u8], private: bool) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = private;
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}
```

- [ ] **Step 4: Run** `cargo test -j 8 -p manifold --bins store::atomic`. Expected: PASS.

- [ ] **Step 5: Route every writer through it.**
- **`store/local.rs`:** delete `fn replace` and its doc. In `write` (~73), call `super::atomic::replace(&path_for(session), &bytes).map_err(JournalError::from)`. Drop the `write_synced` import if it is now unused.
- **`json_file.rs:75`:** `crate::store::atomic::replace(path, json.as_bytes())`.
- **`journal.rs` `Journal::open`:** replace `if !path.exists() {` with `if !path.try_exists()? {`, and add the comment "`exists()` is false on any error; a transient access error must not replace the user's session with an empty one." `JournalError` already has `From<io::Error>`, because `?` on io is used elsewhere in `open`.
- **`journal.rs` `write_compacted`:**

```rust
    super::atomic::replace_with(path, &bytes, false, || {
        if rotate {
            keep_prev(path);
        }
    })?;
    Ok(bytes)
```

- **`journal.rs` `save_damaged`:**

```rust
fn save_damaged(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if path.try_exists()? {
        return Ok(());
    }
    super::atomic::replace(path, bytes)
}
```

- **`journal.rs` test near 1956:** replace `assert!(!with_suffix(&local::path_for(&path), ".tmp").exists());` with a check that no `.tmp` file is left in `dir`:

```rust
        let leftovers: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp")).collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
```

  Use the test's own dir variable; read the test to find its name.
- **`jar.rs` `Jar::save`:**

```rust
        std::fs::create_dir_all(dir)?;
        let mut out = MAGIC.to_vec();
        out.extend(protect(&serde_json::to_vec(self).map_err(std::io::Error::other)?)?);
        crate::store::atomic::replace_private(&Self::path(dir, key), &out)
```

  Then delete `write_private`. Update `save`'s doc: the temp is per call, fsynced, and removed on failure.
- **`jar.rs` `remove_dead_tmps`:** parse with `name.to_str().and_then(|n| crate::store::atomic::temp_pid(n.strip_prefix(file)?.strip_prefix('.')?))`.
- **`jar.rs` `named_jar_of`:** replace the `pid` closure with `let tmp = rest.strip_prefix('.').and_then(crate::store::atomic::temp_pid);` and `|| tmp.is_some()`. Update the doc comment's `<id>.json.<pid>.tmp` to `<id>.json.<pid>[-<n>].tmp`.
- **`redirect_fetch.rs` `save`:** replace the temp/write/rename block with:

```rust
    let path = dir.join(&name);
    crate::store::atomic::replace(&path, &bytes).ok()?;
```

  Update the function doc: the temp is unique per call, so two fetches for one node never share it.

- [ ] **Step 6: Add the jar regression test** in `jar.rs` tests:

```rust
    #[test]
    fn a_dead_counter_form_tmp_is_deleted_too() {
        let dir = temp_dir("jar-dead-tmp-n");
        let dead = dir.join("default.json.4294967295-3.tmp");
        std::fs::write(&dead, "crashed save").unwrap();
        Jar::load(&dir, JarKey::Default);
        assert!(!dead.exists());
    }
```

- [ ] **Step 7: Run** the full gate. The existing jar tests (`load_leaves_a_live_instances_tmp_alone_and_save_leaves_no_tmp`, `load_deletes_a_tmp_whose_process_is_gone`), the journal tests and the redirect_fetch tests must all pass.

- [ ] **Step 8: Commit**

```bash
git add crates/manifold/src/store crates/manifold/src/json_file.rs crates/manifold/src/jar.rs crates/manifold/src/redirect_fetch.rs
git commit -m "Store: one atomic replace (fsync, per-call temp, cleanup on error) for jars, journal, personal state, JSON and fetched content

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: Named sessions: unreadable store set aside, delete removes every copy and asks first, names can't escape

**Findings:**
- **A-Rb5 (low).** `Journal::open` returns `NotAJournal` and leaves the file alone. `main.rs` `open_store` (~2007) then toasts and leaves `self.journal = None`, so nothing is saved for the rest of the run and every later session is lost at quit.
- **A-Rb7 (low).** `session.rs` `delete_named` (92-97) removes only `.mnf`, `.local` and `.prev`. It leaves:
  - `.lock`;
  - the temps;
  - `.damaged-<hex>`, which are full session files holding every URL.
- **C-B4 (med).** `settings_page.rs` (424-426, 602-604): the 12 px X beside "Open" deletes a named session in one click with no confirm. Jar and engine actions have one.
- **B-B8 (low).** `session::named_session_path(name)` (68) joins `name` unsanitized. `OPENSESSION ..\..\x` (`debug_control` → `load_named_session`) escapes `sessions/`.
- **B-R7 (low, document).** Debug commands, which read and write arbitrary paths, are reachable in release builds via `--headless`, `--debug-commands` or `--script`. This is intended for driving the app; document it in README "Debug commands" rather than gate it. B-B8 removes the one traversal through a name.

**Files:**
- Modify: `crates/manifold/src/store/journal.rs`: add `Journal::open_or_set_aside`, plus a test.
- Modify: `crates/manifold/src/main.rs`:
  - `open_store` (~2007);
  - the `App` field `pending_session_delete: Option<String>`, plus Default;
  - the confirm-popup call site next to `build_jar_confirm_popup` (~2621).
- Modify: `crates/manifold/src/session.rs`:
  - `named_session_path`;
  - `delete_named` becomes `delete_session_files(path)`;
  - tests.
- Modify: `crates/manifold/src/popups.rs`:
  - `confirm_open`, `offer_engine_confirm` and `offer_jar_confirm` (~373-385);
  - new `offer_session_delete` and `build_session_delete_popup`.
- Modify: `crates/manifold/src/settings_page.rs` (~602-604)
- Modify: `README.md`: a sentence in the debug-commands section.

**Interfaces:**
- Produces:
  - `Journal::open_or_set_aside(path: &Path, actor: ActorId) -> Result<(Journal, Option<PathBuf>), JournalError>`: `Some(aside)` when a non-journal file was moved aside.
  - `session::delete_session_files(path: &Path)`
  - `session::session_name_is_safe(name: &str) -> bool`
  - `App::offer_session_delete(&mut self, name: String) -> bool`

- [ ] **Step 1: Write the failing tests.**

In `journal.rs` tests:

```rust
    #[test]
    fn a_file_that_isnt_a_journal_is_set_aside_and_replaced() {
        let dir = temp_dir("not-a-journal");
        let path = dir.join("session.mnf");
        std::fs::write(&path, b"this is not a session").unwrap();
        assert!(matches!(Journal::open(&path, ActorId(1)), Err(JournalError::NotAJournal)));
        let (journal, aside) = Journal::open_or_set_aside(&path, ActorId(1)).unwrap();
        let aside = aside.expect("moved aside");
        assert_eq!(std::fs::read(&aside).unwrap(), b"this is not a session");
        assert!(aside.file_name().unwrap().to_string_lossy().starts_with("session.mnf.unreadable-"));
        drop(journal);
        assert!(Journal::open(&path, ActorId(2)).is_ok(), "a real journal now");
    }
```

In `session.rs` (add `#[cfg(test)] mod tests` if absent):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::testutil::temp_dir;

    #[test]
    fn deleting_a_session_removes_every_copy_and_nothing_else() {
        let dir = temp_dir("delete-session");
        let names = ["a.mnf", "a.mnf.local", "a.mnf.prev", "a.mnf.lock", "a.mnf.123-4.tmp", "a.mnf.damaged-00ff", "ab.mnf", "a b.mnf"];
        for n in names {
            std::fs::write(dir.join(n), b"x").unwrap();
        }
        delete_session_files(&dir.join("a.mnf"));
        let left: std::collections::BTreeSet<String> =
            std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(left, ["a b.mnf", "ab.mnf"].map(String::from).into());
    }

    #[test]
    fn a_session_name_cannot_leave_the_sessions_folder() {
        for ok in ["work", "a b", "x-1_2", "名前"] {
            assert!(session_name_is_safe(ok), "{ok}");
        }
        for bad in ["", ".", "..", "..\\..\\x", "../x", "a/b", "c:x", "a\0b"] {
            assert!(!session_name_is_safe(bad), "{bad:?}");
        }
    }
}
```

- [ ] **Step 2: Run** `cargo test -j 8 -p manifold --bins not_a_journal` and `cargo test -j 8 -p manifold --bins session::tests`. Expected: compile failures.

- [ ] **Step 3: Implement.**

In `journal.rs`, after `open`:

```rust
    /// `open`, but a file that isn't a journal at all is renamed to
    /// `<path>.unreadable-<secs>` and a fresh journal is created in its place.
    /// Refusing used to leave the app without a store, so nothing more
    /// was saved that run. Returns where the old file went.
    pub fn open_or_set_aside(path: &Path, actor: ActorId) -> Result<(Journal, Option<PathBuf>), JournalError> {
        match Journal::open(path, actor) {
            Err(JournalError::NotAJournal) => {
                let secs = SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
                let mut aside = with_suffix(path, &format!(".unreadable-{secs}"));
                let mut n = 1;
                while aside.try_exists()? {
                    aside = with_suffix(path, &format!(".unreadable-{secs}-{n}"));
                    n += 1;
                }
                std::fs::rename(path, &aside)?;
                Ok((Journal::open(path, actor)?, Some(aside)))
            }
            other => other.map(|journal| (journal, None)),
        }
    }
```

Check whether `ActorId` is `Copy`. If not, clone it before the first call.

In `main.rs` `open_store`:

```rust
        match store::journal::Journal::open_or_set_aside(&path, store::model::ActorId::random()) {
            Ok((journal, aside)) => {
                self.journal = Some(journal);
                if let Some(aside) = aside {
                    self.queue_toast(format!(
                        "The session file wasn't readable; it was kept as {} and a new one started",
                        aside.display()
                    ));
                }
            }
            Err(err) => self.queue_toast(format!("Couldn't open the session file {}: {err}", path.display())),
        }
```

Update `open_store`'s doc to match.

In `session.rs`:

```rust
/// Whether `name` is a bare file stem: no separator, drive or stream colon,
/// NUL, and not `.`/`..`. "Save As" names are already stricter
/// (`sanitize_session_name`); this guards the other ways in (`OPENSESSION`).
pub(crate) fn session_name_is_safe(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', ':', '\0'])
}

pub(crate) fn named_session_path(name: &str) -> Option<PathBuf> {
    if !session_name_is_safe(name) {
        return None;
    }
    Some(named_sessions_dir()?.join(format!("{name}.mnf")))
}

/// Deletes a named save: the store file and every file the store keeps
/// beside it (`<name>.mnf.*`: personal state, `.prev`, lock, temps and
/// `.damaged-*` copies, which are whole sessions too).
pub(crate) fn delete_session_files(path: &Path) {
    let _ = std::fs::remove_file(path);
    let (Some(dir), Some(file)) = (path.parent(), path.file_name().and_then(|f| f.to_str())) else { return };
    let prefix = format!("{file}.");
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        if entry.file_name().to_str().is_some_and(|n| n.starts_with(&prefix)) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// The "Open Session" list's delete, after its confirm.
pub fn delete_named(name: &str) {
    if let Some(path) = named_session_path(name) {
        delete_session_files(&path);
    }
}
```

`load_named_session` already toasts "No session file" when the path is `None`. Make that toast say "Not a session name: {shown}" when `!session_name_is_safe(name)`, so the reason is visible.

- [ ] **Step 4: The delete confirm.**

Add `pending_session_delete: Option<String>` to `App` (Default `None`).

In `popups.rs`:

```rust
    pub(crate) fn confirm_open(&self) -> bool {
        self.pending_engine_confirm.is_some() || self.pending_jar_confirm.is_some() || self.pending_session_delete.is_some()
    }
```

Change `offer_engine_confirm`'s `other_open` argument to `self.pending_jar_confirm.is_some() || self.pending_session_delete.is_some()`, and `offer_jar_confirm`'s to `self.pending_engine_confirm.is_some() || self.pending_session_delete.is_some()`. Add:

```rust
    /// Asks before deleting a named session (`offer_confirm`).
    pub(crate) fn offer_session_delete(&mut self, name: String) -> bool {
        let other = self.pending_engine_confirm.is_some() || self.pending_jar_confirm.is_some();
        offer_confirm(&mut self.pending_session_delete, other, name)
    }

    /// "Delete session <name>?" for the Settings session list's X.
    pub(crate) fn build_session_delete_popup(&mut self, ui: &mut egui::Ui) {
        let Some(name) = self.pending_session_delete.clone() else { return };
        let shown = crate::names::to_plain(&crate::names::display_name(&name, crate::names::UNTITLED));
        let title = format!("Delete session {shown}?");
        match confirm_dialog(ui, "session_delete_popup", title, "Deletes the saved session and its copies from disk.", "Delete") {
            Some(true) => {
                self.pending_session_delete = None;
                session::delete_named(&name);
            }
            Some(false) => self.pending_session_delete = None,
            None => {}
        }
    }
```

Check `confirm_dialog`'s exact parameter types at its definition (`grep -n "fn confirm_dialog" crates/manifold/src/popups.rs`) and match them. In `main.rs`, call `self.build_session_delete_popup(ui);` right after the `build_jar_confirm_popup` call (~2622). In `settings_page.rs` (~602), replace `session::delete_named(name);` with `self.offer_session_delete(name.clone());`.

- [ ] **Step 5: Add a popup test** next to the existing `offer_confirm` tests in `popups.rs` (grep `fn .*offer_confirm` in its test module):

```rust
    #[test]
    fn a_session_delete_waits_for_a_free_confirm_slot() {
        let mut app = App::default();
        app.pending_jar_confirm = Some(crate::jar_defs::JarConfirm::Clear(crate::jar::JarKey::Default));
        assert!(!app.offer_session_delete("work".into()));
        app.pending_jar_confirm = None;
        assert!(app.offer_session_delete("work".into()));
        assert!(app.confirm_open());
    }
```

Use whatever `JarConfirm`/`JarKey` constructor the existing tests use; check them.

- [ ] **Step 6: README.** In the README section that lists the `--headless` / `--debug-commands` / `--script` flags, add one sentence: "Debug commands read and write any path the user can (EXPORT, IMPORT, SCREENSHOT, OPENSESSION); they are for driving manifold from scripts and are not a security boundary."

- [ ] **Step 7: Run the full gate, then commit.**

```bash
git add crates/manifold/src/store/journal.rs crates/manifold/src/main.rs crates/manifold/src/session.rs crates/manifold/src/popups.rs crates/manifold/src/settings_page.rs README.md
git commit -m "Sessions: set an unreadable store aside and start fresh; delete asks first and removes every copy; names can't leave sessions/

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Phase B: remaining bugs

### Task 9: Engine messages: downloads find their own node; a crash loop stops

**Findings:**
- **B-B2 (med).** In `event_loop.rs` (~1730-1747), `DownloadProgress`, `DownloadFailed` and `DownloadCompleted` find their node with `nodes.values_mut().find_map(.. d.path == path)`. That ignores the tab and the completed/failed state, and walks HashMap order.
  - A re-download to the same path, or two tabs downloading the same name, updates the wrong node. The new one sits at 0% forever.
  - `return true` (the save trigger) fires for the wrong node.
- **B-B3 (med).** `EngineToHost::Crashed` (~1619-1647) always calls `respawn_tab`, with no limit. A page that crashes the engine on load, or a broken engine install, spawns full engine processes and toasts in an endless loop.
- **Task 4 tie-in.** A `Crashed` whose reason starts with `ipc_protocol::PROTOCOL_MISMATCH` must not respawn either: the same stale binary would fail again.

**Design:**
- **Downloads:** match only in-flight downloads (`!completed && !failed`) with that path. Prefer one whose `parent == tab_id`, otherwise the newest; newest means last in `node_order`.
- **Crashes:**
  - Keep crash times per tab id in `App.tab_crashes: HashMap<String, Vec<Instant>>`.
  - The 3rd crash within 60 s, or a protocol mismatch, turns the tab into an `Unloaded` card with `UnloadedState::Failed(reason)`. That is the same card a failed respawn already leaves (`tabs.rs` ~491-509), and opening it retries.

**Files:**
- Modify: `crates/manifold/src/event_loop.rs`: the `Crashed` arm, the three download arms, and tests in `idle_tests`.
- Modify: `crates/manifold/src/tabs.rs`:
  - new `crashes_too_often`, `leave_crashed_card` and `unloaded_from`;
  - `sleep_now` (~763) uses `unloaded_from`.
- Modify: `crates/manifold/src/main.rs`: the `App` field `tab_crashes`, plus Default.
- Modify: `crates/manifold/src/fetched.rs`: new `App::in_flight_download_id`, next to `insert_download_node` (~82).

**Interfaces:**
- Consumes: `ipc_protocol::PROTOCOL_MISMATCH` (Task 4).
- Produces:
  - `tabs::CRASH_LIMIT: usize = 3`
  - `tabs::CRASH_WINDOW: Duration = 60 s`
  - `tabs::crashes_too_often(times: &mut Vec<Instant>, now: Instant) -> bool`
  - `App::leave_crashed_card(&mut self, id: &str, reason: &str)`
  - `App::in_flight_download_id(&self, tab_id: &str, path: &str) -> Option<String>`

- [ ] **Step 1: Write the failing tests.**

In `event_loop.rs` `idle_tests`, using the same harness as `a_failed_download_is_marked_and_no_longer_in_flight`:

```rust
    #[test]
    fn a_redownload_updates_the_new_node_not_the_finished_one() {
        let mut app = App::default();
        app.insert_note_at("n".into(), None);
        app.insert_download_node("n".into(), "f.zip".into(), Some(10), None);
        app.handle_engine_message("n", EngineToHost::DownloadCompleted { path: "f.zip".into() });
        app.insert_download_node("n".into(), "f.zip".into(), Some(20), None);
        app.handle_engine_message("n", EngineToHost::DownloadProgress { path: "f.zip".into(), bytes_received: 7 });
        let downloads: Vec<_> = app.node_order.iter().filter_map(|id| app.nodes[id].as_download()).collect();
        assert_eq!(downloads.len(), 2);
        assert!(downloads[0].completed && downloads[0].bytes_received == 0, "the old node is untouched");
        assert_eq!(downloads[1].bytes_received, 7);
    }

    #[test]
    fn same_named_downloads_from_two_tabs_stay_apart() {
        let mut app = App::default();
        app.insert_note_at("a".into(), None);
        app.insert_note_at("b".into(), None);
        app.insert_download_node("a".into(), "f.zip".into(), None, None);
        app.insert_download_node("b".into(), "f.zip".into(), None, None);
        app.handle_engine_message("a", EngineToHost::DownloadFailed { path: "f.zip".into() });
        let by_parent = |p: &str| {
            app.nodes.values().find(|n| n.parent.as_deref() == Some(p)).and_then(|n| n.as_download()).unwrap().failed
        };
        assert!(by_parent("a") && !by_parent("b"));
    }
```

The note nodes stand in for tabs as download parents, as the existing test does. If `insert_note_at`'s id argument works differently, mirror the existing test exactly.

In `tabs.rs` tests (add `#[cfg(test)] mod crash_tests` if there is no test module):

```rust
#[cfg(test)]
mod crash_tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn the_third_crash_within_the_window_gives_up() {
        let t0 = Instant::now();
        let mut times = Vec::new();
        assert!(!crashes_too_often(&mut times, t0));
        assert!(!crashes_too_often(&mut times, t0 + Duration::from_secs(1)));
        assert!(crashes_too_often(&mut times, t0 + Duration::from_secs(2)));
    }

    #[test]
    fn crashes_spread_past_the_window_keep_respawning() {
        let t0 = Instant::now();
        let mut times = Vec::new();
        for i in 0..10 {
            assert!(!crashes_too_often(&mut times, t0 + CRASH_WINDOW * i), "crash {i}");
        }
    }
}
```

- [ ] **Step 2: Run** `cargo test -j 8 -p manifold --bins download` and `cargo test -j 8 -p manifold --bins crash_tests`. Expected: the download tests FAIL (the new node stays at 0, or the wrong node is marked) and `crash_tests` fails to compile.

- [ ] **Step 3: Implement the download lookup** in `fetched.rs`:

```rust
    /// The in-flight download node an engine's progress, failure or
    /// completion for `path` is about: one `tab_id` started if there is one,
    /// else the newest. A finished node with the same path (a restored one,
    /// or an earlier download of the same file) is never matched.
    pub(crate) fn in_flight_download_id(&self, tab_id: &str, path: &str) -> Option<String> {
        let in_flight = |id: &String| {
            self.nodes.get(id).and_then(|n| n.as_download()).is_some_and(|d| d.path == path && !d.completed && !d.failed)
        };
        let from_tab = |id: &&String| self.nodes.get(*id).is_some_and(|n| n.parent.as_deref() == Some(tab_id));
        let mut newest_first = self.node_order.iter().rev().filter(|id| in_flight(id));
        let first = newest_first.clone().next();
        newest_first.find(from_tab).or(first).cloned()
    }
```

In `event_loop.rs`, rewrite the three arms to use it:

```rust
            EngineToHost::DownloadProgress { path, bytes_received } => {
                if let Some(id) = self.in_flight_download_id(tab_id, &path) {
                    if let Some(dl) = self.nodes.get_mut(&id).and_then(|n| n.as_download_mut()) {
                        dl.bytes_received = bytes_received;
                    }
                }
            }
```

Write the same shape for `DownloadFailed` (toast unchanged; set `failed = true`) and `DownloadCompleted` (toast unchanged; set `completed = true; return true;` only when a node was found).

- [ ] **Step 4: Implement the crash limit** in `tabs.rs`:

```rust
/// A tab whose engine crashes this many times within `CRASH_WINDOW` is left
/// as a failed card instead of respawning again.
pub(crate) const CRASH_LIMIT: usize = 3;
pub(crate) const CRASH_WINDOW: std::time::Duration = std::time::Duration::from_secs(60);

/// Records a crash at `now` in `times` (one tab's) and says whether that tab
/// has now crashed `CRASH_LIMIT` times within `CRASH_WINDOW`.
pub(crate) fn crashes_too_often(times: &mut Vec<std::time::Instant>, now: std::time::Instant) -> bool {
    times.retain(|&t| now.duration_since(t) < CRASH_WINDOW);
    times.push(now);
    times.len() >= CRASH_LIMIT
}
```

Factor the payload conversion out of `sleep_now`:

```rust
/// `tab` as an unloaded card in `state`, keeping what a reopen needs.
fn unloaded_from(tab: &TabPayload, state: UnloadedState) -> UnloadedTabPayload {
    UnloadedTabPayload {
        url: tab.url.clone(),
        title: tab.title.clone(),
        kind: tab.tab.kind,
        pinned: tab.pinned,
        muted: tab.muted,
        zoom_factor: tab.zoom_factor,
        state,
        jar: tab.jar,
    }
}
```

`TabPayload` is the `NodePayload::Tab` payload type; check its exact name in `node.rs`. In `sleep_now`, use `let asleep = unloaded_from(tab, UnloadedState::Sleeping);`. Then add:

```rust
    /// A tab whose engine keeps crashing (`crashes_too_often`), or speaks
    /// another protocol version, becomes a failed card: the reason is its
    /// tooltip, and opening it tries again. Respawning would only loop.
    pub(crate) fn leave_crashed_card(&mut self, id: &str, reason: &str) {
        let Some(node) = self.nodes.get_mut(id) else { return };
        let NodePayload::Tab(tab) = &node.payload else { return };
        let url = tab.url.clone();
        // Replacing the payload drops the `EngineTab`, which reaps the dead host.
        node.payload = NodePayload::Unloaded(unloaded_from(tab, UnloadedState::Failed(reason.to_string())));
        if let Some(gpu) = &mut self.gpu {
            gpu.free_tab_thumbnail(id);
            gpu.free_tab_texture(id);
        }
        self.tab_crashes.remove(id);
        self.queue_toast(format!("Tab keeps crashing, left closed: {url}"));
        self.save_session();
    }
```

Add `tab_crashes: std::collections::HashMap<String, Vec<std::time::Instant>>` to `App` (Default empty). Remove a tab's entry wherever the tab node is finally removed (`remove_tab`).

In `event_loop.rs`, at the top of the `Crashed` arm:

```rust
            EngineToHost::Crashed { reason } => {
                let mismatch = reason.starts_with(ipc_protocol::PROTOCOL_MISMATCH);
                let times = self.tab_crashes.entry(tab_id.to_string()).or_default();
                if mismatch || crate::tabs::crashes_too_often(times, std::time::Instant::now()) {
                    eprintln!("engine-host for {tab_id} crashed ({reason}); not restarting");
                    self.leave_crashed_card(tab_id, &reason);
                    return false;
                }
                // ... existing respawn code unchanged ...
```

Focus: the failed-respawn path in `tabs.rs` (~491) already leaves a focused tab as an Unloaded card under the same id. Read `showing_tab_view` and the Tab-view build to confirm that an Unloaded focused node is handled. If it is not, call the same "leave Tab view" helper that `remove_tab` uses for a closed focused tab, and say so in the task report.

- [ ] **Step 5: Run** `cargo test -j 8 -p manifold --bins download` and `cargo test -j 8 -p manifold --bins crash_tests`, then the full gate. Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/manifold/src/event_loop.rs crates/manifold/src/tabs.rs crates/manifold/src/main.rs crates/manifold/src/fetched.rs
git commit -m "Tabs: download events reach the in-flight node of their tab; a tab crashing 3 times a minute becomes a failed card

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: App shell bugs: print-to-PDF, stuck swallowed keys, SLEEP/WAITTABS panic, Show in folder

**Findings:**
- **B-B1 (med).**
  - In `event_loop.rs` `handle_screenshot_result` (743-767), the `Some(Err)` branch only logs. It does not clear `pending_print_to_pdf`, which `main.rs` `start_print_to_pdf` (~2304) set alongside `pending_screenshot`.
  - The user's next ordinary screenshot then takes the stale PDF target: it writes a PDF there, **deletes the user's PNG**, and toasts "Saved <old.pdf>".
  - A PDF conversion error is only `eprintln`'d.
- **B-B7 (low).** `swallowed_keys` (`event_loop.rs` 394-405) is cleared only by the matching key-up. There is no `WindowEvent::Focused` arm. If the key-up is lost (Alt-Tab mid-chord), the page later sees that key stuck.
- **B-B4 (med).** `debug_control.rs` `parse_line` (674-681) filters SLEEP and WAITTABS seconds only with `>= 0.0`. `inf` and `1e30` pass, and `Duration::from_secs_f64` then panics. That kills the stdin reader thread, so headless and `--script` runs hang forever.
- **B-B6 (low).** `main.rs` `reveal_in_explorer` (Windows, ~2066):
  - The `spawn()` error is discarded, so the function always returns `Ok`.
  - `/select,{path}` is one argument and explorer splits on commas, so `a,b.pdf` opens the wrong folder.
- **B-F5 (low, test gap).** No test pins `COMMAND_HELP` (387-496) to `parse_line` (547-697).

**Files:**
- Modify: `crates/manifold/src/main.rs`:
  - `pending_print_to_pdf: Option<(String, String)>` (~1194, plus Default ~1497);
  - `start_print_to_pdf` (~2304);
  - `reveal_in_explorer` (~2066) and new `explorer_select_arg`.
- Modify: `crates/manifold/src/event_loop.rs`:
  - `handle_screenshot_result`;
  - a `WindowEvent::Focused` arm in `dispatch_window_event` (~223);
  - tests.
- Modify: `crates/manifold/src/debug_control.rs`: `parse_secs`, the SLEEP and WAITTABS arms, and tests.

**Interfaces:**
- Produces:
  - `debug_control::parse_secs(s: &str) -> Option<std::time::Duration>`
  - `debug_control::MAX_WAIT_SECS: f64 = 86_400.0`
  - `main::explorer_select_arg(path: &str) -> String` (Windows only)

- [ ] **Step 1: Write the failing tests.**

In `debug_control.rs` tests:

```rust
    #[test]
    fn wait_seconds_must_be_finite_and_bounded() {
        assert_eq!(parse_secs("1.5"), Some(std::time::Duration::from_millis(1500)));
        assert_eq!(parse_secs("0"), Some(std::time::Duration::ZERO));
        for bad in ["inf", "-inf", "NaN", "1e30", "-1", "86401", "x", ""] {
            assert_eq!(parse_secs(bad), None, "{bad}");
        }
        assert!(parse_line("SLEEP inf").is_none());
        assert!(parse_line("WAITTABS 1e30").is_none());
        assert!(matches!(parse_line("WAITTABS"), Some(DebugCommand::WaitTabs(d)) if d.as_secs() == 60));
    }

    /// Every HELP usage parses with sample arguments, and every sampled verb is in HELP.
    #[test]
    fn help_and_parser_agree() {
        // One sample line per parser verb, written from `parse_line`'s match arms.
        const SAMPLES: &[&str] = &[
            "MOUSEMOVE 1 2",
            "MOUSEDOWN 1 2",
            // ... one line for every verb `parse_line` matches; read its match to fill this in ...
        ];
        let verb = |line: &str| line.split_whitespace().next().unwrap().to_ascii_uppercase();
        for sample in SAMPLES {
            assert!(parse_line(sample).is_some(), "sample {sample:?} parses");
            assert!(COMMAND_HELP.iter().any(|(usage, _)| verb(usage) == verb(sample)), "{sample:?} has HELP");
        }
        for (usage, _) in COMMAND_HELP {
            assert!(SAMPLES.iter().any(|s| verb(s) == verb(usage)), "HELP verb {usage:?} has a sample");
        }
    }
```

**Filling in `SAMPLES`:** the implementer writes one entry per match arm of `parse_line` (about 60 verbs), with arguments that arm accepts. Read the arm, don't guess:
- an engine name such as `cef`;
- a path such as `C:\x.json`;
- coordinates.

`HELP` and `?` count as verbs if `parse_line` matches them. The test is complete when its list matches `parse_line`'s arms one for one; say so in the commit message.

In `event_loop.rs` `idle_tests`:

```rust
    #[test]
    fn a_failed_print_capture_clears_the_pdf_target() {
        let mut app = App::default();
        app.pending_print_to_pdf = Some(("p.png".into(), "out.pdf".into()));
        app.handle_screenshot_result(Some("p.png".into()), Some(Err("no surface".into())));
        assert!(app.pending_print_to_pdf.is_none());
        assert!(app.save_toast_queue.iter().any(|t| t.contains("Couldn't print")), "{:?}", app.save_toast_queue);
    }

    #[test]
    fn an_ordinary_screenshot_never_takes_a_pdf_target() {
        let mut app = App::default();
        app.pending_print_to_pdf = Some(("p.png".into(), "out.pdf".into()));
        app.handle_screenshot_result(Some("shot.png".into()), Some(Ok(())));
        assert!(app.save_toast_queue.iter().any(|t| t == "Saved shot.png"));
        assert!(app.pending_print_to_pdf.is_some(), "still waiting for its own capture");
    }

    #[test]
    fn losing_focus_forgets_swallowed_keys() {
        let mut app = App::default();
        app.swallowed_keys.insert(winit::keyboard::KeyCode::ArrowLeft);
        app.on_window_focus(false);
        assert!(app.swallowed_keys.is_empty());
    }
```

`save_toast_queue` is a `VecDeque<String>` field (see `queue_toast`, `main.rs` ~2495). If the field is private to `main.rs`, the test module in `event_loop.rs` is still inside the crate root's module tree, so it is visible.

In `main.rs` tests (Windows only):

```rust
    #[cfg(windows)]
    #[test]
    fn explorer_select_quotes_the_path() {
        assert_eq!(explorer_select_arg(r"C:\d\a,b.pdf"), r#"/select,"C:\d\a,b.pdf""#);
    }
```

- [ ] **Step 2: Run** `cargo test -j 8 -p manifold --bins wait_seconds`, then `help_and_parser`, `print_capture`, `ordinary_screenshot`, `swallowed` and `explorer_select`. Expected: compile failures or FAIL.

- [ ] **Step 3: Implement.**

In `debug_control.rs`:

```rust
/// The longest SLEEP/WAITTABS a script may ask for: a day.
pub(crate) const MAX_WAIT_SECS: f64 = 86_400.0;

/// SLEEP/WAITTABS seconds: finite and within `0..=MAX_WAIT_SECS`.
/// `Duration::from_secs_f64` panics on infinity or overflow, which killed
/// the stdin reader and left headless runs hanging.
pub(crate) fn parse_secs(s: &str) -> Option<std::time::Duration> {
    let secs: f64 = s.parse().ok()?;
    (secs.is_finite() && (0.0..=MAX_WAIT_SECS).contains(&secs)).then(|| std::time::Duration::from_secs_f64(secs))
}
```

The arms become:

```rust
        "WAITTABS" => Some(DebugCommand::WaitTabs(if rest.is_empty() {
            std::time::Duration::from_secs(60)
        } else {
            parse_secs(rest)?
        })),
        "SLEEP" => Some(DebugCommand::Sleep(parse_secs(rest)?)),
```

In `main.rs`:
- Change `pending_print_to_pdf` to `Option<(String, String)>` and update its doc: "(temp PNG, PDF): the capture `start_print_to_pdf` asked for, and where its PDF goes. Paired so an unrelated screenshot can't take the PDF target."
- In `start_print_to_pdf`: `let png = temp_png.to_string_lossy().into_owned(); self.pending_screenshot = Some(png.clone()); self.pending_print_to_pdf = Some((png, pdf_path));`.

`handle_screenshot_result` becomes:

```rust
    pub(crate) fn handle_screenshot_result(&mut self, screenshot_path: Option<String>, result: Option<Result<(), String>>) {
        let Some(result) = result else { return };
        let Some(path) = screenshot_path else { return };
        let pdf = self.pending_print_to_pdf.take_if(|(png, _)| *png == path).map(|(_, pdf)| pdf);
        match (result, pdf) {
            (Ok(()), Some(pdf_path)) => {
                match pdf::write_image_as_pdf(std::path::Path::new(&path), std::path::Path::new(&pdf_path)) {
                    Ok(()) => self.queue_toast(format!("Saved {pdf_path}")),
                    Err(err) => {
                        eprintln!("print to pdf: {err}");
                        self.queue_toast(format!("Couldn't print to PDF: {err}"));
                    }
                }
                let _ = std::fs::remove_file(&path);
            }
            (Ok(()), None) => self.queue_toast(format!("Saved {path}")),
            (Err(err), Some(_)) => {
                eprintln!("print to pdf: capture failed: {err}");
                self.queue_toast(format!("Couldn't print to PDF: {err}"));
                let _ = std::fs::remove_file(&path);
            }
            (Err(err), None) => {
                eprintln!("screenshot: {err}");
                self.queue_toast(format!("Couldn't save the screenshot: {err}"));
            }
        }
    }
```

`Option::take_if` is stable since Rust 1.80. Check the callers: a headless `SCREENSHOT` with a waiter must still get no toast on success. Read the caller at `event_loop.rs` ~689-710. If it calls `handle_screenshot_result` only when not headless, keep it that way.

Swallowed keys. Add to `App` in `event_loop.rs`:

```rust
    /// Window focus changed. A chord's key-up can be lost while the window is
    /// in the background (Alt-Tab mid-chord); a key left in `swallowed_keys`
    /// would then swallow that key's next real release in the page.
    pub(crate) fn on_window_focus(&mut self, focused: bool) {
        if !focused {
            self.swallowed_keys.clear();
        }
    }
```

Add the arm `WindowEvent::Focused(focused) => self.on_window_focus(focused),` to `dispatch_window_event`'s match. If a `Focused` arm appears later in that match, merge into it instead.

Reveal in Explorer (Windows):

```rust
/// Explorer's `/select,` argument for `path`, quoted: Explorer splits an
/// unquoted argument at commas, so `a,b.pdf` opened the wrong folder.
#[cfg(windows)]
fn explorer_select_arg(path: &str) -> String {
    format!("/select,\"{path}\"")
}

    #[cfg(windows)]
    fn reveal_in_explorer(path: &str) -> Result<(), String> {
        use std::os::windows::process::CommandExt;
        if path.is_empty() {
            return Err("This download has no file path".to_string());
        }
        std::process::Command::new("explorer.exe")
            .raw_arg(explorer_select_arg(path))
            .spawn()
            .map(drop)
            .map_err(|e| format!("Couldn't open Explorer: {e}"))
    }
```

`raw_arg` passes the string verbatim. Plain `.arg()` would escape the quotes.

- [ ] **Step 4: Run** the named tests, then the full gate. Expected: PASS.

- [ ] **Step 5: Live check.** Build, then start manifold at BelowNormal. Use Print to PDF on a tab and confirm a PDF appears and a "Saved" toast shows. Kill only your own PID.

- [ ] **Step 6: Commit**

```bash
git add crates/manifold/src/main.rs crates/manifold/src/event_loop.rs crates/manifold/src/debug_control.rs
git commit -m "Shell: print-to-PDF target is tied to its capture and failures toast; focus loss clears swallowed keys; SLEEP/WAITTABS reject inf; Show in folder quotes and reports errors; HELP pinned to the parser

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 11: Linux CEF install keeps exec bits; stale dead_code allows go

**Findings:**
- **B-B5 (med, Linux only).** `engine_install.rs` `unpack_with` (~431-444) writes each tar entry with `File::create` plus `io::copy` and never applies `entry.header().mode()`. As a result:
  - `chrome-sandbox`, `chrome_crashpad_handler` and the `.so` files land without +x on Linux, so the CEF host can't start its subprocesses.
  - manifold passes `--no-sandbox` to CEF (`manifold-engine-cef/src/app.rs`), so `chrome-sandbox` needs no setuid. That fact gets documented, not fixed.
- **B-F1 (low).** 14 `#[allow(dead_code)]` "later tasks" markers in `engine_install.rs` (lines 17-18, 29-30, 55-56, 64-66, 70-71, 131-132, 170-171, 323-324, 357-358, 390-391, 452-453, 462-463, 474-475, 530-532). The items are all reached from `start_engine_install_at` now, and the blanket allows hide anything really dead.
- **B-F1, also.** The `custom_version` doc (80-89) sits on `type DirCheck`, so `fn custom_version` (94) has no doc.

**Files:**
- Modify: `crates/manifold/src/engine_install.rs`

**Interfaces:**
- Produces: `fn unpacked_mode(header_mode: u32) -> u32`, a pure function used under `cfg(unix)` and tested everywhere.

- [ ] **Step 1: Write the failing test** in `engine_install.rs` tests:

```rust
    #[test]
    fn unpacked_files_keep_exec_bits_but_never_setuid_or_group_write() {
        assert_eq!(unpacked_mode(0o100755), 0o755);
        assert_eq!(unpacked_mode(0o644), 0o644);
        assert_eq!(unpacked_mode(0o4755), 0o755, "setuid dropped: CEF runs with --no-sandbox");
        assert_eq!(unpacked_mode(0o777), 0o755);
    }
```

- [ ] **Step 2: Run** `cargo test -j 8 -p manifold --bins unpacked_files`. Expected: compile failure.

- [ ] **Step 3: Implement.**

```rust
/// The permission bits an unpacked file gets: the archive's own, minus
/// setuid/setgid/sticky and group/other write. `chrome-sandbox` needs setuid
/// root only for the setuid sandbox, which manifold doesn't use (the CEF host
/// passes `--no-sandbox`, see `manifold-engine-cef/src/app.rs`).
#[cfg_attr(not(unix), allow(dead_code))] // only applied on Unix; tested everywhere
fn unpacked_mode(header_mode: u32) -> u32 {
    header_mode & 0o755
}
```

In `unpack_with`, after `std::io::copy(&mut entry, &mut out).map_err(bad)?;`:

```rust
        // Without this every file lands 0644 and the CEF host can't exec its helpers.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = entry.header().mode().map_err(corrupt)?;
            std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(unpacked_mode(mode))).map_err(io)?;
        }
```

- [ ] **Step 4: Remove the 14 `#[allow(dead_code)]` lines** together with their "used by a later task" comments. Run `cargo clippy -j 8 -p manifold --all-targets -- -D warnings`.
  - If an item is now reported dead, check whether it is really unused (grep). If it is, delete it.
  - If an item is used only under `cfg(windows)` or `cfg(unix)`, give it an `#[cfg_attr(not(<os>), allow(dead_code))]` with a why-comment.

  Move the `custom_version` doc block (the lines starting "The CEF version a bring-your-own directory names") from above `type DirCheck` to directly above `fn custom_version`. Keep the one-line `DirCheck` doc where it is.

- [ ] **Step 5: Run** the full gate. Expected: PASS. Linux behaviour is verified by reading only. If WSL is available, `wsl.exe -d Ubuntu -- bash -c 'cd "/mnt/c/Users/Winston Li/Documents/GitHub/manifold" && CARGO_TARGET_DIR=target-linux cargo test -j 8 -p manifold --bins unpacked'` also runs the test on Unix.

- [ ] **Step 6: Commit**

```bash
git add crates/manifold/src/engine_install.rs
git commit -m "Engine install: apply archive file modes on Unix (exec bits, no setuid); drop stale dead_code allows; reattach custom_version's doc

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 12: Store logic: Recently Closed can't hang or panic; the clock can't wrap; imports don't trust foreign machine-local fields

**Findings:**
- **A-B4 (low-med).** `closed.rs` `closed_entry` (~99-116) walks group levels with `levels.push` and no visited set. A group cycle in the live graph, from a bad local move or a damaged import, hangs the UI thread in `record_closed`. Every other group walk has a "the loop must end" guard.
- **A-R5 (low).** The same walk rescans all of `app.node_order` once per level. Build the `group -> members` index once.
- **A-B5 (low, latent).** `closed.rs` `worth_reopening` returns `true` for `Kind::Unknown(_)` (48), while `store/json.rs` `node_json` does `kind.name().expect("a known kind")` (72). Return `false`: a node this build can't show isn't reopenable.
- **A-B6 (low).**
  - `store/model.rs` `Clock::tick` (95) does `ms + 1` when `logical` overflows. A CRC-valid stamp of `{ms: u64::MAX, logical: u32::MAX}`, observed from a file or replica, panics in debug and wraps to `{0, 0}` in release.
  - After the wrap every later local `Set` silently loses last-writer-wins.
  - Saturate instead.
- **A-Rb6 (low).** `store/json.rs` `import` (~116) keeps every known field from the file. That includes:
  - `path`, which is machine-local (`model.rs:229`). "Show in folder" passes a Download's `path` to `explorer.exe`.
  - any `jar` or `default_jar` id that parses. A shared export re-imported on its source machine re-links tabs to that machine's jars and logins.
  
  The decided policy is an explicit trust level:
  - `Own`: Recently Closed and named sessions, files this app wrote. Everything is kept.
  - `Foreign`: Import Session, Import Group and `--import`. A Download node's `path` is dropped, and `jar`/`default_jar` are kept only when they name a node in the same file.

**Files:**
- Modify: `crates/manifold/src/closed.rs` (`worth_reopening` ~48, `closed_entry` ~85-123)
- Modify: `crates/manifold/src/store/model.rs` (`Clock::tick`)
- Modify: `crates/manifold/src/store/json.rs`:
  - `import` gains a `trust: Trust` parameter, plus a `pub enum Trust`;
  - tests.
- Modify, the callers:
  - `closed.rs:169` (`Trust::Own`)
  - `session_sync.rs:1354` `import_group_json` (`Trust::Foreign`)
  - `session_sync.rs:1458` `replace_running_session`, which gains a `trust: Trust` parameter
  - its callers `main.rs:1901` (`load_named_session`, `Own`), `event_loop.rs:1140` (debug IMPORTSESSION, `Foreign`) and `popups.rs:1492` (Import Session, `Foreign`)
  - `cli.rs:78` (`Foreign`)
  - `store/journal.rs:1815`, a test (`Own`)
- Modify: `crates/manifold/src/main.rs` `reveal_node` / `node_file_path`: an empty path is "no file".

**Interfaces:**
- Produces:
  - `store::json::Trust { Own, Foreign }`
  - `store::json::import(json: &JsonDoc, next_id: impl FnMut() -> NodeId, trust: Trust) -> Result<Vec<Imported>, String>`
  - `App::replace_running_session(&mut self, json: &JsonDoc, trust: Trust) -> HashMap<String, String>`

- [ ] **Step 1: Write the failing tests.**

In `store/json.rs` tests (use the existing `ids(actor)` helper):

```rust
    fn doc(nodes: serde_json::Value) -> JsonDoc {
        serde_json::from_value(serde_json::json!({ "nodes": nodes })).unwrap()
    }

    #[test]
    fn a_foreign_import_drops_download_paths_and_outside_jars() {
        let outside = NodeId { actor: ActorId(9), counter: 9 }.to_string();
        let json = doc(serde_json::json!([
            { "id": "j", "kind": "jar", "fields": {} },
            { "id": "d", "kind": "download", "fields": { "path": "C:\\Windows\\x.exe" } },
            { "id": "t", "kind": "tab", "fields": { "url": "https://a", "jar": "j" } },
            { "id": "u", "kind": "tab", "fields": { "url": "https://b", "jar": outside } },
        ]));
        let foreign = import(&json, ids(1), Trust::Foreign).unwrap();
        assert!(!foreign[1].fields.contains_key(&field::PATH));
        assert_eq!(foreign[2].fields.get(&field::JAR), Some(&FieldValue::Id(Some(foreign[0].id))), "in-file jar kept");
        assert!(!foreign[3].fields.contains_key(&field::JAR), "a jar outside the file is dropped");
        let own = import(&json, ids(2), Trust::Own).unwrap();
        assert!(own[1].fields.contains_key(&field::PATH));
        assert!(own[3].fields.contains_key(&field::JAR));
    }
```

**Before relying on this test**, check the JSON shape `JsonNode` deserializes: field names `id`, `kind` and `fields`, and the kind spellings `jar`, `download` and `tab` in `model.rs`'s kind table around line 135. Adjust the literal to match. Also check how a `Kind::Jar` node is spelled.

In `store/model.rs` tests:

```rust
    #[test]
    fn the_clock_saturates_instead_of_wrapping() {
        let mut clock = Clock::default();
        let top = Hlc { ms: u64::MAX, logical: u32::MAX };
        clock.observe(top);
        assert!(clock.tick(0) >= top, "never behind a time seen");
    }
```

In `closed.rs` tests (add a test module if absent; `App::default()` works under test):

```rust
    #[test]
    fn an_unknown_kind_is_not_worth_reopening() {
        assert!(!worth_reopening(store::model::Kind::Unknown(99), &Default::default()));
    }
```

Check `worth_reopening`'s signature and the `Unknown` payload type, and match them. The cycle guard is tested through the pure index walk in Step 3.

- [ ] **Step 2: Run** `cargo test -j 8 -p manifold --bins foreign_import`, then `clock_saturates` and `unknown_kind`. Expected: FAIL or compile errors.

- [ ] **Step 3: Implement.**

`model.rs` `Clock::tick`:

```rust
                // 2^32 events in one millisecond: borrow the next one. At the
                // very top of the range, stay there: wrapping to 0 would put
                // every later local write behind every stamp seen.
                None => match ms.checked_add(1) {
                    Some(ms) => Hlc { ms, logical: 0 },
                    None => Hlc { ms, logical: u32::MAX },
                },
```

`closed.rs`:
- `Unknown(_) => false`. Move it out of the `true` arm, with the comment "a kind this build can't show (and `node_json` can't name) isn't reopenable".
- Replace the `levels` walk with a pure, cycle-safe member walk:

```rust
/// Every node under `root` at any depth, in `order`, each group visited
/// once: a group cycle (never built, but the loop must end) can't hang it.
fn members_under(
    root: store::model::NodeId,
    order: &[store::model::NodeId],
    group_of: impl Fn(&store::model::NodeId) -> Option<store::model::NodeId>,
    is_group: impl Fn(&store::model::NodeId) -> bool,
) -> Vec<store::model::NodeId> {
    let mut members: HashMap<store::model::NodeId, Vec<store::model::NodeId>> = HashMap::new();
    for id in order {
        if let Some(group) = group_of(id) {
            members.entry(group).or_default().push(*id);
        }
    }
    let (mut out, mut seen, mut levels) = (Vec::new(), HashSet::from([root]), vec![root]);
    while let Some(level) = levels.pop() {
        for id in members.get(&level).into_iter().flatten() {
            if is_group(id) && seen.insert(*id) {
                levels.push(*id);
            }
            if *id != root {
                out.push(*id);
            }
        }
    }
    out
}
```

`NodeId` must be `Copy + Hash + Eq`; it is used as a map key in the store. In `closed_entry`, build `let order: Vec<NodeId> = app.node_order.iter().filter_map(|s| s.parse().ok()).collect();` and call `members_under(root_id, &order, |id| shared.get(id).and_then(|n| n.group), |id| shared.get(id).is_some_and(|n| n.kind == Kind::Group))`. For each member, keep the existing body: the `worth_reopening` filter, `node_json` and `record`.

Ordering changes slightly. Before, members appeared level by level, each level in `node_order`. Now they still appear level by level, in a stack order. Recently Closed only needs every member present: reopening imports by label. Check `reopen_closed`'s use of order in `closed.rs` ~169. If anything there depends on draw order, sort `out` by position in `order` before returning.

Add the cycle test next to it:

```rust
    #[test]
    fn a_group_cycle_ends_the_walk() {
        use store::model::{ActorId, NodeId};
        let id = |n| NodeId { actor: ActorId(1), counter: n };
        let (a, b, c) = (id(1), id(2), id(3));
        // a contains b, b contains a (a cycle), and c is in b.
        let group_of = |n: &NodeId| match n.counter { 1 => Some(b), 2 => Some(a), 3 => Some(b), _ => None };
        let out = members_under(a, &[a, b, c], group_of, |n| n.counter < 3);
        assert_eq!(out.len(), 2, "{out:?}"); // b and c, once each
    }
```

`store/json.rs`:

```rust
/// How far an import trusts the file's machine-local fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    /// A file this app wrote for this profile (Recently Closed, a named
    /// session): every field is kept.
    Own,
    /// Anything else (Import Session/Group, `--import`): a Download's `path`
    /// is dropped (it would point "Show in folder" anywhere), and `jar` /
    /// `default_jar` are kept only when they name a node in the same file
    /// (an export re-imported on its source machine must not re-link tabs to
    /// that machine's logins).
    Foreign,
}
```

In `import`, after `let Some(def) = field_by_name(name) else { continue };`:

```rust
            if trust == Trust::Foreign {
                if def.no == field::PATH && kind == Kind::Download {
                    continue;
                }
                let names_in_file = value.as_str().is_some_and(|s| fresh.contains_key(s));
                if matches!(def.no, field::JAR | field::DEFAULT_JAR) && !names_in_file {
                    continue;
                }
            }
```

Extend `import`'s doc with the trust rule. Update every caller listed under Files. For `replace_running_session`, add the `trust` parameter and pass it through. `load_named_session` passes `Trust::Own`; Import Session (popups and the debug command) passes `Trust::Foreign`.

`main.rs`: wherever `node_file_path` returns a Download's path, filter out an empty one (`.filter(|p| !p.is_empty())`), so a foreign download shows no "Show in folder" target. Task 10 also refuses an empty path in `reveal_in_explorer`.

- [ ] **Step 4: Run** the new tests, then the full gate. Expected: PASS. Existing import, closed and journal tests must still pass with `Trust::Own`.

- [ ] **Step 5: Commit**

```bash
git add crates/manifold/src/closed.rs crates/manifold/src/store crates/manifold/src/session_sync.rs crates/manifold/src/main.rs crates/manifold/src/event_loop.rs crates/manifold/src/popups.rs crates/manifold/src/cli.rs
git commit -m "Store: Recently Closed walk is cycle-safe and skips unknown kinds; the clock saturates; foreign imports drop download paths and outside jar links

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 13: Keyboard focus: graph keys, Settings Escape, quick switcher, address-bar dropdown, suggestions

**Findings:**
- **C-B1 (high).** `graph_ui.rs` `handle_graph_keys` (1136-1186) reads ArrowKeys, Enter and Escape with `key_pressed`, gated only on `renaming_group`, `quick_switcher`, `node_menu` and `card_nav_active`. While typing in the graph filter field or a Note editor:
  - Enter runs `open_focused_graph_node`.
  - Arrows move `self.focused`.
  - Escape (meant to end a note edit) also clears `selected_tabs`.

  `handle_card_digits` (~1724) already guards with `egui_wants_keyboard_input()`. That guard is wrong here, because a focused *card* (not a text field) must keep arrow navigation. The right test is egui 0.36.2 `Context::text_edit_focused()` (`context.rs:2991`), read **at the top of `build_graph_ui`**: a singleline `TextEdit` surrenders focus while handling Enter or Escape, so a later read would already say false.
- **C-B5 (med-low).** `settings_page.rs` `build_settings_page` (62-68) closes the page on any Escape. The same press that cancels the CEF-dir or jar-name field (333-339, 515-520) also leaves Settings. The Home page field (163-165) commits on `lost_focus` even when Escape caused it.
- **C-B2 (med).** In `popups.rs`, `quick_switcher.selected` is clamped only in `quick_switcher_move` (1194-1205). Typing after ArrowDown x5 leaves `selected` past the end. No row is highlighted, and `quick_switcher_confirm` (1212-1220) closes without jumping.
- **C-B6 (low-med).** `tab_view.rs` `AddressBar::editing()` (54-56) is true while any suggestion exists, which is a guard against a one-frame focus blip. Clicking into the page never dismisses the dropdown. `build_address_suggestions` (685-698) then still takes ArrowUp and ArrowDown, and the address text is never resynced.
- **C-B7 (low).** `suggestions.rs` `tick` (173-178) dedupes on `last_fetched == query` alone. In the sequence "rust" (fetch, gen g), "rusty", then "rust" (gen g+2):
  - the debounce sees `last_fetched == "rust"` and doesn't fetch;
  - the g reply is then dropped as stale;
  - so no suggestions appear.
- **B-F4, the dedupe half.** Quick-switcher opening is duplicated in `shortcuts.rs:139-142` and `event_loop.rs:1039-1042`.

**Files:**
- Modify: `crates/manifold/src/graph_ui.rs`:
  - `build_graph_ui` (413) captures `typing`;
  - `handle_graph_keys` (1136) takes it;
  - tests.
- Modify: `crates/manifold/src/settings_page.rs`:
  - Escape guard (62);
  - Home page field (163) uses a draft;
  - tests.
- Modify: `crates/manifold/src/main.rs`: `App.home_page_draft: Option<String>`, plus Default near `cef_dir_draft` (1265/1514).
- Modify: `crates/manifold/src/popups.rs`:
  - `quick_switcher_confirm`;
  - the TextEdit `changed()` resets `selected`;
  - the render clamp;
  - new `open_quick_switcher`;
  - tests.
- Modify: `crates/manifold/src/shortcuts.rs:139-142` and `crates/manifold/src/event_loop.rs:1039-1042` to call `open_quick_switcher`.
- Modify: `crates/manifold/src/tab_view.rs`: `AddressBar.unfocused_frames`, `note_focus`, and the arrows gated.
- Modify: `crates/manifold/src/suggestions.rs`: `last_fetched: Option<(String, u64)>`, `due_fetch`, and tests.

**Interfaces:**
- Produces:
  - `App::handle_graph_keys(&mut self, ui: &egui::Ui, typing: bool)`
  - `App::open_quick_switcher(&mut self)`
  - `AddressBar::note_focus(&mut self, has_focus: bool)`
  - `SuggestionFetcher::due_fetch(&mut self, now: Instant) -> Option<(String, u64)>`

- [ ] **Step 1: Write the failing tests.**

In `graph_ui.rs` tests:

```rust
    /// Review C-B1: Escape (or Enter, or an arrow) typed into a text field is the field's, not the graph's.
    #[test]
    fn keys_typed_into_a_text_field_stay_out_of_graph_navigation() {
        let mut app = App::default();
        app.selected_tabs.insert("t".into());
        let ctx = egui::Context::default();
        let field = egui::Id::new("test_field");
        let mut text = String::new();
        let mut frame = |app: &mut App, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 400.0))),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                let typing = ui.ctx().text_edit_focused();
                ui.add(egui::TextEdit::singleline(&mut text).id(field));
                app.handle_graph_keys(ui, typing);
            });
            output.textures_delta.clear();
        };
        ctx.memory_mut(|m| m.request_focus(field));
        frame(&mut app, vec![]);
        frame(&mut app, vec![]);
        let key = |key| egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&mut app, vec![key(egui::Key::Escape)]);
        assert!(app.selected_tabs.contains("t"), "Escape ended the edit only");
    }
```

In `settings_page.rs` tests:

```rust
    /// Review C-B5: Escape in a Settings field reverts that field and keeps the page open.
    #[test]
    fn escape_in_a_settings_field_reverts_it_and_stays_on_the_page() {
        let ctx = egui::Context::default();
        let mut app = App::default();
        app.settings.home_page = "https://old".into();
        app.open_settings_page();
        let input = |events| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 3000.0))),
            events,
            ..Default::default()
        };
        ctx.memory_mut(|m| m.request_focus(HOME_PAGE_FIELD.into()));
        for _ in 0..2 {
            ctx.run_ui(input(vec![]), |ui| app.build_settings_page(ui)).textures_delta.clear();
        }
        ctx.run_ui(input(vec![egui::Event::Text("x".into())]), |ui| app.build_settings_page(ui)).textures_delta.clear();
        let escape = egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: Some(egui::Key::Escape),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        ctx.run_ui(input(vec![escape]), |ui| app.build_settings_page(ui)).textures_delta.clear();
        assert_eq!(app.view_mode, ViewMode::Settings, "still on Settings");
        assert_eq!(app.settings.home_page, "https://old", "Escape reverted the field");
    }
```

`save_settings` under `cfg(test)` writes nothing, because `profile::dir()` is `None`. Check that before relying on it. If it does write, the test must not run as is: stop and report.

In `popups.rs` tests:

```rust
    /// Review C-B2: a query change moves the selection back to the first match.
    #[test]
    fn a_narrowed_query_confirms_its_first_match() {
        let mut app = App::default();
        for _ in 0..6 {
            app.new_note_shortcut();
        }
        app.open_quick_switcher();
        for _ in 0..5 {
            app.quick_switcher_move(1);
        }
        app.quick_switcher.as_mut().unwrap().query = "no such tab".into();
        app.quick_switcher_confirm(); // nothing matches: closes, no panic
        assert!(app.quick_switcher.is_none());
    }
```

Notes are not listed tabs (`listed_tab`), so this only pins the no-panic, closed path. Read `listed_tab`: if `Unloaded` nodes are listed, insert six `Unloaded` nodes with distinct titles. Do it the way `quick_switcher` tests in `popups.rs` (1558+) build tabs; reuse their helper if one exists. Then assert that after `query = <one title>` and confirm, `app.focused` is that node.

In `suggestions.rs` tests:

```rust
    /// Review C-B7: returning to a query whose fetch was superseded fetches it again.
    #[test]
    fn a_query_typed_again_after_a_detour_is_fetched_again() {
        let mut fetcher = SuggestionFetcher::new();
        let later = |f: &SuggestionFetcher| f.pending_since.unwrap() + DEBOUNCE;
        fetcher.note_text_changed("rust");
        let first = fetcher.due_fetch(later(&fetcher)).expect("first fetch");
        assert_eq!(first.0, "rust");
        assert!(fetcher.due_fetch(later(&fetcher)).is_none(), "not twice for one generation");
        fetcher.note_text_changed("rusty");
        fetcher.note_text_changed("rust");
        let again = fetcher.due_fetch(later(&fetcher)).expect("fetched again");
        assert_eq!(again, ("rust".to_string(), fetcher.generation));
    }
```

In `tab_view.rs`, add a test module if none exists (`#[cfg(test)] mod tests { use super::*; ... }`):

```rust
    /// Review C-B6: the dropdown survives one unfocused frame (the focus blip) but not two.
    #[test]
    fn the_dropdown_closes_after_focus_stays_away() {
        let mut bar = AddressBar::new(String::new());
        bar.search_suggestions = vec!["rust".into()];
        bar.note_focus(false);
        assert!(bar.editing(), "one frame is the known blip");
        bar.note_focus(false);
        assert!(!bar.editing(), "dismissed");
    }
```

- [ ] **Step 2: Run** `cargo test -j 8 -p manifold --bins` with each name (`keys_typed_into`, `escape_in_a_settings`, `narrowed_query`, `typed_again_after`, `dropdown_closes`). Expected: compile errors, then FAIL.

- [ ] **Step 3: Implement the graph keys.** At the top of `build_graph_ui`, before anything draws:

```rust
        // Read before any widget runs: a singleline TextEdit gives up focus
        // while handling Enter/Escape, so later this frame it would read false.
        let typing = ui.ctx().text_edit_focused();
```

Pass it at line 486: `self.handle_graph_keys(ui, typing);`. In `handle_graph_keys`, take `typing: bool`, and change the gate `if self.renaming_group.is_none() && self.quick_switcher.is_none() {` to `if !typing && self.renaming_group.is_none() && self.quick_switcher.is_none() {`. The JSON branch (`handle_json_keys`) runs before the gate. Read it: if it also acts on Enter, arrows or Escape without a guard, pass `typing` into it and gate the same way. Add a doc line: "`typing`: a text field had focus at frame start; its keys are its own."

- [ ] **Step 4: Implement Settings.**

```rust
/// The Home page field's id, so tests (and focus requests) can name it.
pub(crate) const HOME_PAGE_FIELD: &str = "settings_home_page";
```

At the top of `build_settings_page`: `let typing = ui.ctx().text_edit_focused();`. Add `&& !typing` to the Escape close condition, with the comment "Escape in a field is that field's: it reverts the edit (CEF dir, jar name, Home page)."

Replace the Home page field:

```rust
        ui.label("Home page (new tabs)");
        let saved = self.settings.home_page.clone();
        let draft = self.home_page_draft.get_or_insert_with(|| saved.clone());
        let response =
            ui.add(egui::TextEdit::singleline(draft).id(egui::Id::new(HOME_PAGE_FIELD)).desired_width(f32::INFINITY));
        if response.lost_focus() {
            // Escape gives up the edit, as the CEF folder row does.
            if !ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.settings.home_page = self.home_page_draft.take().unwrap_or(saved);
                self.save_settings();
            }
            self.home_page_draft = None;
        } else if !response.has_focus() {
            self.home_page_draft = None; // track outside edits (a settings reload)
        }
```

Add `home_page_draft: Option<String>` to `App` with doc "The Home page field's text while it's being edited; Escape drops it (`build_settings_page`)." Default it to `None`.

- [ ] **Step 5: Implement the quick switcher.**

```rust
    /// Opens the quick switcher, empty, with its field focused (Ctrl+K and `QUICKSWITCHOPEN`).
    pub(crate) fn open_quick_switcher(&mut self) {
        self.quick_switcher = Some(QuickSwitcher { query: String::new(), selected: 0 });
        self.pending_focus = Some(FocusTarget::QuickSwitcher);
    }
```

Replace both duplicates with calls to it. In `quick_switcher_confirm`, use `matches.get(switcher.selected.min(matches.len().saturating_sub(1)))`. After the live `TextEdit` in `build_quick_switcher`, add `if response.changed() { switcher.selected = 0; }`, written before `take_focus` uses `response`. For the rendered highlight, clamp `selected_index` the same way after `matches` is computed: `let selected_index = selected_index.min(matches.len().saturating_sub(1));`.

- [ ] **Step 6: Implement the address bar.** Add a field to `AddressBar`:

```rust
    /// Frames in a row the field hasn't had focus. The dropdown survives one
    /// (egui's focus blip as its buttons appear, see `editing`) and closes
    /// after that: a click into the page must dismiss it.
    pub(crate) unfocused_frames: u8,
```

Initialize it to `0` in `new`, then add:

```rust
    /// Records this frame's focus; dismisses the dropdown once focus has
    /// been away for more than one frame.
    pub(crate) fn note_focus(&mut self, has_focus: bool) {
        self.focused = has_focus;
        self.unfocused_frames = if has_focus { 0 } else { self.unfocused_frames.saturating_add(1) };
        if self.unfocused_frames > 1 && (!self.open_tab_suggestions.is_empty() || !self.search_suggestions.is_empty()) {
            self.dismiss();
        }
    }
```

At `tab_view.rs:405`, replace `self.address_bar.focused = response.has_focus();` with `self.address_bar.note_focus(response.has_focus());`. In `build_address_suggestions`, gate the arrows: `let live = self.address_bar.unfocused_frames <= 1;`, then `let arrow_down = live && ui.input(..)` and the same for `arrow_up`.

The other writes `address_bar.focused = false` (`main.rs:1813`, `2571`) leave the dropdown state to `dismiss`. Read both: if either path also means "the field is gone", call `dismiss()` there too, as it may already.

- [ ] **Step 7: Implement suggestions.** Change `last_fetched` to `Option<(String, u64)>`:

```rust
    /// The fetch to start now, if one is due: the debounced query, once per
    /// generation. Keyed on the generation too, so returning to a query whose
    /// reply was superseded ("rust", "rusty", "rust") fetches it again.
    fn due_fetch(&mut self, now: Instant) -> Option<(String, u64)> {
        let (query, since) = (self.pending_query.as_ref()?, self.pending_since?);
        let job = (query.clone(), self.generation);
        if now.duration_since(since) < DEBOUNCE || self.last_fetched.as_ref() == Some(&job) {
            return None;
        }
        self.last_fetched = Some(job.clone());
        Some(job)
    }
```

In `tick`: `if let Some((query, gen)) = self.due_fetch(Instant::now()) { spawn_fetch(query, gen, self.tx.clone()); }`. `cancel` still sets `last_fetched = None`. Update the struct field's type.

- [ ] **Step 8: Run** the five tests, then the full gate. Expected: PASS. The existing `a_cancelled_fetch_cannot_deliver` and quick-switcher tests must still pass.

- [ ] **Step 9: Live check.** Build, launch at BelowNormal, and check:
  - In a Note, press Enter and Escape: no tab opens, and the selection stays.
  - In Settings > Home page, type and press Escape: the text reverts and the page stays.
  - In Tab view, type in the address bar and click the page: the dropdown closes.

  Kill only your PID.

- [ ] **Step 10: Commit**

```bash
git add crates/manifold/src/graph_ui.rs crates/manifold/src/settings_page.rs crates/manifold/src/main.rs crates/manifold/src/popups.rs crates/manifold/src/shortcuts.rs crates/manifold/src/event_loop.rs crates/manifold/src/tab_view.rs crates/manifold/src/suggestions.rs
git commit -m "Keys: text fields keep their keys from graph and Settings handlers; quick switcher resets on typing; address dropdown closes after focus leaves; suggestions refetch per generation

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 14: Note editing ends with its level; group members in a stable order; one level scan per frame

**Findings:**
- **C-B3 (med).** `card.rs` clears `note.editing` only when the editor's `lost_focus()` fires (1414-1446). Two other paths skip it:
  - Culling (`keeps_keyboard`, 917-921) drops an off-screen editing note.
  - `switch_group_level` (`graph_ui.rs:401`) leaves the level.

  In both cases egui drops the focus silently. `editing` stays true and the edit is never saved by `save_session`.
- **C-B8 (low-med).** `card_view` (`card.rs:868-876`) collects a group's members by iterating `self.nodes`, a randomly seeded `HashMap`. The 2x2 thumbnail (`take(4)`, ~1865) shows different members after each restart.
- **C-P2 (med).** The same scan runs once per group card per frame, so O(groups x nodes). `build_graph_ui` computes `at_level` (437) and then `level_items()` (`level.rs:48-66`) recomputes all 8 `nodes_at_current_level` lists.

**Files:**
- Modify: `crates/manifold/src/card.rs`:
  - `card_view` takes the index;
  - `CardPass.members`;
  - `keeps_keyboard`.
- Modify: `crates/manifold/src/graph_ui.rs`:
  - `switch_group_level`;
  - `build_graph_ui` builds the index and calls `level_items_from`.
- Modify: `crates/manifold/src/level.rs`: `level_items_from`.
- Modify: `crates/manifold/src/tabs.rs`: `focus_tab` (636) and the Image arm of `focus_item` (663) call `end_note_edits`.
- Modify: `crates/manifold/src/settings_page.rs`: `open_settings_page` (711).

**Interfaces:**
- Produces:
  - `App::end_note_edits(&mut self) -> bool`
  - `App::group_members(&self) -> HashMap<String, Vec<String>>`
  - `App::card_view(&self, id: &str, members: &HashMap<String, Vec<String>>) -> Option<CardView>`
  - `CardPass.members: &'a HashMap<String, Vec<String>>`
  - `App::level_items_from(&self, at_level: &[Vec<String>]) -> Vec<LevelItem>`

- [ ] **Step 1: Write the failing tests** in `card.rs` tests:

```rust
    /// Review C-B3: leaving a level ends a note edit there.
    #[test]
    fn leaving_the_level_ends_a_note_edit() {
        let mut app = App::default();
        app.new_note_shortcut();
        let id = app.node_order[0].clone();
        app.nodes.get_mut(&id).unwrap().as_note_mut().unwrap().editing = true;
        app.switch_group_level(None);
        assert!(!app.nodes[&id].as_note().unwrap().editing);
    }

    /// Review C-B8: a group's members come in creation order, every time.
    #[test]
    fn group_members_follow_node_order() {
        let mut app = App::default();
        let g = app.alloc_node_id();
        app.insert_group_at(g.clone(), None);
        let mut made = Vec::new();
        for _ in 0..20 {
            app.new_note_shortcut();
            let id = app.node_order.last().unwrap().clone();
            app.nodes.get_mut(&id).unwrap().group = Some(g.clone());
            made.push(id);
        }
        assert_eq!(app.group_members()[&g], made);
        assert_eq!(app.card_view(&g, &app.group_members()).unwrap().members, made);
    }
```

In `level.rs` tests:

```rust
    #[test]
    fn level_items_from_a_precomputed_scan_match_a_fresh_one() {
        let mut app = App::default();
        for _ in 0..3 {
            app.new_note_shortcut();
        }
        let at_level = GRAPH_DRAW_ORDER.map(|kind| app.nodes_at_current_level(kind));
        assert_eq!(app.level_items_from(&at_level), app.level_items());
    }
```

- [ ] **Step 2: Run** `cargo test -j 8 -p manifold --bins` with `leaving_the_level`, `group_members_follow` and `level_items_from`. Expected: FAIL or compile errors.

- [ ] **Step 3: Implement.** Add to `card.rs` `impl App`:

```rust
    /// Ends every note edit, saving if one was on. A note's TextEdit that
    /// stops being laid out (its level left, the view changed) loses focus
    /// without `lost_focus()` ever firing, so `editing` would stay on.
    pub(crate) fn end_note_edits(&mut self) -> bool {
        let mut ended = false;
        for note in self.nodes.values_mut().filter_map(|n| n.as_note_mut()) {
            ended |= std::mem::take(&mut note.editing);
        }
        if ended {
            self.save_session();
        }
        ended
    }

    /// Every group's members, in `node_order`: built once per card pass.
    pub(crate) fn group_members(&self) -> HashMap<String, Vec<String>> {
        let mut members: HashMap<String, Vec<String>> = HashMap::new();
        for id in &self.node_order {
            if let Some(group) = self.nodes.get(id).and_then(|n| n.group.as_ref()) {
                members.entry(group.clone()).or_default().push(id.clone());
            }
        }
        members
    }
```

`HashMap` here is whatever `card.rs` already imports through `use super::*`. If that is foldhash's, the same type works.

Change `card_view` to `(&self, id: &str, members: &HashMap<String, Vec<String>>)`, with `let members = if kind == ItemNodeKind::Group { members.get(id).cloned().unwrap_or_default() } else { Vec::new() };`. Add `pub(crate) members: &'a HashMap<String, Vec<String>>,` to `CardPass`, documented "Every group's members (`App::group_members`), built once before the pass." In `draw_graph_card` call `self.card_view(id, pass.members)`.

In `graph_ui.rs`, before the `CardPass` literal (~1583), add `let members = self.group_members();` and the field `members: &members,`. Fix any other `CardPass {` literal (tests) the compiler reports.

In `keeps_keyboard` add `|| ui.memory(|m| m.has_focus(egui::Id::new(("note_edit", id))))`, and extend the comment: "and a note being edited, whose editor must stay laid out to keep its focus".

Call `self.end_note_edits();` as the first line of:
- `switch_group_level`;
- `focus_tab`;
- the `ItemNodeKind::Image` arm of `focus_item`;
- `open_settings_page`, inside the `if`.

`level.rs`:

```rust
    pub(crate) fn level_items(&self) -> Vec<LevelItem> {
        self.level_items_from(&GRAPH_DRAW_ORDER.map(|kind| self.nodes_at_current_level(kind)))
    }

    /// `level_items` from an `at_level` scan already made this frame
    /// (`build_graph_ui`), so the level isn't walked twice.
    pub(crate) fn level_items_from(&self, at_level: &[Vec<String>]) -> Vec<LevelItem> {
        if let Some(level) = &self.json_level {
            // ... the existing JSON branch, unchanged ...
        }
        at_level
            .iter()
            .flatten()
            .filter_map(|id| {
                let node = self.nodes.get(id)?;
                Some(LevelItem {
                    kind: LevelItemKind::Node(item_node_kind(&node.payload)),
                    rect: node_card_rect(node),
                    id: id.clone(),
                })
            })
            .collect()
    }
```

In `build_graph_ui` (439): `let items = self.level_items_from(&at_level);`. Shorten the long "found during a performance audit" comment above `at_level` to two lines: "One level scan per frame, reused by the toolbar, settle key, minimap, items and cards; `current_group` only changes at the end of this function."

- [ ] **Step 4: Run** the three tests, then the full gate. Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/manifold/src/card.rs crates/manifold/src/graph_ui.rs crates/manifold/src/level.rs crates/manifold/src/tabs.rs crates/manifold/src/settings_page.rs
git commit -m "Notes: leaving a level or the graph ends and saves an edit, and an editing note isn't culled; group members in node order from one index per pass; one level scan per frame

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 15: Graph geometry: layout separates stacked nodes and is deterministic; camera, outline and title fixes

**Findings:**
- **C-B9 (low).**
  - `graph_layout.rs` `step` (56-73): when two nodes coincide, `delta == 0`, so `dir = 0`, so there is no repulsion. They stay stacked forever. This happens on placeholder inserts, imports or duplicates at one position.
  - Iterating `positions.keys()` (a `HashMap`) makes the float summation order, and so the layout, differ run to run.
  - The doc (39-43) says pinned nodes "neither push nor get pushed", but they do push.
- **C-P3 (med).** The same function clones every id each step, rebuilds `forces` as a `HashMap<String, _>`, and does four String hash lookups per pair. That is about 16M hashes per 90-step settle at n = 300.
- **C-B10 (low).** `graph_ui.rs` 524-546 computes `zoom_scale`, `inv_zoom`, `project` and `zoom_pct` before `self.camera.advance()`, while the Scene renders the advanced rect. Strokes and the new-node menu are then one frame behind during a tween or a flick.
- **C-B11 (low).**
  - `graph_world_rect_to_screen` (253-268) subtracts the JSON outline width.
  - `graph_screen_point_to_world` (275-283) doesn't.
  - So a file dropped on a JSON level with the outline open lands offset.
- **C-B12 (low).**
  - `card.rs` ~1944-2019: strip-mode titles are measured with `title_font` (`HEADER_TITLE_SIZE`, 13) but drawn at 12 (2019). A rich (hidden-character) name uses 12 both ways, so adjacent strip titles differ.
  - `node.rs` `accessible_name` (~569-588) gives local Image, Pdf and Json cards no name, and the doc "The web source of an Image..." sits on it instead of on `source_url`.

**Files:**
- Modify: `crates/manifold/src/graph_layout.rs` (`step`, doc, tests)
- Modify: `crates/manifold/src/graph_ui.rs` (524-546, 253-283)
- Modify: `crates/manifold/src/json_level.rs` (test only)
- Modify: `crates/manifold/src/card.rs` (~1944-2019)
- Modify: `crates/manifold/src/node.rs` (`accessible_name`, `source_url` doc, tests)

**Interfaces:**
- `graph_layout::step`'s signature is unchanged.
- Produces: `App::graph_scene_outer_rect(&self) -> egui::Rect`.

- [ ] **Step 1: Write the failing tests.**

In `graph_layout.rs` tests:

```rust
    fn pair_at(a: egui::Vec2, b: egui::Vec2) -> (HashMap<String, egui::Vec2>, HashMap<String, egui::Vec2>) {
        let positions = HashMap::from([("a".to_string(), a), ("b".to_string(), b)]);
        let velocities = HashMap::from([("a".to_string(), egui::Vec2::ZERO), ("b".to_string(), egui::Vec2::ZERO)]);
        (positions, velocities)
    }

    /// Review C-B9: two nodes on one point push apart.
    #[test]
    fn coincident_nodes_separate() {
        let (mut positions, mut velocities) = pair_at(egui::Vec2::ZERO, egui::Vec2::ZERO);
        for _ in 0..600 {
            step(&mut positions, &mut velocities, &[], &HashSet::new(), egui::Vec2::ZERO, 1.0 / 60.0);
        }
        assert!((positions["a"] - positions["b"]).length() > 100.0, "{positions:?}");
    }

    /// Same input, same output, whatever order the maps iterate in.
    #[test]
    fn the_layout_is_deterministic() {
        let ids: Vec<String> = (0..30).map(|i| format!("n{i}")).collect();
        let run = |order: &[String]| {
            let mut positions = HashMap::new();
            let mut velocities = HashMap::new();
            for id in order {
                let i: f32 = id[1..].parse().unwrap();
                positions.insert(id.clone(), egui::vec2(i * 7.0 % 50.0, i * 13.0 % 40.0));
                velocities.insert(id.clone(), egui::Vec2::ZERO);
            }
            let edges = vec![("n0".to_string(), "n1".to_string())];
            for _ in 0..90 {
                step(&mut positions, &mut velocities, &edges, &HashSet::new(), egui::Vec2::ZERO, 1.0 / 60.0);
            }
            let mut out: Vec<_> = positions.into_iter().collect();
            out.sort_by(|a, b| a.0.cmp(&b.0));
            out
        };
        let reversed: Vec<String> = ids.iter().rev().cloned().collect();
        assert_eq!(run(&ids), run(&reversed));
    }
```

In `json_level.rs` tests, next to `entering_builds_the_level_and_leaving_drops_it`:

```rust
    /// Review C-B11: screen<->world agree with the outline open.
    #[test]
    fn screen_to_world_inverts_world_to_screen_with_the_outline_open() {
        let (mut app, j) = app_with_json("inverse", r#"{"a": 1}"#);
        app.switch_group_level(Some(j));
        app.json_outline_visible = true;
        app.json_outline_width = 300.0;
        let world = egui::Rect::from_min_size(egui::pos2(40.0, 60.0), egui::vec2(10.0, 10.0));
        let screen = app.graph_world_rect_to_screen(world);
        let back = app.graph_screen_point_to_world(screen.min);
        assert!((back - world.min.to_vec2()).length() < 0.01, "{back:?}");
    }
```

In `node.rs` tests:

```rust
    /// Review C-B12: a local file's card is named by its file name.
    #[test]
    fn a_local_file_card_has_an_accessible_name() {
        let node = Node::new(NodePayload::Pdf(PdfPayload { path: r"C:\docs\report.pdf".into(), outcome: None, url: None }));
        assert_eq!(node.accessible_name(), "PDF: report.pdf");
    }
```

Construct the node however the existing `node.rs` tests do, around 680-720; `Node::new` is illustrative. Match the real constructor.

- [ ] **Step 2: Run** `cargo test -j 8 -p manifold --bins` with `coincident_nodes`, `layout_is_deterministic`, `inverts_world_to_screen` and `local_file_card`. Expected: `coincident_nodes` FAILs (distance 0), `inverts` FAILs, and `accessible_name` FAILs ("PDF"). `deterministic` may pass or fail by chance; it guards the rewrite.

- [ ] **Step 3: Rewrite `step` index-based**, keeping the signature:

```rust
/// One force-directed simulation tick.
///
/// `positions`/`velocities` are keyed by node id and updated in place.
/// `edges` are `(parent_id, child_id)` pairs. `pinned` holds node ids being
/// dragged: their position is the caller's and they don't move, but they
/// still push the others (a dragged card shoulders its neighbours aside).
/// Ids are processed in sorted order, so the result doesn't depend on the
/// maps' iteration order.
pub fn step(
    positions: &mut HashMap<String, egui::Vec2>,
    velocities: &mut HashMap<String, egui::Vec2>,
    edges: &[(String, String)],
    pinned: &HashSet<String>,
    center: egui::Vec2,
    dt: f32,
) {
    if dt <= 0.0 {
        return;
    }
    let mut ids: Vec<&String> = positions.keys().collect();
    ids.sort();
    let index: HashMap<&str, usize> = ids.iter().enumerate().map(|(i, id)| (id.as_str(), i)).collect();
    let pos: Vec<egui::Vec2> = ids.iter().map(|id| positions[*id]).collect();
    let mut forces = vec![egui::Vec2::ZERO; ids.len()];

    // Pairwise repulsion: O(n^2) over plain vectors.
    for i in 0..pos.len() {
        for j in (i + 1)..pos.len() {
            let delta = pos[i] - pos[j];
            let length = delta.length();
            // Two nodes on one point have no direction between them: pick a
            // fixed one from their ids so they still push apart.
            let dir = if length > f32::EPSILON {
                delta / length
            } else {
                spawn_jitter(&format!("{}\u{0}{}", ids[i], ids[j])).normalized()
            };
            let dist = length.max(MIN_DISTANCE);
            let force = dir * (REPULSION_STRENGTH / (dist * dist));
            forces[i] += force;
            forces[j] -= force;
        }
    }

    // Spring attraction along parent->child edges toward a rest length.
    for (parent, child) in edges {
        // An edge to a since-removed node is ignored.
        let (Some(&p), Some(&c)) = (index.get(parent.as_str()), index.get(child.as_str())) else { continue };
        let delta = pos[c] - pos[p];
        let dist = delta.length().max(MIN_DISTANCE);
        let force = delta / dist * (SPRING_STRENGTH * (dist - REST_LENGTH));
        forces[p] += force;
        forces[c] -= force;
    }

    let ids: Vec<String> = ids.into_iter().cloned().collect();
    for (i, id) in ids.iter().enumerate() {
        // Gentle pull toward the canvas center, so a disconnected node
        // doesn't drift away.
        let force = forces[i] + (center - pos[i]) * CENTERING_STRENGTH;
        // Pinned (dragged) nodes stay exactly where the caller put them, with
        // no velocity, so releasing the drag doesn't fling them.
        if pinned.contains(id) {
            velocities.insert(id.clone(), egui::Vec2::ZERO);
            continue;
        }
        let mut vel = velocities.get(id).copied().unwrap_or(egui::Vec2::ZERO);
        vel = (vel + force * dt) * DAMPING;
        if vel.length() > MAX_SPEED {
            vel *= MAX_SPEED / vel.length();
        }
        if vel.length() < SETTLE_SPEED {
            vel = egui::Vec2::ZERO;
        }
        if let Some(p) = positions.get_mut(id) {
            *p += vel * dt;
        }
        velocities.insert(id.clone(), vel);
    }
}
```

The `ids` borrow of `positions` ends at the `cloned()` line, before `positions.get_mut`. If the borrow checker disagrees, clone into `Vec<String>` up front and build `index` from it. Write a short comment above the clone: "owned ids: `positions` is mutated below".

- [ ] **Step 4: Implement the camera order** (`graph_ui.rs` ~524-546). Move these lines:

```rust
        self.camera.advance();
        if self.camera.tween.is_some() || self.camera.momentum.is_some() {
            ui.ctx().request_repaint();
        }
        self.camera.fill(scene_outer_rect);
```

from after `handle_rect_select` to directly after `self.apply_zoom_requests(...)`, before `graph_world_to_screen_transform`. Add the comment: "Advance first: everything below (strokes, `zoom_pct`, the new-node menu) must use the rect the Scene draws this frame."

- [ ] **Step 5: Implement the shared scene rect** (`graph_ui.rs` 253-283):

```rust
    /// The Graph view's scene area in window points: below the toolbar,
    /// minus a JSON level's outline on the right (plan ruling 12). The one
    /// definition both screen<->world conversions use.
    pub(crate) fn graph_scene_outer_rect(&self) -> egui::Rect {
        let screen = self.screen_size_points();
        let outline =
            if self.json_level.is_some() && self.json_outline_visible { self.json_outline_width } else { 0.0 };
        egui::Rect::from_min_size(
            egui::pos2(0.0, GRAPH_TOOLBAR_HEIGHT_POINTS),
            egui::vec2((screen.x - outline).max(0.0), (screen.y - GRAPH_TOOLBAR_HEIGHT_POINTS).max(0.0)),
        )
    }
```

Both functions use `let (scale, translation) = self.graph_world_to_screen_transform(self.graph_scene_outer_rect());`. Line 524 keeps `ui.available_rect_before_wrap()`: that is the live layout rect, and the helper mirrors it for code outside the frame.

- [ ] **Step 6: Implement the title size and a11y name.** In `card.rs`, move `let title_size = if tall { HEADER_TITLE_SIZE } else { 12.0 };` above `title_font`, and set `let title_font = egui::FontId::proportional(title_size);`. In the strip branch (~2019), pass `title_font` instead of `egui::FontId::proportional(12.0)`. Measure and draw now agree for both rich and plain names.

In `node.rs`, move the doc line "The web source of an Image, Pdf, Json (not a leaf) or Note node." onto `source_url`. Change the three arms:

```rust
            NodePayload::Image(p) => ("Image", self.source_url().map(str::to_string).or_else(|| crate::content::file_name(&p.path))),
            NodePayload::Pdf(p) => ("PDF", self.source_url().map(str::to_string).or_else(|| crate::content::file_name(&p.path))),
            NodePayload::Json(p) => ("JSON", self.source_url().map(str::to_string).or_else(|| crate::content::file_name(&p.path))),
```

Check `content::file_name`'s return for a Windows path in its own tests. It must split on `\` too; if it doesn't, use `p.path.rsplit(['/', '\\']).next().map(str::to_string)` as the Download arm does.

- [ ] **Step 7: Run** the four tests, the existing `graph_layout` tests, and the full gate. Expected: PASS.

- [ ] **Step 8: Live check.** Build and launch at BelowNormal. Duplicate a note twice quickly and check that the copies separate. Enter a group and watch the tween for shimmer. Kill only your PID.

- [ ] **Step 9: Commit**

```bash
git add crates/manifold/src/graph_layout.rs crates/manifold/src/graph_ui.rs crates/manifold/src/json_level.rs crates/manifold/src/card.rs crates/manifold/src/node.rs
git commit -m "Graph: index-based deterministic layout that separates stacked nodes; camera advanced before projecting; one scene rect for both conversions; strip titles one size; local files named for screen readers

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 16: PDF: a failed pdfium bind is retried; Fit fits huge images; deleted nodes' renders are skipped

**Findings:**
- **D-B4 (med).** `pdf_fetch.rs:56-58` does `pdfium.get_or_insert_with(|| bind_pdfium_silent()...)`, which caches an `Err` for the whole run. If the first PDF is added offline, every later PDF fails, including Retry (`fetched.rs:198-203`), until a restart.
- **D-R8 (low).** `image_view.rs:69` clamps Fit to `IMAGE_ZOOM_MIN` (0.1). An image more than 10x the body (an 8000 px image in a 700 px window) overflows while Fit pins `pan = ZERO`, so its edges are unreachable. `zoom_about` (105) would also *raise* a below-min scale to 0.1 on zoom-out.
- **D-R9, the cancel half.** Renders run for nodes deleted meanwhile; `event_loop.rs` ~1415 drops the late result, but the work is wasted. The child-process half is deferred (see Deferred).

**Files:**
- Modify: `crates/manifold/src/pdf_fetch.rs`: `bind_once`, the cancel set, `render_first_page(&Pdfium, ..)`, tests.
- Modify: `crates/manifold/src/image_view.rs`: `image_fit_scale`, `zoom_about`, tests.
- Modify: `crates/manifold/src/main.rs`: `remove_node` (2029) calls `pdf_fetcher.cancel`.

**Interfaces:**
- Produces:
  - `pdf_fetch::bind_once<T, E>(slot: &mut Option<T>, bind: impl FnOnce() -> Result<T, E>) -> Result<&T, E>`
  - `PdfFetcher::cancel(&self, id: &str)`

- [ ] **Step 1: Write the failing tests.**

In `pdf_fetch.rs` tests:

```rust
    /// Review D-B4: a failed bind isn't cached; the next request tries again.
    #[test]
    fn a_failed_bind_is_retried_and_a_good_one_kept() {
        let mut slot: Option<u32> = None;
        let mut calls = 0;
        let mut bind = |ok: bool| {
            calls += 1;
            if ok { Ok(7) } else { Err("offline".to_string()) }
        };
        assert!(bind_once(&mut slot, || bind(false)).is_err());
        assert_eq!(bind_once(&mut slot, || bind(true)), Ok(&7));
        assert_eq!(bind_once(&mut slot, || bind(false)), Ok(&7), "kept, not rebound");
        assert_eq!(calls, 2);
    }

    #[test]
    fn a_cancelled_request_is_skipped() {
        let cancelled = std::sync::Mutex::new(std::collections::HashSet::from(["gone".to_string()]));
        assert!(take_cancelled(&cancelled, "gone"));
        assert!(!take_cancelled(&cancelled, "gone"), "taken once");
        assert!(!take_cancelled(&cancelled, "kept"));
    }
```

The test's `bind` closure captures `calls` mutably while being called through `|| bind(..)`. If the borrow checker rejects it, use a `std::cell::Cell<u32>` for `calls`.

In `image_view.rs` tests:

```rust
    /// Review D-R8: Fit really fits an image over 10x the body, and zooming out from there doesn't jump in.
    #[test]
    fn fit_fits_a_huge_image_and_zoom_out_keeps_going_down() {
        let fit = image_fit_scale(egui::vec2(8000.0, 100.0), egui::vec2(700.0, 500.0));
        assert!((fit - 700.0 / 8000.0).abs() < 1e-6, "{fit}");
        assert_eq!(zoom_about(0.05, egui::Vec2::ZERO, 0.5, egui::Vec2::ZERO).0, 0.05, "never up on zoom-out");
        assert_eq!(zoom_about(0.11, egui::Vec2::ZERO, 0.5, egui::Vec2::ZERO).0, IMAGE_ZOOM_MIN);
    }
```

- [ ] **Step 2: Run** `cargo test -j 8 -p manifold --bins` with `failed_bind`, `cancelled_request` and `fit_fits`. Expected: compile errors, then FAIL.

- [ ] **Step 3: Implement.** In `pdf_fetch.rs`:

```rust
/// The bound value in `slot`, binding it first if needed. Only success is
/// kept: a failed bind (offline on the first PDF of a run, or a failed
/// first-time DLL download) is retried on the next request.
fn bind_once<T, E>(slot: &mut Option<T>, bind: impl FnOnce() -> Result<T, E>) -> Result<&T, E> {
    if slot.is_none() {
        *slot = Some(bind()?);
    }
    Ok(slot.as_ref().expect("just bound"))
}

/// Whether `id` was cancelled (its node deleted) before its render ran;
/// removes the mark.
fn take_cancelled(cancelled: &std::sync::Mutex<std::collections::HashSet<String>>, id: &str) -> bool {
    cancelled.lock().map(|mut set| set.remove(id)).unwrap_or(false)
}
```

Add `cancelled: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<String>>>` to `PdfFetcher`. Clone it into the worker. The loop becomes:

```rust
            let mut pdfium: Option<Pdfium> = None;
            for request in thread_rx {
                if take_cancelled(&worker_cancelled, &request.id) {
                    continue;
                }
                let result = bind_once(&mut pdfium, || pdfium_auto::bind_pdfium_silent().map_err(|e| e.to_string()))
                    .and_then(|pdfium| render_first_page(pdfium, &request.path));
                let _ = result_tx.send((request.id, result));
            }
```

Then:
- `render_first_page(pdfium: &Pdfium, path: &str)`: drop its first line.
- Add `pub fn cancel(&self, id: &str) { if let Ok(mut set) = self.cancelled.lock() { set.insert(id.to_string()); } }`.
- In `tick`, drop marks for results that came back anyway: `.inspect(|(id, _)| { take_cancelled(&self.cancelled, id); })`.
- Rewrite the long thread comment as: "Bound on the first request, not at launch (a first-time bind can download the DLL). A success is kept for the run; a failure is retried next request (`bind_once`)."

In `main.rs` `remove_node`, after `let removed = ...`: `if removed.as_ref().is_some_and(|n| n.as_pdf().is_some()) { self.pdf_fetcher.cancel(id); }`.

In `image_view.rs`:

```rust
/// Contain `image` within `body`, never above 1.0 (a small image isn't
/// magnified). No lower clamp: Fit pins the pan, so a scale clamped above
/// the true fit would leave the edges unreachable.
pub(crate) fn image_fit_scale(image: egui::Vec2, body: egui::Vec2) -> f32 {
    if image.x <= 0.0 || image.y <= 0.0 {
        return 1.0;
    }
    (body.x / image.x).min(body.y / image.y).clamp(f32::MIN_POSITIVE, 1.0)
}
```

In `zoom_about`, clamp against the current scale too: `let new_scale = (scale * factor).clamp(IMAGE_ZOOM_MIN.min(scale), IMAGE_ZOOM_MAX);`. Update its doc: "(clamped to `IMAGE_ZOOM_MIN..=IMAGE_ZOOM_MAX`, or to the current scale if Fit already went below the minimum)".

- [ ] **Step 4: Run** the tests, including the existing `image_fit_scale_contains_and_never_upscales`, then the full gate. Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/manifold/src/pdf_fetch.rs crates/manifold/src/image_view.rs crates/manifold/src/main.rs
git commit -m "PDF: retry a failed pdfium bind and skip renders of deleted nodes; Image Fit fits images over 10x the window

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 17: JSON search: no stale result while a worker runs; cancellable, capped; the value shared, not cloned

**Findings:**
- **D-B6 (low).** `json_level.rs` `set_json_search` (269-291), on the worker path, leaves `search.result` holding the previous query's result. The outline (`poll_json_search`) and `step_json_match` then use the old query's matches until the worker answers.
- **D-R7 (med).** Every query change spawns a full-document worker that is never cancelled. `json_view::search` (428-457) is unbounded: a one-letter query on a large file collects every pointer three times (`matches`, `hits`, `keep`).
- **D-R6, the clone half.** `json_level.rs:209` passes `json.value.clone()` into `JsonLevel::new`. That deep-clones the whole document on every level entry, although `JsonLevel` already holds an `Arc<Value>` (88). The parse-on-worker half is deferred.

**Files:**
- Modify: `crates/manifold/src/json_view.rs`: `search_with`, `MAX_MATCHES`, `SearchResult.truncated`, tests.
- Modify: `crates/manifold/src/json_level.rs`:
  - `set_json_search` takes a generation;
  - `Search.generation: Arc<AtomicU64>`;
  - `JsonLevel::new(.., Arc<Value>, ..)`;
  - tests.
- Modify: `crates/manifold/src/json_level_ui.rs`: the status (801-807) shows the cap.
- Modify: `crates/manifold/src/node.rs`: `JsonPayload.value: Arc<serde_json::Value>` (329).
- Modify: every construction or assignment of `JsonPayload::value` the compiler reports. Found by grep: `fetched.rs:301`, `fetched.rs:825`, `json_level.rs:697`, `node.rs:716`, `session_sync.rs:653`, and the `json_file::load` result site in `fetched.rs` (~560).

**Interfaces:**
- Produces:
  - `json_view::MAX_MATCHES: usize = 10_000`
  - `json_view::search_with(root: &Value, query: &str, cancelled: &dyn Fn() -> bool) -> Option<SearchResult>`
  - `SearchResult.truncated: bool`
  - `JsonPayload.value: Arc<serde_json::Value>`

- [ ] **Step 1: Write the failing tests.**

In `json_view.rs` tests:

```rust
    /// Review D-R7: results stop at MAX_MATCHES, and a cancelled search returns nothing.
    #[test]
    fn search_is_capped_and_cancellable() {
        let big = serde_json::Value::Array((0..MAX_MATCHES + 50).map(|i| serde_json::json!(format!("x{i}"))).collect());
        let result = search(&big, "x");
        assert_eq!(result.matches.len(), MAX_MATCHES);
        assert!(result.truncated);
        assert!(search_with(&big, "x", &|| true).is_none());
        assert!(!search(&serde_json::json!({"a": "x"}), "x").truncated);
    }
```

In `json_level.rs` tests:

```rust
    /// Review D-B6: while a worker searches, the previous query's result is gone.
    #[test]
    fn a_pending_search_hides_the_previous_result() {
        let items: Vec<String> = (0..json_view::WORKER_SEARCH_NODES + 10).map(|i| format!("v{i}")).collect();
        let (mut app, j) = app_with_json("pending", &serde_json::to_string(&items).unwrap());
        app.switch_group_level(Some(j));
        app.set_json_search("v1".into());
        app.set_json_search("v2".into());
        let level = app.json_level.as_ref().unwrap();
        assert!(level.search.pending.is_none() || level.search.result.is_none(), "no stale result beside a pending search");
    }
```

The worker may already have answered by the assert, which is why the test accepts either state. To prove the fix, temporarily remove the new `result = None` line and run the test repeatedly; it should fail most runs. Record the observed pass/fail ratio in the task report.

- [ ] **Step 2: Run** `cargo test -j 8 -p manifold --bins` with `capped_and_cancellable` and `pending_search_hides`. Expected: compile errors, then FAIL.

- [ ] **Step 3: Implement the search.** In `json_view.rs`:

```rust
/// The most matches a search collects; the status line says when it stopped.
pub(crate) const MAX_MATCHES: usize = 10_000;
```

Add to `SearchResult`: `/// The search stopped at MAX_MATCHES; there are more. pub(crate) truncated: bool,`.

```rust
pub(crate) fn search(root: &Value, query: &str) -> SearchResult {
    search_with(root, query, &|| false).expect("never cancelled")
}

/// `search`, checking `cancelled` every 4096 nodes: `None` once it says so
/// (a newer query superseded this worker's).
pub(crate) fn search_with(root: &Value, query: &str, cancelled: &dyn Fn() -> bool) -> Option<SearchResult> {
    let mut result = SearchResult::default();
    let needle = query.to_lowercase();
    if needle.is_empty() {
        return Some(result);
    }
    let mut stack: Vec<(String, Option<&str>, &Value)> = vec![(String::new(), None, root)];
    let mut visited = 0usize;
    while let Some((pointer, key, value)) = stack.pop() {
        visited += 1;
        if visited % 4096 == 0 && cancelled() {
            return None;
        }
        // ... the existing hit test and push code, unchanged ...
        if result.matches.len() == MAX_MATCHES {
            result.truncated = true;
            break;
        }
        // ... the existing children push, unchanged ...
    }
    Some(result)
}
```

The cap check goes right after the match is recorded, so it stops at exactly `MAX_MATCHES`.

In `json_level.rs`, add to `Search`: `/// Bumped per query; a worker whose number is stale stops (search_with). pub(crate) generation: std::sync::Arc<std::sync::atomic::AtomicU64>,`. `Default` derives it as `Arc::new(AtomicU64::new(0))`; if `Search` has a hand Default, add it there. In `set_json_search`:

```rust
        let generation = level.search.generation.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        level.search.result = None; // the old query's matches mustn't show or step while this one runs
        if query.is_empty() {
        } else if level.nodes > json_view::WORKER_SEARCH_NODES {
            let (tx, rx) = mpsc::channel();
            let (value, q, current) = (level.value.clone(), query.clone(), level.search.generation.clone());
            std::thread::spawn(move || {
                let stale = || current.load(std::sync::atomic::Ordering::Relaxed) != generation;
                if let Some(result) = json_view::search_with(&value, &q, &stale) {
                    let _ = tx.send(result); // the level may be gone; a closed channel is fine
                }
            });
            level.search.pending = Some((query.clone(), rx));
        } else {
            level.search.result = Some(json_view::search(&level.value, &query));
        }
```

Restructure the empty branch so clippy doesn't flag it: `if !query.is_empty() { if big { .. } else { .. } }`. A cancelled worker drops `tx`, and `receive_json_search` already clears `pending` on `Disconnected`.

In `json_level_ui.rs` status, add an arm before the plural one:

```rust
            (None, Some(result)) if result.truncated => Some(format!("First {} matches", result.matches.len())),
```

- [ ] **Step 4: Implement the shared value.** In `node.rs`, set `pub(crate) value: std::sync::Arc<serde_json::Value>,` with doc "Shared with an open `JsonLevel` (entering the node doesn't copy it)."
  - `JsonLevel::new` takes `value: Arc<serde_json::Value>` and stores it directly (`value,` instead of `Arc::new(value)`). Line 84 becomes `default_expanded(&value)`, which derefs.
  - The call at 209 is `JsonLevel::new(id, json.value.clone(), &json.view)`, now an `Arc` clone.
  - Fix each compiler error: a `Value::Null` construction becomes `Default::default()` (`Arc<Value>: Default`), and a loaded value is wrapped with `Arc::new(..)`.
  - A `&json.value` passed where `&Value` is expected derefs automatically. A `match`/pattern on it needs `&*json.value`.
  - The session_sync assertion at 1937 compares `(&json.value, ..)` to `(&value, ..)`. Wrap `value` in `Arc` there or compare `*json.value`.

- [ ] **Step 5: Run** the tests and `cargo test -j 8 -p manifold --bins json`, then the full gate. Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/manifold/src/json_view.rs crates/manifold/src/json_level.rs crates/manifold/src/json_level_ui.rs crates/manifold/src/node.rs crates/manifold/src/fetched.rs crates/manifold/src/session_sync.rs
git commit -m "JSON: a new search hides the old result, cancels the old worker and caps at 10k matches; levels share the parsed value

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 18: Engine input: emoji in CEF, typing into iframes, double-click, modifiers and middle button

**Findings:**
- **E-B4 (med).** `manifold-engine-cef/src/input.rs:87-103` sends `Char` as `character: codepoint as u16`. Typing U+1F600 inserts U+F600.
- **E-B6 (med).** `manifold-engine-webview2/src/input.rs:243-245` types through `ExecuteScript("document.execCommand('insertText', ...)")`. That runs in the top document, so focus inside an iframe gets nothing. It is also a different channel from the CDP key events, so a character and a following Backspace can reorder.
- **E-B7 (med).** `InputEvent::MouseDown`/`MouseUp` (`messages.rs:192-193`) carry no click count, so double-click selection and `dblclick` never work in a tab:
  - CEF sends `click_count` 1 (`input.rs:62-69`).
  - WebView2 never sends `LEFT_BUTTON_DOUBLE_CLICK`.
- **E-R8 (low).**
  - WebView2 `input.rs:126-208` ignores `modifiers` and drops `Middle`.
  - WebKit `input.rs:66-98` passes `ModifierType::empty()`.
  - So Ctrl+click, middle-click and Shift+click don't reach pages on those two engines.
  - WebKit's right-click `elementFromPoint({x},{y})` (49-51) ignores page zoom.
- **E-F4, the zoom half.**
  - `TabState.zoom` (`messages.rs:12-15`) is ignored by the host (`event_loop.rs:1558`).
  - CEF and Servo always send 1.0; WebView2 also sends 1.0 (`webview2 main.rs:323`).
  - Remove the field: this task already changes the protocol.

**Files:**
- Modify: `crates/ipc-protocol/src/messages.rs`:
  - `MouseDown`/`MouseUp` gain `click_count: u8`;
  - `TabState` drops `zoom`;
  - the `InputEvent` doc no longer says only CEF reads modifiers.
- Modify: `crates/ipc-protocol/src/lib.rs`: `PROTOCOL_VERSION = 3`.
- Create: `crates/manifold/src/click_count.rs`, the host `ClickCounter`.
- Modify: `crates/manifold/src/main.rs`: `mod click_count;` and the `App.click_counter` field plus Default.
- Modify: `crates/manifold/src/event_loop.rs`: around 349-352, fill `click_count`.
- Modify: `crates/manifold-engine-cef/src/input.rs`: `utf16_units`, `click_count`.
- Modify: `crates/manifold-engine-webview2/src/input.rs`:
  - `insert_text` through CDP;
  - `mouse_keys`;
  - `down_kind`;
  - middle button.
- Modify: `crates/manifold-engine-webview2/src/main.rs:323`, `crates/manifold-engine-cef/src/main.rs:307-310`, `crates/manifold-engine-servo/src/main.rs:288-291` and `crates/manifold-engine-webkit/src/main.rs:337`: `TabState { url }`.
- Modify: `crates/manifold-engine-webkit/src/input.rs`: modifiers, a double press, and the zoom divisor.
- Modify: `crates/manifold-engine-servo/src/main.rs:379-384`: patterns use `..` (Servo counts clicks itself).

**Interfaces:**
- Consumes: Task 4's handshake and Task 5's `PROTOCOL_VERSION = 2`.
- Produces:
  - `InputEvent::MouseDown { x, y, button, modifiers, click_count: u8 }`, with `MouseUp` the same;
  - `TabState { url: String }`;
  - `click_count::ClickCounter::{press(&mut self, button, pos, now) -> u8, current(&self) -> u8}`;
  - `PROTOCOL_VERSION = 3`.

- [ ] **Step 1: Write the failing tests.**

Create `crates/manifold/src/click_count.rs`:

```rust
//! Click counting for page input: engines need a press's click count
//! (double-click selects a word, fires `dblclick`), and the window system
//! only delivers single presses.

use std::time::{Duration, Instant};

use ipc_protocol::MouseButton;

/// Windows' default double-click time.
pub(crate) const MULTI_CLICK_TIME: Duration = Duration::from_millis(500);
/// How far (physical px) a press may land from the last and still count.
pub(crate) const MULTI_CLICK_SLOP: f64 = 4.0;

#[derive(Debug, Default)]
pub(crate) struct ClickCounter {
    last: Option<(MouseButton, (f64, f64), Instant)>,
    count: u8,
}

impl ClickCounter {
    /// Records a press and returns its click count: 1, or one more than the
    /// last press when it was the same button, close by and soon after.
    pub(crate) fn press(&mut self, button: MouseButton, pos: (f64, f64), now: Instant) -> u8 {
        let continues = self.last.is_some_and(|(b, p, t)| {
            std::mem::discriminant(&b) == std::mem::discriminant(&button)
                && now.duration_since(t) <= MULTI_CLICK_TIME
                && (p.0 - pos.0).abs() <= MULTI_CLICK_SLOP
                && (p.1 - pos.1).abs() <= MULTI_CLICK_SLOP
        });
        self.count = if continues { self.count.saturating_add(1) } else { 1 };
        self.last = Some((button, pos, now));
        self.count
    }

    /// The count of the last press, for its release.
    pub(crate) fn current(&self) -> u8 {
        self.count.max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quick_presses_in_one_place_count_up() {
        let mut clicks = ClickCounter::default();
        let t = Instant::now();
        assert_eq!(clicks.press(MouseButton::Left, (10.0, 10.0), t), 1);
        assert_eq!(clicks.press(MouseButton::Left, (12.0, 9.0), t + Duration::from_millis(200)), 2);
        assert_eq!(clicks.press(MouseButton::Left, (12.0, 9.0), t + Duration::from_millis(400)), 3);
        assert_eq!(clicks.current(), 3);
    }

    #[test]
    fn slow_far_or_other_button_presses_start_over() {
        let mut clicks = ClickCounter::default();
        let t = Instant::now();
        clicks.press(MouseButton::Left, (0.0, 0.0), t);
        assert_eq!(clicks.press(MouseButton::Left, (0.0, 0.0), t + Duration::from_millis(600)), 1, "slow");
        assert_eq!(clicks.press(MouseButton::Left, (20.0, 0.0), t + Duration::from_millis(700)), 1, "far");
        assert_eq!(clicks.press(MouseButton::Right, (20.0, 0.0), t + Duration::from_millis(800)), 1, "other button");
    }
}
```

If `ipc_protocol::MouseButton` derives `PartialEq`, compare with `==` instead of `discriminant`. It currently derives `Debug, Clone, Copy, Serialize, Deserialize`; adding `PartialEq, Eq` there is fine and simpler, and is part of this task's protocol edit.

In `manifold-engine-cef/src/input.rs`, add a test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn astral_characters_become_surrogate_pairs() {
        assert_eq!(utf16_units(0x1F600), vec![0xD83D, 0xDE00]);
        assert_eq!(utf16_units('a' as u32), vec![0x61]);
        assert!(utf16_units(0xD800).is_empty(), "a lone surrogate isn't a character");
    }
}
```

In `manifold-engine-webview2/src/input.rs`, add a test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mouse_keys_carry_modifiers_and_the_held_button() {
        let keys = mouse_keys(Some(MouseButton::Left), modifiers::CONTROL | modifiers::SHIFT);
        assert!(keys.contains(COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_CONTROL));
        assert!(keys.contains(COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_SHIFT));
        assert!(keys.contains(COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_LEFT_BUTTON));
        assert_eq!(mouse_keys(None, 0), COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_NONE);
    }

    #[test]
    fn a_second_left_press_is_a_double_click() {
        assert_eq!(down_kind(MouseButton::Left, 1), COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_DOWN);
        assert_eq!(down_kind(MouseButton::Left, 2), COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_DOUBLE_CLICK);
        assert_eq!(down_kind(MouseButton::Middle, 1), COREWEBVIEW2_MOUSE_EVENT_KIND_MIDDLE_BUTTON_DOWN);
    }
}
```

`COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS` is `pub struct (pub i32)` with `contains` (webview2-com-sys 0.39.1 `bindings.rs:422`). Check that it derives `PartialEq` and `Debug` for `assert_eq!`; it does for the `windows`-style newtypes. If it doesn't, compare `.0`.

- [ ] **Step 2: Run the tests.** Expected: compile failures.
  - `cargo test -j 8 -p manifold --bins click_count`
  - `cargo test -j 8 -p manifold-engine-webview2 --bins mouse_keys`
  - `cargo test -j 8 -p manifold-engine-cef --bins astral`. CEF tests build the CEF crate. If `cef-dll-sys` can't build in this checkout, say so and rely on `cargo check -j 8 -p manifold-engine-cef` plus reading. The helper is three lines of std.

- [ ] **Step 3: Change the protocol.**
  - In `messages.rs`, add `click_count: u8` to `MouseDown` and `MouseUp`, documented "1 for a single press, 2 for the second of a double click, and so on (`manifold`'s `ClickCounter`)".
  - Remove `zoom` from `TabState`; its doc keeps the URL-only rationale.
  - Rewrite the `InputEvent` modifiers doc: "the modifier keys held at the time of this mouse event; every engine forwards them".
  - Derive `PartialEq, Eq` on `MouseButton`.
  - In `lib.rs`, set `PROTOCOL_VERSION = 3`.
  - Update `channel.rs`/`messages.rs` round-trip tests that build `MouseDown`/`TabState` (grep `MouseDown {` and `TabState {` under `crates/ipc-protocol`).

- [ ] **Step 4: Host side.** In `main.rs`, add `mod click_count;` and the field `click_counter: click_count::ClickCounter` (Default `Default::default()`), with doc "Click counts for page presses (`InputEvent::MouseDown::click_count`)". In `event_loop.rs` ~347:

```rust
                            let event = match state {
                                ElementState::Pressed => {
                                    let click_count = self.click_counter.press(button, (x, y), std::time::Instant::now());
                                    InputEvent::MouseDown { x, y, button, modifiers: self.key_modifiers, click_count }
                                }
                                ElementState::Released => InputEvent::MouseUp {
                                    x,
                                    y,
                                    button,
                                    modifiers: self.key_modifiers,
                                    click_count: self.click_counter.current(),
                                },
                            };
```

`debug_click(.., 2)` already sends two press/release pairs back to back, so `DOUBLECLICK` now yields count 2 with no change there. In `event_loop.rs` `StateSnapshot`, drop any read of `state.zoom`; per the finding, there is none.

- [ ] **Step 5: CEF.**

```rust
/// `codepoint` as UTF-16 code units: Windows delivers a character outside
/// the BMP as two WM_CHARs (a surrogate pair), and CEF's KEYEVENT_CHAR takes
/// one unit each. Empty for a value that isn't a character.
fn utf16_units(codepoint: u32) -> Vec<u16> {
    char::from_u32(codepoint).map(|c| c.encode_utf16(&mut [0; 2]).to_vec()).unwrap_or_default()
}
```

The `Char` arm sends one event per unit:

```rust
        InputEvent::Char { codepoint } => {
            for unit in utf16_units(codepoint) {
                let e = KeyEvent {
                    size: std::mem::size_of::<KeyEvent>(),
                    type_: KeyEventType::CHAR,
                    modifiers: 0,
                    windows_key_code: unit as i32,
                    native_key_code: 0,
                    is_system_key: 0,
                    character: unit,
                    unmodified_character: unit,
                    focus_on_editable_field: 0,
                };
                host.send_key_event(Some(&e));
            }
        }
```

The mouse arms pass the count: `send_mouse_click_event(Some(&mouse_event), mouse_button_to_cef(button), 0, click_count as i32)`, and the same with `1` for up. Destructure `click_count` in both patterns. In `main.rs:307`, use `TabState { url: nav_state.borrow().url.clone() }`.

- [ ] **Step 6: WebView2.**

```rust
/// The VIRTUAL_KEYS for a mouse event: the held modifiers, plus `held`'s button flag.
fn mouse_keys(held: Option<MouseButton>, mods: u8) -> COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS {
    let mut bits = COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_NONE.0;
    if mods & modifiers::CONTROL != 0 {
        bits |= COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_CONTROL.0;
    }
    if mods & modifiers::SHIFT != 0 {
        bits |= COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_SHIFT.0;
    }
    bits |= match held {
        Some(MouseButton::Left) => COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_LEFT_BUTTON.0,
        Some(MouseButton::Right) => COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_RIGHT_BUTTON.0,
        Some(MouseButton::Middle) => COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_MIDDLE_BUTTON.0,
        None => 0,
    };
    COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS(bits)
}

/// The press kind: Windows sends the second press of a double click as
/// `*_DOUBLE_CLICK` instead of `*_DOWN`, which is what makes the page see a
/// `dblclick` and select a word.
fn down_kind(button: MouseButton, click_count: u8) -> COREWEBVIEW2_MOUSE_EVENT_KIND {
    match (button, click_count >= 2) {
        (MouseButton::Left, false) => COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_DOWN,
        (MouseButton::Left, true) => COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_DOUBLE_CLICK,
        (MouseButton::Right, false) => COREWEBVIEW2_MOUSE_EVENT_KIND_RIGHT_BUTTON_DOWN,
        (MouseButton::Right, true) => COREWEBVIEW2_MOUSE_EVENT_KIND_RIGHT_BUTTON_DOUBLE_CLICK,
        (MouseButton::Middle, false) => COREWEBVIEW2_MOUSE_EVENT_KIND_MIDDLE_BUTTON_DOWN,
        (MouseButton::Middle, true) => COREWEBVIEW2_MOUSE_EVENT_KIND_MIDDLE_BUTTON_DOUBLE_CLICK,
    }
}
```

Rewrite the mouse arms:
- **`MouseDown { x, y, button, modifiers: mods, click_count }`:** `SendMouseInput(down_kind(button, click_count), mouse_keys(Some(button), mods), 0, point)`, keeping `drag_state.set(true)` for Left.
- **`MouseMove { x, y, modifiers: mods }`:** `mouse_keys(drag_state.get().then_some(MouseButton::Left), mods)`.
- **`MouseUp`:** `mouse_keys(None, mods)` for Right (keeping the href script) and for Left/Middle. Middle's up kind is `COREWEBVIEW2_MOUSE_EVENT_KIND_MIDDLE_BUTTON_UP`.
- **`Scroll`:** `mouse_keys(None, mods)`, so Ctrl+wheel reaches the page.

Import the new constants from the same module the file already imports the others from.

Text through CDP:

```rust
        // CDP `Input.insertText` types into the focused frame (an iframe's
        // field too, which `execCommand` in the top document never reached)
        // and shares the key events' channel, so the order is kept.
        InputEvent::Char { codepoint } => {
            if let Some(c) = char::from_u32(codepoint) {
                call_cdp(webview, "Input.insertText", serde_json::json!({ "text": c.to_string() }));
            }
        }
```

Factor the CDP call out of `dispatch_key_event`:

```rust
fn call_cdp(webview: &ICoreWebView2, method: &str, params: serde_json::Value) {
    let handler = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(|_error, _result| Ok(())));
    unsafe {
        let _ = webview.CallDevToolsProtocolMethod(&HSTRING::from(method), &HSTRING::from(params.to_string()), &handler);
    }
}
```

`dispatch_key_event` then ends with `call_cdp(webview, "Input.dispatchKeyEvent", params_json)`, with `params` kept as a `Value`. Remove the "Confirmed live" execCommand comment; the new comment replaces it. If `exec` is now unused, delete it, after grepping `exec(`.

In `main.rs:323`, use `TabState { url }`.

- [ ] **Step 7: WebKit** (Linux; check in WSL). In the three mouse arms, pass `modifiers_to_gdk(modifiers)` instead of `gdk::ModifierType::empty()`, destructuring `modifiers` (and `click_count` in `MouseDown`). After the `ButtonPress` for `click_count == 2`, also send the same event with `gdk::EventType::DoubleButtonPress`: GDK3 itself follows the second press with a `2BUTTON_PRESS`, and that is what WebKitGTK counts.

For the right-click hit test, divide by the page zoom:

```rust
        let zoom = webview.zoom_level().max(0.01);
        // InputEvent coordinates are surface pixels; elementFromPoint takes CSS pixels, which page zoom scales.
        webview.run_javascript(
            &format!("(function(){{var e=document.elementFromPoint({x}/{zoom},{y}/{zoom});var a=e&&e.closest('a[href]');return a?a.href:null;}})();"),
            ...
```

In `main.rs:337`, use `TabState { url }`.

- [ ] **Step 8: Servo.** In `main.rs:379-384`, the patterns already end in `..`, so `click_count` is ignored: Servo counts clicks itself. In `main.rs:288-291`, use `TabState { url: ... }`.

- [ ] **Step 9: Verify.**
  - `cargo test -j 8 -p ipc-protocol`
  - `cargo test -j 8 -p manifold --bins`
  - `cargo test -j 8 -p manifold-engine-webview2 --bins`
  - `cargo clippy -j 8 -p manifold-engine-webview2 --all-targets -- -D warnings`
  - `cargo check -j 8 -p manifold-engine-cef`
  - WebKit: `wsl.exe -d Ubuntu -- bash -c 'cd "/mnt/c/Users/Winston Li/Documents/GitHub/manifold" && CARGO_TARGET_DIR=target-linux cargo check -j 8 -p manifold-engine-webkit'`. If WSL is unavailable, WebKit is verified by reading only; say so.
  - Servo doesn't build on Windows and is verified by reading.
  - Then run the full gate.
  
  Engine hosts must be rebuilt alongside manifold, because the version bump makes old ones fail fast.

- [ ] **Step 10: Live check (Windows).** Rebuild manifold and the CEF and WebView2 hosts (`cargo build -j 8 -p manifold -p manifold-engine-webview2 -p manifold-engine-cef`), then launch at BelowNormal. In a WebView2 tab and a CEF tab, check:
  - double-clicking a word selects it;
  - typing an emoji in a text field (Win+.) inserts it;
  - Ctrl+clicking a link opens a new tab.
  
  For WebView2, also check typing into an iframe form, for example a page with an embedded form. Kill only your PIDs.

- [ ] **Step 11: Commit**

```bash
git add crates/ipc-protocol crates/manifold/src/click_count.rs crates/manifold/src/main.rs crates/manifold/src/event_loop.rs crates/manifold-engine-cef/src crates/manifold-engine-webview2/src crates/manifold-engine-webkit/src crates/manifold-engine-servo/src
git commit -m "Input: click counts in the protocol (double-click works), CEF types astral characters, WebView2 types via CDP into iframes and forwards modifiers and the middle button, WebKit modifiers and zoomed hit test; drop TabState.zoom; protocol v3

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 19: Engine downloads: one naming rule, matching paths, coalesced progress

**Findings:**
- **E-B5 (med).** WebKit (`manifold-engine-webkit/src/main.rs:111-161`) calls `set_destination(&format!("file://{}", dest.display()))` without escaping. The problems:
  - `#` in a name fails.
  - `Started` reports the raw path, but `Progress`, `Failed` and `Completed` report `destination().strip_prefix("file://")`, the URI form. Any escaped character gives a different `path`, and the host keys downloads by path (Task 9), so the card never completes.
  - `allow-overwrite` is false, so a second download of the same file fails.
- **CEF:**
  - `download_handler.rs:82-92` joins `Downloads/<suggested>` without making the name unique. Concurrent same-name downloads share one `path`.
  - `downloads_dir()` (43-45) uses `USERPROFILE` only, which is `""` on Linux.
- **E-R9 (low).** CEF (`download_handler.rs:126-130`), WebKit (`main.rs:132-138`) and WebView2 (`download.rs:85-96`) push a `DownloadProgress` per callback, which can be hundreds per tick.

**Files:**
- Create: `crates/ipc-protocol/src/downloads.rs` (`downloads_dir`, `safe_name`, `unique_path`, `coalesce_progress`, tests)
- Modify: `crates/ipc-protocol/src/lib.rs` (`pub mod downloads;`)
- Modify: `crates/manifold-engine-cef/src/download_handler.rs` (use the helpers, a reserved set) and `main.rs:489-501` (coalesce)
- Modify: `crates/manifold-engine-webkit/src/main.rs:106-161, ~495-505`
- Modify: `crates/manifold-engine-webview2/src/main.rs` (~470-485, coalesce)

**Interfaces:**
- Produces:
  - `ipc_protocol::downloads::downloads_dir() -> PathBuf`
  - `ipc_protocol::downloads::safe_name(suggested: &str) -> String`
  - `ipc_protocol::downloads::unique_path(dir: &Path, name: &str, taken: impl Fn(&Path) -> bool) -> PathBuf`
  - `ipc_protocol::downloads::coalesce_progress(msgs: Vec<EngineToHost>) -> Vec<EngineToHost>`

- [ ] **Step 1: Write the failing tests** in the new `crates/ipc-protocol/src/downloads.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn a_suggested_name_keeps_only_its_last_component() {
        assert_eq!(safe_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_name(""), "download");
        assert_eq!(safe_name("report #3.pdf"), "report #3.pdf");
    }

    #[test]
    fn a_taken_name_gets_a_number_before_its_extension() {
        let dir = Path::new("d");
        let taken = |p: &Path| p == dir.join("a.pdf") || p == dir.join("a (1).pdf");
        assert_eq!(unique_path(dir, "a.pdf", taken), dir.join("a (2).pdf"));
        assert_eq!(unique_path(dir, "b.pdf", taken), dir.join("b.pdf"));
        assert_eq!(unique_path(dir, "a", |p: &Path| p == dir.join("a")), dir.join("a (1)"));
    }

    #[test]
    fn progress_is_coalesced_to_the_latest_per_path_in_order() {
        use crate::EngineToHost::*;
        let path = |p: &str| p.to_string();
        let msgs = vec![
            DownloadStarted { path: path("a"), total_bytes: None, url: None },
            DownloadProgress { path: path("a"), bytes_received: 1 },
            DownloadProgress { path: path("b"), bytes_received: 5 },
            DownloadProgress { path: path("a"), bytes_received: 2 },
            DownloadCompleted { path: path("b") },
        ];
        let out = coalesce_progress(msgs);
        let shown: Vec<String> = out.iter().map(|m| format!("{m:?}")).collect();
        assert_eq!(shown.len(), 4, "{shown:?}");
        assert!(shown[1].contains("\"b\"") && shown[1].contains('5'), "b's progress before its completion");
        assert!(shown[3].contains("\"a\"") && shown[3].contains('2'), "a's latest, at its last place");
    }
}
```

- [ ] **Step 2: Run** `cargo test -j 8 -p ipc-protocol downloads`. Expected: compile failure.

- [ ] **Step 3: Implement** in `downloads.rs`:

```rust
//! Where engines put downloads and how they name them: one rule for every
//! engine, so a download's `path` (the host's key for it) is the same string
//! in every message about it.

use std::path::{Path, PathBuf};

use crate::EngineToHost;

/// `~/Downloads`: `USERPROFILE` on Windows, `HOME` elsewhere (CEF on Linux
/// used `USERPROFILE` and so saved into its working directory).
pub fn downloads_dir() -> PathBuf {
    let home = if cfg!(windows) { std::env::var_os("USERPROFILE") } else { std::env::var_os("HOME") };
    home.map(|h| PathBuf::from(h).join("Downloads")).unwrap_or_default()
}

/// A server-suggested file name reduced to its last component, so `..` or
/// an absolute name can't leave the downloads folder; "download" if empty.
pub fn safe_name(suggested: &str) -> String {
    Path::new(suggested)
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("download")
        .to_string()
}

/// `dir/name`, or `dir/stem (n).ext` with the first n that isn't `taken`
/// (an existing file, or one another download in flight has reserved).
pub fn unique_path(dir: &Path, name: &str, taken: impl Fn(&Path) -> bool) -> PathBuf {
    let first = dir.join(name);
    if !taken(&first) {
        return first;
    }
    let as_path = Path::new(name);
    let stem = as_path.file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    let ext = as_path.extension().and_then(|e| e.to_str());
    (1..)
        .map(|n| dir.join(match ext {
            Some(ext) => format!("{stem} ({n}).{ext}"),
            None => format!("{stem} ({n})"),
        }))
        .find(|p| !taken(p))
        .expect("an unbounded range finds a free name")
}

/// One tick's engine messages with each download's progress reduced to the
/// latest, kept at its last position; every other message keeps its order.
/// Fast downloads report per network chunk, hundreds of times a tick.
pub fn coalesce_progress(msgs: Vec<EngineToHost>) -> Vec<EngineToHost> {
    let last_progress: std::collections::HashMap<String, usize> = msgs
        .iter()
        .enumerate()
        .filter_map(|(i, m)| match m {
            EngineToHost::DownloadProgress { path, .. } => Some((path.clone(), i)),
            _ => None,
        })
        .collect();
    msgs.into_iter()
        .enumerate()
        .filter(|(i, m)| match m {
            EngineToHost::DownloadProgress { path, .. } => last_progress.get(path) == Some(i),
            _ => true,
        })
        .map(|(_, m)| m)
        .collect()
}
```

Add `pub mod downloads;` to `lib.rs`. Check that `EngineToHost` derives `Debug`, which the test uses; it does, since engines log it. If it doesn't, compare by matching.

- [ ] **Step 4: Engines.**

**CEF `download_handler.rs`:**
- Delete the local `downloads_dir`.
- Add `reserved: std::collections::HashSet<std::path::PathBuf>` to the shared results struct (beside `started`/`reported`), with doc "Paths handed out and not yet finished: concurrent same-name downloads get distinct names before either file exists".
- `on_before_download`:

```rust
                let dir = ipc_protocol::downloads::downloads_dir();
                let name = ipc_protocol::downloads::safe_name(&suggested_name.map(|s| s.to_string()).unwrap_or_default());
                let mut results = self.handler.result.borrow_mut();
                let path = ipc_protocol::downloads::unique_path(&dir, &name, |p| p.exists() || results.reserved.contains(p));
                results.reserved.insert(path.clone());
                drop(results);
                callback.cont(Some(&path.to_string_lossy().as_ref().into()), false as _);
```

- In `on_download_updated`, when complete, failed or cancelled is reported, call `results.reserved.remove(std::path::Path::new(&path))`.
- In `main.rs:489-501`, collect the mapped messages into a `Vec`, then `for msg in ipc_protocol::downloads::coalesce_progress(msgs) { let _ = control.send(&msg); }`.

**WebKit `main.rs`:**
- In `decide-destination`:

```rust
                let dir = ipc_protocol::downloads::downloads_dir();
                let _ = std::fs::create_dir_all(&dir);
                let dest = ipc_protocol::downloads::unique_path(
                    &dir,
                    &ipc_protocol::downloads::safe_name(suggested_filename),
                    |p| p.exists(),
                );
                let Ok(uri) = glib::filename_to_uri(&dest, None) else { return false };
                download.set_destination(&uri);
                let path = dest.to_string_lossy().to_string();
                *dest_for_events.borrow_mut() = Some(path.clone());
```

- Each `connect_download_started` gets `let dest_for_events: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));`, cloned into the four handlers.
- `received-data`, `failed` and `finished` read `dest_for_events.borrow().clone()` instead of `destination().strip_prefix(..)`. That way every message carries the string `Started` sent.
- Update the comment block above (98-105), which says paths come back from `destination()`.
- Use the crate's `glib` re-export, `gtk::glib`, as the file already does for `gio`.
- Coalesce at the drain (~495-505) as for CEF.

**WebView2 `main.rs` (~470-485):** collect the mapped messages and send `coalesce_progress(msgs)`. WebView2's naming is Edge's own (it already uniquifies and reports one path), so it is unchanged.

- [ ] **Step 5: Verify.**
  - `cargo test -j 8 -p ipc-protocol`
  - `cargo check -j 8 -p manifold-engine-cef`
  - `cargo clippy -j 8 -p manifold-engine-webview2 --all-targets -- -D warnings`
  - WebKit in WSL: `CARGO_TARGET_DIR=target-linux cargo check -j 8 -p manifold-engine-webkit` (as in Task 18), or reading only if WSL is unavailable.
  - Then run the full gate.

- [ ] **Step 6: Live check.** Rebuild the CEF host, launch at BelowNormal, and download the same file twice in a CEF tab. Expect two cards, `name.ext` and `name (1).ext`, both completing. Kill only your PID.

- [ ] **Step 7: Commit**

```bash
git add crates/ipc-protocol/src crates/manifold-engine-cef/src crates/manifold-engine-webkit/src crates/manifold-engine-webview2/src
git commit -m "Downloads: shared naming (safe, unique, HOME on Linux); WebKit escapes the URI and reports one path; progress coalesced per tick

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Phase C: robustness and performance

### Task 20: Session store and jar: catch-up reads only the tail, CRC without copies, a bounded lock wait, an indexed cookie merge

**Findings:**
- **A-Rb2 (med, perf).** `store/journal.rs` `catch_up` (~972) does `std::fs::read(&self.path)` on every `begin`, so every commit reads the whole session file (snapshot included) on the UI thread, under the lock other instances wait on. The incremental path needs only the header (to compare generations) and the bytes from `self.end` on.
- **A-Rb3 (low-med, perf).**
  - `frame_crc` (~121-126) allocates and copies `4 + body` bytes for every frame read or written, only to checksum them.
  - `resync` (~206-213) falls back to `frame_at` at every byte offset of a damaged region, so each offset whose 4-byte length fits copies and checksums that many bytes.
  - Fix: an incremental `crc32c_append`, so no copy; in `resync`, skip a candidate whose body starts with a zero byte (its frame type would be 0, which `frame_at` refuses anyway) before checksumming. The worst case (a long run of small plausible lengths) stays O(n·len); this removes the copies and most candidates, not the bound.
- **A-R3 (low).** `decode_stamp` (~372-382) runs `decode_hlc` and then loops over the same fields again for field 3. One loop.
- **A-Rb4 (low).** `lock_file` (~1240-1247) calls `file.lock()`, which waits for ever. It is reached from `begin`, `close`, `read_local`, `write_local` and `save_copy` on the UI thread, so another instance stalled while holding the lock freezes this one. Use `try_lock` in a retry loop with a deadline, then an error the app already toasts.
- **A-R2 (low-med, perf).**
  - `jar.rs` `Jar::merge` (~430-455) calls `key()` (three `String` allocations) inside `retain`/`position` for every pulled cookie: about 3·P·J allocations per pull. A long-lived default jar holds thousands.
  - `same_as` (~98-104) clones the whole cookie to compare it.
  - Fix: build a key→index map of the jar once per merge, and compare field by field.

**Files:**
- Modify: `crates/manifold/src/store/crc32c.rs`: add `crc32c_append`; `table` and `sse42` take a starting CRC.
- Modify: `crates/manifold/src/store/journal.rs`: `frame_crc`, `resync`, `decode_stamp`, `catch_up`, `lock_file`; new private `read_tail`, `lock_file_within` and `LOCK_WAIT`; tests.
- Modify: `crates/manifold/src/jar.rs`: `Cookie::same_as`, `Jar::merge`; tests.

**Interfaces:**
- Produces:
  - `store::crc32c::crc32c_append(crc: u32, data: &[u8]) -> u32`, with `crc32c(d) == crc32c_append(0, d)`.
  - `journal::read_tail(path: &Path, from: u64) -> Result<(Header, u64, Vec<u8>), JournalError>` (private).
  - `journal::lock_file_within(path: &Path, wait: Duration) -> Result<File, JournalError>` (private); `lock_file(path)` is `lock_file_within(path, LOCK_WAIT)` with `const LOCK_WAIT: Duration = Duration::from_secs(5)`.
  - Unchanged public signatures: `Jar::merge(&mut self, pulled: Vec<Cookie>, tab: &mut TabCookies) -> bool`.

- [ ] **Step 1: Write the failing tests.**

In `store/crc32c.rs` tests:

```rust
    #[test]
    fn appending_equals_one_pass() {
        let mut rng = XorShift(7);
        for len in 0..200 {
            let data: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
            let split = if len == 0 { 0 } else { rng.below(len as u64 + 1) as usize };
            let (a, b) = data.split_at(split);
            assert_eq!(crc32c_append(crc32c(a), b), crc32c(&data), "len {len} split {split}");
            assert_eq!(crc32c_append(0, &data), crc32c(&data));
        }
    }
```

In `store/journal.rs` tests (beside `two_replicas_on_one_file_merge_each_others_ops`):

```rust
    #[test]
    fn the_tail_read_skips_everything_before_end() {
        let dir = temp_dir("tail");
        let path = dir.join("session.mnf");
        let j = Journal::open(&path, ActorId(1)).unwrap();
        let len = std::fs::metadata(&path).unwrap().len();
        let (header, file_len, tail) = read_tail(&path, j.end).unwrap();
        assert_eq!(header.generation, j.generation);
        assert_eq!(file_len, len);
        assert_eq!(tail.len() as u64, len - j.end);
        let (_, _, past) = read_tail(&path, len + 10).unwrap();
        assert!(past.is_empty(), "a file shorter than `end` gives no tail; catch_up reloads it");
    }

    #[test]
    fn a_held_lock_times_out_with_a_busy_error() {
        let dir = temp_dir("lock_wait");
        let path = dir.join("session.mnf");
        let held = lock_file(&path).unwrap();
        let started = Instant::now();
        match lock_file_within(&path, Duration::from_millis(100)) {
            Err(JournalError::Io(e)) => {
                assert_eq!(e.kind(), io::ErrorKind::WouldBlock);
                assert!(e.to_string().contains("busy"), "{e}");
            }
            Err(other) => panic!("expected a busy error, got {other}"),
            Ok(_) => panic!("a second handle got a held lock"),
        }
        assert!(started.elapsed() < Duration::from_secs(2));
        drop(held);
        assert!(lock_file_within(&path, Duration::from_millis(100)).is_ok(), "free once released");
    }

    #[test]
    fn resync_finds_a_frame_after_noise() {
        let mut rng = XorShift(3);
        let mut buf: Vec<u8> = (0..4096).map(|_| rng.next() as u8).collect();
        let at = buf.len();
        encode_frame(&mut buf, 1, b"after the noise");
        assert_eq!(resync(&buf, 0), Some(at));
    }
```

`a_held_lock_times_out_with_a_busy_error` relies on a second handle to the same `.lock` file conflicting within one process. Windows `LockFileEx` locks belong to the handle, so it does. On Linux, `flock` locks belong to the open file description, and two `open`s are two descriptions, so it conflicts there too.

In `jar.rs` tests, a property test against the current algorithm, kept in the test module as the reference:

```rust
    /// `merge` as it was before the index (4b833af), the reference the indexed one must match.
    fn merge_reference(jar: &mut Vec<Cookie>, pulled: Vec<Cookie>, tab: &mut TabCookies) -> bool {
        let before = jar.clone();
        let t = now();
        let pulled_keys: HashSet<Key> = pulled.iter().map(Cookie::key).collect();
        for gone in tab.reported.difference(&pulled_keys) {
            jar.retain(|c| c.key() != *gone);
        }
        for p in &pulled {
            let k = p.key();
            if tab.baseline.get(&k).is_some_and(|b| b.same_as(p)) {
                continue;
            }
            match jar.iter_mut().position(|c| c.key() == k) {
                _ if p.expired(t) => jar.retain(|c| c.key() != k),
                Some(i) if jar[i].same_as(p) => {}
                Some(i) => jar[i] = p.clone(),
                None => jar.push(p.clone()),
            }
        }
        jar.retain(|c| !c.expired(t));
        let reported = std::mem::replace(&mut tab.reported, pulled_keys);
        tab.baseline.retain(|k, _| !reported.contains(k));
        tab.baseline.extend(pulled.into_iter().map(|p| (p.key(), p)));
        *jar != before
    }

    #[test]
    fn the_indexed_merge_matches_the_reference() {
        let mut rng = crate::store::testutil::XorShift(11);
        let cookie = |rng: &mut crate::store::testutil::XorShift| {
            let mut x = c(&format!("n{}", rng.below(12)), &format!("v{}", rng.below(3)));
            x.expires = match rng.below(4) {
                0 => Some(1.0),
                1 => Some(4_000_000_000.0),
                _ => None,
            };
            x
        };
        let unique = |v: Vec<Cookie>| {
            let mut seen = HashSet::new();
            v.into_iter().filter(|x| seen.insert(x.key())).collect::<Vec<_>>()
        };
        for round in 0..500 {
            let start = unique((0..rng.below(10)).map(|_| cookie(&mut rng)).collect());
            let mut tab = TabCookies::seeded(&start);
            let (mut jar, mut reference) = (Jar { locked: false, cookies: start.clone() }, start.clone());
            let mut ref_tab = TabCookies::seeded(&start);
            for _ in 0..3 {
                let pulled = unique((0..rng.below(10)).map(|_| cookie(&mut rng)).collect());
                let got = jar.merge(pulled.clone(), &mut tab);
                let want = merge_reference(&mut reference, pulled, &mut ref_tab);
                assert_eq!((got, &jar.cookies), (want, &reference), "round {round}");
                assert_eq!(tab.reported, ref_tab.reported);
            }
        }
    }
```

Check `TabCookies::seeded`'s signature (`jar.rs` ~508) and adjust the call if it takes something other than `&[Cookie]`. The jar keeps one cookie per key: grep `cookies.push` and `cookies =` in `jar.rs` and `jar_defs.rs` to confirm nothing else adds a duplicate key. If something does, stop and report: the index assumes unique keys.

- [ ] **Step 2: Run them.** `cargo test -j 8 -p manifold --bins appending_equals_one_pass`, then `tail_read_skips`, `held_lock_times_out`, `resync_finds_a_frame`, `indexed_merge_matches`. Expected: compile errors for the missing functions; `resync_finds_a_frame_after_noise` and `the_indexed_merge_matches_the_reference` already pass (they pin today's behaviour).

- [ ] **Step 3: CRC.** In `crc32c.rs`:

```rust
pub fn crc32c(data: &[u8]) -> u32 {
    crc32c_append(0, data)
}

/// The CRC of `prefix ‖ data`, given `crc = crc32c(prefix)`: checksums a message in pieces
/// without copying them together.
pub fn crc32c_append(crc: u32, data: &[u8]) -> u32 {
    #[cfg(target_arch = "x86_64")]
    if std::arch::is_x86_feature_detected!("sse4.2") {
        // SAFETY: the CPU was just checked for SSE4.2.
        return unsafe { sse42(crc, data) };
    }
    table(crc, data)
}

fn table(crc: u32, data: &[u8]) -> u32 {
    let mut c = !crc;
    for &b in data {
        c = TABLE[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}
```

In `sse42`, take `crc: u32` first and start from `let mut c = u64::from(!crc);`. Update the existing `sse42_path_equals_table_path` test to call `sse42(0, &data)` and `table(0, &data)`.

In `journal.rs`, import `crc32c_append` beside `crc32c` and replace `frame_crc`:

```rust
/// CRC-32C over `len ‖ frame_type ‖ message`, so a zeroed region fails.
fn frame_crc(len: u32, body: &[u8]) -> u32 {
    crc32c_append(crc32c(&len.to_le_bytes()), body)
}
```

- [ ] **Step 4: `resync` prefilter and `decode_stamp`.**

```rust
fn resync(buf: &[u8], at: usize) -> Option<usize> {
    // A body starting with a zero byte has frame type 0, which `frame_at`
    // refuses: skipping it here saves the checksum, not a result.
    let valid = |p: usize| buf.get(p + 8).is_some_and(|&b| b != 0) && frame_at(&buf[p..]).is_ok();
    let claimed = buf.get(at..at + 4).map(|b| at + 8 + u32::from_le_bytes(b.try_into().expect("4 bytes")) as usize);
    if let Some(p) = claimed.filter(|&p| p > at + 8 && valid(p)) {
        return Some(p);
    }
    (at + 1..buf.len()).find(|&p| valid(p))
}
```

The old `p < buf.len()` check is implied: `buf.get(p + 8)` is `None` past the end.

```rust
fn decode_stamp(buf: &[u8], actors: &Actors) -> Result<Stamp, JournalError> {
    let (mut hlc, mut index) = (Hlc::default(), 0);
    for field in Reader::new(buf) {
        let field = field?;
        match field.no {
            1 => hlc.ms = field.value.u64()?,
            2 => hlc.logical = field.value.u32()?,
            3 => index = field.value.u64()?,
            _ => {}
        }
    }
    Ok(Stamp { hlc, actor: actors.get(index)? })
}
```

- [ ] **Step 5: `read_tail` and `catch_up`.** Add `Read` to the `std::io` import, then:

```rust
/// The most a header can take: magic (4), a varint version (at most 10),
/// two u64 generations and a CRC (4).
const HEADER_MAX: u64 = 4 + 10 + 16 + 4;

/// The header, the file's length and its bytes from `from` on: what an
/// incremental catch-up needs, without reading the snapshot before `from`.
/// An empty tail when the file is shorter than `from`.
fn read_tail(path: &Path, from: u64) -> Result<(Header, u64, Vec<u8>), JournalError> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let mut head = Vec::new();
    (&mut file).take(HEADER_MAX).read_to_end(&mut head)?;
    let header = read_header(&head)?;
    let mut tail = Vec::new();
    if from <= len {
        file.seek(SeekFrom::Start(from))?;
        file.read_to_end(&mut tail)?;
    }
    Ok((header, len, tail))
}
```

In `catch_up`, replace the first three lines and the incremental block's slicing. Keep everything else as it is:

```rust
    fn catch_up(&mut self) -> Result<Vec<NodeId>, JournalError> {
        let (header, len, tail) = read_tail(&self.path, self.end)?;
        let mut touched = Vec::new();
        let mut reload = self.stale || header.generation != self.generation || len < self.end;
        if !reload {
            let base = self.end as usize;
            let scan = scan(&tail, base);
            for f in &scan.frames {
                // (the existing loop body, unchanged)
            }
            // A whole frame failing its CRC, or a stop with a valid frame
            // after it, is damage, not a torn tail.
            reload |= scan.stop == Some(Stop::BadCrc) || (scan.stop.is_some() && resync(&tail, scan.end - base).is_some());
            if !reload {
                self.end = scan.end as u64;
                self.dirty |= !scan.frames.is_empty() || scan.stop.is_some();
            }
        }
        if reload {
            // Only now the whole file: another process compacted it, or damage turned up.
            let bytes = std::fs::read(&self.path)?;
            let before = std::mem::take(&mut self.doc.nodes);
            self.load(&bytes)?;
            self.repair(&bytes);
            // (the rest of the reload block, unchanged)
        }
```

`scan`'s `base` makes frame offsets absolute, as before. `scan.end` is absolute, so the offset into `tail` is `scan.end - base`. The lock is held across both reads, so no writer can change the file between them.

- [ ] **Step 6: `lock_file_within`.**

```rust
/// How long taking the session lock may wait for another instance before
/// the caller gets a "busy" error (and the app's usual save-failed toast)
/// instead of a frozen window.
const LOCK_WAIT: Duration = Duration::from_secs(5);

/// Takes the exclusive lock beside `path` (`<path>.lock`), creating it.
fn lock_file(path: &Path) -> Result<File, JournalError> {
    lock_file_within(path, LOCK_WAIT)
}

/// `lock_file`, giving up after `wait` with `WouldBlock`.
fn lock_file_within(path: &Path, wait: Duration) -> Result<File, JournalError> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let file = OpenOptions::new().create(true).truncate(false).write(true).open(with_suffix(path, ".lock"))?;
    let deadline = Instant::now() + wait;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(JournalError::Io(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "the session is busy in another window",
                )))
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
        }
    }
}
```

`File::try_lock` and `TryLockError` are stable since Rust 1.89, the same release as the `File::lock` this file already uses.

- [ ] **Step 7: Jar.** `same_as`, field by field. The destructuring makes a new `Cookie` field a compile error here instead of a silent miss:

```rust
    fn same_as(&self, other: &Cookie) -> bool {
        let Cookie { name, value, domain, path, expires, secure, http_only, same_site } = self;
        let expires_same = match (*expires, other.expires) {
            (Some(a), Some(b)) => (a - b).abs() < EXPIRES_TOLERANCE,
            (a, b) => a == b,
        };
        expires_same
            && *name == other.name
            && *value == other.value
            && *domain == other.domain
            && *path == other.path
            && *secure == other.secure
            && *http_only == other.http_only
            && *same_site == other.same_site
    }
```

`merge`, keeping the doc comment above it and the jar's order (replacements in place, new cookies at the end):

```rust
    pub fn merge(&mut self, pulled: Vec<Cookie>, tab: &mut TabCookies) -> bool {
        let t = now();
        let pulled_keys: HashSet<Key> = pulled.iter().map(Cookie::key).collect();
        // One slot per jar cookie; `None` is a deletion, compacted at the end.
        let mut slots: Vec<Option<Cookie>> = std::mem::take(&mut self.cookies).into_iter().map(Some).collect();
        let mut index: HashMap<Key, usize> =
            slots.iter().enumerate().filter_map(|(i, c)| Some((c.as_ref()?.key(), i))).collect();
        let mut changed = false;
        for gone in tab.reported.difference(&pulled_keys) {
            if let Some(i) = index.remove(gone) {
                slots[i] = None;
                changed = true;
            }
        }
        for p in &pulled {
            let k = p.key();
            if tab.baseline.get(&k).is_some_and(|b| b.same_as(p)) {
                continue;
            }
            match index.get(&k).copied() {
                _ if p.expired(t) => {
                    if let Some(i) = index.remove(&k) {
                        slots[i] = None;
                        changed = true;
                    }
                }
                Some(i) if slots[i].as_ref().is_some_and(|c| c.same_as(p)) => {}
                Some(i) => {
                    slots[i] = Some(p.clone());
                    changed = true;
                }
                None => {
                    index.insert(k, slots.len());
                    slots.push(Some(p.clone()));
                    changed = true;
                }
            }
        }
        self.cookies = slots
            .into_iter()
            .flatten()
            .filter(|c| {
                let keep = !c.expired(t);
                changed |= !keep;
                keep
            })
            .collect();
        // Seeded keys the tab has not reported yet stay in the baseline; reported keys now missing leave it.
        let reported = std::mem::replace(&mut tab.reported, pulled_keys);
        tab.baseline.retain(|k, _| !reported.contains(k));
        tab.baseline.extend(pulled.into_iter().map(|p| (p.key(), p)));
        changed
    }
```

Add `HashMap` to the `std::collections` import if it is not there. `changed` is true exactly when the old `self.cookies != before` was: every branch that sets it removes, replaces with a cookie that `same_as` says differs, or adds.

- [ ] **Step 8: Run** `cargo test -j 8 -p manifold --bins store::` and `cargo test -j 8 -p manifold --bins jar`. Expected: PASS, including every existing journal test (`two_processes_writing_one_file_lose_nothing` and `a_writer_killed_mid_commit_loses_at_most_that_commit` exercise `catch_up` across processes).

- [ ] **Step 9: Timing check.** Run `cargo test -j 8 -p manifold --bins save_path_costs -- --ignored --nocapture` (find its exact attribute first: `grep -n "fn save_path_costs" -B3 crates/manifold/src/store/journal.rs`). Note the "begin" figures before (`git stash`) and after. Report them with the CPU load and Defender state (Global Constraints / LONG_RUNS.md). If the test is not `#[ignore]`d, run it by name without `--ignored`.

- [ ] **Step 10: Full gate, then commit.**

```bash
git add crates/manifold/src/store/crc32c.rs crates/manifold/src/store/journal.rs crates/manifold/src/jar.rs
git commit -m "Store: catch-up reads only the header and tail; CRC without copies; the lock wait is bounded; the cookie merge is indexed

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 21: Profile paths: one parse, a wider guard, the screenshot folder; conflict names, tab-history caps, test temp cleanup

**Findings:**
- **A-R4 (low).** `profile.rs` `dir()` (17-23) collects `std::env::args()` on every call. It is reached through `jars_dir()`, `content_dir()` and the rest, some per UI action. Compute it once in a `OnceLock`, and accept `--profile-dir=<dir>` as well.
- **A-Rb8 (low, test gap).**
  - The guard test `app_default_touches_no_profile_file` (`profile.rs` 98-120) scans for `"\"APPDATA\""` and `etcetera` only. It misses `"LOCALAPPDATA"`, `USERPROFILE` (already used at `main.rs:369` for screenshots), `HOME`, `dirs::` and `known_folder`.
  - `store::testutil::temp_dir` (`store/mod.rs` 41-46) never cleans up, so every test run leaves session, jar (DPAPI) and JSON files under `%TEMP%\manifold-store-<pid>-*`.
  - Fix: widen the needles; move the screenshot folder lookup into `profile` (`pictures_dir`, `None` under test). For cleanup, the first `temp_dir` call of a run sweeps `manifold-store-*` folders of other processes older than an hour. A per-test drop guard (the report's suggestion) would touch every one of the ~100 call sites for the same result.
- **A-R7 (low).** `settings.rs` `conflict_path` (178-184) names the file by the Unix second, so two conflicts in one second overwrite each other, against its own doc. After a conflict, `settings_fingerprint` stays stale, so every later save in the run is a conflict: same-second repeats are realistic.
- **A-R8 (low).** `nav_history.rs`:
  - `load` (165-174) keeps a file's list of any length; `MAX_ENTRIES` (50) applies only when a navigation trims one entry at a time.
  - `save` (178-191) runs its `keep` retain only when some list is dirty, so closed tabs' lists stay in memory and in the file until an unrelated tab navigates.
- **A-R6 (low, doc).** `jar_defs.rs` `set_jar_ephemeral` doc (379-384) says "on: its cookies stay in memory until quit, then its files go". The code deletes the files at once (399-403), as the confirm text says.

**Files:**
- Modify: `crates/manifold/src/profile.rs` (`dir`, `dir_from`, new `pictures_dir`, tests)
- Modify: `crates/manifold/src/main.rs` (`screenshot_default_path` ~368)
- Modify: `crates/manifold/src/settings.rs` (`conflict_path`, new `conflict_path_in`, tests)
- Modify: `crates/manifold/src/nav_history.rs` (`load`, `save`, new `load_from`, `capped`, `save_to`, tests)
- Modify: `crates/manifold/src/store/mod.rs` (`testutil::temp_dir`, new `testutil::sweep_old_dirs`, test)
- Modify: `crates/manifold/src/jar_defs.rs` (doc only)

**Interfaces:**
- Produces:
  - `profile::pictures_dir() -> Option<PathBuf>`
  - `settings::conflict_path_in(dir: &Path, secs: u64) -> PathBuf` (private)
  - `nav_history::load_from(path: &Path) -> HashMap<String, NavHistory>` and `nav_history::save_to(lists: &mut HashMap<String, NavHistory>, keep: impl Fn(&str) -> bool, path: Option<&Path>)` (both `pub(crate)`)
  - `store::testutil::sweep_old_dirs(root: &Path, older_than: Duration) -> usize`
- Unchanged: `profile::dir()`, `nav_history::load()`, `nav_history::save(lists, keep)`.

- [ ] **Step 1: Write the failing tests.**

`profile.rs` tests: extend `tests_have_no_profile_dir` with `assert_eq!(super::pictures_dir(), None);`, and add:

```rust
    #[test]
    fn profile_dir_accepts_the_equals_form() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let appdata = Some(std::ffi::OsString::from("A"));
        assert_eq!(super::dir_from(&args(&["m", "--profile-dir=P"]), appdata.clone()), Some("P".into()));
        assert_eq!(
            super::dir_from(&args(&["m", "--profile-directory", "X"]), appdata),
            Some(Path::new("A").join("manifold")),
            "only the exact flag"
        );
    }
```

In `app_default_touches_no_profile_file`, widen the needles:

```rust
        let needles = ["\"APPDATA\"", "\"LOCALAPPDATA\"", "\"USERPROFILE\"", "\"HOME\"", "etcetera", "dirs::", "known_folder"];
        scan(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &needles, &mut offenders);
```

`settings.rs` tests (add a `#[cfg(test)] mod tests` with `use super::*;` if there is none):

```rust
    #[test]
    fn a_second_conflict_in_one_second_gets_its_own_name() {
        let dir = crate::store::testutil::temp_dir("settings_conflict");
        let first = conflict_path_in(&dir, 7);
        assert_eq!(first, dir.join("settings.conflict-7.json"));
        std::fs::write(&first, "{}").unwrap();
        let second = conflict_path_in(&dir, 7);
        assert_eq!(second, dir.join("settings.conflict-7-1.json"));
        std::fs::write(&second, "{}").unwrap();
        assert_eq!(conflict_path_in(&dir, 7), dir.join("settings.conflict-7-2.json"));
    }
```

`nav_history.rs` tests:

```rust
    #[test]
    fn a_saved_list_past_the_cap_is_cut_around_its_current_entry() {
        let dir = crate::store::testutil::temp_dir("nav_cap");
        let path = dir.join("tab-history.json");
        let entries: Vec<String> = (0..120).map(|n| format!("p{n}")).collect();
        let saved = serde_json::json!({
            "late": { "entries": entries, "index": 100 },
            "early": { "entries": entries, "index": 3 },
        });
        std::fs::write(&path, saved.to_string()).unwrap();
        let lists = load_from(&path);
        let late = &lists["late"];
        assert_eq!((late.entries.len(), late.index), (MAX_ENTRIES, MAX_ENTRIES - 1));
        assert_eq!(late.entries[late.index], "p100");
        let early = &lists["early"];
        assert_eq!((early.entries.len(), early.index), (MAX_ENTRIES, 3));
        assert_eq!(early.entries[0], "p0");
    }

    #[test]
    fn closed_tabs_leave_the_file_even_when_nothing_navigated() {
        let dir = crate::store::testutil::temp_dir("nav_prune");
        let path = dir.join("tab-history.json");
        let mut lists = HashMap::new();
        for id in ["a", "b"] {
            let mut h = NavHistory::default();
            visit(&mut h, &["x", "y"]);
            lists.insert(id.to_string(), h);
        }
        save_to(&mut lists, |_| true, Some(&path));
        assert!(lists.values().all(|h| !h.dirty));
        save_to(&mut lists, |id| id == "a", Some(&path));
        assert_eq!(lists.keys().collect::<Vec<_>>(), vec!["a"]);
        assert_eq!(load_from(&path).keys().cloned().collect::<Vec<_>>(), vec!["a".to_string()]);
    }
```

Check the `Saved` serde shape (`nav_history.rs:41`, fields `entries` and `index`, a map keyed by tab id) and that `HashMap` is imported in the module. Adjust the JSON literal if the shape differs.

`store/mod.rs` testutil tests (add `#[cfg(test)] mod tests` inside `testutil` if needed):

```rust
    #[test]
    fn the_sweep_removes_only_other_runs_store_dirs() {
        let root = temp_dir("sweep_root");
        let other = root.join("manifold-store-1-old");
        let ours = root.join(format!("manifold-store-{}-mine", std::process::id()));
        let unrelated = root.join("something-else");
        for d in [&other, &ours, &unrelated] {
            std::fs::create_dir_all(d).unwrap();
        }
        assert_eq!(sweep_old_dirs(&root, std::time::Duration::ZERO), 1);
        assert!(!other.exists() && ours.exists() && unrelated.exists());
    }
```

- [ ] **Step 2: Run** `cargo test -j 8 -p manifold --bins profile`, `conflict_in_one_second`, `nav_history`, `the_sweep_removes`. Expected: compile errors for the missing functions; `app_default_touches_no_profile_file` FAILS, naming `main.rs` (its `"USERPROFILE"`).

- [ ] **Step 3: `profile.rs`.**

```rust
pub fn dir() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    static DIR: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let args: Vec<String> = std::env::args().collect();
        dir_from(&args, std::env::var_os("APPDATA"))
    })
    .clone()
}

fn dir_from(args: &[String], appdata: Option<std::ffi::OsString>) -> Option<PathBuf> {
    let flag = args.iter().enumerate().find_map(|(i, a)| match a.strip_prefix("--profile-dir") {
        Some("") => args.get(i + 1).cloned(),
        Some(rest) => rest.strip_prefix('=').map(str::to_string),
        None => None,
    });
    match flag {
        Some(dir) => Some(PathBuf::from(dir)),
        None => Some(PathBuf::from(appdata?).join("manifold")),
    }
}

/// The user's Pictures folder, where the toolbar's Screenshot saves. `None`
/// without a home folder, and always under `cargo test`, like `dir()`.
pub fn pictures_dir() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })?;
    Some(PathBuf::from(home).join("Pictures"))
}
```

Update `dir()`'s doc: "Read once per run." and "`--profile-dir <dir>` or `--profile-dir=<dir>`". Add `pictures_dir` to the module doc's list.

- [ ] **Step 4: `main.rs` `screenshot_default_path`.**

```rust
fn screenshot_default_path() -> std::path::PathBuf {
    let dir = profile::pictures_dir().unwrap_or_else(std::env::temp_dir);
    let _ = std::fs::create_dir_all(&dir);
    let timestamp =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    dir.join(format!("manifold-{timestamp}.png"))
}
```

Change its doc's first line to "`profile::pictures_dir()`\manifold-<unix-seconds>.png, or the temp folder without one". Before, a missing `USERPROFILE` made it a relative `Pictures` under the working directory.

- [ ] **Step 5: `settings.rs`.**

```rust
pub fn conflict_path() -> Option<PathBuf> {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    Some(conflict_path_in(&crate::profile::dir()?, secs))
}

/// `settings.conflict-<secs>.json`, or `-<n>` after the seconds when that
/// name is taken, so a second conflict in the same second keeps the first.
fn conflict_path_in(dir: &std::path::Path, secs: u64) -> PathBuf {
    (0u32..)
        .map(|n| match n {
            0 => dir.join(format!("settings.conflict-{secs}.json")),
            n => dir.join(format!("settings.conflict-{secs}-{n}.json")),
        })
        .find(|p| !p.exists())
        .expect("an unbounded range finds a free name")
}
```

- [ ] **Step 6: `nav_history.rs`.**

```rust
/// The saved lists. A missing or corrupt file (moved aside) gives none.
pub(crate) fn load() -> HashMap<String, NavHistory> {
    path().map_or_else(HashMap::new, |path| load_from(&path))
}

pub(crate) fn load_from(path: &std::path::Path) -> HashMap<String, NavHistory> {
    let saved: HashMap<String, Saved> =
        crate::json_file::load_or_move_aside(path, "tab history").ok().flatten().unwrap_or_default();
    saved.into_iter().filter(|(_, s)| s.index < s.entries.len()).map(|(id, s)| (id, capped(s))).collect()
}

/// At most `MAX_ENTRIES` entries: the current one and as many before it as
/// fit, then forward ones, the window `observe` would have kept.
fn capped(s: Saved) -> NavHistory {
    let start = (s.index + 1).saturating_sub(MAX_ENTRIES);
    let entries: Vec<String> = s.entries.into_iter().skip(start).take(MAX_ENTRIES).collect();
    NavHistory { entries, index: s.index - start, ..NavHistory::default() }
}

/// Drops the lists of tabs `keep` refuses, then writes the rest (one-page
/// lists add nothing and are left out) when any changed or any was dropped.
pub(crate) fn save(lists: &mut HashMap<String, NavHistory>, keep: impl Fn(&str) -> bool) {
    save_to(lists, keep, path().as_deref());
}

pub(crate) fn save_to(
    lists: &mut HashMap<String, NavHistory>,
    keep: impl Fn(&str) -> bool,
    path: Option<&std::path::Path>,
) {
    let before = lists.len();
    lists.retain(|id, _| keep(id));
    let dropped = lists.len() != before;
    if !dropped && !lists.values().any(|h| h.dirty) {
        return;
    }
    let Some(path) = path else { return };
    let saved: HashMap<&String, Saved> = lists
        .iter()
        .filter(|(_, h)| h.entries.len() > 1)
        .map(|(id, h)| (id, Saved { entries: h.entries.clone(), index: h.index }))
        .collect();
    crate::json_file::save(&saved, path, "tab history");
    lists.values_mut().for_each(|h| h.dirty = false);
}
```

The caller (`session_sync.rs:895`) is unchanged.

- [ ] **Step 7: `store/mod.rs` testutil.**

```rust
    /// A fresh, empty directory for one test, under the system temp dir.
    /// The first call of a run also sweeps other runs' leftovers.
    pub fn temp_dir(name: &str) -> PathBuf {
        static SWEEP: std::sync::Once = std::sync::Once::new();
        SWEEP.call_once(|| {
            sweep_old_dirs(&std::env::temp_dir(), std::time::Duration::from_secs(60 * 60));
        });
        let dir = std::env::temp_dir().join(format!("manifold-store-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Removes `root`'s `manifold-store-<pid>-*` directories of other
    /// processes last modified more than `older_than` ago. Returns how many.
    /// An hour is far longer than a test run, so a concurrent run's folders stay.
    pub fn sweep_old_dirs(root: &Path, older_than: std::time::Duration) -> usize {
        let ours = format!("manifold-store-{}-", std::process::id());
        let Ok(entries) = std::fs::read_dir(root) else { return 0 };
        entries
            .flatten()
            .filter(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                name.starts_with("manifold-store-") && !name.starts_with(&ours)
            })
            .filter(|e| {
                e.metadata().and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().unwrap_or_default() >= older_than)
            })
            .filter(|e| std::fs::remove_dir_all(e.path()).is_ok())
            .count()
    }
```

Add `use std::path::Path;` to `testutil` if only `PathBuf` is imported. This leaves the user's real profile alone: it lists only the temp folder, and only names this module creates.

- [ ] **Step 8: `jar_defs.rs` doc.** Replace the first bullet of `set_jar_ephemeral`'s doc:

```rust
    /// - on: its files are deleted now (`Jar::delete_files`); its cookies stay in memory until quit. Not a locked
    ///   jar's: an unreadable file is never touched, and a locked jar saves nothing anyway.
```

- [ ] **Step 9: Run** `cargo test -j 8 -p manifold --bins profile`, then `settings`, `nav_history` and `testutil`. Expected: PASS, including the widened guard.

- [ ] **Step 10: Full gate, then commit.**

```bash
git add crates/manifold/src/profile.rs crates/manifold/src/main.rs crates/manifold/src/settings.rs crates/manifold/src/nav_history.rs crates/manifold/src/store/mod.rs crates/manifold/src/jar_defs.rs
git commit -m "Profile: dir parsed once (and --profile-dir=), a wider isolation guard, screenshots via pictures_dir; conflict files never overwrite; tab history capped on load and pruned on every save; test temp dirs swept

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 22: Network fetches: one agent with a User-Agent and bounded waits; one fetch per node; no UNC hosts; clean favicon origins; at most four favicon fetches at once

**Findings:**
- **D-F1, fetch half (low).** `redirect_fetch.rs` `fetch` (88-89) and `favicon_fetch.rs` `fetch_favicon` (129-130) repeat the same `ureq::get(..).config().timeout_global(..)...read_to_vec().ok()?` chain. Both discard the error, so nothing is ever logged. One `net::get_limited` on a shared agent fixes D-R3 and part of D-R4 in one place.
- **D-R3 (med).** The 15 s *global* timeout on a fetch of up to 100 MB (`redirect_fetch.rs:88`) fails real PDF and image conversions on slower links, silently for a tab conversion. Use connect and response-header timeouts, plus a body budget scaled to the size limit (ureq 3's `timeout_recv_body` is a total for the body, not per read).
- **D-R4, part (med, design gap).** Web fetches send no User-Agent, so UA-gated servers refuse them. Set one on the agent. Sending the tab's jar cookies is deferred (see Deferred).
- **D-B3, in-flight half (med).** Two concurrent fetches for one node (Refresh clicked twice, or a restore-time refetch plus Retry) both write `<id>.<ext>`. Task 7 made the temp name unique. This half keeps an in-flight set in `RedirectFetcher` keyed by node id, so a second request for a node already being fetched is a no-op.
- **D-R5 (low, hardening).** `content::file_url_path` turns `file://host/share/x` (and `file:///%5C%5Chost/...`) into a UNC path. `fetch` then opens it, and Windows connects to that host over SMB, offering the user's NTLM credentials. Such URLs arrive in synced, imported or named sessions. Refuse UNC paths in `fetch`. A user who really keeps content on a share re-adds the file from the picker.
- **D-B7 (low).** `favicon_fetch.rs` `url_origin` (106-115) keeps URL userinfo (`https://user:pass@host`), so credentials become part of the favicon cache's file name and key. Strip everything up to the last `@` of the authority and lowercase the scheme and host.
- **D-R2, threads half (med).** `FaviconFetcher::request` (~71-83) spawns one thread per origin. Restoring a session with many origins starts them all at once. Run at most four at a time and queue the rest. (The decode cap half is Task 1.)

**Files:**
- Create: `crates/manifold/src/net.rs`
- Modify: `crates/manifold/src/main.rs` (`mod net;`, alphabetical in the module list)
- Modify: `crates/manifold/src/redirect_fetch.rs` (`fetch`, `RedirectFetcher`, tests)
- Modify: `crates/manifold/src/fetched.rs:252` and `event_loop.rs:~1707` (`request` now takes `&mut self` and returns `bool`; the two callers ignore it), `session_sync.rs:~1583` (`tick` takes `&mut self`)
- Modify: `crates/manifold/src/favicon_fetch.rs` (`url_origin`, `fetch_favicon`, `FaviconFetcher`, tests)
- Modify: `crates/manifold/src/engine_install.rs` (`body_budget` moves to `net`; its test moves with it)

**Interfaces:**
- Consumes: `rgba::decode_limited(bytes, FAVICON_SIDE)` and `rgba::FAVICON_SIDE` (Task 1), already called in `fetch_favicon` after Task 1. Keep that call as it is.
- Produces:
  - `net::Budget { connect: Duration, response: Duration, body: Duration }`
  - `net::get_limited(url: &str, limit: u64, budget: Budget) -> Result<Vec<u8>, String>`
  - `net::body_budget(bytes: u64, bytes_per_sec: u64, floor: Duration) -> Duration`
  - `RedirectFetcher::request(&mut self, job: FetchJob) -> bool` (false: that node is already being fetched)
  - `RedirectFetcher::tick(&mut self) -> Vec<FetchResult>`
  - `redirect_fetch::is_unc_file_url(url: &str) -> bool`
  - `FaviconFetcher` gains `const MAX_FAVICON_FETCHES: usize = 4`, a queue and an injectable `fetch: fn(&str) -> Option<FaviconImage>` (tests only replace it).

- [ ] **Step 1: Write the failing tests.**

`net.rs` tests, against a one-shot server on loopback (no outside network):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Serves `response` to one connection, then hands back the request it read.
    fn serve_once(response: &'static [u8], stall: Option<Duration>) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/x", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            let n = stream.read(&mut request).unwrap();
            stream.write_all(response).unwrap();
            if let Some(stall) = stall {
                std::thread::sleep(stall);
            }
            String::from_utf8_lossy(&request[..n]).into_owned()
        });
        (url, handle)
    }

    const QUICK: Budget =
        Budget { connect: Duration::from_secs(2), response: Duration::from_secs(2), body: Duration::from_millis(300) };

    #[test]
    fn a_fetch_sends_the_user_agent_and_returns_the_body() {
        let (url, server) = serve_once(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello", None);
        assert_eq!(get_limited(&url, 100, QUICK).unwrap(), b"hello");
        let request = server.join().unwrap().to_ascii_lowercase();
        assert!(request.contains("user-agent: manifold/"), "{request}");
    }

    #[test]
    fn a_body_past_the_limit_is_an_error_with_a_reason() {
        let (url, server) = serve_once(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello", None);
        let err = get_limited(&url, 3, QUICK).unwrap_err();
        assert!(!err.is_empty());
        server.join().unwrap();
    }

    #[test]
    fn a_stalled_body_gives_up_within_its_budget() {
        let (url, server) =
            serve_once(b"HTTP/1.1 200 OK\r\nContent-Length: 50\r\n\r\npartial", Some(Duration::from_secs(3)));
        let started = std::time::Instant::now();
        assert!(get_limited(&url, 100, QUICK).is_err());
        assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
        server.join().unwrap();
    }

    #[test]
    fn the_body_budget_scales_with_size_and_has_a_floor() {
        let floor = Duration::from_secs(120);
        assert_eq!(body_budget(0, 32 * 1024, floor), floor);
        assert_eq!(body_budget(171_619_649, 32 * 1024, floor), Duration::from_secs(171_619_649 / (32 * 1024)));
    }
}
```

The last test replaces `engine_install.rs`'s `the_body_budget_scales_with_size_and_has_a_floor` (~809); delete that one when `body_budget` moves.

`redirect_fetch.rs` tests:

```rust
    #[cfg(windows)] // the host forms decode to UNC paths on Windows only
    #[test]
    fn unc_file_urls_are_recognized() {
        assert!(is_unc_file_url("file://server/share/x.pdf"));
        assert!(is_unc_file_url("file:///%5C%5Cserver/share/x.pdf"));
        assert!(is_unc_file_url("file:////server/share/x.pdf"));
        assert!(!is_unc_file_url("file:///C:/x.pdf"));
        assert!(!is_unc_file_url("file://localhost/C:/x.pdf"));
        assert!(!is_unc_file_url("https://server/x.pdf"));
    }

    #[test]
    fn a_second_request_for_a_node_in_flight_is_dropped() {
        // `None`: every fetch fails at once, without the network.
        let mut fetcher = RedirectFetcher::new(None);
        let job = || FetchJob {
            target: FetchTarget::Node("7-1".into()),
            node_id: "7-1".into(),
            url: "https://example.invalid/x.pdf".into(),
            kind: ContentKind::Pdf,
        };
        assert!(fetcher.request(job()));
        assert!(!fetcher.request(job()), "already in flight");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut results = Vec::new();
        while results.is_empty() && std::time::Instant::now() < deadline {
            results.extend(fetcher.tick());
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(results.len(), 1);
        assert!(fetcher.request(job()), "free again once its result is ticked");
    }
```

`file:////server/...` decodes to `\server\share\...`, a root-relative path rather than UNC. It is refused anyway (Step 4): a user never has a reason to write that URL, and a picked file always has a drive letter or a real UNC host.

`favicon_fetch.rs` tests, beside `a_cache_file_name_is_the_origin_made_safe`:

```rust
    #[test]
    fn an_origin_drops_userinfo_and_case() {
        assert_eq!(url_origin("https://user:pass@Example.COM/x?y"), Some("https://example.com".into()));
        assert_eq!(url_origin("HTTP://a@b@Host:8080#f"), Some("http://host:8080".into()));
        assert_eq!(url_origin("https://user@/x"), None, "no host left");
    }

    #[test]
    fn at_most_four_favicon_fetches_run_at_once() {
        let mut fetcher = FaviconFetcher::new(None);
        fetcher.fetch = |_| None;
        for n in 0..6 {
            fetcher.request(format!("https://o{n}.example"));
        }
        fetcher.request("https://o5.example".into());
        assert_eq!((fetcher.in_flight.len(), fetcher.queue.len()), (MAX_FAVICON_FETCHES, 2), "a queued origin isn't queued twice");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut done = Vec::new();
        while done.len() < 6 && std::time::Instant::now() < deadline {
            done.extend(fetcher.tick().into_iter().map(|(origin, _)| origin));
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        done.sort();
        assert_eq!(done, (0..6).map(|n| format!("https://o{n}.example")).collect::<Vec<_>>());
    }
```

Check `FaviconFetcher::new`'s parameter (the cache dir, `Option<PathBuf>`, per ~60-65) and adjust the call.

- [ ] **Step 2: Run** `cargo test -j 8 -p manifold --bins net::`, `unc_file_urls`, `second_request_for_a_node`, `origin_drops_userinfo`, `four_favicon_fetches`. Expected: compile errors for the missing items.

- [ ] **Step 3: Create `net.rs`.**

```rust
//! The one HTTP client for manifold's own fetches (content conversions,
//! Refresh, favicons): a shared ureq agent with a User-Agent, and a fetch
//! that keeps the error's reason. Engines fetch pages themselves; this is
//! only what manifold downloads on its own.

use std::sync::OnceLock;
use std::time::Duration;

/// Sent with every fetch: some servers refuse a request without one.
const USER_AGENT: &str = concat!("manifold/", env!("CARGO_PKG_VERSION"));

/// How long each stage of one fetch may take.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Budget {
    /// Resolving and connecting, TLS included.
    pub connect: Duration,
    /// From the request to the response's headers.
    pub response: Duration,
    /// The whole body, in total (ureq 3 has no per-read idle timeout).
    pub body: Duration,
}

fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| ureq::Agent::config_builder().user_agent(USER_AGENT).build().into())
}

/// `GET url`'s body, at most `limit` bytes, within `budget`. The error says
/// why (status, timeout, too large), for the caller to log.
pub(crate) fn get_limited(url: &str, limit: u64, budget: Budget) -> Result<Vec<u8>, String> {
    let response = agent()
        .get(url)
        .config()
        .timeout_connect(Some(budget.connect))
        .timeout_recv_response(Some(budget.response))
        .timeout_recv_body(Some(budget.body))
        .build()
        .call()
        .map_err(|e| e.to_string())?;
    response.into_body().with_config().limit(limit).read_to_vec().map_err(|e| e.to_string())
}

/// The time `bytes` take at `bytes_per_sec`, at least `floor`: a body
/// budget that lets a large download finish on a slow link but still ends
/// a dead connection.
pub(crate) fn body_budget(bytes: u64, bytes_per_sec: u64, floor: Duration) -> Duration {
    Duration::from_secs(bytes / bytes_per_sec.max(1)).max(floor)
}
```

Check the ureq 3.4.1 names while compiling: `Agent::config_builder()`, `ConfigBuilder::user_agent`, `.build().into()` to make an `Agent`, and the per-request `.config()...build()`. `engine_install.rs` already uses the per-request form. If the agent conversion is spelled differently (for example `Agent::new_with_config(config)`), use that. Add `mod net;` to `main.rs`.

In `engine_install.rs`, delete `body_budget` and its test, and call `crate::net::body_budget(total - have.min(total), SLOW_BYTES_PER_SEC, Duration::from_secs(120))` at the old call site (~251). Keep `SLOW_BYTES_PER_SEC` and the doc comment that explains the budget, moved onto the call.

- [ ] **Step 4: `redirect_fetch.rs`.**

```rust
pub struct RedirectFetcher {
    dir: Option<PathBuf>,
    tx: mpsc::Sender<FetchResult>,
    rx: mpsc::Receiver<FetchResult>,
    /// Nodes with a fetch running: a second Refresh (or a Retry racing a
    /// restore-time refetch) is dropped instead of writing the same file.
    in_flight: HashSet<String>,
}
```

`new` adds `in_flight: HashSet::new()` (import `std::collections::HashSet`).

```rust
    /// Fetches `job.url` on a background thread. `false` (and nothing done)
    /// when `job.node_id` is already being fetched.
    pub fn request(&mut self, job: FetchJob) -> bool {
        if !self.in_flight.insert(job.node_id.clone()) {
            return false;
        }
        let tx = self.tx.clone();
        let dir = self.dir.clone();
        std::thread::spawn(move || {
            let body = dir.as_deref().and_then(|d| fetch(d, &job.node_id, &job.url, job.kind));
            let _ = tx.send(FetchResult { job, body });
        });
        true
    }

    pub fn tick(&mut self) -> Vec<FetchResult> {
        let results: Vec<FetchResult> = self.rx.try_iter().collect();
        for r in &results {
            self.in_flight.remove(&r.job.node_id);
        }
        results
    }
```

Keep `tick`'s doc. `fetch`:

```rust
/// Most a web fetch's body may take: `max_bytes` at 128 KB/s (about
/// 1 Mbit/s), at least a minute. 100 MB gets about 13 minutes.
const SLOW_BYTES_PER_SEC: u64 = 128 * 1024;

pub fn fetch(dir: &Path, node_id: &str, url: &str, kind: ContentKind) -> Option<WebBody> {
    let limit = max_bytes(kind);
    let bytes = match content::file_url_path(url) {
        Some(_) if is_unc_file_url(url) => {
            eprintln!("fetch: refused a file URL on another host (node {node_id})");
            return None;
        }
        Some(path) => {
            let mut bytes = Vec::new();
            std::fs::File::open(path).ok()?.take(limit + 1).read_to_end(&mut bytes).ok()?;
            bytes
        }
        None => {
            let budget = crate::net::Budget {
                connect: Duration::from_secs(10),
                response: Duration::from_secs(20),
                body: crate::net::body_budget(limit, SLOW_BYTES_PER_SEC, Duration::from_secs(60)),
            };
            match crate::net::get_limited(url, limit, budget) {
                Ok(bytes) => bytes,
                Err(e) => {
                    let origin = crate::favicon_fetch::url_origin(url).unwrap_or_default();
                    eprintln!("fetch: {origin} (node {node_id}): {e}");
                    return None;
                }
            }
        }
    };
    if bytes.len() as u64 > limit {
        return None;
    }
    save(dir, node_id, kind, bytes)
}

/// A `file:` URL naming another machine (`file://host/share/...`, or a
/// path that decodes to `\\host\...`): reading it would make Windows
/// connect to that host and offer the user's credentials. Such URLs come
/// from synced or imported sessions, not from the user's own picks.
pub fn is_unc_file_url(url: &str) -> bool {
    content::file_url_path(url).is_some_and(|p| {
        let s = p.to_string_lossy();
        // Windows: `\\host\share` (UNC) and `\x` (root-relative, what
        // `file:////host/...` decodes to); a drive path (`C:\`) passes.
        // Elsewhere: `//host`; an absolute path (`/home/...`) passes.
        if cfg!(windows) {
            s.starts_with('\\') || s.starts_with('/')
        } else {
            s.starts_with("//")
        }
    })
}
```

The log names the origin only (via `url_origin`, now free of userinfo), never the full URL: a query string can carry a token.

How the cases decode on Windows (`content::file_url_path`):
- `file:///C:/x.pdf` and `file://localhost/C:/x.pdf` both give `C:\x.pdf`, allowed.
- `file://server/share/x.pdf` gives `\\server\share\x.pdf`, refused.
- `file:///%5C%5Cserver/...` decodes to `\\server\...`, refused.
- `file:////server/...` gives `\server\...`, refused.

On Linux, `file:///home/x` gives `/home/x` and stays allowed; a host-bearing URL gives `None` there, so `fetch` never opens it.

Callers: `fetched.rs:252` and `event_loop.rs:~1707` already reach `self.redirect_fetcher` through `&mut self`. They ignore the returned `bool`, which Rust does not warn about, so the calls stay as they are. `session_sync.rs:~1583` already holds `app` mutably.

- [ ] **Step 5: `favicon_fetch.rs`.**

`url_origin`:

```rust
pub fn url_origin(url: &str) -> Option<String> {
    let scheme_end = url.find("://")?;
    let rest = &url[scheme_end + 3..];
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    // Userinfo (`user:pass@`) is a credential: never part of a cache name or key.
    let host = rest[..authority_end].rsplit_once('@').map_or(&rest[..authority_end], |(_, host)| host);
    if host.is_empty() {
        return None;
    }
    Some(format!("{}://{}", url[..scheme_end].to_ascii_lowercase(), host.to_ascii_lowercase()))
}
```

`fetch_favicon`: replace the two ureq lines with

```rust
    let budget = crate::net::Budget {
        connect: Duration::from_secs(3),
        response: Duration::from_secs(3),
        body: Duration::from_secs(5),
    };
    let bytes = crate::net::get_limited(&url, 2_000_000, budget).ok()?;
```

and keep Task 1's limited decode after it. A missing favicon is the common case, so its error is not logged. Fix the doc's "now-dead" sentence while here: "The size and timeout caps are safety nets against a misbehaving server; a favicon is small."

`FaviconFetcher`:

```rust
/// Favicon fetches running at once; the rest wait in `queue`. A restored
/// session can name dozens of origins at the same moment.
const MAX_FAVICON_FETCHES: usize = 4;

pub struct FaviconFetcher {
    tx: mpsc::Sender<(String, Option<FaviconImage>)>,
    rx: mpsc::Receiver<(String, Option<FaviconImage>)>,
    in_flight: HashSet<String>,
    queue: std::collections::VecDeque<String>,
    cache_dir: Option<PathBuf>,
    /// `fetch_favicon`; tests swap in a stub so no request leaves the machine.
    fetch: fn(&str) -> Option<FaviconImage>,
}
```

Match the existing field types: copy the `tx`/`rx` item type from the current struct. `new` adds `queue: VecDeque::new(), fetch: fetch_favicon`.

```rust
    pub fn request(&mut self, origin: String) {
        if self.in_flight.contains(&origin) || self.queue.contains(&origin) {
            return;
        }
        if self.in_flight.len() < MAX_FAVICON_FETCHES {
            self.start(origin);
        } else {
            self.queue.push_back(origin);
        }
    }

    fn start(&mut self, origin: String) {
        self.in_flight.insert(origin.clone());
        let (tx, cache_dir, fetch) = (self.tx.clone(), self.cache_dir.clone(), self.fetch);
        std::thread::spawn(move || {
            let result = match &cache_dir {
                Some(dir) => cached_or_fetched(dir, &origin, fetch),
                None => fetch(&origin),
            };
            let _ = tx.send((origin, result));
        });
    }

    pub fn tick(&mut self) -> Vec<(String, Option<FaviconImage>)> {
        let mut results = Vec::new();
        while let Ok((origin, result)) = self.rx.try_recv() {
            self.in_flight.remove(&origin);
            results.push((origin, result));
        }
        while self.in_flight.len() < MAX_FAVICON_FETCHES {
            let Some(origin) = self.queue.pop_front() else { break };
            self.start(origin);
        }
        results
    }
```

Check that `cached_or_fetched`'s third parameter accepts a `fn(&str) -> Option<FaviconImage>`. It is written generically (`fetch_favicon` is passed as a value today); if it takes `impl Fn`, a `fn` pointer fits. Keep the existing docs and update `request`'s: "a no-op if this origin is already running or queued".

- [ ] **Step 6: Run** the Step 2 tests, then `cargo test -j 8 -p manifold --bins redirect_fetch favicon_fetch engine_install`. Expected: PASS.

- [ ] **Step 7: Live check.** Rebuild (`cargo build -j 8 -p manifold`), launch at BelowNormal, open a tab on a PDF URL (for example `https://www.w3.org/WAI/WCAG21/working-examples/pdf-table/table.pdf`) so it converts to a node, then press Refresh twice quickly. Expect one fetch in the log and a working node. Kill only your PID.

- [ ] **Step 8: Full gate, then commit.**

```bash
git add crates/manifold/src/net.rs crates/manifold/src/main.rs crates/manifold/src/redirect_fetch.rs crates/manifold/src/favicon_fetch.rs crates/manifold/src/fetched.rs crates/manifold/src/event_loop.rs crates/manifold/src/session_sync.rs crates/manifold/src/engine_install.rs
git commit -m "Fetch: one agent with a User-Agent and staged timeouts; one fetch per node; UNC file URLs refused; favicon origins drop userinfo; four favicon fetches at a time

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 23: Engine robustness: no redundant WebKit frames; frame-buffer and device loss recover or report; bounded CEF shutdown and cookie pull; sandbox config told straight

**Findings:**
- **E-R2 (med, perf).** WebKit `main.rs` (~509-544) takes `offscreen.pixbuf()`, repacks it and sends the full `w*h*4` bytes every 16 ms while visible, changed or not. At 1080p that is about 500 MB/s over the WSL NAT for a static page, and the send blocks the GTK pump. Send only when the pixels or size changed, at most every `FRAME_INTERVAL`.
- **E-R3 (low-med).** CEF `main.rs` (~506-508) and Servo `main.rs` (~257-259) `.expect("recreate shared frame buffer…")`. `SharedFrameBuffer::create` gives up after 20×10 ms of error 1224 while the host still maps the old file, which a host stalled on the shared eGPU can cause. The engine then dies and respawns. WebView2 (`main.rs:504`) already uses `.ok()` and retries next frame; do the same.
- **E-R4, device half (low-med).** WebView2 `capture.rs` `resize` (168-181) rebuilds the WinRT device with `unwrap_or_else(|_| unreachable!(..))`, which panics on `DXGI_ERROR_DEVICE_REMOVED` (eGPU unplug, driver update, TDR). `try_get_latest_frame` (93-161) swallows `Map`/`CopyResource` failures, so after a device loss the tab freezes and never reports. Cache the WinRT device from `new`; check `GetDeviceRemovedReason` and send `EngineToHost::Crashed` (the host respawns on that, with Task 9's crash limit). The default-adapter half is deferred.
- **E-R5 (low).** CEF `main.rs` (~389-402, 523-526): after `Shutdown`, the loop exits only when `on_before_close` fires. If `browser.host()` is `None` or the callback never comes, it spins for ever. That is the orphan case `spawn_host_reader` synthesizes `Shutdown` for when manifold died. Add a 3 s deadline, then exit without `cef::shutdown()` (calling it with a live browser risks a hang inside libcef). The host side is unchanged: `process_manager.rs:299-301` still kills local hosts right after `Shutdown`; a graceful wait there is not needed for correctness.
- **E-R6 (med posture / low config).**
  - CEF renderers run unsandboxed (`app.rs:33` appends `--no-sandbox`; `main.rs:153-157` sets `no_sandbox`). The comment there says "We disabled the `sandbox` cargo feature", but `Cargo.toml:9` enables it.
  - Fix: drop the unused feature so the comment is true, and record the tradeoff under README "Known limitations". Wiring the sandbox bootstrap is out of scope.
  - `app.rs:43-47` always adds `--enable-logging=stderr --v=1`. `--v=1` makes the per-tab logs very large on a nearly full disk. Gate it behind `MANIFOLD_CEF_VERBOSE=1`. `--enable-logging=stderr` stays, so errors still reach the per-tab log (without it, CEF writes `debug.log` beside the host instead).
- **E-R7 (low).** CEF `jar_cookies.rs` (131-137, 157-171): the pull's timeout applies only while `seen` is empty. A visitor that delivers some cookies and then stops never sets `done`, so `poll` returns `Pending` for ever. The host's own 10 s timeout covers the host, but the engine keeps polling. Add an overall deadline.
- **B-R2 remnant (low).** `process_manager.rs` `engine_job` (~80) ignores `SetInformationJobObject`'s result, so a failed kill-on-close setup is silent. Log it. (The spawn-to-assign race is dropped; see Dropped findings.)

**Files:**
- Modify: `crates/manifold-engine-webkit/src/main.rs` (frame send ~509-544, new `frame_due`, test)
- Modify: `crates/manifold-engine-cef/src/main.rs` (frame buffer ~236 and ~502-510; shutdown ~389-402 and ~523-526; new `shutdown_due`; sandbox comment ~153-157)
- Modify: `crates/manifold-engine-servo/src/main.rs` (~205, ~257-259 and the frame write site)
- Modify: `crates/manifold-engine-webview2/src/capture.rs` (`Capture` field, `resize`, new `device_lost`) and `main.rs` (the frame loop ~487)
- Modify: `crates/manifold-engine-cef/src/jar_cookies.rs` (`poll`, new `given_up`, tests)
- Modify: `crates/manifold-engine-cef/Cargo.toml:9`, `crates/manifold-engine-cef/src/app.rs:43-47`
- Modify: `crates/manifold/src/process_manager.rs` (`engine_job`)
- Modify: `README.md` ("Known limitations", line ~449)

**Interfaces:**
- Produces (all engine-private):
  - webkit `fn frame_due(size: (u32, u32), pixels: &[u8], last: Option<&SentFrame>, since_sent: Duration) -> bool` with `struct SentFrame { size: (u32, u32), pixels: Vec<u8> }` and `const FRAME_INTERVAL: Duration = Duration::from_millis(33)`
  - cef `fn shutdown_due(elapsed: Duration, closed: bool) -> Option<ShutdownPath>`, `enum ShutdownPath { Clean, Forced }`, `const SHUTDOWN_GRACE: Duration = Duration::from_secs(3)`
  - cef `jar_cookies::given_up(failed: bool, seen_any: bool, elapsed: Duration) -> bool`, `const PULL_DEADLINE: Duration = Duration::from_secs(8)`
  - webview2 `Capture::device_lost(&self) -> Option<String>`

Build notes: CEF and WebView2 build and test on Windows. WebKit builds in WSL only (Global Constraints, "Engine build facts"). Servo does not build here: verified by reading only, and the report says so.

- [ ] **Step 1: Write the failing tests.**

CEF `jar_cookies.rs` tests module:

```rust
    #[test]
    fn a_pull_gives_up_when_nothing_comes_or_when_it_stalls_part_way() {
        let s = Duration::from_secs;
        assert!(!given_up(false, false, s(1)), "still waiting for the first cookie");
        assert!(given_up(false, false, EMPTY_PULL_TIMEOUT), "a silent store");
        assert!(!given_up(false, true, s(3)), "cookies arriving: keep waiting");
        assert!(given_up(false, true, PULL_DEADLINE), "a visitor that stopped part way");
        assert!(given_up(true, false, s(0)), "the read never started");
    }
```

CEF `main.rs` (add `#[cfg(test)] mod tests` at the end if the file has none):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shutdown_waits_for_the_close_then_gives_up() {
        assert_eq!(shutdown_due(Duration::ZERO, true), Some(ShutdownPath::Clean));
        assert_eq!(shutdown_due(Duration::from_secs(1), false), None);
        assert_eq!(shutdown_due(SHUTDOWN_GRACE, false), Some(ShutdownPath::Forced));
    }
}
```

WebKit `main.rs` tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_is_sent_only_when_it_changed_and_the_interval_passed() {
        let last = SentFrame { size: (2, 1), pixels: vec![1; 8] };
        let later = FRAME_INTERVAL;
        assert!(frame_due((2, 1), &[1; 8], None, Duration::ZERO), "the first frame always goes");
        assert!(!frame_due((2, 1), &[1; 8], Some(&last), later), "unchanged");
        assert!(frame_due((2, 1), &[2; 8], Some(&last), later), "changed pixels");
        assert!(frame_due((1, 2), &[1; 8], Some(&last), later), "changed size, same bytes");
        assert!(!frame_due((2, 1), &[2; 8], Some(&last), Duration::from_millis(5)), "too soon");
    }
}
```

- [ ] **Step 2: Run** `cargo test -j 8 -p manifold-engine-cef given_up` and `shutdown_waits`. Expected: compile errors. If the CEF test binary cannot start because `libcef.dll` is not beside it, report that and rely on `cargo check -j 8 -p manifold-engine-cef --tests` plus reading; do not copy DLLs around to make it run. The WebKit test runs in WSL: `CARGO_TARGET_DIR=target-linux cargo test -j 8 -p manifold-engine-webkit frame_due` via the `wsl.exe` line in "Engine build facts".

- [ ] **Step 3: CEF cookie pull deadline** (`jar_cookies.rs`):

```rust
/// The most a pull may stay unanswered, even while cookies are arriving:
/// a visitor released part way never completes. Under the host's own 10 s
/// pull timeout, so the engine resolves first.
const PULL_DEADLINE: Duration = Duration::from_secs(8);

/// Whether a pull with no complete answer is given up: the read never
/// started, nothing came within `EMPTY_PULL_TIMEOUT`, or `PULL_DEADLINE` passed.
fn given_up(failed: bool, seen_any: bool, elapsed: Duration) -> bool {
    failed || (!seen_any && elapsed >= EMPTY_PULL_TIMEOUT) || elapsed >= PULL_DEADLINE
}
```

and in `poll`:

```rust
        let seen_any = !self.seen.borrow().is_empty();
        // Some cookies seen means this host holds cookies: a stalled pull is silence, never `[]`.
        no_callback_answer(given_up(self.failed, seen_any, self.started.elapsed()), may_hold_cookies || seen_any)
```

Update `poll`'s doc to mention `PULL_DEADLINE`.

- [ ] **Step 4: CEF frame buffer and shutdown** (`main.rs`).

Make the buffer optional. At ~236 keep the first `create` fatal (no engine works without it) and wrap it: `let mut frame_buffer = Some(ipc_protocol::SharedFrameBuffer::create(...).expect(...));`. At the frame site:

```rust
        if visible {
            if let Some(frame) = latest_frame.borrow_mut().take() {
                if frame_buffer.is_none() || buffer_size != (frame.width, frame.height) {
                    // Unmapped before `create` truncates the file by name.
                    drop(frame_buffer.take());
                    frame_buffer = match ipc_protocol::SharedFrameBuffer::create(&socket_id, frame.width, frame.height) {
                        Ok(buffer) => Some(buffer),
                        Err(e) => {
                            // The host still maps the old file (stalled on upload): retried on the next frame.
                            eprintln!("frame buffer: recreate at {}x{} failed, retrying: {e}", frame.width, frame.height);
                            None
                        }
                    };
                    buffer_size = (frame.width, frame.height);
                }
                if let Some(buffer) = frame_buffer.as_mut() {
                    buffer.write_frame(&frame.bgra);
                    frame_id += 1;
                    let _ = control.send(&EngineToHost::FrameReady {
                        frame_id,
                        width: frame.width,
                        height: frame.height,
                        damage: None,
                        pixels: None,
                    });
                } else {
                    // Kept for the retry unless a newer paint already replaced it.
                    latest_frame.borrow_mut().get_or_insert(frame);
                }
            }
        }
```

The retry runs every tick (8 ms) while the host holds the mapping; `create` itself waits up to 200 ms per attempt, so the log line repeats at most a few times a second. If that proves noisy in the live check, log only the first failure in a row with a `bool`.

Shutdown. Replace `let mut shutting_down = false;` (~245) with `let mut shutting_down: Option<Instant> = None;`, set `shutting_down = Some(Instant::now());` in the `Shutdown` arm (only if it is `None`, so a repeated `Shutdown` does not restart the clock), and replace the end-of-loop check:

```rust
        if let Some(since) = shutting_down {
            match shutdown_due(since.elapsed(), *closed.borrow()) {
                Some(ShutdownPath::Clean) => {
                    cef::shutdown();
                    return;
                }
                Some(ShutdownPath::Forced) => {
                    // `cef::shutdown()` with the browser still alive can hang inside libcef:
                    // leave instead, and let the job object take the subprocesses.
                    eprintln!("shutdown: no on_before_close after {SHUTDOWN_GRACE:?}; exiting");
                    std::process::exit(0);
                }
                None => {}
            }
        }
```

with, at module level:

```rust
/// How long a `Shutdown` waits for `on_before_close` before the host exits
/// anyway. It matters when manifold died: `spawn_host_reader` synthesizes
/// the `Shutdown`, and a host that waited for ever would be the orphan.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);

#[derive(Debug, PartialEq, Eq)]
enum ShutdownPath {
    /// The browser closed: CEF's documented order, then `cef::shutdown()`.
    Clean,
    /// It never did within `SHUTDOWN_GRACE`.
    Forced,
}

fn shutdown_due(elapsed: Duration, closed: bool) -> Option<ShutdownPath> {
    if closed {
        Some(ShutdownPath::Clean)
    } else if elapsed >= SHUTDOWN_GRACE {
        Some(ShutdownPath::Forced)
    } else {
        None
    }
}
```

Check what else reads `shutting_down` (grep it) and adapt each use to `shutting_down.is_some()`. `Instant` is already imported if the file uses `Instant::now()` elsewhere; add it to the `std::time` import otherwise.

- [ ] **Step 5: Servo frame buffer** (`main.rs`, reading only). Make `frame_buffer` an `Option<SharedFrameBuffer>` (first creation ~205 stays fatal: wrap it in `Some(...)`). In the `Resize` arm:

```rust
                    drop(frame_buffer.take());
                    frame_buffer = SharedFrameBuffer::create(&socket_id, w, h)
                        .inspect_err(|e| eprintln!("frame buffer: recreate at {w}x{h} failed, retrying: {e}"))
                        .ok();
```

At the site that writes a frame, retry first and skip the frame while it still fails:

```rust
                if frame_buffer.is_none() {
                    frame_buffer = SharedFrameBuffer::create(&socket_id, width, height).ok();
                }
                if let Some(buffer) = frame_buffer.as_mut() {
                    // (the existing write and FrameReady send, unchanged)
                }
```

Find the write site with `grep -n "write_frame" crates/manifold-engine-servo/src/main.rs`.

- [ ] **Step 6: WebView2 device loss.** In `capture.rs`, add a field `winrt_device: windows::Graphics::DirectX::Direct3D11::IDirect3DDevice` to `Capture`, stored from `new` (the local `winrt_device` already exists there). `resize` uses it:

```rust
    pub fn resize(&mut self, width: u32, height: u32) {
        let size = windows::Graphics::SizeInt32 { Width: width as i32, Height: height as i32 };
        // A lost device fails here; `device_lost` reports it on the next tick.
        let _ = self.frame_pool.Recreate(&self.winrt_device, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, size);
    }

    /// Why the D3D device was removed (eGPU unplugged, driver update, TDR),
    /// or `None` while it works. Capture can't continue on a removed device.
    pub fn device_lost(&self) -> Option<String> {
        // SAFETY: a plain COM query on a live device interface.
        unsafe { self.d3d_device.GetDeviceRemovedReason() }.err().map(|e| e.message())
    }
```

In `main.rs`'s loop, next to `try_get_latest_frame` (~487), check about once a second (keep a `last_device_check: Instant`):

```rust
            if last_device_check.elapsed() >= Duration::from_secs(1) {
                last_device_check = Instant::now();
                if let Some(reason) = capture.device_lost() {
                    let _ = control.send(&EngineToHost::Crashed { reason: format!("graphics device lost: {reason}") });
                    return;
                }
            }
```

Check `e.message()`'s type in windows 0.62 (`windows_core::Error::message() -> String`); if it returns `HSTRING`, use `.to_string()`. Return the way the loop's other exit paths do: if the loop ends with cleanup (dropping the frame buffer file, Task 3), `break` to it instead of `return`.

- [ ] **Step 7: WebKit frames** (`main.rs`):

```rust
/// What was last sent: the next frame goes only if it differs.
struct SentFrame {
    size: (u32, u32),
    pixels: Vec<u8>,
}

/// At most one frame per this, as WebView2's own `FRAME_INTERVAL`: about 30 fps.
const FRAME_INTERVAL: Duration = Duration::from_millis(33);

/// Whether a frame of `size` and `pixels` should be sent: it differs from
/// `last` (or nothing was sent yet) and `FRAME_INTERVAL` has passed.
fn frame_due(size: (u32, u32), pixels: &[u8], last: Option<&SentFrame>, since_sent: Duration) -> bool {
    match last {
        None => true,
        Some(last) => since_sent >= FRAME_INTERVAL && (last.size != size || last.pixels != pixels),
    }
}
```

In the loop, with `let mut last_sent: Option<SentFrame> = None;` and `let mut sent_at = Instant::now();` before it:

```rust
            if let Some(pixbuf) = offscreen.pixbuf() {
                let (pw, ph) = (pixbuf.width() as u32, pixbuf.height() as u32);
                if pw > 0 && ph > 0 {
                    let pixels = tightly_packed_rgba(&pixbuf);
                    if frame_due((pw, ph), &pixels, last_sent.as_ref(), sent_at.elapsed()) {
                        frame_id += 1;
                        let _ = to_host.send(&EngineToHost::FrameReady {
                            frame_id,
                            width: pw,
                            height: ph,
                            damage: None,
                            pixels: Some(pixels.clone()),
                        });
                        last_sent = Some(SentFrame { size: (pw, ph), pixels });
                        sent_at = Instant::now();
                    }
                }
            }
```

In the `HostToEngine::SetVisible(v)` arm (~332), after `visible = v;` add `if v { last_sent = None; }` so a tab shown again gets a frame at once. Keep the long comment above the block; append one line: "Unchanged frames are not sent (`frame_due`)." The clone costs one copy of a frame only when it changed, against a full send every tick before.

- [ ] **Step 8: CEF config.**
  - `Cargo.toml:9`: `features = ["build-util", "resources"]`. Edit it with the Edit tool so any BOM stays.
  - `main.rs:153-157`: the comment becomes "The `sandbox` cargo feature is off (it needs a separate bootstrap process this project doesn't bundle), so CEF must be told not to expect one, or it CHECK()-fails at startup. Renderers run unsandboxed: README, Known limitations."
  - `app.rs:43-47`:

```rust
            command_line.append_switch_with_value(Some(&"enable-logging".into()), Some(&"stderr".into()));
            // Chromium's verbose log is large (it filled the per-tab logs on a nearly full disk):
            // only on request.
            if std::env::var_os("MANIFOLD_CEF_VERBOSE").is_some_and(|v| v == "1") {
                command_line.append_switch_with_value(Some(&"v".into()), Some(&"1".into()));
            }
```

  - `README.md` "Known limitations": add a bullet: "**Blink (CEF) tabs run unsandboxed.** The CEF host starts Chromium with `--no-sandbox`: a renderer exploit in a CEF tab runs with the user's rights. WebView2 tabs keep Edge's sandbox. Enabling it needs CEF's sandbox bootstrap bundled with the host. `MANIFOLD_CEF_VERBOSE=1` turns Chromium's verbose log back on."

- [ ] **Step 9: Job object result** (`process_manager.rs` `engine_job`):

```rust
                let set = SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if set == 0 {
                    // Engines then outlive a crashed manifold: say so once.
                    eprintln!("engine job: kill-on-close not set: {}", std::io::Error::last_os_error());
                }
```

- [ ] **Step 10: Verify.**
  - `cargo test -j 8 -p manifold-engine-cef` (or `check --tests`, see Step 2), `cargo clippy -j 8 -p manifold-engine-cef -- -D warnings`.
  - `cargo clippy -j 8 -p manifold-engine-webview2 --all-targets -- -D warnings`.
  - WebKit in WSL: `cargo test -j 8 -p manifold-engine-webkit frame_due` with `CARGO_TARGET_DIR=target-linux`, or reading only.
  - The full gate.
  - Live: rebuild the CEF host (`cargo build -j 8 -p manifold-engine-cef`) and manifold, launch at BelowNormal, open a CEF tab, resize the window several times quickly (frame buffer path), then close the tab and check in Task Manager that no `manifold-engine-cef` process remains after 5 s. Check the per-tab log (`%TEMP%\manifold-<tab>*.log`) is much smaller than before for the same page. Kill only your PID.

- [ ] **Step 11: Commit**

```bash
git add crates/manifold-engine-webkit/src/main.rs crates/manifold-engine-cef crates/manifold-engine-servo/src/main.rs crates/manifold-engine-webview2/src crates/manifold/src/process_manager.rs README.md
git commit -m "Engines: WebKit sends only changed frames; CEF and Servo retry a failed frame buffer; WebView2 reports a lost device; CEF shutdown and cookie pulls are bounded; unused sandbox feature dropped and documented

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 24: App I/O off the hot path: settings saves debounced, one content-dir scan per restore, a crash state without URLs, Settings page disk reads cached

**Findings:**
- **B-R4 (low-med).** `event_loop.rs` `WindowEvent::Resized` (~252-280) and `Moved` (~281-287) call `save_settings` (`main.rs` ~1822: a fingerprint read plus a file write) on every event. A window drag makes dozens of synchronous writes a second on the UI thread, worse with Defender scanning. Mark the settings due and flush after 500 ms of quiet, and at exit.
- **B-R5 (low-med).** `main.rs` `resolve_content_path` (263-275) does a `read_dir` of the whole content dir, and it is called per Image/Pdf/Json node (`session_sync.rs` ~579-581, in `insert_materialized`). Restore is then O(nodes × files). Build one index per restore or apply pass.
- **B-R8, privacy half (low).** `crash_report.rs` keeps the focused tab's full URL as the "last state" (`event_loop.rs` ~438-449), and a crash writes it to `%TEMP%\manifold-crash-<ts>.log`. A query string can carry a token. Record the origin only, and replace the stored state only when it changed. (The per-frame cost half is dropped; see Dropped findings.)
- **C-P4 (low-med).** The Settings page touches the disk every frame while open:
  - `settings_page.rs:410` `session::list_named()` (a `read_dir`);
  - `:270` `engine_runtime_dir(kind).exists()` per engine;
  - `:286` `has_leftovers` (several `exists()`).
  
  Cache them for a few seconds as `cached_dir_size` (780-791) already does, and forget the cache when the page itself asks for a change (Save As, Delete, Remove, Clear leftovers).

**Files:**
- Modify: `crates/manifold/src/main.rs` (new `App` field `settings_save_due: Option<Instant>`, its `Default`; new `note_settings_changed`, `flush_settings_if_due`; `ContentIndex`; `resolve_content_path`; new `content_index` field)
- Modify: `crates/manifold/src/event_loop.rs` (`Resized`, `Moved`, `about_to_wait`, `exiting`, the `record_state` call)
- Modify: `crates/manifold/src/session_sync.rs` (`restore_session`, `apply_remote`, `materialize_imported`, `insert_materialized`)
- Modify: `crates/manifold/src/fetched.rs` (the `resolve_content_path` test, ~779-798)
- Modify: `crates/manifold/src/crash_report.rs`
- Modify: `crates/manifold/src/settings_page.rs` (new `cached`, `forget_disk_cache`; `cached_dir_size`; the three call sites; tests)

**Interfaces:**
- Consumes: `favicon_fetch::url_origin` (userinfo-free after Task 22).
- Produces:
  - `App::note_settings_changed(&mut self)` and `App::flush_settings_if_due(&mut self, now: Instant) -> bool`; `const SETTINGS_SAVE_DELAY: Duration = Duration::from_millis(500)`
  - `pub(crate) struct ContentIndex` with `ContentIndex::scan(dir: Option<&Path>) -> Self` and `ContentIndex::resolve(&self, id: &str, stored: &str) -> String`; `App.content_index: Option<ContentIndex>`
  - `crash_report::redacted_url(url: &str) -> String`
  - `settings_page::cached<T: Clone + Send + Sync + 'static>(ctx: &egui::Context, key: impl std::hash::Hash, max_age: Duration, compute: impl FnOnce() -> T) -> T` and `settings_page::forget_disk_cache(ctx: &egui::Context)`

- [ ] **Step 1: Write the failing tests.**

In `event_loop.rs`'s or `main.rs`'s test module (wherever `App::default()` tests live; `grep -n "App::default()" crates/manifold/src/main.rs` finds them):

```rust
    #[test]
    fn window_geometry_is_saved_after_a_quiet_spell_not_per_event() {
        let mut app = App::default();
        let t0 = std::time::Instant::now();
        app.note_settings_changed();
        assert!(!app.flush_settings_if_due(t0), "not at once");
        app.note_settings_changed();
        assert!(app.flush_settings_if_due(t0 + SETTINGS_SAVE_DELAY * 2));
        assert!(!app.flush_settings_if_due(t0 + SETTINGS_SAVE_DELAY * 3), "flushed once");
    }
```

`App::default()` has no profile (`profile::dir()` is `None` under test), so the flush's `save_settings` writes nothing.

In `fetched.rs`, turn `a_content_file_resolves_to_this_machines_newest_copy_first` into a test of `ContentIndex`, keeping every assertion. Replace each `resolve_content_path(Some(&dir), id, stored)` with `crate::ContentIndex::scan(Some(&dir)).resolve(id, stored)`, and `resolve_content_path(None, ...)` with `crate::ContentIndex::scan(None).resolve(...)`. Rescan after each `write_aged`, as the old calls did. Add:

```rust
    #[test]
    fn one_index_answers_every_node_of_a_pass() {
        use crate::content::tests::write_aged;
        let dir = crate::store::testutil::temp_dir("content_index");
        write_aged(&dir.join("5-9.png"), std::time::Duration::from_secs(60));
        write_aged(&dir.join("5-9.jpg"), std::time::Duration::ZERO);
        write_aged(&dir.join("5-10.json"), std::time::Duration::ZERO);
        let index = crate::ContentIndex::scan(Some(&dir));
        assert_eq!(index.resolve("5-9", "x"), dir.join("5-9.jpg").to_string_lossy(), "the newest wins");
        assert_eq!(index.resolve("5-10", "x"), dir.join("5-10.json").to_string_lossy());
        assert_eq!(index.resolve("5-1", "x"), "x", "an id that is a prefix of another's isn't matched");
    }
```

In `crash_report.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recorded_url_keeps_only_its_origin() {
        assert_eq!(redacted_url("https://u:p@Mail.example/inbox?token=abc#x"), "https://mail.example");
        assert_eq!(redacted_url("data:text/html,secret"), "data:");
        assert_eq!(redacted_url(""), "");
    }
}
```

In `settings_page.rs` tests:

```rust
    #[test]
    fn a_cached_disk_read_runs_once_until_forgotten() {
        let ctx = egui::Context::default();
        let calls = std::cell::Cell::new(0);
        let read = || {
            calls.set(calls.get() + 1);
            vec!["a".to_string()]
        };
        let hour = std::time::Duration::from_secs(3600);
        assert_eq!(cached(&ctx, "names", hour, read), vec!["a".to_string()]);
        assert_eq!(cached(&ctx, "names", hour, read), vec!["a".to_string()]);
        assert_eq!(calls.get(), 1);
        forget_disk_cache(&ctx);
        cached(&ctx, "names", hour, read);
        assert_eq!(calls.get(), 2);
    }
```

- [ ] **Step 2: Run** `cargo test -j 8 -p manifold --bins quiet_spell`, `content_file_resolves`, `one_index_answers`, `keeps_only_its_origin`, `cached_disk_read`. Expected: compile errors.

- [ ] **Step 3: Debounced settings** (`main.rs`). Field in `App` beside `settings_fingerprint`: `settings_save_due: Option<std::time::Instant>,` with `settings_save_due: None,` in `Default`. Then:

```rust
/// How long settings wait after the last change before they are written:
/// a window drag sends dozens of moves a second.
const SETTINGS_SAVE_DELAY: std::time::Duration = std::time::Duration::from_millis(500);

impl App {
    /// Settings changed (window size or position): written once things
    /// are quiet for `SETTINGS_SAVE_DELAY`, or at exit.
    pub(crate) fn note_settings_changed(&mut self) {
        self.settings_save_due = Some(std::time::Instant::now() + SETTINGS_SAVE_DELAY);
    }

    /// Writes settings if a change is due by `now`. Returns whether it wrote.
    pub(crate) fn flush_settings_if_due(&mut self, now: std::time::Instant) -> bool {
        match self.settings_save_due {
            Some(due) if due <= now => {
                self.settings_save_due = None;
                self.save_settings();
                true
            }
            _ => false,
        }
    }
}
```

Put the two methods in an existing `impl App` block near `save_settings`. In `event_loop.rs`:
- `Resized` and `Moved`: replace `self.save_settings();` with `self.note_settings_changed();`.
- `about_to_wait`, at its end: `self.flush_settings_if_due(std::time::Instant::now());`. The loop wakes at least every idle frame or `MINIMIZED_TICK`, so a due save lands within about a second.
- `exiting`, first line: `if self.settings_save_due.is_some() { self.settings_save_due = None; self.save_settings(); }`.

Every other `save_settings()` call (user actions in Settings) stays immediate.

- [ ] **Step 4: `ContentIndex`** (`main.rs`, replacing `resolve_content_path`'s body and keeping its doc on the struct):

```rust
/// Where nodes' files are on this machine (spec "Where fetched files
/// live"): this machine's own fetch in the content dir (`<id>.<ext>`, the
/// newest if a Refresh changed the extension), which a Refresh replaces
/// without touching the shared `path`; else the stored path. A missing
/// stored path shows as missing, and `session_sync::materialize` refetches
/// it when the node has a URL. An unfinished `.tmp` save is never picked.
/// One directory read answers a whole restore or apply pass.
#[derive(Debug, Default)]
pub(crate) struct ContentIndex(HashMap<String, (std::time::SystemTime, std::path::PathBuf)>);

impl ContentIndex {
    pub(crate) fn scan(dir: Option<&std::path::Path>) -> Self {
        let mut newest: HashMap<String, (std::time::SystemTime, std::path::PathBuf)> = HashMap::new();
        let entries = dir.and_then(|dir| std::fs::read_dir(dir).ok()).into_iter().flatten().flatten();
        for e in entries {
            let name = e.file_name().to_string_lossy().into_owned();
            let Some((id, _)) = name.split_once('.') else { continue };
            if name.ends_with(".tmp") {
                continue;
            }
            let modified = e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
            match newest.get(id) {
                Some((at, _)) if *at >= modified => {}
                _ => {
                    newest.insert(id.to_string(), (modified, e.path()));
                }
            }
        }
        Self(newest)
    }

    pub(crate) fn resolve(&self, id: &str, stored: &str) -> String {
        self.0.get(id).map_or_else(|| stored.to_string(), |(_, path)| path.to_string_lossy().into_owned())
    }
}
```

Match the old tie-break: `max_by_key` kept the *last* of equal maxima, and a file with no readable time sorted lowest (`None < Some`). With `*at >= modified` the first of equal times wins instead. Equal mtimes for two extensions of one node don't happen in practice (a Refresh writes later); note the difference in the commit message rather than chase it.

Remove `resolve_content_path`. Add the `App` field `content_index: Option<ContentIndex>,` (doc: "Set for the length of one restore or apply pass, so each node's file lookup doesn't read the content dir again"), `content_index: None,` in `Default`. In `session_sync.rs` `insert_materialized`:

```rust
        let content_path = matches!(kind, Kind::Image | Kind::Pdf | Kind::Json).then(|| {
            let stored = str_of(fields, field::PATH);
            match &self.content_index {
                Some(index) => index.resolve(&id, &stored),
                None => crate::ContentIndex::scan(crate::profile::content_dir().as_deref()).resolve(&id, &stored),
            }
        });
```

and around each pass that materializes many nodes:
- `restore_session`, before the jar loop: `self.content_index = Some(crate::ContentIndex::scan(crate::profile::content_dir().as_deref()));`, and `self.content_index = None;` before each `return` after it and at its end. Simplest: wrap the body from the jar loop to the end in a local closure-free block and clear it after, or set it at the top and clear it at every exit; there are two exits (`return false` and the end).
- `apply_remote` (~729): set before the loop over `changes`, clear after.
- `materialize_imported` (~1396): set before its loop, clear after.

A refetch that lands mid-pass writes a new `<id>.<ext>` the index doesn't know about, which is fine: the refetch result goes through `reload_fetched_node`, not this lookup.

- [ ] **Step 5: Crash state** (`crash_report.rs`):

```rust
/// Refreshes `LAST_STATE` when `state` differs from it. Called once per frame.
pub fn record_state(state: impl std::fmt::Display) {
    let state = state.to_string();
    if let Ok(mut guard) = LAST_STATE.lock() {
        if guard.as_deref() != Some(state.as_str()) {
            *guard = Some(state);
        }
    }
}

/// `url` as a crash report may keep it: the origin only (no path, query,
/// fragment or userinfo, any of which can carry a token), or just the
/// scheme of a URL with no `://`.
pub fn redacted_url(url: &str) -> String {
    crate::favicon_fetch::url_origin(url)
        .unwrap_or_else(|| url.split_once(':').map_or(String::new(), |(scheme, _)| format!("{scheme}:")))
}
```

Keep the rest of `record_state`'s doc. In `event_loop.rs` (~443) the focused-tab line becomes `.map(|t| format!("{:?} {}", t.tab.kind, crash_report::redacted_url(&t.url)))`. Update the module doc's `LAST_STATE` paragraph: "the focused tab's origin, never its full URL".

- [ ] **Step 6: Settings page cache** (`settings_page.rs`):

```rust
/// The key all disk-read caches on this page share a namespace under, so
/// `forget_disk_cache` clears them together.
const DISK_CACHE: &str = "settings_disk_cache";

/// `compute()`, recomputed at most every `max_age` (egui temp memory): the
/// page redraws every frame, and these are disk reads.
fn cached<T: Clone + Send + Sync + 'static>(
    ctx: &egui::Context,
    key: impl std::hash::Hash,
    max_age: std::time::Duration,
    compute: impl FnOnce() -> T,
) -> T {
    let id = egui::Id::new((DISK_CACHE, key));
    let generation = ctx.data(|d| d.get_temp::<u64>(egui::Id::new(DISK_CACHE)).unwrap_or(0));
    let now = std::time::Instant::now();
    if let Some((at, seen, value)) = ctx.data(|d| d.get_temp::<(std::time::Instant, u64, T)>(id)) {
        if seen == generation && now.duration_since(at) < max_age {
            return value;
        }
    }
    let value = compute();
    ctx.data_mut(|d| d.insert_temp(id, (now, generation, value.clone())));
    value
}

/// Makes every `cached` value recompute on its next read: after this page
/// asked for Save As, Delete, Remove or Clear leftovers.
fn forget_disk_cache(ctx: &egui::Context) {
    ctx.data_mut(|d| {
        let id = egui::Id::new(DISK_CACHE);
        let next = d.get_temp::<u64>(id).unwrap_or(0) + 1;
        d.insert_temp(id, next);
    });
}
```

A generation counter, not removal: egui's temp map can't remove by key prefix. Then:
- `cached_dir_size` becomes `cached(ctx, ("engine_dir_size", dir), Duration::from_secs(3), || crate::engine_install::dir_size(dir))`. Keep its doc.
- `:270`: `let runtime = cached(ui.ctx(), ("engine_runtime", kind), Duration::from_secs(3), || crate::process_manager::engine_runtime_dir(kind).filter(|dir| dir.exists()));`. `EngineKind` must be `Hash`; if it isn't, key on `engine_label(kind)` or `kind as u8`.
- `:286`: wrap the `engine_hosts_dir().is_some_and(...)` expression in `cached(ui.ctx(), "engine_leftovers", Duration::from_secs(3), || ...)`.
- `:410`: `let names = cached(ui.ctx(), "named_sessions", Duration::from_secs(3), session::list_named);`.
- Where the page sets `req.save_as_requested = true`, sets a delete or remove request, or starts Clear leftovers: also call `forget_disk_cache(ui.ctx())`. Grep `req\.` in `settings_page.rs` for those sites.

During an install, `EngineState::Installing` drives its own progress line, not these caches, so the 3 s staleness shows only in the size and leftovers lines.

- [ ] **Step 7: Run** the Step 2 tests, then `cargo test -j 8 -p manifold --bins settings_page session_sync fetched crash_report`. Expected: PASS.

- [ ] **Step 8: Live check.** Rebuild, launch at BelowNormal, drag the window around for a few seconds, release, wait a second, quit. Relaunch: the window comes back at the dragged position. Open Settings and confirm saved sessions list and engine lines render as before. Kill only your PID.

- [ ] **Step 9: Full gate, then commit.**

```bash
git add crates/manifold/src/main.rs crates/manifold/src/event_loop.rs crates/manifold/src/session_sync.rs crates/manifold/src/fetched.rs crates/manifold/src/crash_report.rs crates/manifold/src/settings_page.rs
git commit -m "Perf: window geometry saved after a quiet spell; one content-dir scan per restore pass; crash state keeps origins only; Settings page caches its disk reads

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 25: Graph and popup rendering: notes borrowed, long history windowed, the dot grid as one mesh, truncation without per-call allocation

**Findings:**
- **C-P1, cheap half (med).** `card.rs` (~1463) clones every visible note's text every frame before `CommonMarkViewer::show`. Borrow it: edition 2021 closures capture `self.commonmark_cache` disjointly from `self.nodes`. (Caching the parse or layout is deferred; see Deferred.)
- **C-P5 (low-med).** `popups.rs` `build_history_popup` (~763-816) runs `filter_matches` and `list_row` for every history entry every frame while open, thousands of rows. Cache the filtered index list and lay out only the visible rows once the list is long.
- **C-P6 (low).** `graph_ui.rs` `draw_graph_dot_grid` (~1475-1498) emits about 5,000 `circle_filled` shapes a frame at the 20 px floor on 1920×1080. Build one mesh from a tessellated template dot.
- **C-P7 (low).**
  - `main.rs` `truncate_to_width` (~441-485) builds its cache key with `text.to_owned()` on every call, a hit included.
  - `card.rs` `truncate_start_to_width` (38-51) builds a reversed `String` on every call. Reversing by `char` also splits combining marks and ZWJ sequences, so the width it measures is not what is drawn.
  - Fix: key on a hash of the text and keep the text in the value to confirm a hit; cut on grapheme boundaries; truncate from the start by measuring true suffixes. (The font-family key half is dropped; see Dropped findings.)

**Files:**
- Modify: `crates/manifold/src/card.rs` (~1450-1470, `truncate_start_to_width`)
- Modify: `crates/manifold/src/main.rs` (`TruncateCacheKey`, `truncate_to_width`, new `truncate_core`, `App.truncate_cache`'s type)
- Modify: `crates/manifold/src/popups.rs` (`build_history_popup`, `list_row_nav`, new `history_matches`, `HISTORY_WINDOW_FROM`)
- Modify: `crates/manifold/src/main.rs` (new `App` field `history_matches_cache`)
- Modify: `crates/manifold/src/graph_ui.rs` (`draw_graph_dot_grid`, new `grid_spacing`, `dot_grid_mesh`)

**Interfaces:**
- Produces:
  - `type TruncateCacheKey = (u64, u32, i32, bool)`: text hash, font size bits, width bucket, from-start. `App.truncate_cache: HashMap<TruncateCacheKey, (String, String)>` (text, cut).
  - `fn truncate_core(cache, ctx, text: &str, font_id: &egui::FontId, max_width: f32, from_start: bool) -> String`; `truncate_to_width(..)` and `truncate_start_to_width(..)` keep their signatures apart from the cache's value type.
  - `popups::history_matches(history: &[HistoryEntry], filter: &str) -> Vec<usize>` (newest first) and `App.history_matches_cache: Option<(String, usize, Vec<usize>)>`.
  - `graph_ui::grid_spacing(zoom_scale: f32) -> f32`, `graph_ui::dot_grid_mesh(template: &egui::Mesh, visible: egui::Rect, spacing: f32, scale: f32) -> egui::Mesh`.

- [ ] **Step 1: Write the failing tests.**

In `main.rs`'s test module (create one if absent):

```rust
    fn with_fonts(f: impl FnOnce(&egui::Context)) {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| f(ui.ctx()));
    }

    #[test]
    fn a_cut_from_the_start_keeps_a_true_suffix_on_grapheme_boundaries() {
        use unicode_segmentation::UnicodeSegmentation;
        with_fonts(|ctx| {
            let mut cache = HashMap::new();
            let font = egui::FontId::proportional(14.0);
            let text = format!("{}e\u{301}\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}tail.jpg", "dir\\".repeat(30));
            let cut = truncate_start_to_width(&mut cache, ctx, &text, &font, 80.0);
            let kept = cut.strip_prefix('\u{2026}').expect("cut at the start");
            assert!(text.ends_with(kept), "a true suffix, not a re-reversed one: {cut}");
            let start = text.len() - kept.len();
            assert!(text.grapheme_indices(true).any(|(i, _)| i == start), "starts on a grapheme boundary");
        });
    }

    #[test]
    fn a_cut_from_the_end_never_splits_a_grapheme() {
        use unicode_segmentation::UnicodeSegmentation;
        with_fonts(|ctx| {
            let mut cache = HashMap::new();
            let font = egui::FontId::proportional(14.0);
            let text = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}".repeat(40);
            let cut = truncate_to_width(&mut cache, ctx, &text, &font, 60.0);
            let kept = cut.strip_suffix('\u{2026}').expect("cut at the end");
            assert!(text.grapheme_indices(true).any(|(i, _)| i == kept.len()) || kept.is_empty());
        });
    }

    #[test]
    fn a_repeated_cut_is_one_cache_entry() {
        with_fonts(|ctx| {
            let mut cache = HashMap::new();
            let font = egui::FontId::proportional(14.0);
            let a = truncate_to_width(&mut cache, ctx, "a long title that will not fit", &font, 40.0);
            let b = truncate_to_width(&mut cache, ctx, "a long title that will not fit", &font, 40.0);
            assert_eq!((a, cache.len()), (b, 1));
            truncate_start_to_width(&mut cache, ctx, "a long title that will not fit", &font, 40.0);
            assert_eq!(cache.len(), 2, "start and end cuts are cached apart");
        });
    }
```

`ctx.run_ui(egui::RawInput::default(), |ui| ..)` is the form the existing UI tests use (`card.rs:2271`).

In `popups.rs` tests:

```rust
    #[test]
    fn history_matches_lists_filtered_entries_newest_first() {
        let entry = |title: &str, url: &str| crate::history::HistoryEntry { title: title.into(), url: url.into() };
        let history = vec![entry("Rust", "https://a"), entry("", "https://rust.example"), entry("Go", "https://b")];
        assert_eq!(history_matches(&history, ""), vec![2, 1, 0]);
        assert_eq!(history_matches(&history, "rust"), vec![1, 0]);
    }
```

`HistoryEntry` (`history.rs:9`) has exactly two fields, `title` and `url`.

In `graph_ui.rs` tests:

```rust
    #[test]
    fn grid_spacing_keeps_dots_20_to_80_screen_points_apart() {
        for zoom in [0.15_f32, 0.5, 1.0, 2.0, 7.0] {
            let on_screen = grid_spacing(zoom) * zoom;
            assert!((20.0..=80.0).contains(&on_screen), "zoom {zoom}: {on_screen}");
        }
        assert_eq!(grid_spacing(1.0), 40.0);
    }

    #[test]
    fn the_dot_grid_mesh_has_one_template_copy_per_grid_point() {
        let mut template = egui::Mesh::default();
        template.add_colored_rect(egui::Rect::from_center_size(egui::Pos2::ZERO, egui::vec2(2.0, 2.0)), egui::Color32::WHITE);
        let visible = egui::Rect::from_min_max(egui::pos2(-1.0, -1.0), egui::pos2(81.0, 41.0));
        let mesh = dot_grid_mesh(&template, visible, 40.0, 0.5);
        // x: -40, 0, 40, 80, 120 (floor/ceil like the old loop); y: -40, 0, 40, 80.
        assert_eq!(mesh.vertices.len(), template.vertices.len() * 5 * 4);
        assert_eq!(mesh.indices.len(), template.indices.len() * 5 * 4);
        let first: Vec<egui::Pos2> = mesh.vertices[..4].iter().map(|v| v.pos).collect();
        assert!(first.iter().all(|p| (p.x + 40.0).abs() <= 0.5 && (p.y + 40.0).abs() <= 0.5), "scaled by 0.5 around (-40,-40)");
    }
```

- [ ] **Step 2: Run** `cargo test -j 8 -p manifold --bins grapheme`, `repeated_cut`, `history_matches`, `grid_spacing`, `dot_grid_mesh`. Expected: compile errors; the two grapheme tests may also fail on today's code once it compiles.

- [ ] **Step 3: Truncation** (`main.rs`). Change the key and the cache value, and share one core:

```rust
/// `truncate_core`'s cache key: a hash of the text (the text itself is in
/// the value, compared on a hit, so a lookup allocates nothing), the font
/// size as bits, the width bucket, and which end is cut. Every call site
/// builds its `FontId` with `FontId::proportional`, so the family is not in
/// the key (see `truncate_to_width`).
type TruncateCacheKey = (u64, u32, i32, bool);

fn truncate_to_width(
    cache: &mut HashMap<TruncateCacheKey, (String, String)>,
    ctx: &egui::Context,
    text: &str,
    font_id: &egui::FontId,
    max_width: f32,
) -> String {
    truncate_core(cache, ctx, text, font_id, max_width, false)
}

/// `text` cut to fit `max_width` with "…" at the cut: at the end, or at the
/// start when `from_start` (a path keeps its file name). Cuts fall on
/// grapheme boundaries, so a ZWJ emoji or a combining mark is never split,
/// and the width measured is the width drawn. Memoized on `cache`.
fn truncate_core(
    cache: &mut HashMap<TruncateCacheKey, (String, String)>,
    ctx: &egui::Context,
    text: &str,
    font_id: &egui::FontId,
    max_width: f32,
    from_start: bool,
) -> String {
    use std::hash::{Hash, Hasher};
    use unicode_segmentation::UnicodeSegmentation;
    let bucketed_width = (max_width / TRUNCATE_WIDTH_BUCKET).floor() * TRUNCATE_WIDTH_BUCKET;
    let width_bucket = (max_width / TRUNCATE_WIDTH_BUCKET).floor() as i32;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    let key: TruncateCacheKey = (hasher.finish(), font_id.size.to_bits(), width_bucket, from_start);
    if let Some((cached_text, cut)) = cache.get(&key) {
        if cached_text == text {
            return cut.clone();
        }
    }
    let width_of = |s: &str| -> f32 {
        ctx.fonts_mut(|f| f.layout_no_wrap(s.to_owned(), font_id.clone(), egui::Color32::WHITE)).size().x
    };
    let result = if bucketed_width <= 0.0 {
        String::new()
    } else if width_of(text) <= bucketed_width {
        text.to_owned()
    } else {
        // Byte offsets of every grapheme boundary, both ends included.
        let mut bounds: Vec<usize> = text.grapheme_indices(true).map(|(i, _)| i).collect();
        bounds.push(text.len());
        let candidate = |k: usize| {
            if from_start {
                format!("\u{2026}{}", &text[bounds[bounds.len() - 1 - k]..])
            } else {
                format!("{}\u{2026}", &text[..bounds[k]])
            }
        };
        // The most graphemes `k` kept that still fit.
        let (mut lo, mut hi) = (0usize, bounds.len() - 1);
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if width_of(&candidate(mid)) <= bucketed_width {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        candidate(lo)
    };
    if cache.len() >= TRUNCATE_CACHE_CAP {
        cache.clear();
    }
    cache.insert(key, (text.to_owned(), result.clone()));
    result
}
```

`candidate(0)` is a lone "…", as before. The rest of `truncate_to_width`'s long doc stays on it; move the "binary search" sentence to `truncate_core`. `App.truncate_cache`'s type becomes `HashMap<TruncateCacheKey, (String, String)>`. Two `String`s are stored per entry, but only on a miss.

In `card.rs`:

```rust
/// `text` cut to `max_width` screen points at the start ("…\\Trips\\a.jpg"), so a path's file name
/// stays visible. Shares `truncate_to_width`'s cache.
fn truncate_start_to_width(
    cache: &mut HashMap<TruncateCacheKey, (String, String)>,
    ctx: &egui::Context,
    text: &str,
    font_id: &egui::FontId,
    max_width: f32,
) -> String {
    truncate_core(cache, ctx, text, font_id, max_width, true)
}
```

`card.rs` uses `use super::*;` (or names `truncate_to_width` through it); make `truncate_core` visible the same way `truncate_to_width` is.

- [ ] **Step 4: Note text borrowed** (`card.rs` ~1463):

```rust
            let text = self.nodes.get(id).and_then(|n| n.as_note()).map_or("", |n| n.text.as_str());
            let cache = &mut self.commonmark_cache;
            ui.scope_builder(
                egui::UiBuilder::new().max_rect(text_rect).layout(egui::Layout::top_down(egui::Align::LEFT)),
                |ui| {
                    ui.style_mut().visuals.override_text_color = Some(egui::Color32::from_black_alpha(220));
                    egui_commonmark::CommonMarkViewer::new().show(ui, cache, text);
                },
            );
```

`text` borrows `self.nodes` and `cache` borrows `self.commonmark_cache`: two disjoint fields, both through `self`. That compiles when `self` is `&mut App` and nothing else in this scope holds a borrow of the whole of `self`. Delete the comment block above that says the borrow checker forbids it, keeping its LOD sentences. If the compiler still refuses (a helper method call borrowing all of `self` in between), report the error rather than reintroducing the clone.

- [ ] **Step 5: History popup** (`popups.rs`, `main.rs`).

```rust
/// Above this many matching entries, the History popup lays out only the
/// rows in view. Shorter lists keep the plain layout (and wrapping arrow keys).
const HISTORY_WINDOW_FROM: usize = 100;

/// Indexes into `history` of the entries `filter` matches, newest first.
pub(crate) fn history_matches(history: &[crate::history::HistoryEntry], filter: &str) -> Vec<usize> {
    (0..history.len()).rev().filter(|&i| filter_matches(filter, &[&history[i].title, &history[i].url])).collect()
}
```

`App` field `history_matches_cache: Option<(String, usize, Vec<usize>)>` (doc: "The History popup's filter, the history length it was computed at, and its matches: recomputed only when either changes"), `None` in `Default`. History entries are only appended, cleared or removed whole, all of which change the length; an in-place title update keeps the length, so also set `self.history_matches_cache = None;` at each `self.history_dirty = true;` (`event_loop.rs:1588`, `main.rs:1753`, `popups.rs:805`).

In `build_history_popup`, replace the `ScrollArea` block:

```rust
                let key_ok = matches!(&self.history_matches_cache,
                    Some((f, n, _)) if *f == self.history_filter && *n == self.history.len());
                if !key_ok {
                    let matches = history_matches(&self.history, &self.history_filter);
                    self.history_matches_cache = Some((self.history_filter.clone(), self.history.len(), matches));
                }
                let matches = self.history_matches_cache.as_ref().map_or(&[][..], |(_, _, m)| m.as_slice());
                let row = |ui: &mut egui::Ui, i: usize, navigate_url: &mut Option<String>| {
                    let entry = &self.history[i];
                    let label = if entry.title.is_empty() { &entry.url } else { &entry.title };
                    // Keyed by the entry, not by position: a windowed list starts at a different row each
                    // scroll, and positional ids would move keyboard focus to another entry.
                    if ui.push_id(i, |ui| list_row(ui, label)).inner.clicked() {
                        *navigate_url = Some(entry.url.clone());
                    }
                };
                let scroll = egui::ScrollArea::vertical().max_height(280.0);
                if matches.len() < HISTORY_WINDOW_FROM {
                    scroll.show(ui, |ui| matches.iter().for_each(|&i| row(ui, i, &mut navigate_url)));
                } else {
                    let row_height = ui.text_style_height(&egui::TextStyle::Body);
                    scroll.show_rows(ui, row_height, matches.len(), |ui, range| {
                        matches[range].iter().for_each(|&i| row(ui, i, &mut navigate_url));
                    });
                    mark_list_partial(ui);
                }
```

The borrow checker needs `self.history_filter` (mutably borrowed by the `TextEdit` above) to be released before this; it is, since the `TextEdit` call ends. `row` borrows `self.history` immutably while `self.history_matches_cache` is read: both are shared borrows. If the closure-over-`self` conflicts with the `build_list_popup_frame` closure's own captures, inline `row` at both call sites.

`show_rows` lays out every row that intersects the view, a partly visible last one included, so Down from the last fully visible row reaches it, and `list_row`'s `scroll_to_me` reveals the next one on the following press.

Wrapping: in a windowed list, Up on the first laid-out row would wrap to the last *laid-out* row, not the last entry. Add, beside `list_rows_key`:

```rust
/// Marks this frame's list rows as a window of a longer list: `list_row_nav`
/// then stops at the ends instead of wrapping to the wrong row.
fn mark_list_partial(ui: &egui::Ui) {
    ui.data_mut(|d| d.insert_temp(list_rows_key(ui).with("partial"), true));
}
```

and in `list_row_nav`, after reading `rows`:

```rust
    let partial = ui.data_mut(|d| d.remove_temp::<bool>(key.with("partial"))).unwrap_or(false);
```

then replace the wrapping step:

```rust
    let next = if down {
        if at + 1 < rows.len() { Some(at + 1) } else if partial { None } else { Some(0) }
    } else if up {
        if at > 0 { Some(at - 1) } else if partial { None } else { Some(rows.len() - 1) }
    } else {
        None
    };
    if let Some(next) = next {
        ui.memory_mut(|m| m.request_focus(rows[next]));
    }
```

`list_rows_take_arrows_and_enter` and the other `list_row` tests must still pass unchanged (they don't mark partial).

Add a test with 300 entries that the popup lays out fewer than 300 rows. Use the existing popup test harness (`grep -n "fn list_frame\|build_history_popup" crates/manifold/src/popups.rs`); if no harness reaches `build_history_popup`, test `history_matches` only and verify the windowing live (Step 7).

- [ ] **Step 6: Dot grid** (`graph_ui.rs`):

```rust
/// World units between grid dots at `zoom_scale`: 40 at 100%, doubled or
/// halved to stay 20-80 screen points apart.
pub(crate) fn grid_spacing(zoom_scale: f32) -> f32 {
    let mut spacing = 40.0_f32;
    while spacing * zoom_scale < 20.0 {
        spacing *= 2.0;
    }
    while spacing * zoom_scale > 80.0 {
        spacing /= 2.0;
    }
    spacing
}

/// One mesh holding a copy of `template` (a dot tessellated around the
/// origin) at every grid point of `visible`, scaled by `scale`.
pub(crate) fn dot_grid_mesh(template: &egui::Mesh, visible: egui::Rect, spacing: f32, scale: f32) -> egui::Mesh {
    let (start_x, end_x) = ((visible.min.x / spacing).floor() as i32, (visible.max.x / spacing).ceil() as i32);
    let (start_y, end_y) = ((visible.min.y / spacing).floor() as i32, (visible.max.y / spacing).ceil() as i32);
    let mut mesh = egui::Mesh::default();
    for gx in start_x..=end_x {
        for gy in start_y..=end_y {
            let center = egui::vec2(gx as f32 * spacing, gy as f32 * spacing);
            let mut dot = template.clone();
            for v in &mut dot.vertices {
                v.pos = (v.pos.to_vec2() * scale + center).to_pos2();
            }
            mesh.append(dot);
        }
    }
    mesh
}
```

and `draw_graph_dot_grid`:

```rust
    pub(crate) fn draw_graph_dot_grid(&self, ui: &egui::Ui, zoom_scale: f32, inv_zoom: f32) {
        let dot_color = ui.visuals().weak_text_color().gamma_multiply(0.35);
        // One dot, tessellated at its on-screen size (1.5 points) so its
        // anti-aliased edge matches what `circle_filled` drew; scaled into
        // world units, the layer's zoom brings it back to 1.5 points.
        let mut tessellator = egui::epaint::Tessellator::new(
            ui.ctx().pixels_per_point(),
            ui.ctx().tessellation_options(|o| *o),
            [1, 1],
            Vec::new(),
        );
        let mut template = egui::Mesh::default();
        tessellator.tessellate_circle(egui::epaint::CircleShape::filled(egui::Pos2::ZERO, 1.5, dot_color), &mut template);
        let mesh = dot_grid_mesh(&template, self.camera.rect, grid_spacing(zoom_scale), inv_zoom);
        ui.painter().add(egui::Shape::mesh(mesh));
    }
```

Keep the doc comment, and replace its "turns into tens of thousands of `circle_filled` calls" sentence with: "One mesh, not one shape per dot: about 5,000 dots at the 20-point floor on a full-HD window." The empty prepared-disc list makes the tessellator draw a feathered polygon instead of egui's prerasterized disc, so the dots may differ by a fraction of a pixel. Compare a screenshot of the graph background before and after (`scripts/chrome-shots.sh` if it has a graph view, else the toolbar's Screenshot) and report whether a difference is visible. If one is, stop and report rather than tune it.

Check the names against epaint 0.36.2 while compiling: `Tessellator::new(pixels_per_point, options, font_tex_size, prepared_discs)`, `tessellate_circle(CircleShape, &mut Mesh)` and `Context::tessellation_options` exist with these shapes; `CircleShape::filled` and `Mesh::append` are the usual constructors.

- [ ] **Step 7: Run** the Step 2 tests, then `cargo test -j 8 -p manifold --bins popups card graph_ui`. Expected: PASS. Live: rebuild, launch at BelowNormal, check graph titles (long titles, a path card's start cut), a note card, the History popup with a long history (arrow keys walk past the visible rows, Enter navigates), and the dot grid at several zooms. Kill only your PID.

- [ ] **Step 8: Full gate, then commit.**

```bash
git add crates/manifold/src/card.rs crates/manifold/src/main.rs crates/manifold/src/popups.rs crates/manifold/src/graph_ui.rs
git commit -m "Render: note text borrowed; long history lists lay out only visible rows; dot grid is one mesh; truncation is cached without allocating and cuts on graphemes

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Phase D: behaviour-preserving refactors

Every task in this phase changes no behaviour. Its proof is the existing suite passing unchanged, plus the small tests named in the task. A test that has to change its expected value is a sign the refactor changed behaviour: stop and report.

### Task 26: Docs and dead code: misplaced doc comments, unused raw-frame and readiness code, the hook lints WebView2

**Findings:**
- **D-F4 (low).** `fetched.rs:5-6`: a sentence fragment left over from an edit. `tabs.rs:529-534`: a doc comment describes the function below the one it sits on; the constants and `use` lines at `tabs.rs:47-53` sit mid-file between items.
- **D-F4, also.** `pdf_fetch.rs:52-55` contradicts itself ("once per request after that, not once per request forever"); it means once per run. (`favicon_fetch.rs:123`'s "now-dead" citation is fixed in Task 22.)
- **E-F5 (low).** `ipc-protocol/src/channel.rs:5-16`: the `MessageChannel` doc sits on `MAX_MESSAGE_BYTES`. `transport.rs:151-161`: `listen_remote`'s doc sits on `TOKEN_LEN`. `transport.rs:23-26` mentions `send_raw`, removed below.
- **E-F5, also.** `messages.rs:24-47`: `KeyCode`'s doc sits on the `macro_rules!`, and the `key_codes!` macro drops attributes, so a doc comment on a key variant is a compile error waiting to happen. Pass `$(#[$m:meta])*` through.
- **E-F4, dead-code half (low).** `MessageChannel::recv_raw`/`send_raw` (`channel.rs:49-65`) have no callers in the workspace (`grep -rn "recv_raw\|send_raw" crates` lists only the definitions). CEF `app.rs:74-95` `is_cef_ready` is written and never read.
- **E-F7 (low).** `.githooks/pre-commit:7` lints only `manifold` and `ipc-protocol`, yet `manifold-engine-webview2` builds on Windows and Task 23 changes it. Add it, and say so in `CLAUDE.md`. CEF stays out: its build script needs the CEF download.
- Seen while verifying: `main.rs:118-122`: the swatch doc names a color list that moved. `main.rs:384-399`: the truncation doc still describes the old cache key (fixed in Task 25; check it reads right).

**Files:**
- Modify: `crates/manifold/src/fetched.rs:5-6`, `crates/manifold/src/tabs.rs` (529-534, 47-53), `crates/manifold/src/pdf_fetch.rs:52-55`, `crates/manifold/src/main.rs` (118-122)
- Modify: `crates/ipc-protocol/src/channel.rs` (5-16, 49-65), `crates/ipc-protocol/src/transport.rs` (23-26, 151-161), `crates/ipc-protocol/src/messages.rs` (`key_codes!`)
- Modify: `crates/manifold-engine-cef/src/app.rs` (74-95)
- Modify: `.githooks/pre-commit:7`, `CLAUDE.md` ("Formatting and lints")

**Interfaces:** removes `MessageChannel::recv_raw`, `MessageChannel::send_raw`, and the CEF `is_cef_ready` field. Nothing else changes.

- [ ] **Step 1: Prove the dead code is dead.** `grep -rn "recv_raw\|send_raw\|is_cef_ready" crates docs README.md`. Expected: only the definitions, the CEF field's own write (`app.rs:92`), and the `transport.rs` doc mention. If anything else appears, keep that item and report it.

- [ ] **Step 2: Write the macro test** (`messages.rs` tests). It compiles only once attributes pass through:

```rust
    mod attrs_pass_through {
        // A doc comment on a variant must compile: `key_codes!` forwards attributes.
        use serde::{Deserialize, Serialize};
        key_codes! {
            /// A documented key.
            KeyA 0x41 "a";
        }

        #[test]
        fn a_documented_variant_compiles() {
            assert_eq!(KeyCode::KeyA.dom_key(), "a");
        }
    }
```

Run `cargo test -j 8 -p ipc-protocol attrs_pass_through`. Expected: a "no rules expected the token `#`" error.

- [ ] **Step 3: The macro.** Move `KeyCode`'s doc (`messages.rs:24-40`) from above `macro_rules! key_codes` into the macro's `#[derive(...)] pub enum KeyCode` line as a `///` block emitted by the macro (the macro then documents the generated enum), leaving the macro a one-line `//` comment of its own. In `key_codes!`, the matcher becomes `($($(#[$m:meta])* $name:ident $vk:literal $key:literal;)*)` and the enum body `$($(#[$m])* $name,)*`. The other arms ignore `$m`. If the test module's `use serde::...` is flagged unused (the macro uses the names at the call site), keep it: it is needed. Rerun Step 2's command. Expected: PASS.

- [ ] **Step 4: Remove the dead code.** Delete `recv_raw` and `send_raw` with their docs. Delete the `is_cef_ready` field, its initializer (`Self { is_cef_ready: RefCell::new(false) }` becomes `Self {}` or a unit struct, whichever the surrounding code needs), and the write at `app.rs:92`. Drop `RefCell` from the imports if nothing else uses it.

- [ ] **Step 5: Move and fix docs.**
  - `channel.rs`: move the `MessageChannel` paragraph from above `MAX_MESSAGE_BYTES` to above `pub struct MessageChannel`; `MAX_MESSAGE_BYTES` keeps a one-line doc of its own ("The largest message either side accepts; a longer length prefix is an error, not an allocation.").
  - `transport.rs`: the same for `listen_remote` versus `TOKEN_LEN` (`TOKEN_LEN`: "Bytes in the one-time token a remote engine host sends first."); in 23-26, drop the clause about `send_raw`.
  - `fetched.rs:5-6`: finish the fragment into a sentence or delete it, whichever keeps the module doc true; read the whole module doc first.
  - `tabs.rs`: put the 529-534 doc on the function it describes; move the 47-53 constants and `use` lines up to the other imports and constants at the top of the file.
  - `pdf_fetch.rs:52-55`: "PDFium is downloaded once per machine and bound once per run, on the first request."
  - `main.rs:118-122`: name the list where the swatch colors now live (`grep -n "SWATCH" crates/manifold/src/*.rs`). Re-read `main.rs` 384-399 and make it match Task 25's key.

- [ ] **Step 6: The hook.** `.githooks/pre-commit` line 7 becomes:

```sh
cargo clippy -q -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 --all-targets -- -D warnings || { echo "pre-commit: clippy failed" >&2; exit 1; }
```

and the comment above it: "# WebKit and Servo don't build on Windows, and CEF's build needs its download; lint what does." In `CLAUDE.md`, the clippy command in "Formatting and lints" gains `-p manifold-engine-webview2`, and `-j 12` becomes `-j 8` there if it appears (LONG_RUNS: at most 12 busy threads, and the hook runs beside other work). Run `cargo clippy -j 8 -p manifold-engine-webview2 --all-targets -- -D warnings` first. Expected: clean (Task 23 left it clean). If it is not, fix the warnings in this task; they are lint fixes, not behaviour changes.

- [ ] **Step 7: Verify.** `cargo doc -j 8 -p ipc-protocol --no-deps` (no broken intra-doc links), the full gate, `cargo check -j 8 -p manifold-engine-cef`.

- [ ] **Step 8: Commit**

```bash
git add crates/manifold/src/fetched.rs crates/manifold/src/tabs.rs crates/manifold/src/pdf_fetch.rs crates/manifold/src/main.rs crates/ipc-protocol/src crates/manifold-engine-cef/src/app.rs .githooks/pre-commit CLAUDE.md
git commit -m "Docs: doc comments back on their items; key_codes forwards attributes; unused raw-frame calls and CEF readiness flag removed; the hook lints WebView2

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 27: One scene recorder in gpu.rs; `pdf_fetch` renamed to what it does

**Findings:**
- **D-F2, render half (low).** `gpu.rs` records the same "content then egui, or egui then content when `content_on_top`" sequence twice: once into the swapchain view (~1086-1116) and once into the screenshot readback view (~1130-1177). A change to one order (Task 15's transition fix was one) must be made twice, and the screenshot then silently differs from the screen. One function.
- **D-F1, naming half (low).** `pdf_fetch::PdfFetcher` fetches nothing: it binds PDFium and renders a page off the UI thread. Rename to `pdf_render::PdfRenderer`, field `pdf_renderer`.

**Files:**
- Modify: `crates/manifold/src/gpu.rs` (new `record_scene`, the two call sites)
- Rename: `crates/manifold/src/pdf_fetch.rs` → `crates/manifold/src/pdf_render.rs`
- Modify: users of the old names: `event_loop.rs:1408`, `fetched.rs:201,535`, `main.rs:36,65,1044,1469`, `node.rs:280`, and `README.md` (lines ~411, 494-496, 659)

**Interfaces:**
- Produces: `GpuState::record_scene(encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView, panes: &[(PanePx, Option<&TabTexture>)], target: (u32, u32), pipeline: &wgpu::RenderPipeline, renderer: &mut egui_wgpu::Renderer, egui_draw: Option<&(Vec<egui::ClippedPrimitive>, egui_wgpu::ScreenDescriptor)>, content_on_top: bool)`; `crate::pdf_render::PdfRenderer`, `App.pdf_renderer`.

- [ ] **Step 1: Baseline screenshots.** Before any edit, build and take two screenshots through the debug control (`scripts/drive.sh` with `SCREENSHOT`; see README "Debug control"): one in Tab view with a page loaded, one mid Tab-to-Graph transition if the script can time it (else Graph view). Keep them in the scratchpad.

- [ ] **Step 2: `record_scene`.**

```rust
    /// Records one frame's layers into `view`: the content quads, then egui
    /// on top; or, during a Tab<->Graph transition (`content_on_top`), egui
    /// as the base and the shrinking/expanding content quad over it. The
    /// screen and a screenshot both draw through here, so they can't differ.
    #[allow(clippy::too_many_arguments)] // Disjoint borrows of `GpuState` fields: `self` can't be passed whole.
    fn record_scene(
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        panes: &[(PanePx, Option<&TabTexture>)],
        target: (u32, u32),
        pipeline: &wgpu::RenderPipeline,
        renderer: &mut egui_wgpu::Renderer,
        egui_draw: Option<&(Vec<egui::ClippedPrimitive>, egui_wgpu::ScreenDescriptor)>,
        content_on_top: bool,
    ) {
        match (egui_draw, content_on_top) {
            (Some((primitives, screen)), true) => {
                Self::record_egui_pass(encoder, view, renderer, primitives, screen, true);
                Self::record_content_layers(encoder, view, panes, target, pipeline, false);
            }
            (Some((primitives, screen)), false) => {
                Self::record_content_layers(encoder, view, panes, target, pipeline, true);
                Self::record_egui_pass(encoder, view, renderer, primitives, screen, false);
            }
            (None, _) => Self::record_content_layers(encoder, view, panes, target, pipeline, true),
        }
    }
```

Move the "Normally the content quad is the base layer…" comment into this doc (it is the doc above, reworded); delete it at the call site. Replace both blocks:

```rust
        Self::record_scene(
            &mut encoder,
            &view,
            &panes,
            (width, height),
            &self.pipeline,
            &mut self.egui_renderer,
            egui_draw.as_ref(),
            content_on_top,
        );
```

and the same with `&readback_view` inside the screenshot closure. Check `egui_draw`'s exact type at its `let` (~1060-1081): it is `Option<(Vec<ClippedPrimitive>, ScreenDescriptor)>`; if the tuple's first element is a different collection, match the parameter to it.

- [ ] **Step 3: Verify the scene.** `cargo test -j 8 -p manifold --bins`, rebuild, retake both screenshots, and compare them byte-for-byte or visually with the baselines (`cmp` first; if bytes differ, open both). Expected: the same image.

- [ ] **Step 4: The rename.**

```bash
git mv crates/manifold/src/pdf_fetch.rs crates/manifold/src/pdf_render.rs
grep -rln "pdf_fetch\|PdfFetcher\|pdf_fetcher" crates README.md docs
```

Edit each listed file: `pdf_fetch` → `pdf_render`, `PdfFetcher` → `PdfRenderer`, `pdf_fetcher` → `pdf_renderer`. In `README.md` change only the live references (the module path in the line ~659 `pdf_fetch::pdfium_error`, and any current-architecture description); leave dated history entries' wording alone except for identifiers, which follow the rename so a reader can find them. Plans and specs under `docs/` keep their old names (they are records). Then `grep -rn "pdf_fetch\|PdfFetcher\|pdf_fetcher" crates README.md`. Expected: nothing.

- [ ] **Step 5: Full gate, then commit.**

```bash
git add -A crates/manifold/src README.md
git commit -m "Refactor: one scene recorder for screen and screenshot; pdf_fetch renamed pdf_render (it renders, it doesn't fetch)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

`git add -A crates/manifold/src` records the rename; check `git status` shows only intended paths first.

---

### Task 28: Engine shared pieces: downloads queue protocol messages, one navigation labeller, one first-message wait, text-key detection from the key table

**Findings:**
- **E-F1, download half (med; builds on Task 19).** CEF (`download_handler.rs:13-18`), WebKit (`main.rs:24-29`) and WebView2 (`download.rs:35-40`) each define a private `DownloadEvent` enum that mirrors `EngineToHost::Download*` field for field, then translate it back in the main loop (CEF `main.rs:491-498`, WebKit `main.rs:497-504`, WebView2 `main.rs:473-480`). Each crate already depends on `ipc_protocol`; queue `EngineToHost` values directly.
- **E-F3.** CEF `nav_handler.rs:45-82` and WebView2 `nav_state.rs:53-82` carry the same four fields (`user_started`, `host_started`, `last_input`, `finished`), the same two windows (`HOST_START_WINDOW` 2 s, `GESTURE_WINDOW` 5 s), and the same `under_way`/`label_new` logic, differing only in CEF's `gesture: Option<bool>`. One copy in `ipc_protocol`, tested once.
- **E-F2 (wait half).** CEF `main.rs:68-80` and WebView2 `main.rs:566-579` have the same `wait_for_first_message`, differing only in the pump call. One copy in `ipc_protocol::transport` taking the pump.
- **E-F6.** WebKit `input.rs:151-211` and Servo `main.rs:437-492` each list the text-producing keys (letters, digits, space) by hand. `KeyCode::dom_key()` already knows: a key whose DOM key is one character produces text.

**Files:**
- Create: `crates/ipc-protocol/src/nav_label.rs`
- Modify: `crates/ipc-protocol/src/lib.rs` (`pub mod nav_label;`), `transport.rs` (new `wait_for_first_message`), `messages.rs` (`produces_text`)
- Modify: `crates/manifold-engine-cef/src/{download_handler,nav_handler,main}.rs`
- Modify: `crates/manifold-engine-webview2/src/{download,nav_state,main}.rs`
- Modify: `crates/manifold-engine-webkit/src/{main,input}.rs`, `crates/manifold-engine-servo/src/main.rs`

**Interfaces:**
- Produces:
  - `ipc_protocol::nav_label::NavLabel { pub user_started: Option<bool>, pub host_started: Option<(bool, Instant)>, pub last_input: Option<Instant>, pub finished: bool }` with `NavLabel::created_by_host(now: Instant) -> Self`, `under_way(&self) -> bool`, `label_new(&mut self, gesture: Option<bool>, now: Instant)`; `pub const HOST_START_WINDOW`, `pub const GESTURE_WINDOW`
  - `ipc_protocol::transport::wait_for_first_message(rx: &mpsc::Receiver<HostToEngine>, timeout: Duration, pump: impl FnMut()) -> Option<HostToEngine>`
  - `KeyCode::produces_text(self) -> bool`
- Removes the three `DownloadEvent` enums.

- [ ] **Step 1: Write the failing tests** (`ipc-protocol`).

`nav_label.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn the_hosts_label_wins_inside_its_window() {
        let t0 = Instant::now();
        let mut l = NavLabel { host_started: Some((false, t0)), last_input: Some(t0), ..NavLabel::default() };
        l.label_new(Some(true), t0 + Duration::from_secs(1));
        assert_eq!(l.user_started, Some(false));
        assert!(l.under_way());
    }

    #[test]
    fn a_stale_host_label_falls_back_to_the_gesture_then_input() {
        let t0 = Instant::now();
        let late = t0 + HOST_START_WINDOW;
        let mut l = NavLabel { host_started: Some((false, t0)), ..NavLabel::default() };
        l.label_new(Some(true), late);
        assert_eq!(l.user_started, Some(true), "CEF's user_gesture");
        let mut l = NavLabel { last_input: Some(t0), ..NavLabel::default() };
        l.label_new(None, t0 + Duration::from_secs(4));
        assert_eq!(l.user_started, Some(true), "a click 4 s ago");
        let mut l = NavLabel { last_input: Some(t0), ..NavLabel::default() };
        l.label_new(None, t0 + GESTURE_WINDOW);
        assert_eq!(l.user_started, Some(false), "a click too long ago");
    }

    #[test]
    fn labelling_consumes_both_marks_and_reopens_the_navigation() {
        let t0 = Instant::now();
        let mut l = NavLabel { host_started: Some((true, t0)), last_input: Some(t0), finished: true, ..NavLabel::default() };
        l.label_new(None, t0);
        assert_eq!((l.host_started, l.last_input, l.finished), (None, None, false));
    }

    #[test]
    fn a_new_browser_counts_as_the_hosts_navigation() {
        let t0 = Instant::now();
        let mut l = NavLabel::created_by_host(t0);
        l.label_new(None, t0);
        assert_eq!(l.user_started, Some(false));
    }
}
```

`transport.rs` tests:

```rust
    #[test]
    fn the_first_message_wait_pumps_until_one_arrives() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut pumps = 0;
        let got = wait_for_first_message(&rx, std::time::Duration::from_secs(5), || {
            pumps += 1;
            if pumps == 3 {
                tx.send(crate::HostToEngine::GetState).unwrap();
            }
        });
        assert!(matches!(got, Some(crate::HostToEngine::GetState)));
        assert_eq!(pumps, 3);
    }

    #[test]
    fn the_first_message_wait_ends_on_a_closed_channel() {
        let (tx, rx) = std::sync::mpsc::channel::<crate::HostToEngine>();
        drop(tx);
        assert!(wait_for_first_message(&rx, std::time::Duration::from_secs(5), || {}).is_none());
    }
```

`messages.rs` tests:

```rust
    #[test]
    fn text_keys_are_letters_digits_and_space() {
        let text: Vec<KeyCode> = KeyCode::ALL.iter().copied().filter(|k| k.produces_text()).collect();
        assert_eq!(text.len(), 26 + 10 + 1, "{text:?}");
        assert!(KeyCode::KeyQ.produces_text() && KeyCode::Digit7.produces_text() && KeyCode::Space.produces_text());
        assert!(!KeyCode::Enter.produces_text() && !KeyCode::ShiftLeft.produces_text() && !KeyCode::Tab.produces_text());
    }
```

The count pins today's key set: a new text key added later must update the engines' tables' reasoning too, so the test failing then is wanted. Check `HostToEngine::GetState` is a unit variant (`messages.rs`); use any unit variant if not.

- [ ] **Step 2: Run** `cargo test -j 8 -p ipc-protocol nav_label first_message text_keys`. Expected: compile errors.

- [ ] **Step 3: `NavLabel`** (`nav_label.rs`). The docs below merge CEF `nav_handler.rs`'s field docs with WebView2's runtime-test note on `GESTURE_WINDOW` (the evidence for 5 s), made engine-neutral:

```rust
//! Whether a navigation was started by the user or by the host (manifold
//! itself), shared by the CEF and WebView2 engine hosts. Only user-started
//! navigations are shared with the tab's other instances.

use std::time::{Duration, Instant};

/// How long after the host asks for a navigation its label applies: long
/// enough for a navigation to start, short enough that a click after a
/// request that changed nothing isn't taken for the host's.
pub const HOST_START_WINDOW: Duration = Duration::from_secs(2);

/// How long after a click or key press a navigation counts as the user's:
/// Chromium's own lifespan for transient user activation. It covers a click
/// whose handler navigates only after a fetch, and a page's own timer
/// navigation within 5 s of a click or key press, as Chromium does: CEF's
/// `user_gesture` labelled a fetch-then-navigate click, a timer 3 s after a
/// click and one 2 s after a key press all as user-started (runtime tested
/// 2026-09-26). Too long only shares one navigation too many: the one it
/// starts in another instance is the host's, and never shared back.
pub const GESTURE_WINDOW: Duration = Duration::from_secs(5);

/// Who started the current navigation, and the marks that decide it.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct NavLabel {
    /// Whether the current navigation was user-started, sent as
    /// `NavigationStateChanged::user_started`. Set when a navigation starts
    /// and kept through its redirects; cleared once its load's end has been
    /// sent (`finished`), so a later change nothing started isn't labelled.
    pub user_started: Option<bool>,
    /// The label of a navigation the host just asked for, and when:
    /// `false` for `Navigate` (a typed url, another instance's url, the
    /// initial load), `true` for back/forward. An engine's own gesture flag
    /// reports only the page's gestures, so an API call needs its label from
    /// here. It applies to the next navigation to start within
    /// `HOST_START_WINDOW`, same-document url changes included: a request
    /// can change nothing (loading the current url), and a stale label
    /// would mislabel the user's next click.
    pub host_started: Option<(bool, Instant)>,
    /// When the page last got a click or a key press. A navigation within
    /// `GESTURE_WINDOW` of it is the user's (for CEF, only where Chromium's
    /// `user_gesture` doesn't answer: same-document url changes). Any
    /// navigation uses it up.
    pub last_input: Option<Instant>,
    /// The current navigation's load ended; the engine's main loop clears
    /// `user_started` after sending it.
    pub finished: bool,
}

impl NavLabel {
    /// A browser just created at its initial URL: that first navigation is the host's.
    pub fn created_by_host(now: Instant) -> Self {
        Self { host_started: Some((false, now)), ..Self::default() }
    }

    /// A navigation is started and not yet finished.
    pub fn under_way(&self) -> bool {
        self.user_started.is_some() && !self.finished
    }

    /// Labels a navigation starting at `now`: the host's label if it just
    /// asked for one, else `gesture` (CEF's `user_gesture`) if the engine
    /// has one, else the user's if they just clicked or typed.
    pub fn label_new(&mut self, gesture: Option<bool>, now: Instant) {
        let host = self.host_started.take().filter(|(_, at)| now.duration_since(*at) < HOST_START_WINDOW);
        let input = self.last_input.take().is_some_and(|at| now.duration_since(at) < GESTURE_WINDOW);
        self.user_started = Some(host.map_or(gesture.unwrap_or(input), |(label, _)| label));
        self.finished = false;
    }
}
```

Delete the field docs and constants from both engines' `NavState` once these exist. WebView2's `label_new` had no gesture, so `gesture.unwrap_or(input)` with `None` is its old `input`: unchanged.

- [ ] **Step 4: Use it in both engines.** In CEF `NavState` and WebView2 `NavState`, replace the four fields with `pub label: NavLabel`, delete the local constants and the two methods, and update every use (`grep -n "user_started\|host_started\|last_input\|\.finished\|under_way\|label_new" crates/manifold-engine-{cef,webview2}/src/*.rs`, about 49 lines). `x.user_started` becomes `x.label.user_started`; `x.label_new(g)` becomes `x.label.label_new(g, Instant::now())`; CEF's `NavState::new` uses `label: NavLabel::created_by_host(Instant::now())`; WebView2's `new_shared_nav_state` (`nav_state.rs:94`) set `host_started: Some((false, Instant::now()))`, so it uses `NavLabel::created_by_host(Instant::now())` too. Run `cargo test -j 8 -p manifold-engine-webview2` and `cargo check -j 8 -p manifold-engine-cef --tests`.

- [ ] **Step 5: `wait_for_first_message`** in `transport.rs`:

```rust
/// The first message from manifold, waiting up to `timeout` and calling
/// `pump` between polls so the engine's own loop keeps running (CEF work,
/// window messages). `None` on timeout or a closed channel.
pub fn wait_for_first_message(
    rx: &std::sync::mpsc::Receiver<crate::HostToEngine>,
    timeout: std::time::Duration,
    mut pump: impl FnMut(),
) -> Option<crate::HostToEngine> {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        pump();
        match rx.try_recv() {
            Ok(msg) => return Some(msg),
            Err(std::sync::mpsc::TryRecvError::Empty) => std::thread::sleep(std::time::Duration::from_millis(8)),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => return None,
        }
    }
    None
}
```

Delete both engine copies; the call sites become `ipc_protocol::transport::wait_for_first_message(&rx, timeout, do_message_loop_work)` (CEF) and `…(&rx, timeout, window::pump_messages)` (WebView2). If either pump function's signature isn't `fn()`, pass `|| { pump_call(); }`.

- [ ] **Step 6: Downloads queue `EngineToHost`.** In each of the three engines:
  - Delete the `DownloadEvent` enum and its doc.
  - The queue's element type becomes `ipc_protocol::EngineToHost` (CEF `DownloadResults.queue: Vec<EngineToHost>`; WebView2 `PendingDownload = Rc<RefCell<Vec<EngineToHost>>>`; WebKit `pending_download: Rc<RefCell<Vec<EngineToHost>>>`).
  - Each push builds the protocol variant directly: `DownloadEvent::Started { path, total_bytes, url }` → `EngineToHost::DownloadStarted { path, total_bytes, url }`, and likewise `Progress` → `DownloadProgress`, `Completed` → `DownloadCompleted`, `Failed` → `DownloadFailed`. Copy the exact variant names and fields from the existing translation matches (CEF `main.rs:491-498`) before deleting them.
  - The drain keeps Task 19's coalescing and loses only the translation: `for msg in ipc_protocol::downloads::coalesce_progress(queue.drain(..).collect()) { let _ = control.send(&msg); }` (keep each engine's own sender name: `control` or `to_host`). After Task 19 the translation match sits where it collects into a `Vec` for `coalesce_progress`; that collect becomes the `drain(..).collect()` above.
  - Fix the docs that justified the separate type ("kept as its own type here rather than depending on `ipc_protocol` from this leaf module"): it now depends on it.

  `cargo test -j 8 -p manifold-engine-webview2`, `cargo check -j 8 -p manifold-engine-cef`; WebKit via WSL (`CARGO_TARGET_DIR=target-linux cargo check -j 8 -p manifold-engine-webkit`).

- [ ] **Step 7: `produces_text`** in the `key_codes!` impl:

```rust
            /// The key types a character of its own (letters, digits,
            /// space): its DOM `key` is that one character. Engines send
            /// such keys without text and let `InputEvent::Char` insert it.
            pub fn produces_text(self) -> bool {
                let mut chars = self.dom_key().chars();
                chars.next().is_some() && chars.next().is_none()
            }
```

WebKit `key_to_gdk_keyval`: replace the 37-line letter/digit/space arm with an early return, keeping the rest of the match:

```rust
    if key.produces_text() {
        return Some(GDK_KEY_VoidSymbol as u32);
    }
    let keyval = match key {
        KeyCode::Enter => GDK_KEY_Return,
        // (the named arms, unchanged)
        KeyCode::AltRight => GDK_KEY_Alt_R,
        // Text keys returned above; a new non-text key lands here until it gets its own arm.
        _ => GDK_KEY_VoidSymbol,
    };
```

Servo `key_to_servo_key`: the same, `if key.produces_text() { return Key::Named(NamedKey::Unidentified); }` ahead of the named arms, with `_ => Key::Named(NamedKey::Unidentified)` last. `key_to_servo_code` (496-) maps every key to its own `Code` and stays as it is.

The `_` arm gives up the compiler's exhaustiveness check for a future key. `text_keys_are_letters_digits_and_space` fails when the key set changes, which is the reminder to revisit both tables; say so in the `produces_text` doc's last line: "A new key fails `text_keys_are_letters_digits_and_space`; check the WebKit and Servo key tables then."

- [ ] **Step 8: Run** `cargo test -j 8 -p ipc-protocol`. Expected: PASS. WebKit: WSL `cargo test -j 8 -p manifold-engine-webkit` with `CARGO_TARGET_DIR=target-linux`. Servo: reading only; report it.

- [ ] **Step 9: Live check.** Rebuild CEF and WebView2 hosts and manifold. In a WebView2 tab and a CEF tab: click a link (it shares to the tab's other instance if one is open: see README "Navigation sharing"), type in a text field (no doubled characters), and download a small file (the download card appears, progresses, completes). Kill only your PID.

- [ ] **Step 10: Full gate, then commit.**

```bash
git add crates/ipc-protocol/src crates/manifold-engine-cef/src crates/manifold-engine-webview2/src crates/manifold-engine-webkit/src crates/manifold-engine-servo/src/main.rs
git commit -m "Engines: downloads queue protocol messages; one navigation labeller and first-message wait in ipc-protocol; text keys come from the key table

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 29: App code split: debug commands in debug_control, the tab-event drain in three, one shortcut table, session_sync's tab helper and its tests in their own file

**Findings:**
- **B-F2, B-F3 (low).** `event_loop.rs` is 1,790 lines. `handle_debug_command` (854-1274, about 420 lines) belongs beside the parser in `debug_control.rs`. `drain_tab_events` (1283-1518, about 235 lines) does three jobs: drain engine IPC, tick the fetchers, run the idle sweep.
- **B-F4, table half (low; Task 13 did the quick-switcher dedupe).** `shortcuts.rs`: the chords are listed twice, once as `consume_key` calls in `handle_global_shortcuts` (46-104) and once as winit codes in `claims_chord` (8-32), with a "keep this in step" comment. One table drives both.
- **B-F6 (low), with B-F7's test-file move.** `session_sync.rs` (3,255 lines; tests from 1559): the tab fields common to `Tab` and `Unloaded` (title, muted, zoom factor, url) are read and written by hand at ~451-563, ~1076-1087 and ~1203-1216. `save_session` (905-1005) computes the shared node set more than once. The test module is over half the file.

**Files:**
- Modify: `crates/manifold/src/event_loop.rs`, `crates/manifold/src/debug_control.rs`
- Modify: `crates/manifold/src/shortcuts.rs`
- Modify: `crates/manifold/src/session_sync.rs`; Create: `crates/manifold/src/session_sync/tests.rs`

**Interfaces:**
- `App::handle_debug_command` keeps its signature, now in `debug_control.rs`.
- New private `App::drain_engine_ipc(&mut self)`, `App::tick_fetchers(&mut self)`, `App::idle_sweep(&mut self)`; `drain_tab_events` calls them in that order.
- `shortcuts.rs`: `enum Shortcut`, `struct Chord`, `const CHORDS: &[Chord]`; `claims_chord` keeps its signature.
- `session_sync.rs`: a private `struct TabCommon<'a> { title: &'a str, muted: bool, zoom_factor: f64, url: &'a str }` and `fn tab_common(payload: &NodePayload) -> Option<TabCommon<'_>>`.

- [ ] **Step 1: A characterization test for `claims_chord`** (`shortcuts.rs`), written before the table so it pins today's behaviour:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// `claims_chord` as it was before the table, kept verbatim as the reference.
    fn claims_chord_before(code: WinitKeyCode, modifiers: u8) -> bool {
        use WinitKeyCode::*;
        if modifiers & ipc_protocol::modifiers::CONTROL != 0 {
            matches!(
                code,
                KeyT | KeyW | KeyL | KeyF | KeyD | KeyP | KeyK | KeyG | KeyR | Tab | Equal | NumpadAdd | Minus
                    | NumpadSubtract | Digit0 | Numpad0
            )
        } else {
            modifiers == 0 && matches!(code, F1 | F2 | F5 | F12)
        }
    }

    #[test]
    fn the_table_claims_exactly_the_chords_it_did_before() {
        use WinitKeyCode::*;
        let codes = [
            KeyA, KeyD, KeyF, KeyG, KeyK, KeyL, KeyP, KeyR, KeyT, KeyW, KeyZ, Tab, Equal, NumpadAdd, Minus,
            NumpadSubtract, Digit0, Digit1, Numpad0, F1, F2, F3, F5, F12, Enter, Escape,
        ];
        let m = ipc_protocol::modifiers::CONTROL | ipc_protocol::modifiers::SHIFT;
        for code in codes {
            for mods in [0, ipc_protocol::modifiers::CONTROL, ipc_protocol::modifiers::SHIFT, m] {
                assert_eq!(claims_chord(code, mods), claims_chord_before(code, mods), "{code:?} {mods:#x}");
            }
        }
    }

    #[test]
    fn every_ctrl_shift_chord_is_tried_before_its_plain_ctrl_twin() {
        for (i, c) in CHORDS.iter().enumerate() {
            if c.mods.shift {
                assert!(
                    !CHORDS[..i].iter().any(|p| p.key == c.key && p.mods == egui::Modifiers::CTRL),
                    "{:?}: plain Ctrl would consume Ctrl+Shift first",
                    c.action
                );
            }
        }
    }
}
```

Check the modifier constant names in `ipc_protocol::modifiers` (`CONTROL` is used above; `SHIFT` may be spelled differently: `grep -n "pub const" crates/ipc-protocol/src/messages.rs | grep -i shift`). Run `cargo test -j 8 -p manifold --bins the_table_claims`: it fails to compile only on `CHORDS` (the second test). Comment the second test out for this run if needed to see the first pass against today's code, then restore it.

- [ ] **Step 2: The table** (`shortcuts.rs`):

```rust
/// What a global shortcut does (`App::handle_global_shortcuts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shortcut {
    ReopenTab,
    NewTab,
    CloseAllLevel,
    CloseTab,
    FocusAddress,
    OpenFind,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    OpenDevtools,
    PrevTab,
    NextTab,
    QuickSwitch,
    NewGroup,
    ViewToggle,
    PinToggle,
    Reload,
    RenameGroup,
    DuplicateTab,
    ShortcutsHelp,
}

/// One key chord: egui's modifiers and key (what `consume_key` matches),
/// the winit codes the same press arrives as on its way to a page
/// (`claims_chord`), and what it does.
pub(crate) struct Chord {
    pub(crate) mods: egui::Modifiers,
    pub(crate) key: egui::Key,
    pub(crate) codes: &'static [WinitKeyCode],
    pub(crate) action: Shortcut,
}

const CTRL_SHIFT: egui::Modifiers = egui::Modifiers { shift: true, ..egui::Modifiers::CTRL };

/// Every global shortcut, in the order they are tried. egui's matching
/// ignores an extra Shift, so each `Ctrl+Shift+X` comes before its plain
/// `Ctrl+X`, or the plain one would consume it. An action with two chords
/// fires on the first that matches.
#[rustfmt::skip] // One chord per line reads as the shortcut list it is.
pub(crate) const CHORDS: &[Chord] = {
    use egui::Key as K;
    use egui::Modifiers as M;
    use Shortcut as S;
    use WinitKeyCode as W;
    &[
        Chord { mods: CTRL_SHIFT, key: K::T, codes: &[W::KeyT], action: S::ReopenTab },
        Chord { mods: M::CTRL, key: K::T, codes: &[W::KeyT], action: S::NewTab },
        Chord { mods: CTRL_SHIFT, key: K::W, codes: &[W::KeyW], action: S::CloseAllLevel },
        Chord { mods: M::CTRL, key: K::W, codes: &[W::KeyW], action: S::CloseTab },
        Chord { mods: M::CTRL, key: K::L, codes: &[W::KeyL], action: S::FocusAddress },
        Chord { mods: M::CTRL, key: K::F, codes: &[W::KeyF], action: S::OpenFind },
        // `Ctrl+=` is what most keyboards send for "zoom in" (`+` needs Shift): accept both.
        Chord { mods: M::CTRL, key: K::Equals, codes: &[W::Equal], action: S::ZoomIn },
        Chord { mods: M::CTRL, key: K::Plus, codes: &[W::NumpadAdd], action: S::ZoomIn },
        Chord { mods: M::CTRL, key: K::Minus, codes: &[W::Minus, W::NumpadSubtract], action: S::ZoomOut },
        Chord { mods: M::CTRL, key: K::Num0, codes: &[W::Digit0, W::Numpad0], action: S::ZoomReset },
        Chord { mods: M::NONE, key: K::F12, codes: &[W::F12], action: S::OpenDevtools },
        Chord { mods: CTRL_SHIFT, key: K::Tab, codes: &[W::Tab], action: S::PrevTab },
        Chord { mods: M::CTRL, key: K::Tab, codes: &[W::Tab], action: S::NextTab },
        Chord { mods: M::CTRL, key: K::K, codes: &[W::KeyK], action: S::QuickSwitch },
        Chord { mods: CTRL_SHIFT, key: K::G, codes: &[W::KeyG], action: S::NewGroup },
        Chord { mods: M::CTRL, key: K::G, codes: &[W::KeyG], action: S::ViewToggle },
        Chord { mods: M::CTRL, key: K::P, codes: &[W::KeyP], action: S::PinToggle },
        Chord { mods: M::CTRL, key: K::R, codes: &[W::KeyR], action: S::Reload },
        Chord { mods: M::NONE, key: K::F5, codes: &[W::F5], action: S::Reload },
        Chord { mods: M::NONE, key: K::F2, codes: &[W::F2], action: S::RenameGroup },
        Chord { mods: M::CTRL, key: K::D, codes: &[W::KeyD], action: S::DuplicateTab },
        Chord { mods: M::NONE, key: K::F1, codes: &[W::F1], action: S::ShortcutsHelp },
    ]
};

/// Whether `code` pressed with `modifiers` (the `ipc_protocol::modifiers` bits) is one of `CHORDS`, so
/// the page must not get it as well. Extra Shift is ignored, as egui's matching ignores it.
pub(crate) fn claims_chord(code: WinitKeyCode, modifiers: u8) -> bool {
    let ctrl = modifiers & ipc_protocol::modifiers::CONTROL != 0;
    CHORDS.iter().any(|c| {
        c.codes.contains(&code) && if ctrl { c.mods.ctrl } else { modifiers == 0 && c.mods == egui::Modifiers::NONE }
    })
}
```

The order follows the old tuple exactly (lines 70-102), which is the order `consume_key` ran in. `use … as` inside a `const` block works on the pinned toolchain (1.89+); if it doesn't, write the paths out. If `WinitKeyCode` isn't `PartialEq` for `contains`, compare with `matches!`/`==` in a loop.

`handle_global_shortcuts`:

```rust
    pub(crate) fn handle_global_shortcuts(&mut self, ui: &mut egui::Ui) {
        let fired: Vec<Shortcut> = ui.input_mut(|i| {
            let mut fired = Vec::new();
            for c in CHORDS {
                // An action's second chord is tried only if its first didn't fire, as `a || b` did.
                if !fired.contains(&c.action) && i.consume_key(c.mods, c.key) {
                    fired.push(c.action);
                }
            }
            fired
        });
        let on = |s: Shortcut| fired.contains(&s);
        if on(Shortcut::ReopenTab) {
            self.reopen_closed_shortcut();
        }
        if on(Shortcut::NewTab) {
            self.new_tab_shortcut();
        }
        if on(Shortcut::CloseAllLevel) {
            self.close_all_at_current_level();
        }
        if on(Shortcut::CloseTab) {
            self.close_focused_tab_shortcut();
        }
        if on(Shortcut::FocusAddress) {
            self.focus_address_bar_shortcut();
        }
        if on(Shortcut::OpenFind) {
            self.open_find_bar_shortcut();
        }
        if on(Shortcut::ZoomIn) {
            self.zoom_in_shortcut();
        }
        if on(Shortcut::ZoomOut) {
            self.zoom_out_shortcut();
        }
        if on(Shortcut::ZoomReset) {
            self.zoom_reset_shortcut();
        }
        if on(Shortcut::OpenDevtools) {
            self.open_devtools_shortcut();
        }
        if on(Shortcut::NextTab) || on(Shortcut::PrevTab) {
            self.cycle_focused_tab(on(Shortcut::NextTab));
        }
        if on(Shortcut::QuickSwitch) {
            self.open_quick_switcher(); // Task 13's helper
        }
        if on(Shortcut::NewGroup) {
            self.new_group_shortcut();
        }
        // Neutralized while the quick-switcher is open, same reasoning as
        // its own Escape/arrow handling in `build_quick_switcher` -- a
        // `Ctrl+G` typed while it's open shouldn't also flip the view
        // underneath it.
        if on(Shortcut::ViewToggle) && self.quick_switcher.is_none() {
            self.toggle_view_mode_shortcut();
        }
        if on(Shortcut::PinToggle) {
            self.pin_focused_tab_shortcut();
        }
        if on(Shortcut::Reload) {
            self.reload_focused_tab_shortcut();
        }
        if on(Shortcut::RenameGroup) {
            self.rename_current_group_shortcut();
        }
        if on(Shortcut::DuplicateTab) {
            self.duplicate_focused_tab_shortcut();
        }
        if on(Shortcut::ShortcutsHelp) {
            self.shortcuts_help_open = !self.shortcuts_help_open;
        }
    }
```

This is the chain at 106-167 with each flag replaced. Keep the method's long doc; replace its ordering sentences with "Order and chords: `CHORDS`."

- [ ] **Step 3: Run** `cargo test -j 8 -p manifold --bins shortcuts`. Expected: both tests PASS, and every existing shortcut test (`grep -n "fn .*shortcut" crates/manifold/src/*.rs` in test modules) passes unchanged.

- [ ] **Step 4: Move `handle_debug_command`.** Cut `event_loop.rs` 854-1274 (the method and its doc) into a new `impl App { … }` block at the end of `debug_control.rs`'s non-test code, above its `#[cfg(test)]` (line 778). Add whatever `use` lines it needs (`use super::*;` if `debug_control.rs` doesn't already have it; `ActiveEventLoop` from `winit::event_loop`). The visibility stays `pub(crate)`. Expected: `git diff --stat` shows about 420 lines out of one file and into the other, and `cargo test` is unchanged. Use `git diff --color-moved=zebra` to confirm the body moved byte for byte.

- [ ] **Step 5: Split `drain_tab_events`.** Read 1283-1518 and cut at its three phases (each already starts with a comment block): the per-tab IPC drain loop → `fn drain_engine_ipc(&mut self)`; the fetcher ticks (`redirect_fetcher`, `favicon_fetcher`, `pdf_renderer`, suggestions, jar pulls) → `fn tick_fetchers(&mut self)`; the idle unload and cleanup → `fn idle_sweep(&mut self)`. Locals used across phases (a `now`, an `any_changed` flag) become return values or are recomputed; if a phase needs a value from an earlier one, return it (`fn drain_engine_ipc(&mut self) -> DrainOutcome` with only those fields) rather than storing it on `App`. `drain_tab_events` becomes three calls with its doc. Confirm with `--color-moved` that bodies moved unchanged apart from the seams.

- [ ] **Step 6: `session_sync` tab helper.**

```rust
/// The fields a live tab and an unloaded one share, as the store records them.
struct TabCommon<'a> {
    title: &'a str,
    muted: bool,
    zoom_factor: f64,
    url: &'a str,
}

fn tab_common(payload: &NodePayload) -> Option<TabCommon<'_>> {
    match payload {
        NodePayload::Tab(t) => Some(TabCommon { title: &t.title, muted: t.muted, zoom_factor: t.zoom_factor, url: &t.url }),
        NodePayload::Unloaded(u) => Some(TabCommon { title: &u.title, muted: u.muted, zoom_factor: u.zoom_factor, url: &u.url }),
        _ => None,
    }
}
```

Check the four field names and types on `TabPayload` and `UnloadedTabPayload` (`node.rs`) and match them (`zoom_factor` may be `f32`). Replace the duplicated Tab/Unloaded arms at ~1076-1087 and ~1203-1216 (the readers) with one `tab_common` call each; at ~451-563, use it where the code reads the fields, and leave writes (which need `&mut` on two different types) as they are. In `save_session` (905-1005), compute the shared-node set once into a local at the top and pass it where it is recomputed.

- [ ] **Step 7: Move the tests.** Cut `session_sync.rs` from `#[cfg(test)]` (1559) to the end into `crates/manifold/src/session_sync/tests.rs` (without the `mod tests {` wrapper and its closing brace; dedent one level with `cargo fmt`), and leave in `session_sync.rs`:

```rust
#[cfg(test)]
mod tests;
```

A `session_sync.rs` file with a `session_sync/` directory beside it is the 2018 module layout; no `mod.rs` needed. `cargo test -j 8 -p manifold --bins session_sync`: the same test count as before (note it from the run in Step 3).

- [ ] **Step 8: Full gate, live smoke (launch, open two tabs, switch with Ctrl+Tab, Ctrl+Shift+T after closing one, F1 help, quit, relaunch restores), then commit.**

```bash
git add crates/manifold/src/event_loop.rs crates/manifold/src/debug_control.rs crates/manifold/src/shortcuts.rs crates/manifold/src/session_sync.rs crates/manifold/src/session_sync/tests.rs
git commit -m "Refactor: debug commands live in debug_control; tab-event drain split in three; one shortcut table drives consumption and page suppression; session_sync tab helper and tests file

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 30: Cards and nodes: one accessor macro, named constants, shared text helper, fewer long parameter lists

**Findings:**
- **C-R4 (low).** `node.rs` has 15 hand-written `as_*`/`as_*_mut` matches on `NodePayload` (25-170) and 15 forwarding copies on `GraphNode` (517-570). A macro states each pair once.
- **C-R2 (low).** `card.rs` repeats `Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0))` (282, 1979, 2062; also `image_view.rs:277`, `main.rs:2608`, `tab_view.rs:418`), and literal colors with meaning: scrim alphas 190 (363) and 160 (2134), the waveform color `(150, 190, 90)` (703), the note text `from_black_alpha(220)` (1425, 1467), and a `28.0` right reserve (1786, 1947, 2101) that is `HEADER_TEXT_X`'s mirror.
- **C-R2, also.** Text centered in a rect is built by hand at ~636, 668, 1578, 1681, 1693 although `push_centered_text` (84) does it.
- **C-R3 (low).** `paint_card_body` and `paint_group_body` carry `#[allow(clippy::too_many_arguments)]` while their caller holds a `CardPass` with three of those values.
- **C-R1 (low).** Tab cards draw their header by hand (`paint_tab_body`) (~2028-2182) while every other card goes through `CardHeader` (9).

**Files:**
- Modify: `crates/manifold/src/node.rs`, `crates/manifold/src/card.rs`, `crates/manifold/src/image_view.rs:277`, `crates/manifold/src/main.rs:2608`, `crates/manifold/src/tab_view.rs:418`

**Interfaces:**
- Produces: `pub(crate) const FULL_UV: egui::Rect` (in `card.rs`, re-exported or reached as `crate::card::FULL_UV`); `paint_card_body(&mut self, ui, pass: &mut CardPass, id, view, rect)` and `paint_group_body(&mut self, ui, pass: &mut CardPass, id, view, rect)`.
- Accessor names and signatures on `NodePayload` and `GraphNode` are unchanged.

- [ ] **Step 1: Baseline screenshots.** Build and capture Graph view with one card of each kind (tab, unloaded tab, group, note, image, PDF, JSON, download) through the debug control (`SCREENSHOT`). Keep them in the scratchpad.

- [ ] **Step 2: The accessor macro** (`node.rs`):

```rust
/// `as_x` (and `as_x_mut` where listed) on `NodePayload`, and the same
/// pair on `GraphNode` forwarding to its payload.
macro_rules! payload_accessors {
    ($( $variant:ident($ty:ty) => $as:ident $(, $as_mut:ident)?; )*) => {
        impl NodePayload {
            $(
                pub(crate) fn $as(&self) -> Option<&$ty> {
                    match self {
                        NodePayload::$variant(p) => Some(p),
                        _ => None,
                    }
                }
                $(
                    pub(crate) fn $as_mut(&mut self) -> Option<&mut $ty> {
                        match self {
                            NodePayload::$variant(p) => Some(p),
                            _ => None,
                        }
                    }
                )?
            )*
        }

        impl GraphNode {
            $(
                pub(crate) fn $as(&self) -> Option<&$ty> {
                    self.payload.$as()
                }
                $(
                    pub(crate) fn $as_mut(&mut self) -> Option<&mut $ty> {
                        self.payload.$as_mut()
                    }
                )?
            )*
        }
    };
}

payload_accessors! {
    Tab(TabPayload) => as_tab, as_tab_mut;
    Unloaded(UnloadedTabPayload) => as_unloaded;
    Download(DownloadPayload) => as_download, as_download_mut;
    Group(GroupPayload) => as_group, as_group_mut;
    Note(NotePayload) => as_note, as_note_mut;
    Image(ImagePayload) => as_image;
    Pdf(PdfPayload) => as_pdf, as_pdf_mut;
    Json(JsonPayload) => as_json;
}
```

Delete the 15 + 15 hand-written methods; keep every other method in those `impl` blocks (`saved_tab`, `pinned_mut` and the rest) where they are. If any hand-written accessor had a doc comment, the macro arm can't carry it as written: add `$(#[$doc:meta])*` before `$variant` and emit it on `$as`, or move the doc to the variant. If any accessor's body did more than the plain match (read each one before deleting), keep that one by hand and leave it out of the macro list. `cargo test -j 8 -p manifold --bins`: unchanged.

- [ ] **Step 3: Named constants** (`card.rs`, near the other card constants):

```rust
/// The whole texture: UVs for an image drawn uncropped.
pub(crate) const FULL_UV: egui::Rect = egui::Rect { min: egui::Pos2::ZERO, max: egui::pos2(1.0, 1.0) };

/// Behind text drawn over an image: dark enough to read white text on any picture.
const IMAGE_TEXT_SCRIM: egui::Color32 = egui::Color32::from_black_alpha(190);

/// Behind a tab card's title strip over its page thumbnail: a lighter scrim, the strip is small.
const TITLE_STRIP_SCRIM: egui::Color32 = egui::Color32::from_black_alpha(160);

/// An audio card's waveform.
const WAVEFORM_COLOR: egui::Color32 = egui::Color32::from_rgb(150, 190, 90);

/// A note's text on its pale paper.
const NOTE_TEXT: egui::Color32 = egui::Color32::from_black_alpha(220);

/// Kept clear at a header's right edge, mirroring `HEADER_TEXT_X` at its left.
const HEADER_RIGHT_RESERVE: f32 = HEADER_TEXT_X;
```

Read each site before replacing to confirm the literal means what the name says (190 at 363 is behind text over an image; 160 at 2134 is the title strip; if either is something else, name it for what it is). `from_black_alpha`, `from_rgb` and `pos2` are `const fn` in egui 0.36; if one isn't, use a `fn` returning the value. Replace the six `FULL_UV` sites (`crate::card::FULL_UV` outside `card.rs`), the three `28.0` reserves (not the `vec2(0.0, 28.0)` at 2120, which is `HEADER_SOURCE_Y`: use that constant there instead), and the colors. Task 25 already touched 1467; use `NOTE_TEXT` there too.

- [ ] **Step 4: `push_centered_text`.** Read `push_centered_text` (84) and each site (~636, 668, 1578, 1681, 1693). Where a site lays out a galley and places it at `rect.center() - galley.size() / 2.0` (or the same arithmetic spelled out) with the same overlay/painter destination the helper uses, replace it with the helper call. Where a site differs (a different anchor, a clip, wrapping), leave it and note why in one line. Report how many were replaced.

- [ ] **Step 5: `CardPass` for the bodies.** Change:

```rust
    pub(crate) fn paint_card_body(
        &mut self,
        ui: &mut egui::Ui,
        pass: &mut CardPass,
        id: &str,
        view: &CardView,
        rect: egui::Rect,
    ) -> CardBody {
        let (inv_zoom, zoom_scale) = (pass.inv_zoom, pass.zoom_scale);
        let deferred_overlays = &mut *pass.deferred_overlays;
        // (the match, unchanged)
    }
```

and the same for `paint_group_body`; drop both `#[allow(clippy::too_many_arguments)]`. The caller (`draw_graph_card`, 927) passes `pass` instead of its three values; if `draw_graph_card` holds a reborrow of `pass.deferred_overlays` across that call, end it before the call or reborrow after. The test at ~2495 calls `paint_group_body` directly: build a `CardPass` there:

```rust
                let mut actions = Vec::new();
                let drop_preview = None;
                let mut pass = CardPass {
                    deferred_overlays: &mut overlays,
                    actions: &mut actions,
                    inv_zoom: 1.0,
                    zoom_scale: 1.0,
                    drop_preview: &drop_preview,
                    show_order: false,
                    order: &[],
                    any_focused: false,
                };
                let body = app.paint_group_body(ui, &mut pass, &g, &view, rect);
```

`paint_card_control` and `paint_control_square` are free functions called outside a card pass's ownership: keep their allows and add the reason, e.g. `#[allow(clippy::too_many_arguments)] // Free function: callers hold no CardPass, and a struct for one call site would only rename the arguments.`

- [ ] **Step 6: Tab cards through `CardHeader`.** Read `CardHeader` (9-60) and the tab card's hand-drawn header (~2028-2182). If the tab header draws the same parts `CardHeader` takes (icon, title, source line, the reserves), build a `CardHeader` there and call its painter, deleting the hand-drawn parts. If the tab header has parts `CardHeader` lacks (the favicon texture, the mute/audio glyph, the loading spinner), add them to `CardHeader` only if another card would use them; otherwise keep the tab header's own drawing for those parts and route only the shared ones. Report which parts moved.

- [ ] **Step 7: Compare screenshots.** Rebuild, retake Step 1's screenshots, compare (`cmp`, then by eye). Expected: identical. A one-pixel shift in a tab header means Step 6 changed a measurement: find and fix it, don't accept it.

- [ ] **Step 8: Full gate, then commit.**

```bash
git add crates/manifold/src/node.rs crates/manifold/src/card.rs crates/manifold/src/image_view.rs crates/manifold/src/main.rs crates/manifold/src/tab_view.rs
git commit -m "Refactor: node accessors from one macro; named card colors and FULL_UV; shared centered-text helper; card bodies take the CardPass; tab headers through CardHeader

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Dropped findings

- **B-F8, part 1 (remove stricter than install):** install also refuses while an engine is spawning, so remove is not stricter; there is no asymmetry to fix.
- **B-R2, spawn-to-assign race:** the child is assigned to the job right after spawn, before it can start grandchildren; only the unlogged `SetInformationJobObject` failure is real (Task 23).
- **B-R8, per-frame cost:** one `format!` and a mutex per frame is negligible; only the URL redaction is kept (Task 24).
- **C-P7, font family missing from the truncate key:** every caller uses `FontId::proportional`, a documented invariant on the key; adding the family costs a hash for nothing.
- **E, WebView2 `jar_cdp` item:** dropped by the report itself after re-reading (InPrivate-only seeding is the intended design).
- **D-F1, a generic background `Worker` type:** the four fetchers differ in what `tick()` does with results; a shared type saves little (README item 8 reached the same verdict). YAGNI.
- **D-F3, unifying the fetch result structs:** YAGNI; the one shared piece, the size-limited body read, is Task 1's helper (now `net::get_limited`, Task 22).

## Deferred

- **D-R1, image decode on a worker:** decode runs on the UI thread once per load; needs a texture-upload handoff design, not a local fix.
- **D-R6, JSON parse on a worker:** same shape as D-R1; the 64 MB body cap (Task 22) bounds the stall meanwhile.
- **D-R4, jar cookies for app-side fetches:** sending cookies outside the engines is a privacy decision for the user; Task 22 sets the User-Agent and documents the limit.
- **D-R9, PDF rendering in a child process:** isolates a PDFium crash, but needs an IPC design; the render already runs off the UI thread.
- **C-P1, cache markdown layout:** the borrow fix (Task 25) removes the copy; caching parsed layout needs `egui_commonmark` support or a fork.
- **C-R5, trimming long narrative doc comments:** wide churn across many files for readability only; better done file by file when each is next edited.
- **B-F7, splitting `App` into sub-structs:** a large cross-cutting move; Tasks 29-30 reduce the pressure first.
- **B-F8, part 2, expanding the forwarded key table:** a feature (more keys to pages), not a fix; needs per-engine testing in WSL and Servo builds that aren't available here.
- **E-F1, the other per-message slots (one ordered outbox per engine):** each slot has its own latest-wins or dedupe rule (page state, find result, audio), so a plain queue changes what the host receives; it needs a per-slot review with WSL and Servo builds. Task 28 does the download half.
- **E-F2, job-object helper shared across engines:** CEF assigns itself while manifold assigns WebView2's host; unifying them changes who owns the job and needs a design.
- **E-R4, adapter mismatch (WebView2 capture on the default adapter):** choosing the eGPU explicitly needs the host to pass an adapter LUID; Task 23 handles the device-loss half.
