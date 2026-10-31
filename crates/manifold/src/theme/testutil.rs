//! A test `egui::Context` with the app's fonts and style, in a chosen theme.

/// WCAG contrast ratio of two opaque colors.
pub(crate) fn contrast(a: egui::Color32, b: egui::Color32) -> f32 {
    let lum = |c: egui::Color32| {
        let lin = |v: u8| {
            let v = v as f32 / 255.0;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
    };
    let (hi, lo) = (lum(a).max(lum(b)), lum(a).min(lum(b)));
    (hi + 0.05) / (lo + 0.05)
}

pub(crate) fn ctx(dark: bool) -> egui::Context {
    let ctx = egui::Context::default();
    super::apply_fonts(&ctx);
    super::apply_style(&ctx);
    ctx.set_theme(if dark { egui::Theme::Dark } else { egui::Theme::Light });
    ctx
}

/// A press of `key` with no modifiers.
pub(crate) fn key_event(key: egui::Key) -> egui::Event {
    egui::Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }
}

/// One frame of the graph UI on a 1024 x 768 screen; `modifiers` is the keyboard state it starts with (egui reads it
/// off `ModifiersChanged`).
pub(crate) fn graph_frame(
    ctx: &egui::Context,
    app: &mut crate::App,
    mut events: Vec<egui::Event>,
    modifiers: egui::Modifiers,
) {
    events.insert(0, egui::Event::ModifiersChanged(modifiers));
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024.0, 768.0))),
        events,
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, |ui| app.build_graph_ui(ui));
    output.textures_delta.clear();
}
