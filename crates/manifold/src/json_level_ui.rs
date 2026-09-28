//! A JSON level's painter and input (spec "Cards"): record cards drawn
//! in the Scene, their text queued through `deferred_overlays` (plan
//! ruling 11). The bar, the menu, keys (Task 7) and the outline (Task 8)
//! live here too.

use serde_json::Value;

use super::*;
use crate::json_level::JsonEdit;
use crate::json_view::{self, Card};

/// Text height in world units (plan ruling 11).
const TEXT_SIZE: f32 = 12.0;
/// Inset of a card's text from its edges.
const PAD: f32 = 8.0;
/// An object row's key column.
const KEY_WIDTH: f32 = 84.0;
/// An array row's index column, right-aligned.
const INDEX_WIDTH: f32 = 22.0;
/// A container row's `{n}` / `[n]` column.
const COUNT_WIDTH: f32 = 44.0;
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
// Called once Task 7's context menu stores a picked colour.
#[allow(dead_code)]
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
}

/// Queues a card's text as one overlay. The scale comes from `project`
/// at paint time, so the text never lags the card by a frame.
fn push_card_text(deferred_overlays: &mut DeferredOverlays, items: Vec<TextItem>) {
    if items.is_empty() {
        return;
    }
    deferred_overlays.push(Box::new(move |painter, project| {
        let scale = project(egui::pos2(1.0, 0.0)).x - project(egui::Pos2::ZERO).x;
        let font = egui::FontId::proportional(TEXT_SIZE * scale);
        for item in items {
            let mut job = egui::text::LayoutJob::simple_singleline(item.text, font.clone(), item.color);
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
    pub(crate) hit_rows: &'a [usize],
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

/// A card's contents (the frame is the caller's): the header, then the
/// first `rows` entries of `value`, then `+ N more` when `more > 0`.
/// Shared by the JSON level and the Json card at its parent's level.
#[allow(clippy::too_many_arguments)] // one card's full description; a struct for it would only rename these
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
    use json_view::{HEADER_HEIGHT, ROW_HEIGHT};
    let pal = look.palette;
    let row_rect = |row: usize| {
        egui::Rect::from_min_size(
            rect.min + egui::vec2(0.0, HEADER_HEIGHT + ROW_HEIGHT * row as f32),
            egui::vec2(rect.width(), ROW_HEIGHT),
        )
    };
    if rows + usize::from(more > 0) > 0 {
        painter.hline(
            rect.x_range(),
            rect.top() + HEADER_HEIGHT,
            egui::Stroke::new(look.inv_zoom, pal.muted.gamma_multiply(0.4)),
        );
    }
    for &row in look.hit_rows {
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
        let y = rect.top() + HEADER_HEIGHT / 2.0;
        let count_color = if header.count_kind == ValueKind::Container { pal.muted } else { pal.of(header.count_kind) };
        text.push(TextItem {
            pos: egui::pos2(left, y),
            align: egui::Align2::LEFT_CENTER,
            text: header.icon.into(),
            color: pal.container,
            max_width: 16.0,
        });
        let title_width = if header.label.is_some() { 88.0 } else { inner - 18.0 - COUNT_WIDTH };
        text.push(TextItem {
            pos: egui::pos2(left + 18.0, y),
            align: egui::Align2::LEFT_CENTER,
            text: header.title.clone(),
            color: look.text_color,
            max_width: title_width,
        });
        if let Some(label) = &header.label {
            text.push(TextItem {
                pos: egui::pos2(left + 110.0, y),
                align: egui::Align2::LEFT_CENTER,
                text: label.clone(),
                color: pal.muted,
                max_width: inner - 110.0 - COUNT_WIDTH,
            });
        }
        text.push(TextItem {
            pos: egui::pos2(right, y),
            align: egui::Align2::RIGHT_CENTER,
            text: header.count.clone(),
            color: count_color,
            max_width: COUNT_WIDTH,
        });
    }
    for (row, (key, child)) in json_view::entries(value).take(rows).enumerate() {
        let r = row_rect(row);
        let y = r.center().y;
        let parts = row_parts(value, &key, child);
        if parts.port {
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
            });
        }
        text.push(TextItem {
            pos: egui::pos2(right, y),
            align: egui::Align2::RIGHT_CENTER,
            text: parts.value,
            color: pal.of(parts.kind),
            max_width: value_width,
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
            });
        } else {
            painter.rect_filled(r.shrink2(egui::vec2(PAD, 7.0)), 2.0, pal.muted.gamma_multiply(0.2));
        }
    }
    push_card_text(deferred_overlays, text);
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
        let pal = palette(ui.visuals().dark_mode);
        let base_fill = ui.visuals().extreme_bg_color;
        let idle_border = ui.visuals().weak_text_color();
        let text_color = ui.visuals().text_color();
        let hits = level.search.result.as_ref().map(|r| &r.hits);
        let mut moves = Vec::new();
        for (i, card) in level.cards.iter().enumerate() {
            let rect = level.rects[i];
            if !rect.intersects(self.camera.rect) {
                continue;
            }
            let Some(value) = level.value.pointer(&card.pointer) else { continue };
            let shown: Vec<String> = json_view::entries(value)
                .take(card.rows)
                .map(|(key, _)| json_view::child_pointer(&card.pointer, &key))
                .collect();
            let selected_row = level.selected.as_ref().and_then(|s| shown.iter().position(|p| p == s));
            let hit_rows: Vec<usize> = match hits {
                Some(hits) => shown.iter().enumerate().filter(|(_, p)| hits.contains(*p)).map(|(r, _)| r).collect(),
                None => Vec::new(),
            };
            let border = if level.selected.as_deref() == Some(card.pointer.as_str()) {
                (2.0, ACCENT)
            } else if hits.is_some_and(|h| h.contains(&card.pointer)) || !hit_rows.is_empty() {
                (2.0, SEARCH_HIT)
            } else {
                (1.0, idle_border)
            };
            let fill = if value.is_array() { base_fill.lerp_to_gamma(ARRAY_TINT, 0.12) } else { base_fill };
            let tint = json.view.get(&card.pointer).and_then(|o| o.color).map(rgb);
            paint_json_frame(ui.painter(), rect, fill, tint, border, inv_zoom);
            let look = CardLook {
                palette: &pal,
                text_color,
                inv_zoom,
                text: zoom_scale >= LOD_TEXT_ZOOM,
                selected_row,
                hit_rows: &hit_rows,
            };
            let header = header_parts(&level.value, card, &root_name);
            paint_json_content(ui.painter(), deferred_overlays, rect, value, &header, card.rows, card.more, &look);

            let response = ui.interact(
                rect,
                egui::Id::new(("json_card", &level.node_id, &card.pointer)),
                egui::Sense::click_and_drag(),
            );
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
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::json_view::{default_expanded, visible_cards, JsonView};

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
