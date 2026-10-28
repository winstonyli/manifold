//! The Image item view: its toolbar, the maximized image with wheel zoom
//! and drag pan, and the fit/zoom/pan math behind them.

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

/// Contain `image` within `body`, never above 1.0 (a small image isn't
/// magnified). No lower clamp: Fit pins the pan, so a scale clamped above
/// the true fit would leave the edges unreachable.
pub(crate) fn image_fit_scale(image: egui::Vec2, body: egui::Vec2) -> f32 {
    if image.x <= 0.0 || image.y <= 0.0 {
        return 1.0;
    }
    (body.x / image.x).min(body.y / image.y).clamp(f32::MIN_POSITIVE, 1.0)
}

pub(crate) fn image_display_rect(body: egui::Rect, image: egui::Vec2, scale: f32, pan: egui::Vec2) -> egui::Rect {
    egui::Rect::from_center_size(body.center() + pan, image * scale)
}

/// Per axis: when the scaled image is larger than the body, no gap may
/// open past its edges; when it fits, it stays centered on that axis.
pub(crate) fn clamp_image_pan(pan: egui::Vec2, scaled: egui::Vec2, body: egui::Vec2) -> egui::Vec2 {
    let axis = |pan: f32, scaled: f32, body: f32| {
        let slack = (scaled - body) / 2.0;
        if slack <= 0.0 {
            0.0
        } else {
            pan.clamp(-slack, slack)
        }
    };
    egui::vec2(axis(pan.x, scaled.x, body.x), axis(pan.y, scaled.y, body.y))
}

/// Zooms by `factor` (clamped to `IMAGE_ZOOM_MIN..=IMAGE_ZOOM_MAX`, or to
/// the current scale if Fit already went below the minimum) so the image
/// point under `anchor` (relative to the body center) stays put.
pub(crate) fn zoom_about(scale: f32, pan: egui::Vec2, factor: f32, anchor: egui::Vec2) -> (f32, egui::Vec2) {
    let new_scale = (scale * factor).clamp(IMAGE_ZOOM_MIN.min(scale), IMAGE_ZOOM_MAX);
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

impl App {
    /// Image view's layer-3 toolbar content: identity (filename · pixel
    /// size, full path on hover) and the zoom chip on the leading side;
    /// Open Containing Folder + Copy Path in the trailing cluster.
    pub(crate) fn render_image_toolbar_content(
        &mut self,
        image_id: NodeId,
        ui: &mut egui::Ui,
        trailing_ui: &mut egui::Ui,
    ) -> ItemToolbarActions {
        let Some((path, image)) =
            self.nodes.get(&image_id).and_then(|n| n.as_image()).map(|i| (i.path.clone(), image_pixel_size(i)))
        else {
            return ItemToolbarActions::default();
        };

        // (reveal, copy) clicked
        let mut clicked = (false, false);
        toolbar_chip(trailing_ui, |ui| {
            let (reveal, copy) = in_reading_order(
                ui,
                &mut clicked,
                |clicked, ui| {
                    let response =
                        chip_icon_button(ui, egui::Id::new("icon_btn_image_reveal"), egui_phosphor::light::FOLDER_OPEN)
                            .named_tip("Open Containing Folder");
                    clicked.0 = response.clicked();
                    response.rect
                },
                |clicked, ui| {
                    let response =
                        chip_icon_button(ui, egui::Id::new("icon_btn_image_copy_path"), egui_phosphor::light::COPY)
                            .named_tip("Copy Path");
                    clicked.1 = response.clicked();
                    response.rect
                },
            );
            reveal.union(copy)
        });

        // Vertically centered in the row (which is top-aligned -- see
        // `render_item_toolbar`), the same problem the Tab toolbar's
        // favicon slot solves; width capped so a long filename truncates
        // instead of running under the trailing cluster (Reveal/Copy/Help/
        // Settings, already in `trailing_ui` by this point -- see
        // `render_item_toolbar`) or the zoom chip that follows this label
        // in the same row. `IMAGE_ZOOM_CHIP_RESERVED_WIDTH` is a measured
        // constant rather than `trailing_width`-style live measurement
        // because the chip hasn't been laid out yet when this label needs
        // its own width: it's drawn after the label, in this same
        // leading-side `ui` (see the call below).
        let trailing_width = trailing_ui.min_rect().width();
        let identity = format!("{} · {}×{}", download_filename(&path), image.x as u32, image.y as u32);
        let label_max_width =
            (ui.available_width() - trailing_width - IMAGE_ZOOM_CHIP_RESERVED_WIDTH).clamp(80.0, 360.0);
        ui.allocate_ui_with_layout(
            egui::vec2(label_max_width, ui.available_height()),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| ui.add(egui::Label::new(identity).truncate()).on_hover_text(&path),
        );

        let (scale, _) = self.image_view.effective(image, self.item_body_rect().size());
        if let Some(action) = render_zoom_chip(
            ui,
            scale * 100.0,
            scale > IMAGE_ZOOM_MIN + 1e-3,
            scale < IMAGE_ZOOM_MAX - 1e-3,
            "Fit to window",
        ) {
            self.apply_image_zoom_action(action);
        }

        if clicked.0 {
            self.reveal_node(image_id);
        }
        if clicked.1 {
            ui.ctx().copy_text(path);
        }
        ItemToolbarActions::default()
    }

    /// Zoom-chip click or its keyboard equivalent, applied to the Image
    /// currently in Item view. A no-op outside Image view.
    pub(crate) fn apply_image_zoom_action(&mut self, action: ZoomChipAction) {
        let Some(image) =
            self.viewed_image.and_then(|id| self.nodes.get(&id)).and_then(|n| n.as_image()).map(image_pixel_size)
        else {
            return;
        };
        self.image_view = image_view_after_action(self.image_view, action, image, self.item_body_rect().size());
    }

    /// Image view: the shared Item toolbar plus the image itself, zoomable
    /// (wheel/pinch about the cursor) and pannable (drag) -- see
    /// `ImageViewState`. Leaves to Graph if the image node is gone.
    pub(crate) fn build_image_view_ui(&mut self, ui: &mut egui::Ui, image_id: NodeId) {
        let Some(image) = self.nodes.get(&image_id).and_then(|n| n.as_image()).map(image_pixel_size) else {
            self.viewed_image = None;
            self.view_mode = ViewMode::Graph;
            self.save_session();
            return;
        };
        let frame = self.render_item_toolbar(image_id, ui);
        self.build_shortcuts_help(ui, frame.shortcuts_help_button_rect);

        let body = self.item_body_rect();
        let response = ui.interact(body, egui::Id::new("image_view_body"), egui::Sense::drag());
        let (orig_scale, orig_pan) = self.image_view.effective(image, body.size());
        let (mut scale, mut pan) = (orig_scale, orig_pan);
        let mut changed = false;
        if response.hovered() {
            let factor = ui.input(|i| i.zoom_delta() * (i.smooth_scroll_delta.y * IMAGE_WHEEL_ZOOM_RATE).exp());
            if factor != 1.0 {
                let anchor = ui.input(|i| i.pointer.hover_pos()).map_or(egui::Vec2::ZERO, |p| p - body.center());
                (scale, pan) = zoom_about(scale, pan, factor, anchor);
                changed = true;
            }
        }
        if response.dragged() {
            pan += response.drag_delta();
            changed = true;
        }
        if changed {
            pan = clamp_image_pan(pan, image * scale, body.size());
            // A drag attempt while at Fit with the image fully contained
            // (no room to pan) reaches here with `scale`/`pan` clamped
            // right back to what `effective` already reported -- committing
            // a fixed `Scale` in that case would silently turn off Fit's
            // "re-fit on window resize" behavior for no visible change.
            // Only commit when something about the view actually moved.
            if scale != orig_scale || pan != orig_pan {
                self.image_view = ImageViewState { zoom: ImageZoom::Scale(scale), pan };
            }
        }

        let painter = ui.painter_at(body);
        painter.rect_filled(body, 0.0, ui.visuals().panel_fill);
        let rect = image_display_rect(body, image, scale, pan);
        match self.gpu.as_ref().and_then(|g| g.image_id(image_id)) {
            Some(tex_id) => {
                painter.image(tex_id, rect, crate::card::FULL_UV, egui::Color32::WHITE);
            }
            // Same placeholder the graph card uses for a missing texture.
            None => {
                paint_centered(
                    &painter,
                    body.center(),
                    egui_phosphor::light::IMAGE_BROKEN,
                    48.0,
                    ui.visuals().weak_text_color(),
                );
            }
        }

        let escape_pressed = ui.input(|i| i.key_pressed(egui::Key::Escape)) && self.menus.omni_popup.open.is_none();
        if frame.back_to_graph || escape_pressed {
            self.leave_to_graph();
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

    /// Fit really fits an image over 10x the body, and zooming out from there doesn't jump in.
    #[test]
    fn fit_fits_a_huge_image_and_zoom_out_never_goes_up() {
        let fit = image_fit_scale(egui::vec2(8000.0, 100.0), egui::vec2(700.0, 500.0));
        assert!((fit - 700.0 / 8000.0).abs() < 1e-6, "{fit}");
        assert_eq!(zoom_about(0.05, egui::Vec2::ZERO, 0.5, egui::Vec2::ZERO).0, 0.05, "never up on zoom-out");
        assert_eq!(zoom_about(0.11, egui::Vec2::ZERO, 0.5, egui::Vec2::ZERO).0, IMAGE_ZOOM_MIN);
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
