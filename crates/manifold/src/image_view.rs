//! The Image item view's zoom and pan math: fit scale, anchored zoom, pan clamping and the zoom-chip actions.

use super::*;

pub(crate) const IMAGE_ZOOM_MIN: f32 = 0.1;

pub(crate) const IMAGE_ZOOM_MAX: f32 = 16.0;

/// Image view mouse-wheel sensitivity: zoom factor per point of vertical
/// scroll, applied as `exp(delta * rate)` so equal scroll amounts in and
/// out cancel exactly.
pub(crate) const IMAGE_WHEEL_ZOOM_RATE: f32 = 0.002;

/// `render_zoom_chip`'s own on-screen width in the Image toolbar row,
/// including the gap `render_image_toolbar_content`'s identity label
/// leaves before it -- measured live (cursor-position delta across the
/// `render_zoom_chip` call, 1300×800 window) since the chip is drawn
/// after the label in the same row and so isn't known yet when the label
/// picks its own max width.
pub(crate) const IMAGE_ZOOM_CHIP_RESERVED_WIDTH: f32 = 171.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ImageZoom {
    /// Contain within the body, never above 100% (`image_fit_scale`).
    Fit,
    Scale(f32),
}

/// Image view's zoom/pan. `pan` is the image center's offset from the
/// body center, in points; ignored while `zoom` is `Fit`. Reset to the
/// default (Fit) every time an Image enters Item view; not persisted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ImageViewState {
    pub(crate) zoom: ImageZoom,
    pub(crate) pan: egui::Vec2,
}

impl Default for ImageViewState {
    fn default() -> Self {
        Self { zoom: ImageZoom::Fit, pan: egui::Vec2::ZERO }
    }
}

impl ImageViewState {
    /// The concrete `(scale, pan)` to draw with for an `image`-sized
    /// picture in a `body`-sized area -- Fit resolved, pan re-clamped (the
    /// window may have been resized since it was stored).
    pub(crate) fn effective(&self, image: egui::Vec2, body: egui::Vec2) -> (f32, egui::Vec2) {
        match self.zoom {
            ImageZoom::Fit => (image_fit_scale(image, body), egui::Vec2::ZERO),
            ImageZoom::Scale(scale) => (scale, clamp_image_pan(self.pan, image * scale, body)),
        }
    }
}

/// An Image's pixel size as a `Vec2` (at least 1×1).
pub(crate) fn image_pixel_size(image: &ImagePayload) -> egui::Vec2 {
    egui::vec2(image.width.max(1) as f32, image.height.max(1) as f32)
}

/// Contain `image` within `body`, never above 1.0 -- same "scale down,
/// never up" rule `image_node_card_size` uses, so a small image isn't
/// blurred by magnification it didn't ask for.
pub(crate) fn image_fit_scale(image: egui::Vec2, body: egui::Vec2) -> f32 {
    if image.x <= 0.0 || image.y <= 0.0 {
        return 1.0;
    }
    (body.x / image.x).min(body.y / image.y).clamp(IMAGE_ZOOM_MIN, 1.0)
}

/// Whether a just-(re)spawned tab's engine view should be shown, used by
/// `handle_tab_spawn_result`. The focused tab stays hidden while Image view
/// is up front of it (relaunching straight into Image view, or a crash
/// respawn of the tab hidden behind an open image) -- `focused` keeps
/// pointing at it per the Item-view design (see the design doc's Plan-time
/// corrections on `App::focused`), but nothing should render or receive
/// input for it until the image is closed. A split's secondary pane is
/// always visible regardless, same as before this fn existed.
pub(crate) fn spawned_tab_visible(is_focused: bool, is_split: bool, image_view_showing: bool) -> bool {
    (is_focused && !image_view_showing) || is_split
}

pub(crate) fn image_display_rect(body: egui::Rect, image: egui::Vec2, scale: f32, pan: egui::Vec2) -> egui::Rect {
    egui::Rect::from_center_size(body.center() + pan, image * scale)
}

/// Per axis: when the scaled image is larger than the body, no gap may
/// open past its edges; when it fits, it stays centered on that axis.
pub(crate) fn clamp_image_pan(pan: egui::Vec2, scaled: egui::Vec2, body: egui::Vec2) -> egui::Vec2 {
    let axis = |pan: f32, scaled: f32, body: f32| {
        let slack = (scaled - body) / 2.0;
        if slack <= 0.0 { 0.0 } else { pan.clamp(-slack, slack) }
    };
    egui::vec2(axis(pan.x, scaled.x, body.x), axis(pan.y, scaled.y, body.y))
}

/// Zooms by `factor` (clamped to `IMAGE_ZOOM_MIN..=IMAGE_ZOOM_MAX`) so the
/// image point under `anchor` (relative to the body center) stays put.
pub(crate) fn zoom_about(scale: f32, pan: egui::Vec2, factor: f32, anchor: egui::Vec2) -> (f32, egui::Vec2) {
    let new_scale = (scale * factor).clamp(IMAGE_ZOOM_MIN, IMAGE_ZOOM_MAX);
    let ratio = new_scale / scale;
    (new_scale, anchor - (anchor - pan) * ratio)
}

/// Applies a zoom-chip click (or its keyboard equivalent) to Image view's
/// state. −/+ zoom about the body center.
pub(crate) fn image_view_after_action(
    state: ImageViewState,
    action: ZoomChipAction,
    image: egui::Vec2,
    body: egui::Vec2,
) -> ImageViewState {
    match action {
        ZoomChipAction::Fit => ImageViewState::default(),
        ZoomChipAction::Reset100 => ImageViewState { zoom: ImageZoom::Scale(1.0), pan: egui::Vec2::ZERO },
        ZoomChipAction::In | ZoomChipAction::Out => {
            let factor = if action == ZoomChipAction::In { ZOOM_STEP } else { 1.0 / ZOOM_STEP };
            let (scale, pan) = state.effective(image, body);
            let (scale, pan) = zoom_about(scale, pan, factor, egui::Vec2::ZERO);
            ImageViewState { zoom: ImageZoom::Scale(scale), pan: clamp_image_pan(pan, image * scale, body) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_fit_scale_contains_and_never_upscales() {
        let body = egui::vec2(1000.0, 800.0);
        assert_eq!(image_fit_scale(egui::vec2(2000.0, 1000.0), body), 0.5);
        assert_eq!(image_fit_scale(egui::vec2(1000.0, 4000.0), body), 0.2);
        assert_eq!(image_fit_scale(egui::vec2(64.0, 64.0), body), 1.0);
    }

    #[test]
    fn spawned_tab_visible_hides_focused_tab_behind_image_view() {
        // Ordinary focused tab, no Image view up: visible, as before.
        assert!(spawned_tab_visible(true, false, false));
        // Focused tab, but Image view is showing in front of it: hidden.
        assert!(!spawned_tab_visible(true, false, true));
        // Split's secondary pane is always visible, even behind Image view.
        assert!(spawned_tab_visible(false, true, true));
        assert!(spawned_tab_visible(false, true, false));
        // Unfocused, non-split tab: never visible.
        assert!(!spawned_tab_visible(false, false, false));
        assert!(!spawned_tab_visible(false, false, true));
    }

    #[test]
    fn zoom_about_keeps_the_anchor_point_fixed() {
        let (scale, pan) = (0.5, egui::vec2(30.0, -20.0));
        let anchor = egui::vec2(100.0, 50.0);
        let (new_scale, new_pan) = zoom_about(scale, pan, 2.0, anchor);
        assert_eq!(new_scale, 1.0);
        let before = (anchor - pan) / scale;
        let after = (anchor - new_pan) / new_scale;
        assert!((before - after).length() < 1e-4, "{before:?} vs {after:?}");
    }

    #[test]
    fn zoom_about_clamps_scale() {
        assert_eq!(zoom_about(15.0, egui::Vec2::ZERO, 2.0, egui::Vec2::ZERO).0, IMAGE_ZOOM_MAX);
        assert_eq!(zoom_about(0.11, egui::Vec2::ZERO, 0.5, egui::Vec2::ZERO).0, IMAGE_ZOOM_MIN);
    }

    #[test]
    fn clamp_image_pan_limits_each_axis_independently() {
        let body = egui::vec2(1000.0, 800.0);
        let scaled = egui::vec2(2000.0, 500.0); // wider than the body, shorter than it
        assert_eq!(clamp_image_pan(egui::vec2(900.0, 40.0), scaled, body), egui::vec2(500.0, 0.0));
        assert_eq!(clamp_image_pan(egui::vec2(-120.0, 0.0), scaled, body), egui::vec2(-120.0, 0.0));
    }

    #[test]
    fn image_view_after_action_steps_from_fit_and_resets() {
        let image = egui::vec2(2000.0, 1000.0);
        let body = egui::vec2(1000.0, 800.0);
        let zoomed = image_view_after_action(ImageViewState::default(), ZoomChipAction::In, image, body);
        assert_eq!(zoomed.zoom, ImageZoom::Scale(0.5 * ZOOM_STEP));
        let actual = image_view_after_action(zoomed, ZoomChipAction::Reset100, image, body);
        assert_eq!(actual, ImageViewState { zoom: ImageZoom::Scale(1.0), pan: egui::Vec2::ZERO });
        assert_eq!(image_view_after_action(zoomed, ZoomChipAction::Fit, image, body), ImageViewState::default());
    }
}
