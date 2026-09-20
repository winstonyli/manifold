# WebView2 Composition Hosting Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `manifold-engine-webview2`'s windowed WebView2 hosting + `Windows.Graphics.Capture`-on-a-window + JS/CDP mouse input with Composition hosting (`ICoreWebView2CompositionController`) + `GraphicsCaptureItem::CreateFromVisual` capture + native `SendMouseInput`, while keeping keyboard input (`CDP dispatchKeyEvent` / JS `execCommand`) unchanged.

**Architecture:** A new `composition.rs` module owns the `Compositor`/`DesktopWindowTarget`/`ContainerVisual` setup. `main.rs`'s controller creation and resize logic switch to the Composition APIs. `capture.rs` swaps its window-capture-plus-crop for direct visual capture (the crop step disappears entirely — a visual has no non-client chrome to crop out). `input.rs`'s mouse handlers swap their `ExecuteScript`/`Selection` JS for `SendMouseInput`; its keyboard handlers are untouched.

**Tech Stack:** Rust, `windows-rs` 0.62 (`Win32::UI::HiDpi`, `Win32::System::WinRT::Composition`, `UI::Composition`, `Graphics::Capture`), `webview2-com` 0.39.1, WinRT COM interop.

**Spec:** `docs/superpowers/specs/2026-09-20-composition-hosting-migration-design.md`

## Global Constraints

- Full replacement — no feature flag, no fallback to windowed hosting. Delete the old code once each phase is verified.
- Keyboard (`KeyDown`/`KeyUp`/`Char`) stays exactly as-is on CDP `dispatchKeyEvent` / JS `execCommand` — no native alternative exists (confirmed: absent from bindings, confirmed absent by Microsoft's own explanation, confirmed `PostMessage`'d `WM_KEYDOWN` doesn't reach the page either way).
- No minimum-runtime soft-degrade path. Composition controller creation failure is a hard `.expect()`, matching this file's existing `create_environment().expect(...)` / `create_controller(...).expect(...)` posture.
- Two independently-verifiable phases: (1) hosting + capture, with `input.rs` untouched; (2) mouse input. Do not start Phase 2 until Phase 1 is verified live.
- Every verification step in this plan is a **live run of the real binary**, not a unit test — this migration is fundamentally COM/WinRT/GPU integration with no existing test harness for the render/capture pipeline, matching how every finding this session was actually confirmed (build, run, screenshot or diagnostic log, not `cargo test`).

---

### Task 1: `composition.rs` — Compositor/visual-tree setup module

**Files:**
- Create: `crates/manifold-engine-webview2/src/composition.rs`
- Modify: `crates/manifold-engine-webview2/src/main.rs:9-17` (add `mod composition;`)
- Modify: `crates/manifold-engine-webview2/Cargo.toml` (promote `windows-numerics` out of `[dev-dependencies]`)

**Interfaces:**
- Produces: `pub fn create_composition_target(hwnd: HWND, width: u32, height: u32) -> windows::core::Result<(DesktopWindowTarget, ContainerVisual)>` — the target keeps the visual tree alive on that window; the visual is what both `main.rs` (`SetRootVisualTarget`) and `capture.rs` (`CreateFromVisual`) need a reference to. `pub fn resize_root_visual(root: &ContainerVisual, width: u32, height: u32)`.
- Consumes: nothing from other tasks — this is the first, self-contained piece.

This is the same setup already proven live in `examples/composition_input_spike.rs` and `examples/composition_capture_spike.rs` this session, extracted into a real module instead of copy-pasted per spike.

- [ ] **Step 1: Move `windows-numerics` to a real dependency**

In `crates/manifold-engine-webview2/Cargo.toml`, the current `[dev-dependencies]` section reads:

```toml
[dev-dependencies]
# Only used by `examples/composition_input_spike.rs` for `Vector2` -- pinned
# to match the version `windows` 0.62.2 itself depends on (see that crate's
# own `[dependencies.windows-numerics]`), same reasoning as the `webview2-com`
# pin above.
windows-numerics = { version = "0.3.1", default-features = false, features = ["std"] }
```

Delete that `[dev-dependencies]` section entirely, and add the same line to `[dependencies]` instead, right after the `windows = { ... }` block, with an updated comment:

```toml
# Real dependency now (`composition.rs`'s `Vector2` for visual sizing), not
# example-only -- pinned to match the version `windows` 0.62.2 itself
# depends on (see that crate's own `[dependencies.windows-numerics]`), same
# reasoning as the `webview2-com` pin above.
windows-numerics = { version = "0.3.1", default-features = false, features = ["std"] }
```

- [ ] **Step 2: Write `composition.rs`**

```rust
//! Sets up the `Windows.UI.Composition` visual tree WebView2's Composition
//! hosting (`ICoreWebView2CompositionController`) connects to, and that
//! `capture.rs` captures directly via `GraphicsCaptureItem::CreateFromVisual`
//! -- see the migration design doc
//! (`docs/superpowers/specs/2026-09-20-composition-hosting-migration-design.md`)
//! for why this replaced plain windowed hosting.

use windows::core::{Interface, Result};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::WinRT::Composition::ICompositorDesktopInterop;
use windows::UI::Composition::Desktop::DesktopWindowTarget;
use windows::UI::Composition::{Compositor, ContainerVisual};
use windows_numerics::Vector2;

/// `hwnd` still needs to be a real top-level window (Composition hosting
/// doesn't render into its client area, but it's what the visual tree
/// attaches to and what `ICoreWebView2CompositionController` is created
/// against). Returns `(target, root)` -- `target` must be kept alive for
/// as long as the visual tree should stay connected to the window (drop it
/// and the tree detaches); `root` is what `SetRootVisualTarget` and
/// `GraphicsCaptureItem::CreateFromVisual` both need a reference to.
pub fn create_composition_target(
    hwnd: HWND,
    width: u32,
    height: u32,
) -> Result<(DesktopWindowTarget, ContainerVisual)> {
    let compositor = Compositor::new()?;
    let desktop_interop: ICompositorDesktopInterop = compositor.cast()?;
    let target = unsafe { desktop_interop.CreateDesktopWindowTarget(hwnd, false)? };
    let root = compositor.CreateContainerVisual()?;
    resize_root_visual(&root, width, height);
    target.SetRoot(&root)?;
    Ok((target, root))
}

pub fn resize_root_visual(root: &ContainerVisual, width: u32, height: u32) {
    let _ = root.SetSize(Vector2 { X: width as f32, Y: height as f32 });
}
```

- [ ] **Step 3: Register the module**

In `crates/manifold-engine-webview2/src/main.rs`, the module list currently reads:

```rust
mod audio;
mod capture;
mod download;
mod find;
mod input;
mod nav_state;
mod popup;
mod scheme;
mod window;
```

Add `mod composition;` alphabetically:

```rust
mod audio;
mod capture;
mod composition;
mod download;
mod find;
mod input;
mod nav_state;
mod popup;
mod scheme;
mod window;
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo build -p manifold-engine-webview2`
Expected: builds clean (this module isn't called from anywhere yet, so no runtime behavior to check — a `cargo build` warning about `create_composition_target`/`resize_root_visual` being unused is expected and fine at this point, it gets consumed in Task 3).

- [ ] **Step 5: Commit**

```bash
git add crates/manifold-engine-webview2/src/composition.rs crates/manifold-engine-webview2/src/main.rs crates/manifold-engine-webview2/Cargo.toml
git commit -m "Add composition.rs: Compositor/visual-tree setup for WebView2 Composition hosting"
```

---

### Task 2: `capture.rs` — capture from the visual, delete window-cropping

**Files:**
- Modify: `crates/manifold-engine-webview2/src/capture.rs` (whole file restructured; see below for exact before/after)

**Interfaces:**
- Consumes: `ContainerVisual` (from Task 1, via `windows::UI::Composition::ContainerVisual` — Task 2 doesn't call `composition.rs` directly, `main.rs` does in Task 3 and passes the visual in).
- Produces: `pub fn Capture::new(visual: &ContainerVisual) -> Result<Self>` (signature changed from `Capture::new(hwnd: HWND)` — every caller must update, which is Task 3).

The current file (`crates/manifold-engine-webview2/src/capture.rs`) has this shape:

```rust
pub struct Capture {
    hwnd: HWND,
    d3d_device: ID3D11Device,
    d3d_context: ID3D11DeviceContext,
    frame_pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    latest: Arc<Mutex<Option<Direct3D11CaptureFrame>>>,
    staging: Option<(ID3D11Texture2D, u32, u32)>,
}

fn client_rect_in_window(hwnd: HWND) -> Option<(i32, i32, u32, u32)> { /* ... */ }

impl Capture {
    pub fn new(hwnd: HWND) -> Result<Self> {
        /* ... creates item via IGraphicsCaptureItemInterop::CreateForWindow(hwnd) ... */
    }
    pub fn try_get_latest_frame(&mut self) -> Option<(Vec<u8>, u32, u32)> {
        /* ... crops using client_rect_in_window(self.hwnd) ... */
    }
    pub fn resize(&mut self, width: u32, height: u32) { /* ... */ }
}
```

- [ ] **Step 1: Remove the `hwnd` field and `client_rect_in_window`**

Delete the entire `client_rect_in_window` function (it existed only to crop out a real window's non-client chrome — a `ContainerVisual` has none to crop). Remove the `hwnd: HWND` field from the `Capture` struct. Remove the now-unused imports `windows::Win32::Graphics::Gdi::ClientToScreen` and `windows::Win32::UI::WindowsAndMessaging::{GetClientRect, GetWindowRect}` and `windows::Win32::Foundation::POINT` (all were only used by `client_rect_in_window`).

- [ ] **Step 2: Rewrite `Capture::new`**

Replace:

```rust
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
```

with:

```rust
use windows::Graphics::Capture::GraphicsCaptureItem;
use windows::UI::Composition::ContainerVisual;
```

(`GraphicsCaptureItem` was already imported for its type in frame-pool creation; keep that one import, just add the `ContainerVisual` one and drop the now-unused `IGraphicsCaptureItemInterop`.)

Change the signature and body:

```rust
impl Capture {
    pub fn new(visual: &ContainerVisual) -> Result<Self> {
        let (d3d_device, d3d_context) = create_d3d11_device()?;
        let dxgi_device: IDXGIDevice = d3d_device.cast()?;
        let winrt_device = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi_device)? };
        let winrt_device: windows::Graphics::DirectX::Direct3D11::IDirect3DDevice =
            winrt_device.cast()?;

        let item: GraphicsCaptureItem = GraphicsCaptureItem::CreateFromVisual(visual)?;

        let frame_pool = Direct3D11CaptureFramePool::Create(
            &winrt_device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            2,
            item.Size()?,
        )?;
        let session = frame_pool.CreateCaptureSession(&item)?;

        let latest: Arc<Mutex<Option<Direct3D11CaptureFrame>>> = Arc::new(Mutex::new(None));
        {
            let latest = latest.clone();
            frame_pool.FrameArrived(&TypedEventHandler::new(
                move |pool: windows::core::Ref<Direct3D11CaptureFramePool>, _| {
                    if let Some(pool) = pool.as_ref() {
                        if let Ok(frame) = pool.TryGetNextFrame() {
                            if let Some(old) = latest.lock().unwrap().replace(frame) {
                                let _ = old.Close();
                            }
                        }
                    }
                    Ok(())
                },
            ))?;
        }

        session.StartCapture()?;

        Ok(Self {
            d3d_device,
            d3d_context,
            frame_pool,
            session,
            latest,
            staging: None,
        })
    }
    // ... rest unchanged for now, fixed in Step 3
}
```

(This is `examples/composition_capture_spike.rs`'s already-verified-live capture setup, adapted to `Capture::new`'s existing constructor shape — same field names, same `Arc<Mutex<...>>`/`FrameArrived` pattern the file already uses, just `CreateFromVisual` instead of `CreateForWindow`.)

- [ ] **Step 3: Remove the crop from `try_get_latest_frame`**

The current body computes `(crop_x, crop_y, crop_w, crop_h)` via `client_rect_in_window(self.hwnd)` with a fallback to the full frame, then copies only that cropped region. Since there's no `self.hwnd` anymore and nothing to crop, replace that whole crop-then-copy block with a direct full-frame copy:

```rust
        // No cropping needed -- unlike `CreateForWindow` (which captured a
        // whole window including non-client chrome, requiring the old
        // `client_rect_in_window` crop), `CreateFromVisual` captures exactly
        // the visual's own content, nothing else.
        let mut pixels = vec![0u8; (desc.Width * desc.Height * 4) as usize];
        unsafe {
            let src = mapped.pData as *const u8;
            for row in 0..desc.Height as usize {
                let src_row = src.add(row * mapped.RowPitch as usize);
                let dst_row = pixels.as_mut_ptr().add(row * desc.Width as usize * 4);
                std::ptr::copy_nonoverlapping(src_row, dst_row, desc.Width as usize * 4);
            }
            self.d3d_context.Unmap(staging, 0);
        }

        Some((pixels, desc.Width, desc.Height))
```

(replacing the previous crop-rect computation, the cropped-size `pixels` allocation, and the crop-aware copy loop that referenced `crop_x`/`crop_y`/`crop_w`/`crop_h`).

- [ ] **Step 4: `resize` takes the visual, not just dimensions**

Current signature: `pub fn resize(&mut self, width: u32, height: u32)` — this only recreates the frame pool, it never touched `self.hwnd` for sizing (the window resize was `window.rs`'s job). No signature change is actually needed here; the body is unchanged. Leave `resize` exactly as it is.

- [ ] **Step 5: Verify it compiles**

Run: `cargo build -p manifold-engine-webview2`
Expected: fails — `main.rs`'s `Capture::new(hwnd)` call site doesn't match the new `Capture::new(&ContainerVisual)` signature yet. This is expected; Task 3 fixes the call site. Confirm the *only* errors are at that one call site (in `main.rs`), not inside `capture.rs` itself — that isolates that this file's own rewrite is internally correct.

- [ ] **Step 6: Commit**

```bash
git add crates/manifold-engine-webview2/src/capture.rs
git commit -m "capture.rs: capture from the Composition visual instead of the window, drop chrome-cropping"
```

---

### Task 3: `main.rs` — switch controller creation and resize to Composition hosting

**Files:**
- Modify: `crates/manifold-engine-webview2/src/main.rs:9-17` (imports), `:122-132` (hosting setup), `:172` (`Capture::new` call), `:217-233` (`Resize` handler), `:410-478` (`create_controller`/`resize_controller`)

**Interfaces:**
- Consumes: `composition::create_composition_target`/`resize_root_visual` (Task 1), `Capture::new(&ContainerVisual)` (Task 2).
- Produces: the running binary now hosts via `ICoreWebView2CompositionController`; `ICoreWebView2Controller`-typed `controller` variable stays the same type used everywhere else in the file (zoom, bounds, focus) since the composition controller object also implements it.

- [ ] **Step 1: Update imports**

In the `webview2_com::Microsoft::Web::WebView2::Win32::{...}` import block, add `ICoreWebView2CompositionController` and `ICoreWebView2Environment3`:

```rust
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2CompositionController, ICoreWebView2Controller,
    ICoreWebView2Environment, ICoreWebView2Environment3, ICoreWebView2_8,
    COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC,
};
use webview2_com::{
    CreateCoreWebView2CompositionControllerCompletedHandler, CreateCoreWebView2EnvironmentCompletedHandler,
};
use windows::core::{Interface, BOOL, HSTRING};
use windows::UI::Composition::ContainerVisual;
```

(`Interface` is needed for `.cast::<ICoreWebView2Environment3>()`; `ContainerVisual` for the new variable holding the root visual.)

- [ ] **Step 2: Rewrite `create_controller` into `create_composition_controller`**

Replace the whole function (lines 435-466, including its now-stale doc comment that literally already predicted this migration: *"Phase 2 swaps this for `CreateCoreWebView2CompositionController`/`ICoreWebView2CompositionController` once the real capture pipeline needs a composition visual tree"*):

```rust
/// Returns `(comp_controller, controller, webview)` -- `comp_controller`
/// (`ICoreWebView2CompositionController`) is used for `SendMouseInput`/
/// `SetRootVisualTarget`; `controller` (`ICoreWebView2Controller`) for
/// bounds/visibility/zoom/focus exactly as before (the object
/// `CreateCoreWebView2CompositionController` returns implements both
/// interfaces -- confirmed live this session, not assumed); `webview`
/// (`ICoreWebView2`) for navigation/events/keyboard input, also unchanged.
fn create_composition_controller(
    environment: ICoreWebView2Environment,
    parent: windows::Win32::Foundation::HWND,
) -> webview2_com::Result<(ICoreWebView2CompositionController, ICoreWebView2Controller, ICoreWebView2)> {
    let environment3: ICoreWebView2Environment3 =
        environment.cast().map_err(webview2_com::Error::WindowsError)?;
    let (tx, rx) = mpsc::channel();
    CreateCoreWebView2CompositionControllerCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| unsafe {
            environment3
                .CreateCoreWebView2CompositionController(parent, &handler)
                .map_err(webview2_com::Error::WindowsError)
        }),
        Box::new(move |error_code, controller| {
            error_code?;
            tx.send(controller.ok_or_else(|| {
                windows::core::Error::from(windows::Win32::Foundation::E_POINTER)
            }))
            .expect("send composition controller over mpsc channel");
            Ok(())
        }),
    )?;
    let comp_controller = webview2_com::wait_with_pump(rx)??;
    let controller: ICoreWebView2Controller =
        comp_controller.cast().map_err(webview2_com::Error::WindowsError)?;
    let webview = unsafe { controller.CoreWebView2() }?;
    Ok((comp_controller, controller, webview))
}
```

- [ ] **Step 3: Update `resize_controller` to also resize the visual**

Current:

```rust
fn resize_controller(controller: &ICoreWebView2Controller, width: u32, height: u32) {
    let bounds = RECT {
        left: 0,
        top: 0,
        right: width as i32,
        bottom: height as i32,
    };
    unsafe {
        let _ = controller.SetBounds(bounds);
    }
}
```

Add a `root_visual: &ContainerVisual` parameter and resize it too:

```rust
fn resize_controller(controller: &ICoreWebView2Controller, root_visual: &ContainerVisual, width: u32, height: u32) {
    let bounds = RECT {
        left: 0,
        top: 0,
        right: width as i32,
        bottom: height as i32,
    };
    unsafe {
        let _ = controller.SetBounds(bounds);
    }
    composition::resize_root_visual(root_visual, width, height);
}
```

- [ ] **Step 4: Rewire `main()`'s hosting setup**

Current (lines 122-132):

```rust
    // Plain *windowed* WebView2 hosting against a real, off-screen host
    // window -- see `window.rs`'s doc comment for why this ended up
    // simpler than the originally-planned Composition/Visual hosting once
    // `Windows.Graphics.Capture`'s actual constraints were discovered live.
    let hwnd = window::create_hidden_window().expect("create hidden host window");

    let environment = create_environment().expect("create CoreWebView2Environment");
    let (controller, webview) =
        create_controller(environment.clone(), hwnd).expect("create CoreWebView2Controller");

    resize_controller(&controller, DEFAULT_WIDTH, DEFAULT_HEIGHT);
```

Replace with:

```rust
    // Composition hosting against a real, off-screen host window -- see
    // docs/superpowers/specs/2026-09-20-composition-hosting-migration-design.md
    // for why this replaced the plain windowed hosting `window.rs`'s own
    // doc comment used to describe here (that doc comment was itself
    // written expecting this exact migration -- see `create_composition_controller`'s
    // own doc comment).
    let hwnd = window::create_hidden_window().expect("create hidden host window");
    let (_composition_target, root_visual) =
        composition::create_composition_target(hwnd, DEFAULT_WIDTH, DEFAULT_HEIGHT)
            .expect("create composition target");

    let environment = create_environment().expect("create CoreWebView2Environment");
    let (comp_controller, controller, webview) =
        create_composition_controller(environment.clone(), hwnd).expect("create CoreWebView2CompositionController");
    unsafe {
        let _ = comp_controller.SetRootVisualTarget(&root_visual);
    }

    resize_controller(&controller, &root_visual, DEFAULT_WIDTH, DEFAULT_HEIGHT);
```

`_composition_target` is prefixed `_` deliberately live but unread: it must stay alive for the process lifetime (dropping it detaches the visual tree from the window) but nothing ever calls a method on it again after `SetRoot` happened inside `create_composition_target`.

- [ ] **Step 5: Update the `Capture::new` call site**

Current (line 172): `let mut capture = Capture::new(hwnd).expect("create capture pipeline");`

Replace with:

```rust
    let mut capture = Capture::new(&root_visual).expect("create capture pipeline");
```

- [ ] **Step 6: Update the `Resize` handler**

Current (lines 217-233, includes the temp diagnostic from this session's earlier DPI debugging):

```rust
                HostToEngine::Resize { width, height, dpi_scale } => {
                    // TEMP diagnostic for the live black-bar bug -- remove
                    // once resolved.
                    eprintln!("[input-diag] Resize received width={width} height={height} dpi_scale={dpi_scale}");
                    window::resize(hwnd, width, height);
                    unsafe {
                        let mut client_rect = RECT::default();
                        let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut client_rect);
                        eprintln!(
                            "[input-diag]   GetClientRect after resize: {}x{}",
                            client_rect.right - client_rect.left,
                            client_rect.bottom - client_rect.top
                        );
                    }
                    resize_controller(&controller, width, height);
                    capture.resize(width, height);
                }
```

Replace with (dropping the temp diagnostic now that its job -- finding the DPI-awareness bugs -- is done and confirmed fixed, and adding `&root_visual` to the `resize_controller` call):

```rust
                HostToEngine::Resize { width, height, .. } => {
                    window::resize(hwnd, width, height);
                    resize_controller(&controller, &root_visual, width, height);
                    capture.resize(width, height);
                }
```

- [ ] **Step 7: Build**

Run: `cargo build -p manifold-engine-webview2`
Expected: builds clean. If `unused import` warnings appear for anything removed by Task 2's cleanup (e.g. `RECT` if it's no longer referenced elsewhere in `main.rs` -- check first, `resize_controller` still uses it, so it should remain used), fix them.

- [ ] **Step 8: Live-verify Phase 1 against the real multi-tab app**

This is the live-verification step the Global Constraints call for -- there is no automated test for this.

Run: `cargo build -p manifold -p manifold-engine-webview2`, then launch `target\debug\manifold.exe` directly (not via a background job whose output handle you'll lose, per this session's own earlier lesson -- use `Start-Process -RedirectStandardError <file>` for the main process, both processes' `[input-diag]`-style output already goes through the same inherited stderr).

Check, by screenshot or by looking at the real window:
1. A tab's real page content renders correctly (no black screen, no blank/blank-colored quad).
2. Resizing the window live still resizes the content correctly (no white/black bar at any edge -- this session's own earlier DPI bugs are a good regression check).
3. Opening a second tab and using split view still renders both correctly.
4. No `[input-diag] Resize received...`/`GetClientRect after resize` lines appear anymore (confirms Step 6 actually removed them) -- but a "captured frame size changed to WxH" line (if any similar diagnostic remains elsewhere) is fine to still exist as a real, non-temporary log; check `capture.rs`/`main.rs` don't have any other leftover TEMP-labeled diagnostics from this session's DPI investigation before calling Phase 1 done.

Expected: all four hold. If not, this is exactly the kind of thing to root-cause live (add a temporary `eprintln!` at the suspect point, rebuild, rerun) the same way every bug this session was actually found -- do not guess at a fix without reproducing first.

- [ ] **Step 9: Commit**

```bash
git add crates/manifold-engine-webview2/src/main.rs
git commit -m "main.rs: host WebView2 via Composition controller, capture from the visual"
```

---

### Task 4: `input.rs` — native `SendMouseInput` for mouse, keyboard unchanged

**Files:**
- Modify: `crates/manifold-engine-webview2/src/input.rs` (whole `send_input` mouse-handling body; keyboard handling untouched)
- Modify: `crates/manifold-engine-webview2/src/main.rs:234-236` (the one `input::send_input` call site)

**Interfaces:**
- Consumes: `ICoreWebView2CompositionController` (Task 3's `comp_controller`).
- Produces: `pub fn send_input(webview: &ICoreWebView2, comp_controller: &ICoreWebView2CompositionController, event: InputEvent, pending_context_menu: &PendingContextMenu, drag_state: &DragState)` -- one new parameter inserted; every other type in the signature is unchanged from today.

The current `input.rs` (from this session's earlier work) has, inside `send_input`'s `match event`:

```rust
        InputEvent::MouseDown { x, y, button: MouseButton::Left, .. } => {
            drag_state.set(Drag { active: true, moved: false });
            exec(
                webview,
                format!(
                    "(function(){{{guard}var r=document.caretRangeFromPoint({x},{y});if(r){{var s=window.getSelection();s.removeAllRanges();s.collapse(r.startContainer,r.startOffset);}}}})();",
                    guard = skip_form_controls_guard(x, y),
                ),
            );
        }
        InputEvent::MouseDown { .. } => {}
        InputEvent::MouseMove { x, y, .. } => {
            let mut drag = drag_state.get();
            if drag.active {
                drag.moved = true;
                drag_state.set(drag);
                exec(
                    webview,
                    format!(
                        "(function(){{{guard}var r=document.caretRangeFromPoint({x},{y});if(r){{window.getSelection().extend(r.startContainer,r.startOffset);}}}})();",
                        guard = skip_form_controls_guard(x, y),
                    ),
                );
            }
        }
        InputEvent::MouseUp { x, y, button: MouseButton::Right, .. } => {
            drag_state.set(Drag::default());
            /* ... right-click href-detection for context menu, unchanged ... */
        }
        InputEvent::MouseUp { x, y, .. } => {
            let moved = drag_state.get().moved;
            drag_state.set(Drag::default());
            if !moved {
                exec(
                    webview,
                    format!(
                        "(function(){{var e=document.elementFromPoint({x},{y});if(e){{if(e.focus){{e.focus();}}e.click();}}}})();"
                    ),
                );
            }
        }
        InputEvent::Scroll { x, y, delta_x, delta_y, .. } => {
            exec(
                webview,
                format!(
                    "(function(){{var el=document.elementFromPoint({x},{y});while(el&&el!==document.body&&el!==document.documentElement){{/* scrollable-ancestor walk */}}window.scrollBy({delta_x},{delta_y});}})();"
                ),
            );
        }
```

- [ ] **Step 1: Add imports**

```rust
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2CompositionController, COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_DOWN,
    COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_UP, COREWEBVIEW2_MOUSE_EVENT_KIND_MOVE,
    COREWEBVIEW2_MOUSE_EVENT_KIND_RIGHT_BUTTON_DOWN, COREWEBVIEW2_MOUSE_EVENT_KIND_RIGHT_BUTTON_UP,
    COREWEBVIEW2_MOUSE_EVENT_KIND_WHEEL, COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_LEFT_BUTTON,
    COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_NONE,
};
use windows::Win32::Foundation::POINT;
```

(alongside the existing `webview2_com::Microsoft::Web::WebView2::Win32::{ICoreWebView2, ...}` import already there for keyboard).

- [ ] **Step 2: Change `send_input`'s signature**

```rust
pub fn send_input(
    webview: &ICoreWebView2,
    comp_controller: &ICoreWebView2CompositionController,
    event: InputEvent,
    pending_context_menu: &PendingContextMenu,
    drag_state: &DragState,
) {
```

- [ ] **Step 3: Rewrite `MouseDown`/`MouseMove`/`MouseUp` (left button)**

Replace the `MouseDown { button: MouseButton::Left, .. }`, the fallthrough `MouseDown { .. }`, `MouseMove`, and the non-right-click `MouseUp` arms with:

```rust
        InputEvent::MouseDown { x, y, button, .. } => {
            let (kind, keys) = match button {
                MouseButton::Left => (
                    COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_DOWN,
                    COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_LEFT_BUTTON,
                ),
                MouseButton::Right => (
                    COREWEBVIEW2_MOUSE_EVENT_KIND_RIGHT_BUTTON_DOWN,
                    COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_NONE,
                ),
                MouseButton::Middle => return,
            };
            if matches!(button, MouseButton::Left) {
                drag_state.set(Drag { active: true, moved: false });
            }
            unsafe {
                let _ = comp_controller.SendMouseInput(kind, keys, 0, POINT { x: x as i32, y: y as i32 });
            }
        }
        InputEvent::MouseMove { x, y, .. } => {
            let mut drag = drag_state.get();
            let keys = if drag.active {
                drag.moved = true;
                drag_state.set(drag);
                COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_LEFT_BUTTON
            } else {
                COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_NONE
            };
            unsafe {
                let _ = comp_controller.SendMouseInput(
                    COREWEBVIEW2_MOUSE_EVENT_KIND_MOVE,
                    keys,
                    0,
                    POINT { x: x as i32, y: y as i32 },
                );
            }
        }
```

Note `x`/`y` are `f64` in `InputEvent` but `POINT` wants `i32` -- `as i32` truncates, matching how the JS approach also implicitly truncated (JS coordinates were interpolated into a string via `format!`, which for a `f64` produces a decimal the browser's own CSS-pixel parsing accepted; `SendMouseInput` wants whole pixels, so truncation here is the correct equivalent, not a regression).

- [ ] **Step 4: Rewrite `MouseUp` (both right-click and left/other)**

Keep the right-click arm's *href-detection* logic (it doesn't depend on how the click was delivered), but send a real button-up first, and change the plain arm to send a real up instead of the old `.focus()`/`.click()` JS:

```rust
        InputEvent::MouseUp { x, y, button: MouseButton::Right, .. } => {
            drag_state.set(Drag::default());
            unsafe {
                let _ = comp_controller.SendMouseInput(
                    COREWEBVIEW2_MOUSE_EVENT_KIND_RIGHT_BUTTON_UP,
                    COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_NONE,
                    0,
                    POINT { x: x as i32, y: y as i32 },
                );
            }
            let script = format!(
                "(function(){{var e=document.elementFromPoint({x},{y});var a=e&&e.closest('a[href]');return a?a.href:null;}})();"
            );
            let pending = pending_context_menu.clone();
            let handler = ExecuteScriptCompletedHandler::create(Box::new(move |_error, result| {
                let json = result.to_string();
                if let Some(href) = parse_json_string_or_null(&json) {
                    *pending.borrow_mut() = Some((x, y, href));
                }
                Ok(())
            }));
            unsafe {
                let _ = webview.ExecuteScript(&HSTRING::from(script), &handler);
            }
        }
        InputEvent::MouseUp { x, y, button, .. } => {
            drag_state.set(Drag::default());
            let kind = match button {
                MouseButton::Left => COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_UP,
                MouseButton::Middle => return,
                MouseButton::Right => unreachable!("handled above"),
            };
            unsafe {
                let _ = comp_controller.SendMouseInput(
                    kind,
                    COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_NONE,
                    0,
                    POINT { x: x as i32, y: y as i32 },
                );
            }
        }
```

Note the old `moved` check (skip the synthetic click after a real drag) is gone entirely -- it existed only because the JS approach's `.click()` was a *separate, extra* action from the browser's own native mousedown/mouseup handling, and firing it after a drag would wrongly re-trigger a click. `SendMouseInput`'s up/down *are* the real mousedown/mouseup Chromium's own click-vs-drag disambiguation already runs on internally -- there's nothing extra to suppress. Keep `drag_state.get()`/`.set(Drag::default())` only for what `MouseMove` still needs (whether to include the left-button virtual key on subsequent moves).

- [ ] **Step 5: Rewrite `Scroll`**

Replace the whole scrollable-ancestor-walk JS block with:

```rust
        InputEvent::Scroll { x, y, delta_x, delta_y, .. } => {
            unsafe {
                let _ = comp_controller.SendMouseInput(
                    COREWEBVIEW2_MOUSE_EVENT_KIND_WHEEL,
                    COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_NONE,
                    delta_y as i32 as u32,
                    POINT { x: x as i32, y: y as i32 },
                );
            }
        }
```

`mousedata` (the third argument) carries the wheel delta, matching Win32's real `WM_MOUSEWHEEL` `HIWORD(wParam)` convention -- confirmed this exact convention live earlier this session's `composition_input_spike.rs`. `delta_x` (horizontal scroll) has no dedicated `COREWEBVIEW2_MOUSE_EVENT_KIND` in this SDK version; if horizontal scroll turns out to matter in practice, that's `COREWEBVIEW2_MOUSE_EVENT_KIND_HORIZONTAL_WHEEL` as a follow-up, not part of this task.

- [ ] **Step 6: Leave `KeyDown`/`KeyUp`/`Char` untouched**

Confirm (don't change) that `dispatch_key_event`, the `InputEvent::KeyDown`/`KeyUp` arms, and the `InputEvent::Char` arm still call `CallDevToolsProtocolMethod`/`ExecuteScript` on `webview: &ICoreWebView2` exactly as before Task 1 started. `key_info`, `cdp_modifiers`, `dispatch_key_event` need zero changes.

- [ ] **Step 7: Remove now-unused helpers**

`skip_form_controls_guard` was only used by the deleted `caretRangeFromPoint`-based mouse scripts -- delete that function. `exec` (the plain fire-and-forget `ExecuteScript` helper) is still used by `Char` and the right-click href-detection script -- keep it.

- [ ] **Step 8: Update the call site in `main.rs`**

Current (`main.rs`, inside the `HostToEngine::Input` arm):

```rust
                HostToEngine::Input(event) => {
                    input::send_input(&webview, event, &pending_context_menu, &drag_state);
                }
```

Replace with:

```rust
                HostToEngine::Input(event) => {
                    input::send_input(&webview, &comp_controller, event, &pending_context_menu, &drag_state);
                }
```

- [ ] **Step 9: Build**

Run: `cargo build -p manifold-engine-webview2`
Expected: builds clean.

- [ ] **Step 10: Commit**

```bash
git add crates/manifold-engine-webview2/src/input.rs crates/manifold-engine-webview2/src/main.rs
git commit -m "input.rs: drive mouse input via native SendMouseInput, keyboard unchanged"
```

---

### Task 5: Live-verify Phase 2 against the real app, decide on the `.focus()` open question

**Files:**
- None modified unless verification surfaces a real gap (see Step 3).

**Interfaces:**
- Consumes: everything from Tasks 1-4.
- Produces: a settled answer to the spec's deferred question ("does native mousedown+click still need the explicit `.focus()` workaround `input.rs` added earlier this session for plain `<input>` elements").

- [ ] **Step 1: Build and launch the real app**

Run: `cargo build -p manifold -p manifold-engine-webview2`, then launch `target\debug\manifold.exe` directly and open a tab with real content (e.g. a page with a plain `<input>`, a paragraph of text, and a nested scrollable div -- the same shape this session's `drag_select_verify.rs` used).

- [ ] **Step 2: Manually verify each behavior**

Check, by directly interacting with the running app (real mouse, not a spike):
1. Clicking a link navigates.
2. Clicking into a plain `<input>` focuses it and typing works (this is the specific case that needed the `.focus()` workaround under the old JS approach -- confirm whether native mousedown+click focuses it without any extra help).
3. Dragging across a line of text selects it, and copy (Ctrl+C, via keyboard which is unchanged) copies the selected text.
4. Scrolling a nested scrollable panel (not just the page) scrolls that panel, not the whole page.
5. Right-click on a link still brings up this app's own context menu with the right href.
6. Click-and-drag inside a same-origin `<iframe>` on a real page (e.g. an embedded video player's own controls) reaches the iframe's content correctly -- the spec calls this out as a theorized advantage of native input (Chromium's own hit-testing routes it, unlike CDP which would need explicit `Target.attachToTarget` session bookkeeping this project doesn't have); confirm rather than leave theorized.
7. This app's find-in-page bar, a real download, and the audio-playing indicator (tab icon) each still work on a real page -- none of `find.rs`/`download.rs`/`audio.rs` were touched by this migration, but the spec explicitly calls for confirming that rather than assuming it from the interface types alone.

Expected: all seven hold.

- [ ] **Step 3: Act on the `.focus()` finding**

If (2) works without any extra code (native mousedown's own default focus-assignment behavior handles it, unlike the old JS `.click()` which never did): nothing to change, this confirms the spec's open question in the "removes a workaround" direction -- update the spec's "Open items deliberately deferred" section to record the finding, but no code change needed since `input.rs`'s new `MouseDown`/`MouseUp` never reintroduced the old `.focus()` call in Task 4.

If (2) does *not* focus the field: that's a real regression to root-cause before calling this task done -- add a temporary diagnostic (`document.activeElement.id` read-back via `ExecuteScript`, matching this session's own established diagnostic pattern) rather than guessing at a fix.

- [ ] **Step 4: Commit (only if Step 3 required a code or spec change)**

```bash
git add -A
git commit -m "Record Phase 2 verification results; note focus-on-click behavior under native input"
```

---

### Task 6: Delete now-dead example spikes, sweep for stale references

**Files:**
- Delete: `crates/manifold-engine-webview2/examples/composition_input_spike.rs`, `composition_capture_spike.rs`, `composition_stress_spike.rs`, `composition_keyboard_spike.rs`, `drag_select_verify.rs` (all five were explicitly throwaway, built solely to answer questions this migration now definitively answers in real code)
- Modify: `crates/manifold-engine-webview2/src/main.rs:1-7` (top-of-file doc comment), `crates/manifold-engine-webview2/src/input.rs` (its own doc comment history, if it still describes the old JS approach as current)
- Modify (if present): `crates/manifold-engine-webview2/Cargo.toml` -- remove any feature that was *only* needed by a deleted example (re-check after deletion; `Win32_UI_Input_Pointer` in particular was added for `composition_keyboard_spike.rs`'s touch/pointer exploration and may now be genuinely unused by both the real code and any remaining example)

**Interfaces:** None -- pure cleanup, no behavior change.

- [ ] **Step 1: Delete the spike files**

```bash
git rm crates/manifold-engine-webview2/examples/composition_input_spike.rs \
       crates/manifold-engine-webview2/examples/composition_capture_spike.rs \
       crates/manifold-engine-webview2/examples/composition_stress_spike.rs \
       crates/manifold-engine-webview2/examples/composition_keyboard_spike.rs \
       crates/manifold-engine-webview2/examples/drag_select_verify.rs
```

(`capture_spike.rs`, the *original* Phase-1/round-4-era spike referenced throughout this project's own history, stays -- it's a different, older, still-referenced-by-doc-comments artifact, not part of this migration's throwaway work.)

- [ ] **Step 2: Update `main.rs`'s top-of-file doc comment**

Current:

```rust
//! WebView2 engine-host (see the M6 plan). Structured to mirror
//! `manifold-engine-cef/src/main.rs`'s shape (fixed-tick poll loop,
//! `SharedNavState` + `dirty` flag) since CEF is the closest existing
//! analog: a real Windows process, no visible window, same-machine IPC.
//! Real frame capture via `Windows.Graphics.Capture` -- see `capture.rs`'s
//! doc comment for why it captures the host window directly rather than
//! the originally-planned DirectComposition Visual.
```

Replace the last two lines (now factually wrong -- it captures the visual, not the window):

```rust
//! Real frame capture via `Windows.Graphics.Capture`, capturing the
//! Composition visual WebView2 renders into directly (`capture.rs`) --
//! see docs/superpowers/specs/2026-09-20-composition-hosting-migration-design.md
//! for why this replaced the earlier windowed-hosting + window-capture
//! approach.
```

- [ ] **Step 3: Check `input.rs`'s module doc comment for stale claims**

Its doc comment currently narrates the whole round-1-through-4 JS/CDP history in detail. Read it fresh after Task 4's changes land and add one final paragraph noting the mouse side moved to native `SendMouseInput` under Composition hosting (pointing at the spec), rather than rewriting the whole history -- the earlier rounds' documented failures (raw `WM_*`, round 2's abandoned attempt) are still true, valuable history explaining *why* round 4's JS approach was chosen at the time, even though it's now superseded for mouse.

- [ ] **Step 4: Re-check Cargo.toml features**

Run: `cargo build -p manifold-engine-webview2 --all-targets 2>&1 | grep -i "unused"` (or just read the build output) after Step 1's deletions. If `Win32_UI_Input_Pointer` (added for `composition_keyboard_spike.rs`'s `SendPointerInput` exploration, per that file's own now-deleted doc comment) is not referenced by any remaining file, remove it from the `windows = { features = [...] }` list in Cargo.toml. Do not remove `Win32_UI_Input_KeyboardAndMouse`, `UI_Composition`, `UI_Composition_Desktop`, `Win32_System_WinRT_Composition`, or `System` -- all four are used by the real, non-example code now (`composition.rs`, `main.rs`).

- [ ] **Step 5: Full workspace build check**

Run: `cargo build -p manifold -p manifold-engine-webview2`
Expected: clean build, no warnings about unused imports/features anywhere touched by this plan.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "Remove throwaway Composition/keyboard spikes now superseded by real code; update stale doc comments"
```
