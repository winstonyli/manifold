use cef::{rc::Rc, *};
use std::cell::RefCell;

/// A single painted frame handed from CEF's `on_paint` (BGRA8, CEF's native
/// OSR pixel format — unlike Servo's `read_to_image()`, which is RGBA8; see
/// `ipc_protocol::frame_buffer`'s doc comment) to the main loop.
pub struct PaintedFrame {
    pub bgra: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Headless OSR render handler. No window, no wgpu — CEF's classic
/// software OSR path, matching manifold-engine-servo's shared-memory frame
/// hand-off instead of the accelerated/shared-texture path (which only
/// makes sense for a single-process embedder, not our separate-process
/// engine-host architecture).
#[derive(Clone)]
pub struct EngineRenderHandler {
    size: std::rc::Rc<RefCell<(f32, f32)>>,
    latest_frame: std::rc::Rc<RefCell<Option<PaintedFrame>>>,
}

impl EngineRenderHandler {
    pub fn new(
        initial_width: u32,
        initial_height: u32,
    ) -> (
        Self,
        std::rc::Rc<RefCell<(f32, f32)>>,
        std::rc::Rc<RefCell<Option<PaintedFrame>>>,
    ) {
        let size = std::rc::Rc::new(RefCell::new((initial_width as f32, initial_height as f32)));
        let latest_frame = std::rc::Rc::new(RefCell::new(None));
        (
            Self {
                size: size.clone(),
                latest_frame: latest_frame.clone(),
            },
            size,
            latest_frame,
        )
    }
}

wrap_render_handler! {
    pub struct RenderHandlerBuilder {
        handler: EngineRenderHandler,
    }

    impl RenderHandler {
        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            if let Some(rect) = rect {
                let (width, height) = *self.handler.size.borrow();
                if width > 0.0 && height > 0.0 {
                    rect.width = width as _;
                    rect.height = height as _;
                }
            }
        }

        fn screen_info(
            &self,
            _browser: Option<&mut Browser>,
            screen_info: Option<&mut ScreenInfo>,
        ) -> ::std::os::raw::c_int {
            if let Some(screen_info) = screen_info {
                screen_info.device_scale_factor = 1.0;
                return true as _;
            }
            false as _
        }

        fn on_paint(
            &self,
            _browser: Option<&mut Browser>,
            type_: PaintElementType,
            _dirty_rects: Option<&[Rect]>,
            buffer: *const u8,
            width: ::std::os::raw::c_int,
            height: ::std::os::raw::c_int,
        ) {
            if type_ != PaintElementType::default() || buffer.is_null() || width <= 0 || height <= 0
            {
                return;
            }
            let len = (width as usize) * (height as usize) * 4;
            let bgra = unsafe { std::slice::from_raw_parts(buffer, len) }.to_vec();
            *self.handler.latest_frame.borrow_mut() = Some(PaintedFrame {
                bgra,
                width: width as u32,
                height: height as u32,
            });
        }
    }
}

impl RenderHandlerBuilder {
    pub fn build(handler: EngineRenderHandler) -> RenderHandler {
        Self::new(handler)
    }
}
