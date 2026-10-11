use cef::{rc::Rc, *};
use ipc_protocol::FrameSink;
use std::cell::RefCell;

/// The view size in pixels, shared between the handler and the main loop.
pub type SharedSize = std::rc::Rc<RefCell<(f32, f32)>>;
/// The frame store `on_paint` offers into and the main loop takes from. Both run on the main thread (`on_paint` fires
/// inside the CEF pump), so the loop must not hold a borrow across that pump.
pub type SharedFrameSink = std::rc::Rc<RefCell<FrameSink>>;

/// Headless OSR render handler. No window, no wgpu — CEF's classic
/// software OSR path, matching manifold-engine-servo's shared-memory frame
/// hand-off instead of the accelerated/shared-texture path (which only
/// makes sense for a single-process embedder, not our separate-process
/// engine-host architecture).
#[derive(Clone)]
pub struct EngineRenderHandler {
    size: SharedSize,
    sink: SharedFrameSink,
}

impl EngineRenderHandler {
    pub fn new(initial_width: u32, initial_height: u32, sink: SharedFrameSink) -> (Self, SharedSize) {
        let size = std::rc::Rc::new(RefCell::new((initial_width as f32, initial_height as f32)));
        (Self { size: size.clone(), sink }, size)
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
            // SAFETY: CEF hands `on_paint` a `width * height * 4` byte BGRA buffer, valid for the call.
            let bgra = unsafe { std::slice::from_raw_parts(buffer, len) };
            self.handler.sink.borrow_mut().offer(bgra, width as usize * 4, width as u32, height as u32);
        }
    }
}

impl RenderHandlerBuilder {
    pub fn build(handler: EngineRenderHandler) -> RenderHandler {
        Self::new(handler)
    }
}
