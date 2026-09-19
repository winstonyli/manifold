//! Real frame capture via `Windows.Graphics.Capture`, replacing Phase 1's
//! fixed placeholder buffer. Captures the same `HWND` `window.rs` creates
//! for WebView2's windowed hosting directly -- see that module's doc
//! comment on `create_hidden_window` for why it's a plain
//! `WS_OVERLAPPEDWINDOW`, not the `WS_POPUP`/`WS_EX_TOOLWINDOW` originally
//! planned (both rejected by `CreateForWindow` with `E_INVALIDARG`,
//! confirmed live via `examples/capture_spike.rs` before this file was
//! written, not guessed).
//!
//! No `DirectComposition`/`Compositor`/`Visual` involved at all -- the
//! approved plan's original design (`IGraphicsCaptureItemInterop::
//! CreateForVisual`) doesn't exist in this project's pinned windows-rs
//! version (confirmed by reading the crate's own generated bindings:
//! `IGraphicsCaptureItemInterop` only has `CreateForWindow`/
//! `CreateForMonitor`). Capturing the real WebView2 host window directly
//! turned out simpler than the planned Visual-capture pipeline, not just a
//! fallback.

use std::sync::{Arc, Mutex};

use windows::core::{Interface, Result};
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{
    Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, GetWindowRect};

use windows::Win32::Foundation::POINT;

pub struct Capture {
    hwnd: HWND,
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

/// `Windows.Graphics.Capture`'s `CreateForWindow` captures the *whole*
/// window including its non-client chrome (title bar, borders) -- even a
/// `WS_OVERLAPPED` window with no `WS_CAPTION` still gets a minimal default
/// title bar drawn (confirmed live via `examples/capture_spike.rs`: the
/// captured PNG showed a visible title strip despite no caption style
/// requested). `WS_POPUP`/`WS_EX_TOOLWINDOW`, which would suppress that
/// chrome entirely, are exactly the styles `CreateForWindow` itself rejects
/// (see `window.rs`'s doc comment) -- so rather than fight window styles
/// further, this crops the captured frame down to just the client-area
/// rectangle, computed fresh each call via `GetClientRect`/`ClientToScreen`
/// (cheap Win32 calls) rather than cached, so it stays correct across any
/// resize without needing its own invalidation logic.
fn client_rect_in_window(hwnd: HWND) -> Option<(i32, i32, u32, u32)> {
    let mut window_rect = Default::default();
    unsafe { GetWindowRect(hwnd, &mut window_rect) }.ok()?;
    let mut client_rect = Default::default();
    unsafe { GetClientRect(hwnd, &mut client_rect) }.ok()?;
    let mut top_left = POINT { x: 0, y: 0 };
    if !unsafe { ClientToScreen(hwnd, &mut top_left) }.as_bool() {
        return None;
    }
    let offset_x = top_left.x - window_rect.left;
    let offset_y = top_left.y - window_rect.top;
    let width = (client_rect.right - client_rect.left).max(0) as u32;
    let height = (client_rect.bottom - client_rect.top).max(0) as u32;
    Some((offset_x, offset_y, width, height))
}

impl Capture {
    pub fn new(hwnd: HWND) -> Result<Self> {
        let (d3d_device, d3d_context) = create_d3d11_device()?;
        let dxgi_device: IDXGIDevice = d3d_device.cast()?;
        let winrt_device = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi_device)? };
        let winrt_device: windows::Graphics::DirectX::Direct3D11::IDirect3DDevice =
            winrt_device.cast()?;

        let interop: IGraphicsCaptureItemInterop =
            windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
        let item: GraphicsCaptureItem = unsafe { interop.CreateForWindow(hwnd)? };

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
                            *latest.lock().unwrap() = Some(frame);
                        }
                    }
                    Ok(())
                },
            ))?;
        }

        session.StartCapture()?;

        Ok(Self {
            hwnd,
            d3d_device,
            d3d_context,
            frame_pool,
            session,
            latest,
            staging: None,
        })
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
                self.d3d_device
                    .CreateTexture2D(&staging_desc, None, Some(&mut staging))
                    .ok()?;
            }
            self.staging = Some((staging?, desc.Width, desc.Height));
        }
        let staging = &self.staging.as_ref()?.0;

        unsafe { self.d3d_context.CopyResource(staging, &texture) };

        let mut mapped = Default::default();
        unsafe {
            self.d3d_context
                .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .ok()?;
        }
        // Crop to just the client area (see `client_rect_in_window`'s doc
        // comment) -- falls back to the full captured frame if the window
        // handle is somehow invalid by the time this runs (process
        // shutting down mid-capture), rather than losing the frame
        // entirely.
        let (crop_x, crop_y, crop_w, crop_h) = client_rect_in_window(self.hwnd)
            .filter(|(x, y, w, h)| {
                *w > 0 && *h > 0 && *x >= 0 && *y >= 0 && x + *w as i32 <= desc.Width as i32 && y + *h as i32 <= desc.Height as i32
            })
            .unwrap_or((0, 0, desc.Width, desc.Height));

        let mut pixels = vec![0u8; (crop_w * crop_h * 4) as usize];
        unsafe {
            let src = mapped.pData as *const u8;
            for row in 0..crop_h as usize {
                let src_row = src
                    .add((row + crop_y as usize) * mapped.RowPitch as usize)
                    .add(crop_x as usize * 4);
                let dst_row = pixels.as_mut_ptr().add(row * crop_w as usize * 4);
                std::ptr::copy_nonoverlapping(src_row, dst_row, crop_w as usize * 4);
            }
            self.d3d_context.Unmap(staging, 0);
        }

        Some((pixels, crop_w, crop_h))
    }

    /// Recreates the frame pool at the new size -- unlike CEF (lazy resize,
    /// keyed off the next painted frame's own reported dimensions), this
    /// pipeline owns the capture's pixel dimensions directly, so `Resize`
    /// can apply synchronously with no "wait for the next frame to tell us
    /// its real size" step needed.
    pub fn resize(&mut self, width: u32, height: u32) {
        let size = windows::Graphics::SizeInt32 {
            Width: width as i32,
            Height: height as i32,
        };
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
