//! A JSON level's painter and input (spec "Cards"): record cards drawn
//! in the Scene, their text queued through `deferred_overlays` (plan
//! ruling 11). The bar, the menu, keys (Task 7) and the outline (Task 8)
//! live here too.

use serde_json::Value;

use super::*;
use crate::card::card_focus_ring;
use crate::json_level::{CopyWhat, JsonEdit};
use crate::json_view::{self, Card};

/// Text height in world units (plan ruling 11).
const TEXT_SIZE: f32 = 12.0;
use crate::card::{
    HEADER_CENTER_Y, HEADER_GLYPH_X, HEADER_SOURCE_SIZE, HEADER_SOURCE_Y, HEADER_TEXT_X, HEADER_TITLE_SIZE,
    HEADER_TITLE_Y, TAB_HEADER_POINTS,
};
/// Inset of a card's text from its edges.
pub(crate) const PAD: f32 = 8.0;
/// An object row's key column.
const KEY_WIDTH: f32 = 84.0;
/// An array row's index column, right-aligned.
const INDEX_WIDTH: f32 = 22.0;
/// A container row's `{n}` / `[n]` column.
pub(crate) const COUNT_WIDTH: f32 = 44.0;
/// Below this zoom, rows are gray bars without text (spec "Level of
/// detail").
const LOD_TEXT_ZOOM: f32 = 0.5;
/// A search hit's border and row wash: blue, apart from the selection's
/// ACCENT gold.
const SEARCH_HIT: egui::Color32 = egui::Color32::from_rgb(90, 150, 230);
/// Mixed into an array card's fill so array and object cards read apart.
const ARRAY_TINT: egui::Color32 = egui::Color32::from_rgb(90, 100, 160);
/// A row tooltip's cap, in characters.
const TOOLTIP_MAX_CHARS: usize = 2000;
/// One outline row's least height, in points; a taller font raises it.
const OUTLINE_ROW_HEIGHT: f32 = 20.0;
/// An outline row's open/close caret, and the gap left where a row has none.
const CARET_SIZE: egui::Vec2 = egui::vec2(16.0, 16.0);
/// The bar's selected-path label's widest, in points.
const PATH_LABEL_MAX_WIDTH: f32 = 240.0;

/// Which colour a value's text takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ValueKind {
    String,
    Number,
    /// `true`, `false` and `null`.
    Literal,
    Container,
}

pub(crate) fn value_kind(value: &Value) -> ValueKind {
    match value {
        Value::String(_) => ValueKind::String,
        Value::Number(_) => ValueKind::Number,
        Value::Bool(_) | Value::Null => ValueKind::Literal,
        Value::Array(_) | Value::Object(_) => ValueKind::Container,
    }
}

/// `0x00RRGGBB` to a colour (plan ruling 4).
pub(crate) fn rgb(hex: u32) -> egui::Color32 {
    let [_, r, g, b] = hex.to_be_bytes();
    egui::Color32::from_rgb(r, g, b)
}

/// A colour as `0x00RRGGBB`, the way `session_sync::color_value` stores one.
pub(crate) fn rgb_u32(color: egui::Color32) -> u32 {
    u32::from_be_bytes([0, color.r(), color.g(), color.b()])
}

/// The level's text colours: VS Code's JSON colours for the theme.
pub(crate) struct Palette {
    pub(crate) key: egui::Color32,
    pub(crate) string: egui::Color32,
    pub(crate) number: egui::Color32,
    pub(crate) literal: egui::Color32,
    pub(crate) container: egui::Color32,
    /// Indices, labels, counts and `+ N more`.
    pub(crate) muted: egui::Color32,
}

pub(crate) fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            key: rgb(0x9cdcfe),
            string: rgb(0xce9178),
            number: rgb(0xb5cea8),
            literal: rgb(0x569cd6),
            container: rgb(0xc586c0),
            muted: rgb(0x8a8a8a),
        }
    } else {
        Palette {
            key: rgb(0x0451a5),
            string: rgb(0xa31515),
            number: rgb(0x098658),
            literal: rgb(0x0000ff),
            container: rgb(0xaf00db),
            muted: rgb(0x6e6e6e),
        }
    }
}

impl Palette {
    pub(crate) fn of(&self, kind: ValueKind) -> egui::Color32 {
        match kind {
            ValueKind::String => self.string,
            ValueKind::Number => self.number,
            ValueKind::Literal => self.literal,
            ValueKind::Container => self.container,
        }
    }
}

/// What one row shows (spec "Cards").
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RowParts {
    /// An object entry's key.
    pub(crate) key: Option<String>,
    /// An array element's index.
    pub(crate) index: Option<String>,
    /// A container element's label (the label rule).
    pub(crate) label: Option<String>,
    /// `display_text`: a scalar as JSON, or `{n}` / `[n]`.
    pub(crate) value: String,
    pub(crate) kind: ValueKind,
    /// A child card hangs off this row's port.
    pub(crate) port: bool,
}

/// The row for entry `key` (an index for an array) of `parent`.
pub(crate) fn row_parts(parent: &Value, key: &str, value: &Value) -> RowParts {
    let in_array = parent.is_array();
    let kind = value_kind(value);
    RowParts {
        key: (!in_array).then(|| key.to_string()),
        index: in_array.then(|| key.to_string()),
        label: if in_array && kind == ValueKind::Container { json_view::label(value) } else { None },
        value: json_view::display_text(value),
        kind,
        port: json_view::is_card(value),
    }
}

/// What a card's header shows.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct HeaderParts {
    pub(crate) icon: &'static str,
    pub(crate) title: String,
    /// The label rule, only while collapsed.
    pub(crate) label: Option<String>,
    /// `{n}` / `[n]`, or a scalar root's value.
    pub(crate) count: String,
    pub(crate) count_kind: ValueKind,
    /// A dim second line under the title (a file's folder) on the Json card's preview.
    pub(crate) source: Option<String>,
}

pub(crate) fn header_parts(root: &Value, card: &Card, root_name: &str) -> HeaderParts {
    let value = root.pointer(&card.pointer).unwrap_or(&Value::Null);
    HeaderParts {
        icon: if value.is_array() {
            egui_phosphor::light::BRACKETS_SQUARE
        } else {
            egui_phosphor::light::BRACKETS_CURLY
        },
        title: json_view::card_title(root, &card.pointer, root_name),
        label: if card.collapsed { json_view::label(value) } else { None },
        count: json_view::display_text(value),
        count_kind: value_kind(value),
        source: None,
    }
}

/// One piece of a card's text, in world units.
struct TextItem {
    pos: egui::Pos2,
    align: egui::Align2,
    text: String,
    color: egui::Color32,
    /// Cut to this width with `…`.
    max_width: f32,
    /// Font size in world units.
    size: f32,
}

/// Snaps a zoom scale to one of `STEPS_PER_OCTAVE` geometric steps per
/// doubling. egui caches a text layout by font size and width, so a size that
/// changes every zoom frame re-shapes all visible text every frame; snapped,
/// the layouts survive until the zoom crosses a step. The text ends up at
/// most half a step (about 4%) off the card's true scale.
fn quantize_scale(scale: f32) -> f32 {
    const STEPS_PER_OCTAVE: f32 = 8.0;
    if !(scale.is_finite() && scale > 0.0) {
        return scale;
    }
    ((scale.log2() * STEPS_PER_OCTAVE).round() / STEPS_PER_OCTAVE).exp2()
}

/// Queues a card's text as one overlay. The scale comes from `project`
/// at paint time, so the text never lags the card by a frame.
fn push_card_text(deferred_overlays: &mut DeferredOverlays, items: Vec<TextItem>) {
    if items.is_empty() {
        return;
    }
    deferred_overlays.push(Box::new(move |painter, project| {
        let scale = quantize_scale(project(egui::pos2(1.0, 0.0)).x - project(egui::Pos2::ZERO).x);
        for item in items {
            let font = egui::FontId::proportional(item.size * scale);
            let mut job = egui::text::LayoutJob::simple_singleline(item.text, font, item.color);
            job.wrap = egui::text::TextWrapping::truncate_at_width(item.max_width * scale);
            let galley = painter.layout_job(job);
            let min = item.align.anchor_size(project(item.pos), galley.size()).min;
            painter.galley(min, galley, item.color);
        }
    }));
}

/// How one card looks this frame.
pub(crate) struct CardLook<'a> {
    pub(crate) palette: &'a Palette,
    pub(crate) text_color: egui::Color32,
    pub(crate) inv_zoom: f32,
    /// Text, or gray bars (level of detail).
    pub(crate) text: bool,
    pub(crate) selected_row: Option<usize>,
    /// Sorted entry rows that match the search.
    pub(crate) hit_rows: &'a [usize],
    /// The world rect shown (the camera's): only entry rows intersecting it are painted. `Rect::EVERYTHING`
    /// paints them all (the Json card's preview).
    pub(crate) visible: egui::Rect,
    /// A container row's port dot: true on the JSON level itself, where it
    /// marks a row with a child card and an edge to it. False for the Json
    /// card's preview at its parent's level, which has no child cards or
    /// edges to show a dot for.
    pub(crate) ports: bool,
    /// The Json card's preview at its parent's level: header F's look and height
    /// (`json_view::PREVIEW_HEADER_HEIGHT`), not the level cards' one-line header.
    pub(crate) preview: bool,
}

/// A JSON level card's background, header tint and border.
fn paint_json_frame(
    painter: &egui::Painter,
    rect: egui::Rect,
    fill: egui::Color32,
    tint: Option<egui::Color32>,
    border: (f32, egui::Color32),
    inv_zoom: f32,
) {
    let rounding = 2.0 * inv_zoom;
    painter.rect_filled(rect, rounding, fill);
    if let Some(tint) = tint {
        let header = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), json_view::HEADER_HEIGHT));
        painter.rect_filled(header, rounding, tint.gamma_multiply(0.35));
    }
    painter.rect_stroke(rect, rounding, egui::Stroke::new(border.0 * inv_zoom, border.1), egui::StrokeKind::Inside);
}

/// A card's contents (the frame is the caller's): the header, then those of the first `rows` entries of `value` that
/// `look.visible` shows, then `+ N more` when `more > 0`.
/// Shared by the JSON level and the Json card at its parent's level.
#[allow(clippy::too_many_arguments, reason = "one card's full description; a struct for it would only rename these")]
pub(crate) fn paint_json_content(
    painter: &egui::Painter,
    deferred_overlays: &mut DeferredOverlays,
    rect: egui::Rect,
    value: &Value,
    header: &HeaderParts,
    rows: usize,
    more: usize,
    look: &CardLook,
) {
    use json_view::ROW_HEIGHT;
    // The preview's header is header F's 40 screen points, whatever the zoom; the level's is world-sized.
    let header_height = if look.preview { TAB_HEADER_POINTS * look.inv_zoom } else { json_view::HEADER_HEIGHT };
    let pal = look.palette;
    let row_rect = |row: usize| {
        egui::Rect::from_min_size(
            rect.min + egui::vec2(0.0, header_height + ROW_HEIGHT * row as f32),
            egui::vec2(rect.width(), ROW_HEIGHT),
        )
    };
    let shown = json_view::visible_rows(rect.top() + header_height, rows, look.visible);
    if !look.preview && rows + usize::from(more > 0) > 0 {
        painter.hline(
            rect.x_range(),
            rect.top() + header_height,
            egui::Stroke::new(look.inv_zoom, pal.muted.gamma_multiply(0.4)),
        );
    }
    let (lo, hi) =
        (look.hit_rows.partition_point(|&r| r < shown.start), look.hit_rows.partition_point(|&r| r < shown.end));
    for &row in &look.hit_rows[lo..hi] {
        painter.rect_filled(row_rect(row), 0.0, SEARCH_HIT.gamma_multiply(0.18));
    }
    if let Some(row) = look.selected_row {
        painter.rect_filled(row_rect(row), 0.0, ACCENT.gamma_multiply(0.22));
    }

    let mut text = Vec::new();
    let inner = rect.width() - 2.0 * PAD;
    let left = rect.left() + PAD;
    let right = rect.right() - PAD;
    if look.text {
        let y = rect.top() + header_height / 2.0;
        let count_color = if header.count_kind == ValueKind::Container { pal.muted } else { pal.of(header.count_kind) };
        if look.preview {
            // Header F's layout, as `paint_card_header` places it: the glyph centred on the title and
            // source lines, placed from the `HEADER_*` offsets in `card`.
            let z = look.inv_zoom;
            let top = rect.top();
            let title_y = if header.source.is_some() { top + HEADER_TITLE_Y * z } else { top + HEADER_CENTER_Y * z };
            let text_left = rect.left() + HEADER_TEXT_X * z;
            let text_width = rect.right() - text_left - COUNT_WIDTH - PAD;
            text.push(TextItem {
                pos: egui::pos2(rect.left() + HEADER_GLYPH_X * z, top + HEADER_CENTER_Y * z),
                align: egui::Align2::LEFT_CENTER,
                text: header.icon.into(),
                color: pal.container,
                max_width: 16.0,
                size: 14.0 * z,
            });
            text.push(TextItem {
                pos: egui::pos2(text_left, title_y),
                align: egui::Align2::LEFT_CENTER,
                text: header.title.clone(),
                color: look.text_color,
                max_width: text_width,
                size: HEADER_TITLE_SIZE * z,
            });
            if let Some(source) = &header.source {
                text.push(TextItem {
                    pos: egui::pos2(text_left, top + HEADER_SOURCE_Y * z),
                    align: egui::Align2::LEFT_CENTER,
                    text: source.clone(),
                    color: pal.muted,
                    max_width: text_width,
                    size: HEADER_SOURCE_SIZE * z,
                });
            }
        } else {
            text.push(TextItem {
                pos: egui::pos2(left, y),
                align: egui::Align2::LEFT_CENTER,
                text: header.icon.into(),
                color: pal.container,
                max_width: 16.0,
                size: TEXT_SIZE,
            });
            let title_width = if header.label.is_some() { 88.0 } else { inner - 18.0 - COUNT_WIDTH };
            text.push(TextItem {
                pos: egui::pos2(left + 18.0, y),
                align: egui::Align2::LEFT_CENTER,
                text: header.title.clone(),
                color: look.text_color,
                max_width: title_width,
                size: TEXT_SIZE,
            });
            if let Some(label) = &header.label {
                text.push(TextItem {
                    pos: egui::pos2(left + 110.0, y),
                    align: egui::Align2::LEFT_CENTER,
                    text: label.clone(),
                    color: pal.muted,
                    max_width: inner - 110.0 - COUNT_WIDTH,
                    size: TEXT_SIZE,
                });
            }
        }
        text.push(TextItem {
            pos: egui::pos2(right, y),
            align: egui::Align2::RIGHT_CENTER,
            text: header.count.clone(),
            color: count_color,
            max_width: COUNT_WIDTH,
            size: if look.preview { HEADER_TITLE_SIZE * look.inv_zoom } else { TEXT_SIZE },
        });
    }
    let first = shown.start;
    for (row, (key, child)) in
        json_view::entries_from(value, first).take(shown.len()).enumerate().map(|(i, e)| (first + i, e))
    {
        let r = row_rect(row);
        let y = r.center().y;
        let parts = row_parts(value, &key, child);
        if parts.port && look.ports {
            painter.circle_filled(egui::pos2(r.right(), y), 3.5, pal.container);
        }
        if !look.text {
            painter.rect_filled(r.shrink2(egui::vec2(PAD, 7.0)), 2.0, pal.muted.gamma_multiply(0.35));
            continue;
        }
        let mut used = 0.0;
        if let Some(key) = parts.key {
            text.push(TextItem {
                pos: egui::pos2(left, y),
                align: egui::Align2::LEFT_CENTER,
                text: key,
                color: pal.key,
                max_width: KEY_WIDTH,
                size: TEXT_SIZE,
            });
            used = KEY_WIDTH + 6.0;
        }
        if let Some(index) = parts.index {
            text.push(TextItem {
                pos: egui::pos2(left + INDEX_WIDTH, y),
                align: egui::Align2::RIGHT_CENTER,
                text: index,
                color: pal.muted,
                max_width: INDEX_WIDTH,
                size: TEXT_SIZE,
            });
            used = INDEX_WIDTH + 6.0;
        }
        let value_width = if parts.kind == ValueKind::Container { COUNT_WIDTH } else { inner - used };
        if let Some(label) = parts.label {
            text.push(TextItem {
                pos: egui::pos2(left + used, y),
                align: egui::Align2::LEFT_CENTER,
                text: label,
                color: pal.muted,
                max_width: inner - used - COUNT_WIDTH - 6.0,
                size: TEXT_SIZE,
            });
        }
        text.push(TextItem {
            pos: egui::pos2(right, y),
            align: egui::Align2::RIGHT_CENTER,
            text: parts.value,
            color: pal.of(parts.kind),
            max_width: value_width,
            size: TEXT_SIZE,
        });
    }
    if more > 0 {
        let r = row_rect(rows);
        if look.text {
            text.push(TextItem {
                pos: egui::pos2(left, r.center().y),
                align: egui::Align2::LEFT_CENTER,
                text: format!("+ {more} more"),
                color: pal.muted,
                max_width: inner,
                size: TEXT_SIZE,
            });
        } else {
            painter.rect_filled(r.shrink2(egui::vec2(PAD, 7.0)), 2.0, pal.muted.gamma_multiply(0.2));
        }
    }
    push_card_text(deferred_overlays, text);
}

/// Set when a JSON level is entered by keyboard: the root card takes focus on its first draw.
pub(crate) fn json_focus_root_key() -> egui::Id {
    egui::Id::new("json_focus_root")
}

impl App {
    /// The JSON level's cards, inside `build_graph_ui`'s Scene: painted,
    /// culled to the viewport, and their clicks and drags turned into
    /// `edits`, applied once the Scene closure returns. A drag moves the
    /// card live (its rect, so edges and the minimap follow) and saves
    /// its position on release.
    pub(crate) fn draw_json_level(
        &mut self,
        ui: &mut egui::Ui,
        deferred_overlays: &mut DeferredOverlays,
        edits: &mut Vec<JsonEdit>,
        inv_zoom: f32,
        zoom_scale: f32,
    ) {
        let Some(level) = self.json_level.as_ref() else { return };
        let Some(json) = self.nodes.get(&level.node_id).and_then(|n| n.as_json()) else { return };
        let root_name = download_filename(&json.path).to_string();
        let view = self.mirror.view(&level.node_id); // the decoded cache: no parse per frame (2b-4 Ruling 8)
        let pal = palette(ui.visuals().dark_mode);
        let base_fill = ui.visuals().extreme_bg_color;
        let idle_border = ui.visuals().weak_text_color();
        let text_color = ui.visuals().text_color();
        let hits = level.search.result.as_ref().map(|r| &r.hits);
        let mut moves = Vec::new();
        let card_id = |i: usize| egui::Id::new(("json_card", &level.node_id, &level.cards[i].pointer));
        // Tab / Shift+Tab walk the cards in order, including ones culled off screen (egui's own
        // Tab order only knows the cards drawn). The target is drawn this frame whatever the camera
        // shows, takes focus, and its `gained_focus` selects it, which pans to it. A focused card is
        // never culled either, or panning away would drop the focus.
        let tab_target_key = egui::Id::new("json_tab_target");
        let mut tab_target: Option<usize> = ui.data(|d| d.get_temp(tab_target_key));
        // The focused widget, read again only where it can change (the Tab request, each drawn card's interact),
        // not once per card: `has_focus(id)` is `focused() == Some(id)`, and egui applies focus moves at once.
        let mut focused = ui.memory(|m| m.focused());
        if let Some(at) = (0..level.cards.len()).find(|&i| focused == Some(card_id(i))) {
            tab_target = None;
            let shift = ui.input(|i| {
                i.events.iter().any(|e| {
                    matches!(
                        e,
                        egui::Event::Key { key: egui::Key::Tab, pressed: true, modifiers, .. } if modifiers.shift
                    )
                })
            });
            let next = if shift { at.checked_sub(1) } else { Some(at + 1).filter(|&n| n < level.cards.len()) };
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(card_id(at), egui::EventFilter { tab: next.is_some(), ..Default::default() })
            });
            if let (true, Some(next)) = (ui.input(|i| i.key_pressed(egui::Key::Tab)), next) {
                ui.memory_mut(|m| m.request_focus(card_id(next)));
                tab_target = Some(next);
                focused = ui.memory(|m| m.focused());
            }
        }
        ui.data_mut(|d| match tab_target {
            Some(t) => {
                d.insert_temp(tab_target_key, t);
            }
            None => {
                d.remove_temp::<usize>(tab_target_key);
            }
        });
        for (i, card) in level.cards.iter().enumerate() {
            let rect = level.rects[i];
            if !rect.intersects(self.camera.rect) && tab_target != Some(i) && focused != Some(card_id(i)) {
                continue;
            }
            let Some(value) = level.value.pointer(&card.pointer) else { continue };
            let selected_row =
                level.selected.as_deref().and_then(|s| json_view::row_of(value, &card.pointer, s, 0..card.rows));
            // This card's hit rows, cut to the rows it shows.
            let hit_rows: &[usize] = level
                .search
                .hit_rows
                .get(&card.pointer)
                .map_or(&[][..], |rows| &rows[..rows.partition_point(|&r| r < card.rows)]);
            let border = if level.selected.as_deref() == Some(card.pointer.as_str()) {
                (2.0, ACCENT)
            } else if hits.is_some_and(|h| h.contains(&card.pointer)) || !hit_rows.is_empty() {
                (2.0, SEARCH_HIT)
            } else {
                (1.0, idle_border)
            };
            let fill = if value.is_array() { base_fill.lerp_to_gamma(ARRAY_TINT, 0.12) } else { base_fill };
            let tint = view.and_then(|v| v.get(&card.pointer)).and_then(|o| o.color).map(rgb);
            paint_json_frame(ui.painter(), rect, fill, tint, border, inv_zoom);
            let look = CardLook {
                palette: &pal,
                text_color,
                inv_zoom,
                text: zoom_scale >= LOD_TEXT_ZOOM,
                selected_row,
                hit_rows,
                visible: self.camera.rect,
                ports: true,
                preview: false,
            };
            let header = header_parts(&level.value, card, &root_name);
            paint_json_content(ui.painter(), deferred_overlays, rect, value, &header, card.rows, card.more, &look);

            let response = ui.interact(
                rect,
                egui::Id::new(("json_card", &level.node_id, &card.pointer)),
                egui::Sense::click_and_drag(),
            );
            if i == 0 && ui.data_mut(|d| d.remove_temp::<bool>(json_focus_root_key())).unwrap_or(false) {
                response.request_focus();
            }
            focused = ui.memory(|m| m.focused());
            // Keyboard focus (Tab) selects the card; Enter then toggles it (`handle_json_keys`).
            if response.has_focus() {
                card_focus_ring(ui, rect, inv_zoom);
            }
            if response.gained_focus() {
                edits.push(JsonEdit::Select(card.pointer.clone()));
            }
            let at = response.interact_pointer_pos().or(response.hover_pos());
            let in_header = at.is_some_and(|p| p.y < rect.top() + json_view::HEADER_HEIGHT);
            if response.double_clicked() && in_header && json_view::entry_count(value) > 0 {
                edits.push(JsonEdit::Toggle(card.pointer.clone()));
            } else if response.clicked() {
                if let Some(p) = at {
                    if level.more_row_at(i, p) {
                        edits.push(JsonEdit::More(card.pointer.clone()));
                    } else {
                        edits.push(JsonEdit::Select(level.row_at(i, p).unwrap_or_else(|| card.pointer.clone())));
                    }
                }
            }
            if response.dragged() {
                moves.push((i, response.drag_delta()));
            }
            if response.drag_stopped() {
                edits.push(JsonEdit::Moved(card.pointer.clone(), rect.min));
            }
            // A scalar row's full text; the row itself is cut to fit.
            let tooltip = response
                .hover_pos()
                .filter(|_| !response.dragged())
                .and_then(|p| level.row_at(i, p))
                .and_then(|p| level.value.pointer(&p))
                .and_then(json_view::scalar_text)
                .map(|t| {
                    if t.chars().count() > TOOLTIP_MAX_CHARS {
                        format!("{}…", t.chars().take(TOOLTIP_MAX_CHARS).collect::<String>())
                    } else {
                        t
                    }
                });
            if let Some(tooltip) = tooltip {
                response.on_hover_text(tooltip);
            }
        }
        if let Some(level) = self.json_level.as_mut() {
            for (i, delta) in moves {
                level.rects[i] = level.rects[i].translate(delta);
            }
        }
    }

    /// The JSON level's part of the graph toolbar, after the breadcrumb
    /// (spec "Copy"): the selected path, Copy value, Copy path with its
    /// form menu, Relayout and the outline toggle.
    pub(crate) fn render_json_bar(&mut self, ui: &mut egui::Ui) {
        let Some(level) = self.json_level.as_ref() else { return };
        let selected = level.selected.clone();
        ui.separator();
        let shown = selected.as_deref().map(|p| if p.is_empty() { "/" } else { p }).unwrap_or("nothing selected");
        // Capped, so a deep pointer can't push the buttons after it off
        // the bar; the whole path is in the hover.
        let label = egui::Label::new(egui::RichText::new(shown).monospace().weak()).truncate();
        ui.allocate_ui(egui::vec2(PATH_LABEL_MAX_WIDTH, ui.available_height()), |ui| ui.add(label))
            .inner
            .on_hover_text(shown);
        let enabled = selected.is_some();
        toolbar_chip(ui, |ui| {
            // Invisible at rest like the icon chips' buttons: the chip is the background.
            let saved = ui.visuals().widgets.inactive.weak_bg_fill;
            ui.visuals_mut().widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
            let value = ui.add_enabled(enabled, egui::Button::new("Copy value")).on_hover_text("Ctrl+C");
            if value.clicked() {
                self.copy_json(ui.ctx(), selected.as_deref(), CopyWhat::Value);
            }
            let path = ui.add_enabled(enabled, egui::Button::new("Copy path")).on_hover_text("Ctrl+Shift+C");
            if path.clicked() {
                self.copy_json(ui.ctx(), selected.as_deref(), CopyWhat::Path);
            }
            // A phosphor caret, not a raw "▾": the app font's tofu-box rule.
            let form = ui
                .menu_button(egui_phosphor::light::CARET_DOWN, |ui| {
                    if selectable_label(ui, !self.json_accessor_paths, "JSON Pointer  /users/0/name").clicked() {
                        self.json_accessor_paths = false;
                        ui.close();
                    }
                    if selectable_label(ui, self.json_accessor_paths, "Accessor  users[0].name").clicked() {
                        self.json_accessor_paths = true;
                        ui.close();
                    }
                })
                .response
                .on_hover_text("Copy path's form");
            ui.visuals_mut().widgets.inactive.weak_bg_fill = saved;
            value.rect.union(path.rect).union(form.rect)
        });
        if ui.button("Relayout").on_hover_text("Forget every moved card's position").clicked() {
            self.apply_json_edit(JsonEdit::Relayout);
        }
        let outline = egui_phosphor::light::LIST_BULLETS;
        // On is a raised fill, not the gold selection color: gold is the primary action's.
        let on = if self.json_outline_visible { SURFACE_HOVER } else { egui::Color32::TRANSPARENT };
        let toggle =
            add_borderless(ui, true, egui::Button::new(egui::RichText::new(outline).color(TEXT_PRIMARY)).fill(on));
        if toggle.on_hover_text("Outline").clicked() {
            self.json_outline_visible = !self.json_outline_visible;
        }
    }

    fn copy_json(&mut self, ctx: &egui::Context, pointer: Option<&str>, what: CopyWhat) {
        if let Some(text) = pointer.and_then(|p| self.json_copy_text(p, what)) {
            ctx.copy_text(text);
        }
    }

    /// The right-click menu of a JSON card or row: Copy value / key /
    /// path, Collapse or Expand (cards), and the colour swatches (cards).
    /// Closes on a click elsewhere or Escape.
    pub(crate) fn build_json_menu(&mut self, ui: &mut egui::Ui) {
        let Some(level) = self.json_level.as_ref() else { return };
        let Some((pointer, pos)) = level.menu.clone() else { return };
        let card = level.index.get(&pointer).map(|&i| &level.cards[i]);
        let is_card = card.is_some();
        // Only a card with entries collapses, as with a header double-click:
        // a scalar or empty root has nothing to hide.
        let has_entries = level.value.pointer(&pointer).is_some_and(|v| json_view::entry_count(v) > 0);
        let collapsed = card.filter(|_| has_entries).map(|c| c.collapsed);
        let view = self.mirror.view(&level.node_id);
        let color = view.and_then(|v| v.get(&pointer)).and_then(|o| o.color).map(rgb);
        let mut close = false;
        let mut edit = None;
        let area = egui::Area::new(egui::Id::new("json_context_menu"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(ui.ctx(), |ui| {
                popup_frame(ui).show(ui, |ui| {
                    menu_item_style(ui);
                    ui.set_min_width(150.0);
                    for (label, what) in
                        [("Copy value", CopyWhat::Value), ("Copy key", CopyWhat::Key), ("Copy path", CopyWhat::Path)]
                    {
                        // The root has no key.
                        if what == CopyWhat::Key && pointer.is_empty() {
                            continue;
                        }
                        if ui.button(label).clicked() {
                            self.copy_json(ui.ctx(), Some(&pointer), what);
                            close = true;
                        }
                    }
                    if let Some(collapsed) = collapsed {
                        ui.add_space(3.0);
                        ui.separator();
                        ui.add_space(3.0);
                        if ui.button(if collapsed { "Expand" } else { "Collapse" }).clicked() {
                            edit = Some(JsonEdit::Toggle(pointer.clone()));
                            close = true;
                        }
                    }
                    if is_card {
                        if let Some(picked) = color_swatch_row(ui, color) {
                            edit = Some(JsonEdit::Color(pointer.clone(), picked.map(rgb_u32)));
                            close = true;
                        }
                    }
                });
            });
        if area.response.clicked_elsewhere() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            close = true;
        }
        if let Some(edit) = edit {
            self.apply_json_edit(edit);
        }
        if close {
            if let Some(level) = self.json_level.as_mut() {
                level.menu = None;
            }
        }
    }

    /// Ctrl+F on a JSON level (routed here from the global shortcut, which
    /// consumes the key): shows the outline and focuses its search box.
    pub(crate) fn focus_json_search(&mut self) {
        if let Some(level) = self.json_level.as_mut() {
            self.json_outline_visible = true;
            level.search.focus = true;
        }
    }

    /// Keys on a JSON level, in place of the graph's arrow-key focus
    /// moves: Up/Down select the previous/next outline row, Enter expands or collapses the
    /// selected card, Right/Left open/close its outline row (Left on a closed one selects the
    /// parent), copy (plan ruling 13) and Escape to clear the selection (Ctrl+F is
    /// `focus_json_search`). Skipped while a text field has this pass's keys (`typing`, from
    /// `shortcuts::text_field_has_keys` at the frame's start: on the Escape frame the field has
    /// already let go) or any widget has focus.
    pub(crate) fn handle_json_keys(&mut self, ui: &egui::Ui, typing: bool) {
        if typing || ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        let copy = ui.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy)).then_some(i.modifiers.shift));
        if let Some(shift) = copy {
            let selected = self.json_level.as_ref().and_then(|l| l.selected.clone());
            self.copy_json(ui.ctx(), selected.as_deref(), if shift { CopyWhat::Path } else { CopyWhat::Value });
        }
        let key = |k| ui.input(|i| i.key_pressed(k));
        if key(egui::Key::ArrowDown) {
            self.step_json_selection(1);
        } else if key(egui::Key::ArrowUp) {
            self.step_json_selection(-1);
        } else if key(egui::Key::Enter) {
            self.toggle_json_selection();
        } else if key(egui::Key::ArrowRight) {
            self.open_json_selection(true);
        } else if key(egui::Key::ArrowLeft) {
            self.open_json_selection(false);
        }
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            if let Some(level) = self.json_level.as_mut() {
                level.menu = None;
                level.selected = None;
            }
        }
    }

    /// The outline (spec "Outline"): a resizable right panel holding the
    /// search box and the value as an indented list, virtualized with
    /// `show_rows`. Drawn before the canvas takes the remaining space.
    pub(crate) fn build_json_outline(&mut self, ui: &mut egui::Ui) {
        if self.json_level.is_none() || !self.json_outline_visible {
            return;
        }
        self.poll_json_search();
        let panel = egui::Panel::right("json_outline")
            .resizable(true)
            .default_size(self.json_outline_width)
            .size_range(160.0..=640.0)
            .show(ui, |ui| self.json_outline_contents(ui));
        self.json_outline_width = panel.response.rect.width();
    }

    fn json_outline_contents(&mut self, ui: &mut egui::Ui) {
        let root_name = self.current_json_node().map(|j| download_filename(&j.path).to_string()).unwrap_or_default();
        let Some(level) = self.json_level.as_mut() else { return };
        let mut query = level.search.query.clone();
        let search = ui.add(
            egui::TextEdit::singleline(&mut query).hint_text("Search keys and values").desired_width(f32::INFINITY),
        );
        if std::mem::take(&mut level.search.focus) {
            search.request_focus();
        }
        let status = match (&level.search.pending, &level.search.result) {
            (Some(_), _) => Some("Searching…".to_string()),
            (None, Some(result)) if result.matches.is_empty() => Some("No matches".to_string()),
            (None, Some(result)) if result.matches.len() == 1 => Some("1 match".to_string()),
            (None, Some(result)) if result.truncated => Some(format!("First {} matches", result.matches.len())),
            (None, Some(result)) => Some(format!("{} matches", result.matches.len())),
            (None, None) => None,
        };
        if let Some(status) = status {
            ui.label(egui::RichText::new(status).weak().small());
        }
        // A single-line TextEdit gives up focus on Enter; taking it back
        // lets Enter (and then ↓) keep stepping.
        let enter = search.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if enter {
            search.request_focus();
        }
        let step = enter || search.has_focus() && ui.input(|i| i.key_pressed(egui::Key::ArrowDown));
        ui.separator();

        let pal = palette(ui.visuals().dark_mode);
        let rows = std::mem::take(&mut level.outline.rows);
        let searching = level.search.result.is_some();
        // Compact against the app's airy defaults (`theme.rs`), set on the
        // ui `show_rows` is given: it reads `item_spacing.y` from there for
        // its row pitch. With none, and no row taller than `row_height`
        // (the app's font can be), rows are exactly `row_height` apart, as
        // `show_rows` and `scroll_to` count on.
        ui.spacing_mut().item_spacing = egui::vec2(6.0, 0.0);
        ui.spacing_mut().button_padding = egui::vec2(4.0, 0.0);
        let row_height = [egui::TextStyle::Body, egui::TextStyle::Button]
            .iter()
            .fold(OUTLINE_ROW_HEIGHT, |h, style| h.max(ui.text_style_height(style)));
        let mut scroll = egui::ScrollArea::vertical().auto_shrink([false, false]);
        // `poll_json_search` has just rebuilt any stale rows, so the target
        // resolves against what's drawn.
        if let Some(row) = level.outline.scroll_to.take().and_then(|p| rows.iter().position(|r| r.pointer == p)) {
            scroll = scroll.vertical_scroll_offset((row as f32 * row_height - 80.0).max(0.0));
        }
        let mut clicked = None;
        let mut flip = None;
        scroll.show_rows(ui, row_height, rows.len(), |ui, range| {
            for row in &rows[range] {
                let Some(value) = level.value.pointer(&row.pointer) else { continue };
                let selected = level.selected.as_deref() == Some(row.pointer.as_str());
                let hit = level.search.result.as_ref().is_some_and(|r| r.hits.contains(&row.pointer));
                ui.horizontal(|ui| {
                    ui.set_height(row_height);
                    ui.add_space(row.depth as f32 * 12.0);
                    let open = searching || level.outline.open.contains(&row.pointer);
                    if json_view::is_card(value) && !searching {
                        let caret =
                            if open { egui_phosphor::light::CARET_DOWN } else { egui_phosphor::light::CARET_RIGHT };
                        let caret_button = ui.add_sized(
                            CARET_SIZE,
                            egui::Button::new(egui::RichText::new(caret).color(TEXT_PRIMARY)).frame(false),
                        );
                        focus_ring(ui, &caret_button);
                        if caret_button.clicked() {
                            flip = Some(row.pointer.clone());
                        }
                    } else {
                        // Same footprint as the caret, spacing included.
                        ui.allocate_exact_size(CARET_SIZE, egui::Sense::hover());
                    }
                    let parent = json_view::parent_pointer(&row.pointer).and_then(|p| level.value.pointer(p));
                    let (name, color) = match (parent, json_view::last_key(&row.pointer)) {
                        (Some(parent), Some(key)) if parent.is_array() => (key, pal.muted),
                        (_, Some(key)) => (key, pal.key),
                        (_, None) => (root_name.clone(), ui.visuals().text_color()),
                    };
                    let mut text = egui::RichText::new(name).color(color);
                    if hit {
                        text = text.background_color(SEARCH_HIT.gamma_multiply(0.3));
                    }
                    if rest_borderless(ui, |ui| ui.add(egui::Button::selectable(selected, text))).clicked() {
                        clicked = Some(row.pointer.clone());
                    }
                    if let Some(label) = parent.filter(|p| p.is_array()).and_then(|_| json_view::label(value)) {
                        ui.add(egui::Label::new(egui::RichText::new(label).color(pal.muted)).truncate());
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let kind = value_kind(value);
                        let color = if kind == ValueKind::Container { pal.muted } else { pal.of(kind) };
                        ui.add(
                            egui::Label::new(egui::RichText::new(json_view::display_text(value)).color(color))
                                .truncate(),
                        );
                    });
                });
            }
        });
        level.outline.rows = rows;
        if let Some(pointer) = flip {
            if !level.outline.open.remove(&pointer) {
                level.outline.open.insert(pointer);
            }
            level.outline.dirty = true;
        }
        if query != level.search.query {
            self.set_json_search(query);
        } else if step {
            self.step_json_match();
        }
        if let Some(pointer) = clicked {
            self.apply_json_edit(JsonEdit::Select(pointer));
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::json_view::{default_expanded, visible_cards, JsonView};

    /// The texts one level card paints, in order: header, rows, `+ N more`.
    fn painted_texts(value: &Value, card: &Card, visible: egui::Rect) -> Vec<String> {
        let ctx = egui::Context::default();
        crate::theme::apply_fonts(&ctx);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let mut overlays: DeferredOverlays = Vec::new();
            let pal = palette(true);
            let look = CardLook {
                palette: &pal,
                text_color: egui::Color32::WHITE,
                inv_zoom: 1.0,
                text: true,
                selected_row: None,
                hit_rows: &[],
                visible,
                ports: true,
                preview: false,
            };
            let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(json_view::CARD_WIDTH, card.height()));
            let header = header_parts(value, card, "f.json");
            paint_json_content(ui.painter(), &mut overlays, rect, value, &header, card.rows, card.more, &look);
            for overlay in overlays {
                overlay(ui.painter(), &|p| p);
            }
        });
        output.textures_delta.clear();
        output
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                _ => None,
            })
            .collect()
    }

    /// Wave 2 (H1a): a card's painted texts, pinned before rows are culled to the camera.
    #[test]
    fn a_card_paints_its_rows_texts() {
        let value = Value::Array((0..30).map(|i| Value::String(format!("v{i}"))).collect());
        let card = Card { pointer: String::new(), parent: None, depth: 0, collapsed: false, rows: 25, more: 5 };
        let texts = painted_texts(&value, &card, egui::Rect::EVERYTHING);
        assert_eq!(texts.len(), 54, "{texts:?}");
        assert_eq!(texts[1..5], ["f.json", "[30]", "0", "\"v0\""], "{texts:?}");
        assert_eq!(texts[53], "+ 5 more");
    }

    /// Wave 2 (H1a): only the rows the camera shows are painted; the header and `+ N more` always are.
    #[test]
    fn a_culled_card_paints_only_its_visible_rows() {
        let value = Value::Array((0..30).map(|i| Value::String(format!("v{i}"))).collect());
        let card = Card { pointer: String::new(), parent: None, depth: 0, collapsed: false, rows: 25, more: 5 };
        let full = painted_texts(&value, &card, egui::Rect::EVERYTHING);
        let visible = egui::Rect::from_min_max(egui::pos2(0.0, 137.0), egui::pos2(220.0, 245.0)); // rows 5..10
        assert_eq!(painted_texts(&value, &card, visible), [&full[..3], &full[13..23], &full[53..]].concat());
    }

    #[test]
    fn quantized_scale_is_stable_within_a_step_and_close_to_the_input() {
        // Nearby zoom frames share one layout size...
        assert_eq!(quantize_scale(1.00), quantize_scale(1.02));
        // ...a doubling is exactly 8 steps, powers of two are fixed points...
        assert!((quantize_scale(2.0) - 2.0).abs() < 1e-6 && (quantize_scale(0.25) - 0.25).abs() < 1e-6);
        // ...and no scale is more than half a step (2^(1/16), about 4.4%) off.
        for i in 1..400 {
            let s = 0.05 * i as f32;
            assert!((quantize_scale(s) / s - 1.0).abs() < 0.045, "scale {s}");
        }
        // Non-finite and non-positive scales pass through unchanged.
        assert!(quantize_scale(f32::NAN).is_nan() && quantize_scale(0.0) == 0.0);
    }

    #[test]
    fn rows_show_keys_or_indices_labels_and_typed_values() {
        let obj = json!({"name": "Ada", "n": 3, "ok": true, "none": null, "roles": ["a"], "empty": {}});
        let row = |k: &str| row_parts(&obj, k, &obj[k]);
        assert_eq!(
            row("name"),
            RowParts {
                key: Some("name".into()),
                index: None,
                label: None,
                value: "\"Ada\"".into(),
                kind: ValueKind::String,
                port: false,
            }
        );
        assert_eq!(row("n").kind, ValueKind::Number);
        assert_eq!((row("ok").kind, row("none").kind), (ValueKind::Literal, ValueKind::Literal));
        // An object's container row has a port but no label (spec mockup).
        let roles = row("roles");
        assert_eq!(
            (roles.value.as_str(), roles.kind, roles.port, roles.label),
            ("[1]", ValueKind::Container, true, None)
        );
        assert!(!row("empty").port, "{{}} shows inline, with no card");

        let arr = json!([{"name": "Ada"}, [-0.12, 51.5], 7]);
        let element = row_parts(&arr, "0", &arr[0]);
        assert_eq!(
            (element.key, element.index.as_deref(), element.label.as_deref()),
            (None, Some("0"), Some("\"Ada\""))
        );
        assert_eq!(row_parts(&arr, "1", &arr[1]).label.as_deref(), Some("[-0.12, 51.5]"));
        assert_eq!(row_parts(&arr, "2", &arr[2]).label, None, "scalars don't label");
    }

    #[test]
    fn headers_show_title_count_and_a_label_when_collapsed() {
        let root = json!({"users": [{"name": "Ada", "id": 1}]});
        let cards = visible_cards(&root, &JsonView::default(), &default_expanded(&root));
        let header = header_parts(&root, &cards[0], "users.json");
        assert_eq!((header.title.as_str(), header.count.as_str(), header.label), ("users.json", "{1}", None));
        let mut user = cards[2].clone();
        assert_eq!(user.pointer, "/users/0");
        user.collapsed = true;
        let header = header_parts(&root, &user, "users.json");
        assert_eq!(
            (header.title.as_str(), header.count.as_str(), header.label.as_deref()),
            ("users[0]", "{2}", Some("\"Ada\""))
        );

        let scalar = json!(42);
        let cards = visible_cards(&scalar, &JsonView::default(), &default_expanded(&scalar));
        let header = header_parts(&scalar, &cards[0], "n.json");
        assert_eq!((header.count.as_str(), header.count_kind), ("42", ValueKind::Number));
    }

    #[test]
    fn palettes_follow_the_theme_and_colours_round_trip() {
        assert_eq!(palette(true).key, egui::Color32::from_rgb(0x9c, 0xdc, 0xfe));
        assert_eq!(palette(false).key, egui::Color32::from_rgb(0x04, 0x51, 0xa5));
        let c = egui::Color32::from_rgb(1, 2, 3);
        assert_eq!(rgb_u32(c), 0x010203);
        assert_eq!(rgb(rgb_u32(c)), c);
    }
}
