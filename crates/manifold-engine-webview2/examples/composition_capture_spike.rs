//! Throwaway spike -- NOT wired into the real engine-host. Answers the one
//! question left open after `capture.rs`'s own history: that module's doc
//! comment says the *originally planned* Composition/Visual capture was
//! abandoned because this project's pinned windows-rs version's
//! `IGraphicsCaptureItemInterop` only exposes `CreateForWindow`/
//! `CreateForMonitor`, not a visual-based creator. That's the *desktop
//! interop* factory though -- there is a separate, direct WinRT static
//! method, `GraphicsCaptureItem::CreateFromVisual(visual)`, which takes a
//! `Windows.UI.Composition.Visual` with no desktop-interop handle at all,
//! and it *is* present in this project's currently-pinned windows-rs
//! 0.62.2 (confirmed by reading its own generated bindings, gated behind
//! the already-enabled `UI_Composition` feature -- not assumed). This spike
//! combines `composition_input_spike.rs`'s WebView2 Composition-hosting
//! setup with `CreateFromVisual` instead of `CreateForWindow`, to check
//! whether that actually captures real WebView2 pixels -- the API existing
//! doesn't guarantee it works for a visual WebView2 itself owns and renders
//! into, only a real capture attempt does.
//!
//! Run with `cargo run -p manifold-engine-webview2 --example composition_capture_spike`.

use std::sync::mpsc;
use std::time::Duration;

use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2Controller, ICoreWebView2Environment, ICoreWebView2Environment3,
};
use webview2_com::{
    CreateCoreWebView2CompositionControllerCompletedHandler, CreateCoreWebView2EnvironmentCompletedHandler,
    NavigationCompletedEventHandler,
};
use windows::core::{w, Interface, Result, HSTRING, PCWSTR};
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem};
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::HBRUSH;
use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
use windows::Win32::System::WinRT::Composition::ICompositorDesktopInterop;
use windows::Win32::System::WinRT::{
    CreateDispatcherQueueController, RoInitialize, DQTAT_COM_NONE, DQTYPE_THREAD_CURRENT, RO_INIT_SINGLETHREADED,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, PeekMessageW, RegisterClassW, ShowWindow,
    TranslateMessage, CS_HREDRAW, CS_VREDRAW, MSG, PM_REMOVE, SW_SHOWNOACTIVATE, WINDOW_EX_STYLE,
    WNDCLASSW, WS_OVERLAPPEDWINDOW,
};
use windows::UI::Composition::Compositor;
use windows_numerics::Vector2;

const WIDTH: u32 = 640;
const HEIGHT: u32 = 400;
const CLASS_NAME: PCWSTR = w!("ManifoldCompositionCaptureSpikeHost");

const TEST_HTML: &str = r##"<!doctype html>
<html><body style="margin:0;padding:0;background:#2a6df5;">
<div style="position:absolute;left:40px;top:40px;width:400px;height:200px;background:#ffcc00;
  font-family:sans-serif;font-size:28px;color:#000;display:flex;align-items:center;justify-content:center;">
CAPTURE ME
</div>
</body></html>"##;

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
            hbrBackground: HBRUSH::default(),
            ..Default::default()
        };
        let _ = RegisterClassW(&class);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS_NAME,
            w!("manifold-composition-capture-spike"),
            WS_OVERLAPPEDWINDOW,
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

fn pump_messages() {
    unsafe {
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn pump_until(timeout: Duration, mut done: impl FnMut() -> bool) {
    let start = std::time::Instant::now();
    while !done() && start.elapsed() < timeout {
        pump_messages();
        std::thread::sleep(Duration::from_millis(15));
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

fn create_environment() -> webview2_com::Result<ICoreWebView2Environment> {
    let (tx, rx) = mpsc::channel();
    CreateCoreWebView2EnvironmentCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| unsafe {
            webview2_com::Microsoft::Web::WebView2::Win32::CreateCoreWebView2EnvironmentWithOptions(
                PCWSTR::null(),
                PCWSTR::null(),
                None,
                &handler,
            )
            .map_err(webview2_com::Error::WindowsError)
        }),
        Box::new(move |error_code, environment| {
            error_code?;
            tx.send(environment.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_POINTER)))
                .expect("send environment");
            Ok(())
        }),
    )?;
    Ok(webview2_com::wait_with_pump(rx)??)
}

fn main() -> Result<()> {
    unsafe {
        RoInitialize(RO_INIT_SINGLETHREADED).ok();
    }
    let queue_options = windows::Win32::System::WinRT::DispatcherQueueOptions {
        dwSize: std::mem::size_of::<windows::Win32::System::WinRT::DispatcherQueueOptions>() as u32,
        threadType: DQTYPE_THREAD_CURRENT,
        apartmentType: DQTAT_COM_NONE,
    };
    let _dispatcher_queue_controller = unsafe { CreateDispatcherQueueController(queue_options)? };

    let hwnd = create_window()?;

    let compositor = Compositor::new()?;
    let desktop_interop: ICompositorDesktopInterop = compositor.cast()?;
    let target = unsafe { desktop_interop.CreateDesktopWindowTarget(hwnd, false)? };
    let root = compositor.CreateContainerVisual()?;
    root.SetSize(Vector2 { X: WIDTH as f32, Y: HEIGHT as f32 })?;
    target.SetRoot(&root)?;

    println!("[spike] environment...");
    let environment = create_environment().expect("create environment");
    let environment3: ICoreWebView2Environment3 = environment.cast().expect("cast Environment3");

    println!("[spike] composition controller...");
    let (tx, rx) = mpsc::channel();
    CreateCoreWebView2CompositionControllerCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| unsafe {
            environment3
                .CreateCoreWebView2CompositionController(hwnd, &handler)
                .map_err(webview2_com::Error::WindowsError)
        }),
        Box::new(move |error_code, controller| {
            error_code?;
            tx.send(controller.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_POINTER)))
                .expect("send composition controller");
            Ok(())
        }),
    )
    .expect("create composition controller (start)");
    let comp_controller = webview2_com::wait_with_pump(rx).expect("recv").expect("controller");
    let controller: ICoreWebView2Controller = comp_controller.cast().expect("cast to Controller");
    let webview = unsafe { controller.CoreWebView2() }.expect("CoreWebView2");

    unsafe {
        comp_controller.SetRootVisualTarget(&root)?;
        controller.SetBounds(RECT { left: 0, top: 0, right: WIDTH as i32, bottom: HEIGHT as i32 })?;
        controller.SetIsVisible(true)?;
    }

    println!("[spike] navigating...");
    let navigated = std::rc::Rc::new(std::cell::Cell::new(false));
    {
        let navigated = navigated.clone();
        let handler = NavigationCompletedEventHandler::create(Box::new(move |_sender, _args| {
            navigated.set(true);
            Ok(())
        }));
        let mut token = Default::default();
        unsafe {
            let _ = webview.add_NavigationCompleted(&handler, &mut token);
        }
    }
    unsafe {
        webview.NavigateToString(&HSTRING::from(TEST_HTML))?;
    }
    pump_until(Duration::from_secs(5), || navigated.get());
    if !navigated.get() {
        println!("[spike] FAIL: navigation never completed");
        return Ok(());
    }
    // Give the page a moment to actually paint before capturing.
    for _ in 0..30 {
        pump_messages();
        std::thread::sleep(Duration::from_millis(33));
    }

    println!("[spike] GraphicsCaptureItem::CreateFromVisual(root)...");
    let item = match GraphicsCaptureItem::CreateFromVisual(&root) {
        Ok(item) => {
            println!("[spike]   PASS: CreateFromVisual succeeded, item size = {:?}", item.Size());
            item
        }
        Err(e) => {
            println!("[spike]   FAIL: CreateFromVisual returned an error: {e:?}");
            return Ok(());
        }
    };

    let (d3d_device, d3d_context) = create_d3d11_device()?;
    let dxgi_device: IDXGIDevice = d3d_device.cast()?;
    let winrt_device = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi_device)? };
    let winrt_device: windows::Graphics::DirectX::Direct3D11::IDirect3DDevice = winrt_device.cast()?;

    let frame_pool = Direct3D11CaptureFramePool::Create(
        &winrt_device,
        DirectXPixelFormat::B8G8R8A8UIntNormalized,
        2,
        item.Size()?,
    )?;
    let session = frame_pool.CreateCaptureSession(&item)?;

    let (frame_tx, frame_rx) = mpsc::channel();
    frame_pool.FrameArrived(&TypedEventHandler::new(
        move |pool: windows::core::Ref<Direct3D11CaptureFramePool>, _| {
            if let Some(pool) = pool.as_ref() {
                if let Ok(frame) = pool.TryGetNextFrame() {
                    let _ = frame_tx.send(frame);
                }
            }
            Ok(())
        },
    ))?;

    println!("[spike] StartCapture...");
    session.StartCapture()?;

    let frame = {
        let mut got = None;
        let start = std::time::Instant::now();
        while got.is_none() && start.elapsed() < Duration::from_secs(3) {
            pump_messages();
            if let Ok(frame) = frame_rx.try_recv() {
                got = Some(frame);
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        got
    };

    let Some(frame) = frame else {
        println!("[spike]   FAIL: no frame arrived within 3s -- CreateFromVisual capture never produced output");
        return Ok(());
    };
    println!("[spike]   PASS: a frame arrived from the Visual-based capture session");

    let surface = frame.Surface()?;
    let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;
    let texture: ID3D11Texture2D = unsafe { access.GetInterface()? };
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe { texture.GetDesc(&mut desc) };
    println!("[spike]   captured texture size = {}x{}", desc.Width, desc.Height);

    let staging_desc = D3D11_TEXTURE2D_DESC {
        Usage: D3D11_USAGE_STAGING,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        BindFlags: 0,
        MiscFlags: 0,
        ..desc
    };
    let mut staging: Option<ID3D11Texture2D> = None;
    unsafe { d3d_device.CreateTexture2D(&staging_desc, None, Some(&mut staging))? };
    let staging = staging.expect("staging texture");
    unsafe { d3d_context.CopyResource(&staging, &texture) };

    let mut mapped = Default::default();
    unsafe { d3d_context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))? };

    // Sample a pixel from inside where the yellow box should be (roughly
    // center of the window) to check this isn't just a blank/black capture.
    let sample_x = (WIDTH / 2) as usize;
    let sample_y = (HEIGHT / 2) as usize;
    let (b, g, r, a) = unsafe {
        let src = mapped.pData as *const u8;
        let px = src.add(sample_y * mapped.RowPitch as usize + sample_x * 4);
        (*px, *px.add(1), *px.add(2), *px.add(3))
    };
    unsafe { d3d_context.Unmap(&staging, 0) };
    println!("[spike]   sampled center pixel (BGRA) = ({b},{g},{r},{a}) -- expect yellow-ish (~b<100,g>150,r>150) since the test page's yellow box covers the center");

    let _ = frame.Close();
    session.Close().ok();

    println!();
    println!("=== VERDICT ===");
    let looks_yellow = b < 120 && g > 140 && r > 140;
    println!("CreateFromVisual capture of a live WebView2 visual: {}", if looks_yellow { "PASS (real content captured)" } else { "AMBIGUOUS/FAIL (check sampled color above)" });

    Ok(())
}
