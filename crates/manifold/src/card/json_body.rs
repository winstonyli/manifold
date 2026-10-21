//! A JSON card's body.

use super::*;

impl App {
    /// Json: an error card keeps the missing-file box (fixed at
    /// `JSON_NODE_SIZE`). A node with a file instead previews its root --
    /// the file name, `{n}`/`[n]`, up to `PREVIEW_ROWS` root rows and
    /// `+ N more` -- sized by `json_view::preview_size` and painted by the
    /// JSON level's own `paint_json_content`, so the two never disagree
    /// about what a root looks like. Double-click enters the level
    /// (`current_group`/`sync_json_level`).
    pub(crate) fn paint_json_body(
        &mut self,
        ui: &mut egui::Ui,
        pass: &mut CardPass,
        id: &str,
        _view: &CardView,
        rect: egui::Rect,
    ) -> CardBody {
        let zoom_scale = pass.zoom_scale;
        let deferred_overlays = &mut *pass.deferred_overlays;
        let mut body = CardBody { interact_rect: rect, tooltip: None };
        let error = self.nodes.get(id).and_then(|n| n.as_json()).and_then(|j| j.error.clone());
        if let Some(error) = error {
            if let Some(path) = self.nodes.get(id).and_then(|n| n.as_json()).map(|j| j.path.clone()) {
                let text = file_header_text(&path, None);
                self.paint_missing_file_card(ui, deferred_overlays, id, rect, 1.0 / zoom_scale, zoom_scale, text);
            }
            body.tooltip = Some(error);
            return body;
        }
        let Some(json) = self.nodes.get(id).and_then(|n| n.as_json()) else { return body };
        let text = !self.graph_lod_simplified && label_fits(rect, zoom_scale);
        let len = crate::json_view::entry_count(&json.value);
        // The header is 40 screen points, so at other zooms the rows get more or less of the card:
        // show only the rows that fit, with a `+ N more` row when some are cut. A card too short for
        // the header (`tab_has_header`) keeps the level cards' compact one.
        let inv_zoom = 1.0 / zoom_scale;
        let preview = tab_has_header(rect, inv_zoom);
        let header_height = if preview { TAB_HEADER_POINTS * inv_zoom } else { crate::json_view::HEADER_HEIGHT };
        let fit = ((rect.height() - header_height) / crate::json_view::ROW_HEIGHT).floor().max(0.0) as usize;
        let mut rows = len.min(crate::json_view::PREVIEW_ROWS).min(fit);
        if rows < len {
            rows = rows.min(fit.saturating_sub(1));
        }
        let card = crate::json_view::Card {
            pointer: String::new(),
            parent: None,
            depth: 0,
            collapsed: false,
            skip: 0,
            rows,
            more: len - rows,
        };
        let mut header = crate::json_level_ui::header_parts(&json.value, &card, download_filename(&json.path));
        if preview && text {
            // The folder, cut at the start so its last part stays visible, as a download's path is.
            let folder = file_header_text(&json.path, None).1;
            let folder = folder.rsplit_once(['\\', '/']).map(|(dir, _)| dir.to_owned());
            // `paint_json_content`'s text width: the source starts at `HEADER_TEXT_X`.
            let width = (rect.width() - crate::json_level_ui::COUNT_WIDTH - crate::json_level_ui::PAD) * zoom_scale
                - HEADER_TEXT_X;
            header.source = folder.map(|dir| {
                let font = egui::FontId::proportional(11.0);
                truncate_start_to_width(&mut self.truncate_cache, ui.ctx(), &dir, &font, width.max(0.0))
            });
        }
        let pal = crate::json_level_ui::palette(ui.visuals().dark_mode);
        let look = crate::json_level_ui::CardLook {
            palette: &pal,
            text_color: ui.visuals().text_color(),
            inv_zoom,
            text,
            selected_row: None,
            hit_rows: &[],
            visible: egui::Rect::EVERYTHING,
            // No child cards or edges at the parent level: a port dot here
            // would point at nothing.
            ports: false,
            preview,
        };
        crate::json_level_ui::paint_json_content(
            ui.painter(),
            deferred_overlays,
            rect,
            &json.value,
            &header,
            0,
            rows,
            len - rows,
            &look,
        );
        body
    }
}
