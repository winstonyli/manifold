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
