//! The icon that says what a card or tab is: one type and one painter for the tab card's header and strip, the
//! address bar, and every kind's card header.

use super::*;

/// What a card header or tab shows in its icon slot. A tab resolves it with `Identity::of_tab`; a file kind states its
/// glyph.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Identity {
    /// A page is loading; replaces whatever else would show.
    Spinner,
    /// A favicon.
    Image(egui::TextureId),
    /// A phosphor glyph in `color`, the header's dim text color if `None`.
    Glyph { glyph: &'static str, color: Option<egui::Color32> },
    /// A page with no favicon: a tile in the engine's color with the host's first letter, or a globe when the
    /// address has no host.
    Tile { fill: egui::Color32, letter: Option<char> },
}

/// Text an `Identity` leaves to the caller, who draws it in screen space (glyph and letter rasterise badly under a
/// card's zoom, so a card queues it as an overlay).
pub(crate) struct IdentityText {
    /// World position of the text's center.
    pub(crate) center: egui::Pos2,
    pub(crate) text: String,
    /// Font size in screen points.
    pub(crate) size: f32,
    pub(crate) color: egui::Color32,
}

impl IdentityText {
    /// Draws the text with the middle of its ink at `at` (egui's `CENTER_CENTER` centres the line box, which sits a
    /// descender's height too low for a capital or a digit).
    pub(crate) fn draw(&self, painter: &egui::Painter, at: egui::Pos2) {
        let galley = painter.layout_no_wrap(self.text.clone(), egui::FontId::proportional(self.size), self.color);
        let ink = galley.rows.first().and_then(|row| {
            let glyph = row.glyphs.first()?;
            Some(egui::Rect::from_min_size(row.pos + glyph.pos.to_vec2() + glyph.uv_rect.offset, glyph.uv_rect.size))
        });
        let origin = at - ink.map_or(galley.size() / 2.0, |r| r.center().to_vec2());
        painter.galley(origin, galley, self.color);
    }
}

impl Identity {
    /// A live tab: the spinner while it loads, else its favicon, else the engine-colored tile.
    pub(crate) fn of_tab(loading: bool, favicon: Option<egui::TextureId>, kind: ViewKind, url: &str) -> Self {
        match (loading, favicon) {
            (true, _) => Self::Spinner,
            (false, Some(tex)) => Self::Image(tex),
            (false, None) => Self::Tile { fill: view_color(kind), letter: UrlParts::parse(url).initial() },
        }
    }

    /// Paints the shapes at `center`, `size` screen points across (`inv_zoom` maps points to world units), and
    /// returns the text to draw on top. `default_ink` colors a glyph that names none.
    pub(crate) fn paint(
        self,
        ui: &egui::Ui,
        center: egui::Pos2,
        size: f32,
        inv_zoom: f32,
        default_ink: egui::Color32,
    ) -> Option<IdentityText> {
        let side = size * inv_zoom;
        let square = egui::Rect::from_center_size(center, egui::Vec2::splat(side));
        let text = |text: &str, size: f32, color| Some(IdentityText { center, text: text.to_owned(), size, color });
        match self {
            Self::Spinner => {
                egui::Spinner::new().size(side).paint_at(ui, square);
                None
            }
            Self::Image(tex) => {
                ui.painter().image(tex, square, FULL_UV, egui::Color32::WHITE);
                None
            }
            Self::Glyph { glyph, color } => text(glyph, size * 0.875, color.unwrap_or(default_ink)),
            Self::Tile { fill, letter } => {
                ui.painter().rect_filled(square, (size * 0.25 * inv_zoom).min(255.0), fill);
                let ink = ink_on(fill);
                match letter {
                    Some(c) => text(&c.to_string(), size * 0.7, ink),
                    None => text(egui_phosphor::light::GLOBE, size * 0.75, ink),
                }
            }
        }
    }
}

impl App {
    /// `url`'s favicon texture, once its origin's favicon has been fetched.
    pub(crate) fn favicon_texture(&self, url: &str) -> Option<egui::TextureId> {
        let origin = UrlParts::parse(url).origin()?;
        self.gpu.as_ref().and_then(|g| g.favicon_id(&origin))
    }

    /// What `url` shows in its icon slot (`Identity::of_tab`), in a tab's card, the address bar and the trail strip alike.
    pub(crate) fn tab_identity(&self, url: &str, loading: bool, kind: ViewKind) -> Identity {
        Identity::of_tab(loading, self.favicon_texture(url), kind, url)
    }
}

/// The dark or the light ink, whichever reads better on `fill` (relative luminance, sRGB).
pub(crate) fn ink_on(fill: egui::Color32) -> egui::Color32 {
    let lin = |c: u8| {
        let c = c as f32 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let luminance = 0.2126 * lin(fill.r()) + 0.7152 * lin(fill.g()) + 0.0722 * lin(fill.b());
    if luminance > 0.3 {
        BG
    } else {
        TEXT_PRIMARY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loading_beats_a_favicon_which_beats_the_tile() {
        let tex = egui::TextureId::Managed(7);
        let url = "https://example.com";
        assert_eq!(Identity::of_tab(true, Some(tex), ViewKind::Servo, url), Identity::Spinner);
        assert_eq!(Identity::of_tab(true, None, ViewKind::Servo, url), Identity::Spinner);
        assert_eq!(Identity::of_tab(false, Some(tex), ViewKind::Servo, url), Identity::Image(tex));
        assert_eq!(
            Identity::of_tab(false, None, ViewKind::Cef, url),
            Identity::Tile { fill: view_color(ViewKind::Cef), letter: Some('E') }
        );
        assert_eq!(
            Identity::of_tab(false, None, ViewKind::Cef, "about:blank"),
            Identity::Tile { fill: view_color(ViewKind::Cef), letter: None }
        );
    }

    #[test]
    fn ink_is_dark_on_light_engines_and_light_on_dark_ones() {
        assert_eq!(ink_on(view_color(ViewKind::Servo)), BG);
        assert_eq!(ink_on(view_color(ViewKind::Webkit)), BG);
        assert_eq!(ink_on(view_color(ViewKind::Webview2)), TEXT_PRIMARY);
    }

    #[test]
    fn a_letter_is_centred_on_its_ink_not_its_line_box() {
        let ctx = egui::Context::default();
        crate::theme::apply_fonts(&ctx);
        let at = egui::pos2(50.0, 50.0);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let text = IdentityText { center: at, text: "E".into(), size: 11.0, color: BG };
            text.draw(ui.painter(), at);
        });
        output.textures_delta.clear();
        let ink_center = output.shapes.iter().find_map(|s| match &s.shape {
            egui::Shape::Text(t) => {
                let row = t.galley.rows.first()?;
                let glyph = row.glyphs.first()?;
                let ink = egui::Rect::from_min_size(
                    t.pos + row.pos.to_vec2() + glyph.pos.to_vec2() + glyph.uv_rect.offset,
                    glyph.uv_rect.size,
                );
                Some(ink.center())
            }
            _ => None,
        });
        let ink_center = ink_center.expect("the letter was drawn");
        assert!((ink_center - at).length() < 0.5, "{ink_center:?} vs {at:?}");
    }

    #[test]
    fn a_tile_paints_a_filled_square_and_hands_back_its_letter() {
        let ctx = egui::Context::default();
        let fill = view_color(ViewKind::Cef);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let text = Identity::Tile { fill, letter: Some('E') }
                .paint(ui, egui::pos2(10.0, 10.0), 16.0, 1.0, TEXT_WEAK)
                .expect("a tile has a letter");
            assert_eq!((text.text.as_str(), text.center, text.color), ("E", egui::pos2(10.0, 10.0), ink_on(fill)));
            let globe = Identity::Tile { fill, letter: None }.paint(ui, egui::Pos2::ZERO, 16.0, 1.0, TEXT_WEAK);
            assert_eq!(globe.unwrap().text, egui_phosphor::light::GLOBE);
        });
        output.textures_delta.clear();
    }
}
