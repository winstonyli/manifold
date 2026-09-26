//! Real frame capture via `Windows.Graphics.Capture`, replacing Phase 1's
//! fixed placeholder buffer. Captures the `ContainerVisual` `composition.rs`
//! creates and WebView2's Composition controller renders into, via
//! `GraphicsCaptureItem::CreateFromVisual` -- see that module's doc comment
//! and the migration design doc
//! (`docs/superpowers/specs/2026-09-20-composition-hosting-migration-design.md`)
//! for why this replaced the earlier `CreateForWindow`-against-the-host-
//! window approach.

use std::sync::{Arc, Mutex};

use windows::core::{Interface, Result};
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{
    Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
use windows::UI::Composition::ContainerVisual;

pub struct Capture {
    d3d_device: ID3D11Device,
    d3d_context: ID3D11DeviceContext,
    frame_pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    latest: Arc<Mutex<Option<Direct3D11CaptureFrame>>>,
    // Staging texture is recreated lazily whenever the captured frame's own
    // size changes (window resize) -- tracked here so `try_get_latest_frame`
    // doesn't recreate it every single call.
    staging: Option<(ID3D11Texture2D, u32, u32)>,
}

impl Capture {
    pub fn new(visual: &ContainerVisual) -> Result<Self> {
        let (d3d_device, d3d_context) = create_d3d11_device()?;
        let dxgi_device: IDXGIDevice = d3d_device.cast()?;
        let winrt_device = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi_device)? };
        let winrt_device: windows::Graphics::DirectX::Direct3D11::IDirect3DDevice = winrt_device.cast()?;

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
                            // If a previous frame was still sitting here
                            // unconsumed (the main loop hasn't called
                            // `try_get_latest_frame` since it arrived), it
                            // gets replaced here and must be explicitly
                            // `Close()`d -- same real leak `try_get_latest_
                            // frame` below fixes on the consume side, just
                            // hit here on the discard side instead.
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

        Ok(Self { d3d_device, d3d_context, frame_pool, session, latest, staging: None })
    }

    /// Non-blocking: `None` most calls (no new frame since the last one --
    /// capture cadence is tied to the compositor's own vsync, not something
    /// this process can throttle, see the M6 plan's own note on this).
    /// Copies out a tightly-packed BGRA8 buffer, handling the D3D11 staging
    /// texture's row-pitch padding explicitly -- a naive `RowPitch*height`
    /// copy into a tightly-packed destination produces a visibly skewed
    /// image whenever `RowPitch != width*4` (common; textures are often
    /// pitch-aligned wider than their nominal width).
    pub fn try_get_latest_frame(&mut self) -> Option<(Vec<u8>, u32, u32)> {
        let frame = self.latest.lock().unwrap().take()?;
        // `Direct3D11CaptureFrame::Close()` must be called explicitly once
        // we're done reading it -- dropping the COM wrapper alone (plain
        // `Release()`) does not return the frame to the capture pipeline
        // or free its underlying D3D11 surface. This was a real,
        // confirmed leak: every captured frame accumulated indefinitely,
        // reproduced live via a headless idle-memory test (WebView2 as
        // the default engine: ~20MB/s continuous growth from a single
        // idle tab; CEF as the default: flat over the same window,
        // ruling out anything shared on the manifold side). The rest of
        // this function's body is unchanged and wrapped in a closure
        // purely so its several early `?` returns still reach the
        // `frame.Close()` call below via `result`, rather than needing
        // one repeated at each early-return point.
        let result = (|| {
            let surface = frame.Surface().ok()?;
            let access: IDirect3DDxgiInterfaceAccess = surface.cast().ok()?;
            let texture: ID3D11Texture2D = unsafe { access.GetInterface().ok()? };

            let mut desc = D3D11_TEXTURE2D_DESC::default();
            unsafe { texture.GetDesc(&mut desc) };

            let need_new_staging = match &self.staging {
                Some((_, w, h)) => *w != desc.Width || *h != desc.Height,
                None => true,
            };
            if need_new_staging {
                let staging_desc = D3D11_TEXTURE2D_DESC {
                    Usage: D3D11_USAGE_STAGING,
                    CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                    BindFlags: 0,
                    MiscFlags: 0,
                    ..desc
                };
                let mut staging: Option<ID3D11Texture2D> = None;
                unsafe {
                    self.d3d_device.CreateTexture2D(&staging_desc, None, Some(&mut staging)).ok()?;
                }
                self.staging = Some((staging?, desc.Width, desc.Height));
            }
            let staging = &self.staging.as_ref()?.0;

            unsafe { self.d3d_context.CopyResource(staging, &texture) };

            let mut mapped = Default::default();
            unsafe {
                self.d3d_context.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).ok()?;
            }
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
        })();
        let _ = frame.Close();
        result
    }

    /// Recreates the frame pool at the new size -- unlike CEF (lazy resize,
    /// keyed off the next painted frame's own reported dimensions), this
    /// pipeline owns the capture's pixel dimensions directly, so `Resize`
    /// can apply synchronously with no "wait for the next frame to tell us
    /// its real size" step needed.
    pub fn resize(&mut self, width: u32, height: u32) {
        let size = windows::Graphics::SizeInt32 { Width: width as i32, Height: height as i32 };
        let _ = self.frame_pool.Recreate(
            &self
                .d3d_device
                .cast::<IDXGIDevice>()
                .and_then(|d| unsafe { CreateDirect3D11DeviceFromDXGIDevice(&d) })
                .and_then(|d| d.cast::<windows::Graphics::DirectX::Direct3D11::IDirect3DDevice>())
                .unwrap_or_else(|_| unreachable!("device was already valid at Capture::new")),
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            2,
            size,
        );
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.session.Close();
    }
}

fn create_d3d11_device() -> Result<(ID3D11Device, ID3D11DeviceContext)> {
    let mut device = None;
    let mut context = None;
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            windows::Win32::Foundation::HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )?;
    }
    Ok((device.unwrap(), context.unwrap()))
}
