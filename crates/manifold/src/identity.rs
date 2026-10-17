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
    /// Draws the text centred on `at` (`paint_centered`).
    pub(crate) fn draw(&self, painter: &egui::Painter, at: egui::Pos2) {
        paint_centered(painter, at, &self.text, self.size, self.color);
    }
}

/// Draws `text` at `size` points centred on `at`. A single character (an icon glyph, a letter, a digit) is centred on its
/// ink: egui's `CENTER_CENTER` centres the line box, which sits a descender's height too low for it. Longer text is a
/// line and is centred on its line box, so it does not shift with the letters it happens to hold.
pub(crate) fn paint_centered(painter: &egui::Painter, at: egui::Pos2, text: &str, size: f32, color: egui::Color32) {
    let galley = painter.layout_no_wrap(text.to_owned(), egui::FontId::proportional(size), color);
    let mut chars = text.chars();
    let single = chars.next().is_some() && chars.next().is_none();
    let ink = galley.rows.first().filter(|_| single).and_then(|row| {
        let glyph = row.glyphs.first()?;
        Some(egui::Rect::from_min_size(row.pos + glyph.pos.to_vec2() + glyph.uv_rect.offset, glyph.uv_rect.size))
    });
    let origin = at - ink.map_or(galley.size() / 2.0, |r| r.center().to_vec2());
    painter.galley(origin, galley, color);
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

    /// A spinner while `busy` (an image decoding, a PDF rendering, a tab waking), else `self`.
    pub(crate) fn busy(self, busy: bool) -> Self {
        if busy {
            Self::Spinner
        } else {
            self
        }
    }

    /// The text part alone: a glyph, or a tile's letter or globe, `size` screen points across at `center`.
    /// `default_ink` colors a glyph that names none. For a caller that has a `Painter` but no `Ui`.
    pub(crate) fn text(self, center: egui::Pos2, size: f32, default_ink: egui::Color32) -> Option<IdentityText> {
        let text = |text: &str, size: f32, color| Some(IdentityText { center, text: text.to_owned(), size, color });
        match self {
            Self::Spinner | Self::Image(_) => None,
            Self::Glyph { glyph, color } => text(glyph, size * 0.875, color.unwrap_or(default_ink)),
            Self::Tile { fill, letter: Some(c) } => text(&c.to_string(), size * 0.7, ink_on(fill)),
            Self::Tile { fill, letter: None } => text(egui_phosphor::light::GLOBE, size * 0.75, ink_on(fill)),
        }
    }

    /// Paints the shapes at `center`, `size` screen points across (`inv_zoom` maps points to world units), and
    /// returns the text (`Identity::text`) to draw on top.
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
        match self {
            Self::Spinner => egui::Spinner::new().size(side).paint_at(ui, square),
            Self::Image(tex) => {
                ui.painter().image(tex, square, FULL_UV, egui::Color32::WHITE);
            }
            Self::Tile { fill, .. } => {
                ui.painter().rect_filled(square, (size * 0.25 * inv_zoom).min(255.0), fill);
            }
            Self::Glyph { .. } => {}
        }
        self.text(center, size, default_ink)
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

/// The icon's size in a list row, in points; rows are text-high, so it is smaller than a header's.
const ROW_ICON_SIZE: f32 = 12.0;
/// The gap between a row's icon and its text.
const ROW_ICON_GAP: f32 = 6.0;
/// How far a bare list row (`popups::list_row`, a label with no button padding) insets its icon from its left edge.
pub(crate) const LIST_ROW_INSET: f32 = 4.0;
/// The tile fill of a page that no engine is known for (a history or bookmark row).
const NEUTRAL_TILE: egui::Color32 = egui::Color32::from_rgb(96, 98, 108);

/// `label` for a list row with room before it for `paint_row_icon`; `inset` is the same as that call's `pad`, less
/// the padding the row itself puts before its text (a button's), so 0 for a button.
pub(crate) fn row_label(ui: &egui::Ui, label: &str, inset: f32) -> egui::WidgetText {
    let mut job = egui::text::LayoutJob::default();
    let format = egui::TextFormat {
        font_id: egui::TextStyle::Button.resolve(ui.style()),
        color: egui::Color32::PLACEHOLDER,
        ..Default::default()
    };
    job.append(label, inset + ROW_ICON_SIZE + ROW_ICON_GAP, format);
    job.into()
}

/// Paints `identity` in the room `row_label` left at the left of a row's `rect`, `pad` points in (a button's padding, or `LIST_ROW_INSET`).
pub(crate) fn paint_row_icon(ui: &egui::Ui, rect: egui::Rect, pad: f32, identity: Identity) {
    let center = egui::pos2(rect.left() + pad + ROW_ICON_SIZE / 2.0, rect.center().y);
    if let Some(text) = identity.paint(ui, center, ROW_ICON_SIZE, 1.0, ui.visuals().weak_text_color()) {
        text.draw(ui.painter(), text.center);
    }
}

impl App {
    /// The icon of a list row for the tab `id` showing `url`: its favicon, else the tile in its engine's color.
    pub(crate) fn row_identity(&self, id: &str, url: &str) -> Identity {
        self.tab_identity(url, false, self.mirror.view_kind(id))
    }

    /// The icon of a list row for a page no tab shows (history, a bookmark): its favicon, else a neutral tile. Takes
    /// the GPU state, not `self`, so a popup can call it while it holds another field of `App`.
    pub(crate) fn page_row_identity(gpu: Option<&GpuState>, url: &str) -> Identity {
        match UrlParts::parse(url).origin().and_then(|origin| gpu?.favicon_id(&origin)) {
            Some(tex) => Identity::Image(tex),
            None => Identity::Tile { fill: NEUTRAL_TILE, letter: UrlParts::parse(url).initial() },
        }
    }

    /// Asks for the favicon of each of `urls`' origins not yet fetched (a list row only has one once it is).
    pub(crate) fn request_favicons<'a>(&mut self, urls: impl IntoIterator<Item = &'a str>) {
        for origin in urls.into_iter().filter_map(|url| UrlParts::parse(url).origin()) {
            if !self.favicon_resolved.contains(&origin) {
                self.favicon_fetcher.request(origin);
            }
        }
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
    fn a_line_of_text_is_centred_on_its_line_box() {
        let ctx = egui::Context::default();
        crate::theme::apply_fonts(&ctx);
        let at = egui::pos2(50.0, 50.0);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| paint_centered(ui.painter(), at, "Egg", 11.0, BG));
        output.textures_delta.clear();
        let middle = output.shapes.iter().find_map(|s| match &s.shape {
            egui::Shape::Text(t) => Some(t.pos + t.galley.size() / 2.0),
            _ => None,
        });
        let middle = middle.expect("the line was drawn");
        assert!((middle - at).length() < 0.5, "{middle:?} vs {at:?}");
    }

    #[test]
    fn busy_replaces_any_identity_with_the_spinner() {
        let glyph = Identity::Glyph { glyph: "x", color: None };
        assert_eq!(glyph.busy(true), Identity::Spinner);
        assert_eq!(glyph.busy(false), glyph);
    }

    #[test]
    fn a_spinner_paints_shapes_and_no_text() {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let text = Identity::Spinner.paint(ui, egui::pos2(20.0, 20.0), 16.0, 1.0, TEXT_WEAK);
            assert!(text.is_none());
        });
        output.textures_delta.clear();
        assert!(!output.shapes.is_empty(), "the spinner drew nothing");
    }

    #[test]
    fn a_row_label_leaves_room_for_the_icon_before_its_text() {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let egui::WidgetText::LayoutJob(job) = row_label(ui, "page", LIST_ROW_INSET) else {
                panic!("not a layout job")
            };
            assert_eq!(job.sections[0].leading_space, LIST_ROW_INSET + ROW_ICON_SIZE + ROW_ICON_GAP);
        });
        output.textures_delta.clear();
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
