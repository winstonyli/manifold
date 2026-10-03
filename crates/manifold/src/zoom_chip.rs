//! The −/100%/+/Fit zoom chip shared by the Graph and Image toolbars, and the small
//! layout and zoom-step helpers it needs.

use super::*;

/// What a click on `render_zoom_chip` asked for. The chip holds no zoom
/// state of its own -- each caller applies this to whatever "zoom" means
/// in its own view (Graph: the camera's `camera.rect`; Image view:
/// `ImageViewState`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ZoomChipAction {
    Out,
    In,
    Reset100,
    Fit,
}

/// Multiplicative step for one click of a zoom chip's −/+ (and, in Image
/// view, one `Ctrl+=`/`Ctrl+-`) -- shared so Graph and Image zoom feel the
/// same.
pub(crate) const ZOOM_STEP: f32 = 1.25;

/// `rect` (a Graph camera's world-space `camera.rect`) resized about
/// its own center so the zoom scale `graph_world_to_screen_transform`
/// derives from it (`screen / rect.size()`, min axis) is multiplied by
/// `factor`, clamped to `min..=max`. The ratio is computed from the raw,
/// unclamped scale so the result lands exactly on the clamped target.
pub(crate) fn step_scene_rect(rect: egui::Rect, factor: f32, screen: egui::Vec2, min: f32, max: f32) -> egui::Rect {
    let raw = (screen / rect.size()).min_elem();
    let new_scale = (raw.clamp(min, max) * factor).clamp(min, max);
    egui::Rect::from_center_size(rect.center(), rect.size() * (raw / new_scale))
}

/// −, a clickable `NN%` readout (back to 100%), +, and Fit, as one chip.
/// Left-to-right rows only (both callers are): in a right-to-left `Ui`
/// its buttons would come out reversed -- see `in_reading_order` for the
/// pattern if a right-to-left caller ever needs it. The readout is a
/// frameless `Button` rather than a `Label` so it has the same height as
/// its icon-button neighbors (a `Label` sits high in the Item toolbar's
/// top-aligned row), with a fixed minimum width so the chip doesn't
/// jitter as the digit count changes.
pub(crate) fn render_zoom_chip(
    ui: &mut egui::Ui,
    pct: f32,
    can_zoom_out: bool,
    can_zoom_in: bool,
    fit_hover: &str,
) -> Option<ZoomChipAction> {
    debug_assert_eq!(
        ui.layout().main_dir(),
        egui::Direction::LeftToRight,
        "render_zoom_chip lays out left to right only"
    );
    let mut action = None;
    toolbar_chip(ui, |ui| {
        let out = chip_icon_button_enabled(
            ui,
            egui::Id::new("icon_btn_zoom_out"),
            egui_phosphor::light::MAGNIFYING_GLASS_MINUS,
            can_zoom_out,
        )
        .named_tip("Zoom out (Ctrl+-)");
        if out.clicked() {
            action = Some(ZoomChipAction::Out);
        }
        let readout = ui
            .add(
                egui::Button::new(egui::RichText::new(format!("{}%", pct.round() as i32)).color(TEXT_PRIMARY))
                    .frame(false)
                    .min_size(egui::vec2(44.0, 0.0)),
            )
            .named_tip("Reset zoom to 100% (Ctrl+0)")
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        focus_ring(ui, &readout);
        if readout.clicked() {
            action = Some(ZoomChipAction::Reset100);
        }
        let zoom_in = chip_icon_button_enabled(
            ui,
            egui::Id::new("icon_btn_zoom_in"),
            egui_phosphor::light::MAGNIFYING_GLASS_PLUS,
            can_zoom_in,
        )
        .named_tip("Zoom in (Ctrl+=)");
        if zoom_in.clicked() {
            action = Some(ZoomChipAction::In);
        }
        let fit = chip_icon_button(ui, egui::Id::new("icon_btn_zoom_fit"), egui_phosphor::light::FRAME_CORNERS)
            .named_tip(fit_hover);
        if fit.clicked() {
            action = Some(ZoomChipAction::Fit);
        }
        out.rect.union(readout.rect).union(zoom_in.rect).union(fit.rect)
    });
    action
}

/// Calls `first` then `second` so that their widgets read first-then-second
/// left to right in either layout direction. A right-to-left `ui` places
/// each new widget to the *left* of the previous one, so there the calls
/// are made in reverse. `cx` is threaded through rather than captured so
/// both callbacks can take the same `&mut` state (e.g. `&mut App`).
pub(crate) fn in_reading_order<C: ?Sized, A, B>(
    ui: &mut egui::Ui,
    cx: &mut C,
    first: impl FnOnce(&mut C, &mut egui::Ui) -> A,
    second: impl FnOnce(&mut C, &mut egui::Ui) -> B,
) -> (A, B) {
    if ui.layout().main_dir() == egui::Direction::RightToLeft {
        let b = second(cx, ui);
        let a = first(cx, ui);
        (a, b)
    } else {
        let a = first(cx, ui);
        let b = second(cx, ui);
        (a, b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_reading_order_reads_first_then_second_in_both_directions() {
        for layout in [egui::Layout::left_to_right(egui::Align::Min), egui::Layout::right_to_left(egui::Align::Min)] {
            egui::__run_test_ui(|ui| {
                ui.with_layout(layout, |ui| {
                    let slot = |_: &mut (), ui: &mut egui::Ui| {
                        ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover()).0
                    };
                    let (first, second) = in_reading_order(ui, &mut (), slot, slot);
                    assert!(first.min.x < second.min.x, "{layout:?}: first {first:?} not left of second {second:?}");
                });
            });
        }
    }

    #[test]
    fn step_scene_rect_zooms_about_center_and_clamps() {
        let screen = egui::vec2(1000.0, 800.0);
        let scale_of = |r: egui::Rect| (screen / r.size()).min_elem();

        let at_100 = egui::Rect::from_center_size(egui::pos2(50.0, 60.0), screen);
        let zoomed_in = step_scene_rect(at_100, ZOOM_STEP, screen, GRAPH_ZOOM_MIN, GRAPH_ZOOM_MAX);
        assert_eq!(zoomed_in.center(), at_100.center());
        assert!((scale_of(zoomed_in) - ZOOM_STEP).abs() < 1e-4);

        let zoomed_out = step_scene_rect(at_100, 1.0 / ZOOM_STEP, screen, GRAPH_ZOOM_MIN, GRAPH_ZOOM_MAX);
        assert!((scale_of(zoomed_out) - 1.0 / ZOOM_STEP).abs() < 1e-4);

        let near_max = egui::Rect::from_center_size(egui::pos2(0.0, 0.0), screen / 2.8);
        let clamped = step_scene_rect(near_max, ZOOM_STEP, screen, GRAPH_ZOOM_MIN, GRAPH_ZOOM_MAX);
        assert!((scale_of(clamped) - GRAPH_ZOOM_MAX).abs() < 1e-3);

        let near_min = egui::Rect::from_center_size(egui::pos2(0.0, 0.0), screen / 0.16);
        let clamped = step_scene_rect(near_min, 1.0 / ZOOM_STEP, screen, GRAPH_ZOOM_MIN, GRAPH_ZOOM_MAX);
        assert!((scale_of(clamped) - GRAPH_ZOOM_MIN).abs() < 1e-3);
    }
}
