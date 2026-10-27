//! The icon that says what a card or tab is: one type and one painter for the tab card's header and strip, the
//! address bar, and every kind's card header.

use super::*;
use crate::store::model::NodeId;

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
        record_icon(ui, self, square);
        match self {
            Self::Spinner => crate::theme::spinner(ui.visuals()).color(default_ink).size(side).paint_at(ui, square),
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

/// An icon `Identity::paint` drew in the last finished frame: what `export_state`'s `identities` lists.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DrawnIcon {
    /// `spinner`, `image`, `glyph` or `tile`.
    pub(crate) kind: &'static str,
    /// The glyph, or a tile's letter; empty for a spinner, an image and a letterless tile.
    pub(crate) text: String,
    /// Where it is on screen.
    pub(crate) rect: egui::Rect,
}

/// A labelled list row: its text, screen rect, and whether it is the selected (highlighted or focused) one.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DrawnRow {
    pub(crate) text: String,
    pub(crate) rect: egui::Rect,
    pub(crate) selected: bool,
}

/// A control with a name (`NamedTip`): its tooltip text and screen rect.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DrawnControl {
    pub(crate) label: String,
    pub(crate) rect: egui::Rect,
}

/// The icons drawn so far this frame and in the last finished one.
#[derive(Clone, Default)]
struct IconLog {
    building: Vec<DrawnIcon>,
    done: Vec<DrawnIcon>,
    rows_building: Vec<DrawnRow>,
    rows_done: Vec<DrawnRow>,
    controls_building: Vec<DrawnControl>,
    controls_done: Vec<DrawnControl>,
}

fn icon_log_id() -> egui::Id {
    egui::Id::new("identity_icon_log")
}

/// Starts a frame's icon log: what was drawn becomes the finished frame's list.
pub(crate) fn begin_icon_frame(ctx: &egui::Context) {
    ctx.data_mut(|d| {
        let log = d.get_temp_mut_or_default::<IconLog>(icon_log_id());
        log.done = std::mem::take(&mut log.building);
        log.rows_done = std::mem::take(&mut log.rows_building);
        log.controls_done = std::mem::take(&mut log.controls_building);
    });
}

/// The icons the last finished frame drew, in paint order.
pub(crate) fn drawn_icons(ctx: &egui::Context) -> Vec<DrawnIcon> {
    ctx.data(|d| d.get_temp::<IconLog>(icon_log_id()).map(|log| log.done).unwrap_or_default())
}

/// The labelled list rows (an icon row with a label) the last finished frame drew: text and screen rect.
pub(crate) fn drawn_rows(ctx: &egui::Context) -> Vec<DrawnRow> {
    ctx.data(|d| d.get_temp::<IconLog>(icon_log_id()).map(|log| log.rows_done).unwrap_or_default())
}

/// The named controls the last finished frame drew, in paint order.
pub(crate) fn drawn_controls(ctx: &egui::Context) -> Vec<DrawnControl> {
    ctx.data(|d| d.get_temp::<IconLog>(icon_log_id()).map(|log| log.controls_done).unwrap_or_default())
}

/// Notes the control `name` drawn at `rect` (in `layer`'s space).
pub(crate) fn note_control(ctx: &egui::Context, layer: egui::LayerId, name: &str, rect: egui::Rect) {
    let rect = ctx.layer_transform_to_global(layer).map_or(rect, |t| t * rect);
    ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<IconLog>(icon_log_id())
            .controls_building
            .push(DrawnControl { label: name.to_owned(), rect })
    });
}

/// Notes the labelled row `label` at `rect`; an empty label is no row.
fn note_row(ui: &egui::Ui, label: &str, selected: bool, rect: egui::Rect) {
    if label.is_empty() {
        return;
    }
    let rect = ui.ctx().layer_transform_to_global(ui.layer_id()).map_or(rect, |t| t * rect);
    ui.ctx().data_mut(|d| {
        d.get_temp_mut_or_default::<IconLog>(icon_log_id()).rows_building.push(DrawnRow {
            text: label.to_owned(),
            rect,
            selected,
        })
    });
}

/// Notes that `identity` is drawn in `square` (in `ui`'s layer space).
fn record_icon(ui: &egui::Ui, identity: Identity, square: egui::Rect) {
    let (kind, text) = match identity {
        Identity::Spinner => ("spinner", String::new()),
        Identity::Image(_) => ("image", String::new()),
        Identity::Glyph { glyph, .. } => ("glyph", glyph.to_owned()),
        Identity::Tile { letter, .. } => ("tile", letter.map(String::from).unwrap_or_default()),
    };
    let rect = ui.ctx().layer_transform_to_global(ui.layer_id()).map_or(square, |t| t * square);
    ui.ctx().data_mut(|d| {
        d.get_temp_mut_or_default::<IconLog>(icon_log_id()).building.push(DrawnIcon { kind, text, rect })
    });
}

impl App {
    /// `url`'s favicon texture, once its origin's favicon has been fetched.
    pub(crate) fn favicon_texture(&self, url: &str) -> Option<egui::TextureId> {
        Self::favicon_in(self.gpu.as_ref(), url)
    }

    /// `favicon_texture` from the GPU state alone, for a popup that holds another field of `App`.
    pub(crate) fn favicon_in(gpu: Option<&GpuState>, url: &str) -> Option<egui::TextureId> {
        gpu?.favicon_id(&UrlParts::parse(url).origin()?)
    }

    /// What `url` shows in its icon slot (`Identity::of_tab`), in a tab's card, the address bar and the trail strip alike.
    pub(crate) fn tab_identity(&self, url: &str, loading: bool, kind: ViewKind) -> Identity {
        Identity::of_tab(loading, self.favicon_texture(url), kind, url)
    }
}

/// The icon size in points: a header or the address bar (`ICON_SIZE`), and the smaller one for a title strip, a list row
/// or the trail strip (`ICON_SIZE_SMALL`).
pub(crate) const ICON_SIZE: f32 = 16.0;
pub(crate) const ICON_SIZE_SMALL: f32 = 12.0;
/// The gap between a row's icon and its text.
const ROW_ICON_GAP: f32 = 6.0;
/// How far a bare list row (`popups::list_row`, a label with no button padding) insets its icon from its left edge.
pub(crate) const LIST_ROW_INSET: f32 = 4.0;
/// The tile fill of a page that no engine is known for (a history or bookmark row).
const NEUTRAL_TILE: egui::Color32 = egui::Color32::from_rgb(96, 98, 108);

/// `label` for a list row with room before it for `paint_row_icon`; `inset` is the same as that call's `pad`, less
/// the padding the row itself puts before its text (a button's), so 0 for a button.
/// A `qualifier` (`disambiguate::qualifiers`) follows in italics: the row's own text colour, so it stays readable on
/// a selected row's fill.
pub(crate) fn row_label(ui: &egui::Ui, label: &str, qualifier: Option<&str>, inset: f32) -> egui::WidgetText {
    let mut job = egui::text::LayoutJob::default();
    let format = egui::TextFormat {
        font_id: egui::TextStyle::Button.resolve(ui.style()),
        color: egui::Color32::PLACEHOLDER,
        ..Default::default()
    };
    job.append(label, inset + ICON_SIZE_SMALL + ROW_ICON_GAP, format.clone());
    if let Some(qualifier) = qualifier {
        job.append(&format!("  {qualifier}"), 0.0, egui::TextFormat { italics: true, ..format });
    }
    job.into()
}

/// Paints `identity` in the room `row_label` left at the left of a row's `rect`, `pad` points in (a button's padding, or `LIST_ROW_INSET`).
pub(crate) fn paint_row_icon(
    ui: &egui::Ui,
    rect: egui::Rect,
    pad: f32,
    label: &str,
    selected: bool,
    identity: Identity,
) {
    note_row(ui, label, selected, rect);
    let center = egui::pos2(rect.left() + pad + ICON_SIZE_SMALL / 2.0, rect.center().y);
    if let Some(text) = identity.paint(ui, center, ICON_SIZE_SMALL, 1.0, ui.visuals().weak_text_color()) {
        text.draw(ui.painter(), text.center);
    }
}

impl App {
    /// The icon of a list row for the tab `id` showing `url`: its favicon, else the tile in its engine's color.
    pub(crate) fn row_identity(&self, id: NodeId, url: &str) -> Identity {
        self.tab_identity(url, false, self.doc.mirror.view_kind(id))
    }

    /// The icon of a list row for a page no tab shows (history, a bookmark): its favicon, else a neutral tile. Takes
    /// the GPU state, not `self`, so a popup can call it while it holds another field of `App`.
    pub(crate) fn page_row_identity(gpu: Option<&GpuState>, url: &str) -> Identity {
        match Self::favicon_in(gpu, url) {
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
        crate::theme::INK_DARK
    } else {
        crate::theme::INK_LIGHT
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
        assert_eq!(ink_on(view_color(ViewKind::Servo)), crate::theme::INK_DARK);
        assert_eq!(ink_on(view_color(ViewKind::Webkit)), crate::theme::INK_DARK);
        assert_eq!(ink_on(view_color(ViewKind::Webview2)), crate::theme::INK_LIGHT);
    }

    #[test]
    fn a_letter_is_centred_on_its_ink_not_its_line_box() {
        let ctx = crate::theme::testutil::ctx(true);
        let at = egui::pos2(50.0, 50.0);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let text = IdentityText { center: at, text: "E".into(), size: 11.0, color: INK_DARK };
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
        let ctx = crate::theme::testutil::ctx(true);
        let at = egui::pos2(50.0, 50.0);
        let mut output =
            ctx.run_ui(egui::RawInput::default(), |ui| paint_centered(ui.painter(), at, "Egg", 11.0, INK_DARK));
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
            let text = Identity::Spinner.paint(ui, egui::pos2(20.0, 20.0), 16.0, 1.0, egui::Color32::GRAY);
            assert!(text.is_none());
        });
        output.textures_delta.clear();
        assert!(!output.shapes.is_empty(), "the spinner drew nothing");
    }

    #[test]
    fn a_row_label_leaves_room_for_the_icon_before_its_text() {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let egui::WidgetText::LayoutJob(job) = row_label(ui, "page", None, LIST_ROW_INSET) else {
                panic!("not a layout job")
            };
            assert_eq!(job.sections[0].leading_space, LIST_ROW_INSET + ICON_SIZE_SMALL + ROW_ICON_GAP);
        });
        output.textures_delta.clear();
    }

    #[test]
    fn a_tile_paints_a_filled_square_and_hands_back_its_letter() {
        let ctx = egui::Context::default();
        let fill = view_color(ViewKind::Cef);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let text = Identity::Tile { fill, letter: Some('E') }
                .paint(ui, egui::pos2(10.0, 10.0), 16.0, 1.0, egui::Color32::GRAY)
                .expect("a tile has a letter");
            assert_eq!((text.text.as_str(), text.center, text.color), ("E", egui::pos2(10.0, 10.0), ink_on(fill)));
            let globe =
                Identity::Tile { fill, letter: None }.paint(ui, egui::Pos2::ZERO, 16.0, 1.0, egui::Color32::GRAY);
            assert_eq!(globe.unwrap().text, egui_phosphor::light::GLOBE);
        });
        output.textures_delta.clear();
    }

    #[test]
    fn painted_icons_are_logged_for_the_finished_frame_only() {
        let ctx = egui::Context::default();
        let paint = |ctx: &egui::Context| {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                begin_icon_frame(ui.ctx());
                Identity::Tile { fill: view_color(ViewKind::Cef), letter: Some('E') }.paint(
                    ui,
                    egui::pos2(10.0, 10.0),
                    16.0,
                    1.0,
                    egui::Color32::GRAY,
                );
                Identity::Spinner.paint(ui, egui::pos2(50.0, 10.0), 16.0, 1.0, egui::Color32::GRAY);
                paint_row_icon(
                    ui,
                    egui::Rect::from_min_size(egui::pos2(0.0, 30.0), egui::vec2(100.0, 14.0)),
                    4.0,
                    "page",
                    true,
                    Identity::Spinner,
                );
                paint_row_icon(ui, egui::Rect::ZERO, 0.0, "", false, Identity::Spinner);
            });
            output.textures_delta.clear();
        };
        assert!(drawn_icons(&ctx).is_empty());
        paint(&ctx);
        assert!(drawn_icons(&ctx).is_empty(), "the frame is unfinished until the next begins");
        paint(&ctx);
        let icons = drawn_icons(&ctx);
        let seen: Vec<_> = icons.iter().map(|i| (i.kind, i.text.as_str(), i.rect.center())).collect();
        assert_eq!(seen.len(), 4);
        assert_eq!(seen[..2], [("tile", "E", egui::pos2(10.0, 10.0)), ("spinner", "", egui::pos2(50.0, 10.0))]);
        let rows = drawn_rows(&ctx);
        assert_eq!(rows.iter().map(|r| (r.text.as_str(), r.selected)).collect::<Vec<_>>(), [("page", true)]);
    }
}
