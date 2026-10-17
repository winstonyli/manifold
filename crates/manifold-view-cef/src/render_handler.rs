use cef::{rc::Rc, *};
use manifold_view_protocol::FrameSink;
use std::cell::RefCell;

/// The view size in pixels, shared between the handler and the main loop.
pub type SharedSize = std::rc::Rc<RefCell<(f32, f32)>>;
/// The frame store `on_paint` offers into and the main loop takes from. Both run on the main thread (`on_paint` fires
/// inside the CEF pump), so the loop must not hold a borrow across that pump.
pub type SharedFrameSink = std::rc::Rc<RefCell<FrameSink>>;

/// The rows `dirty_rects` cover (their union, clamped to `0..height`), as the sink's row hint. `None` for a missing or
/// empty list or rects wholly outside the frame.
fn dirty_rows(dirty_rects: Option<&[Rect]>, height: i32) -> Option<std::ops::Range<u32>> {
    let rows = dirty_rects?
        .iter()
        .map(|r| r.y.clamp(0, height)..r.y.saturating_add(r.height).clamp(0, height))
        .filter(|r| r.start < r.end)
        .reduce(|a, b| a.start.min(b.start)..a.end.max(b.end))?;
    Some(rows.start as u32..rows.end as u32)
}

/// Headless OSR render handler. No window, no wgpu — CEF's classic
/// software OSR path, matching manifold-engine-servo's shared-memory frame
/// hand-off instead of the accelerated/shared-texture path (which only
/// makes sense for a single-process embedder, not our separate-process
/// view-host architecture).
#[derive(Clone)]
pub struct ViewRenderHandler {
    size: SharedSize,
    sink: SharedFrameSink,
}

impl ViewRenderHandler {
    pub fn new(initial_width: u32, initial_height: u32, sink: SharedFrameSink) -> (Self, SharedSize) {
        let size = std::rc::Rc::new(RefCell::new((initial_width as f32, initial_height as f32)));
        (Self { size: size.clone(), sink }, size)
    }
}

wrap_render_handler! {
    pub struct RenderHandlerBuilder {
        handler: ViewRenderHandler,
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
            dirty_rects: Option<&[Rect]>,
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
            let hint = dirty_rows(dirty_rects, height);
            self.handler.sink.borrow_mut().offer_hinted(bgra, width as usize * 4, width as u32, height as u32, hint);
        }
    }
}

impl RenderHandlerBuilder {
    pub fn build(handler: ViewRenderHandler) -> RenderHandler {
        Self::new(handler)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(y: i32, height: i32) -> Rect {
        Rect { x: 0, y, width: 10, height }
    }

    #[test]
    fn dirty_rows_are_the_clamped_union() {
        assert_eq!(dirty_rows(None, 100), None);
        assert_eq!(dirty_rows(Some(&[]), 100), None);
        assert_eq!(dirty_rows(Some(&[rect(10, 5)]), 100), Some(10..15));
        assert_eq!(dirty_rows(Some(&[rect(10, 5), rect(60, 10)]), 100), Some(10..70));
        assert_eq!(dirty_rows(Some(&[rect(-5, 10), rect(95, 50)]), 100), Some(0..100), "clamped to the frame");
        assert_eq!(dirty_rows(Some(&[rect(200, 10)]), 100), None, "wholly outside");
        assert_eq!(dirty_rows(Some(&[rect(10, 0), rect(5, -3)]), 100), None, "empty rects");
    }
}
