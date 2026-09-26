//! Throwaway pre-integration spike (see the M6 plan, Phase 2): proves the
//! Windows.Graphics.Capture + Direct3D11-interop chain works in isolation
//! before wiring it into the real engine-host's tick loop.
//!
//! Originally planned to capture an off-screen DirectComposition `Visual`
//! (per the approved plan), but this windows-rs version's
//! `IGraphicsCaptureItemInterop` only exposes `CreateForWindow`/
//! `CreateForMonitor` -- no `CreateForVisual` at all (confirmed by reading
//! the crate's own generated bindings, not assumed). That's actually a
//! simplification: Phase 1 already uses plain *windowed* WebView2 hosting
//! against a real, off-screen-positioned HWND -- so window-capture against
//! that same HWND should get WebView2's actual pixels directly, with no
//! DirectComposition/Compositor/Visual/DispatcherQueueController machinery
//! needed at all. This spike proves that with a plain GDI-painted rectangle
//! standing in for WebView2 (no WebView2 involved yet) -- real integration
//! happens in `src/capture.rs` once this confirms the chain works.
//!
//! Run with `cargo run -p manifold-engine-webview2 --example capture_spike`.

use std::time::Duration;

use windows::core::{w, PCWSTR};
use windows::core::{Interface, Result};
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem};
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::{CreateSolidBrush, FillRect, GetDC, ReleaseDC, HBRUSH, HDC};
use windows::Win32::System::WinRT::Direct3D11::CreateDirect3D11DeviceFromDXGIDevice;
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_SINGLETHREADED};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetClientRect, PeekMessageW, RegisterClassW, ShowWindow,
    TranslateMessage, CS_HREDRAW, CS_VREDRAW, MSG, PM_REMOVE, SW_SHOWNOACTIVATE, WNDCLASSW, WS_EX_TOOLWINDOW, WS_POPUP,
};

const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;
const CLASS_NAME: PCWSTR = w!("ManifoldCaptureSpikeHost");

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn create_window() -> Result<HWND> {
    unsafe {
        let instance = windows::Win32::System::LibraryLoader::GetModuleHandleW(None)?;
        let class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        let _ = RegisterClassW(&class);
        let hwnd = CreateWindowExW(
            windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(0),
            CLASS_NAME,
            w!("manifold-capture-spike"),
            windows::Win32::UI::WindowsAndMessaging::WS_OVERLAPPED,
            -32000,
            -32000,
            WIDTH as i32,
            HEIGHT as i32,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        Ok(hwnd)
    }
}

/// Stands in for WebView2 actually painting something -- a plain GDI-filled
/// rectangle, just to prove the *capture* side works independent of
/// WebView2 entirely.
fn paint_test_pattern(hwnd: HWND) {
    unsafe {
        let hdc: HDC = GetDC(Some(hwnd));
        let mut rect = RECT::default();
        let _ = GetClientRect(hwnd, &mut rect);
        let brush: HBRUSH = CreateSolidBrush(windows::Win32::Foundation::COLORREF(0x005AA028)); // BGR: green-ish
        FillRect(hdc, &rect, brush);
        ReleaseDC(Some(hwnd), hdc);
    }
}

fn pump_messages() {
    unsafe {
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn main() -> Result<()> {
    unsafe {
        RoInitialize(RO_INIT_SINGLETHREADED).ok();
    }

    let hwnd = create_window()?;
    paint_test_pattern(hwnd);

    let (d3d_device, d3d_context) = create_d3d11_device()?;
    let dxgi_device: IDXGIDevice = d3d_device.cast()?;
    let winrt_device = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi_device)? };
    let winrt_device: windows::Graphics::DirectX::Direct3D11::IDirect3DDevice = winrt_device.cast()?;

    let interop: IGraphicsCaptureItemInterop =
        windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
    let item: GraphicsCaptureItem = unsafe { interop.CreateForWindow(hwnd)? };

    let frame_pool =
        Direct3D11CaptureFramePool::Create(&winrt_device, DirectXPixelFormat::B8G8R8A8UIntNormalized, 1, item.Size()?)?;
    let session = frame_pool.CreateCaptureSession(&item)?;

    let (tx, rx) = std::sync::mpsc::channel();
    frame_pool.FrameArrived(&TypedEventHandler::new(
        move |pool: windows::core::Ref<Direct3D11CaptureFramePool>, _| {
            if let Some(pool) = pool.as_ref() {
                if let Ok(frame) = pool.TryGetNextFrame() {
                    let _ = tx.send(frame);
                }
            }
            Ok(())
        },
    ))?;

    session.StartCapture()?;

    // Repaint + pump messages in a loop until a frame arrives -- a single
    // WM_PAINT right after StartCapture can race the capture session not
    // being fully armed yet.
    let frame = loop {
        pump_messages();
        paint_test_pattern(hwnd);
        if let Ok(frame) = rx.recv_timeout(Duration::from_millis(200)) {
            break frame;
        }
    };

    let surface = frame.Surface()?;
    let access: windows::Win32::System::WinRT::Direct3D11::IDirect3DDxgiInterfaceAccess = surface.cast()?;
    let texture: ID3D11Texture2D = unsafe { access.GetInterface()? };

    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe { texture.GetDesc(&mut desc) };

    let staging_desc = D3D11_TEXTURE2D_DESC {
        Usage: D3D11_USAGE_STAGING,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        BindFlags: 0,
        MiscFlags: 0,
        ..desc
    };
    let mut staging: Option<ID3D11Texture2D> = None;
    unsafe { d3d_device.CreateTexture2D(&staging_desc, None, Some(&mut staging))? };
    let staging = staging.expect("staging texture created");

    unsafe { d3d_context.CopyResource(&staging, &texture) };

    let mut mapped = Default::default();
    unsafe { d3d_context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))? };

    let mut pixels = vec![0u8; (desc.Width * desc.Height * 4) as usize];
    unsafe {
        let src = mapped.pData as *const u8;
        for row in 0..desc.Height as usize {
            let src_row = src.add(row * mapped.RowPitch as usize);
            let dst_row = pixels.as_mut_ptr().add(row * desc.Width as usize * 4);
            std::ptr::copy_nonoverlapping(src_row, dst_row, desc.Width as usize * 4);
        }
        d3d_context.Unmap(&staging, 0);
    }

    // Captured surface is BGRA -- swap to RGBA for `image`'s encoder.
    for px in pixels.chunks_exact_mut(4) {
        px.swap(0, 2);
    }

    let out_path = std::env::args().nth(1).unwrap_or_else(|| "capture_spike_output.png".to_string());
    image::save_buffer(&out_path, &pixels, desc.Width, desc.Height, image::ColorType::Rgba8).expect("save PNG");
    println!("wrote {out_path} ({}x{})", desc.Width, desc.Height);

    session.Close().ok();
    Ok(())
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
