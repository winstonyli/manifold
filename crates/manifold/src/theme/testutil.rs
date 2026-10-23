//! A test `egui::Context` with the app's fonts and style, in a chosen theme.

pub(crate) fn ctx(dark: bool) -> egui::Context {
    let ctx = egui::Context::default();
    super::apply_fonts(&ctx);
    super::apply_style(&ctx);
    ctx.set_theme(if dark { egui::Theme::Dark } else { egui::Theme::Light });
    ctx
}
