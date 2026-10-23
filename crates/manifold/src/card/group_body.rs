//! A group card's body.

use super::*;
use crate::store::model::NodeId;

impl App {
    /// Header F (folder glyph, name, item count) over a member thumbnail grid (header only when
    /// empty or with "Collapse Groups" on). LOD: skipped once simplified, leaving the frame's
    /// plain card.
    pub(crate) fn paint_group_body(
        &mut self,
        ui: &mut egui::Ui,
        pass: &mut CardPass,
        id: NodeId,
        view: &CardView,
        rect: egui::Rect,
    ) -> CardBody {
        let (inv_zoom, zoom_scale) = (pass.inv_zoom, pass.zoom_scale);
        let deferred_overlays = &mut *pass.deferred_overlays;
        let mut body = CardBody { interact_rect: rect, tooltip: None };
        let member_ids = &view.members;
        let member_count = member_ids.len();
        if self.graph_detail.content() {
            // Owned, so the header can borrow it while `paint_card_header` borrows `self`.
            // name rule: drawn by display_name just below
            if let Some(stored) = self.is_container(id).then(|| self.doc.mirror.name(id).to_owned()) {
                let segs = crate::names::display_name(&stored, crate::names::UNTITLED);
                body.tooltip = crate::names::hidden_hint(&segs);
                let header = CardHeader {
                    identity: Identity::Glyph { glyph: egui_phosphor::light::FOLDER, color: None },
                    name: String::new(),
                    user_name: Some(&segs),
                    source: format!("{member_count} item{}", if member_count == 1 { "" } else { "s" }),
                    source_is_path: false,
                    scrim: false,
                    right_margin: 0.0,
                };
                let tall = self.paint_card_header(ui, deferred_overlays, id, rect, inv_zoom, zoom_scale, header);
                // Up to 4 member thumbnails, a 2x2 grid in the body below the header, like a folder
                // previewing its contents (`GpuState::thumbnail_id`, as a tab's own card does). A cell
                // with no thumbnail yet gets a plain tinted square. "Collapse Groups" leaves the
                // header alone.
                if member_count > 0 && !self.groups_collapsed {
                    let (top, bottom) =
                        if tall { (TAB_HEADER_POINTS * inv_zoom, 0.0) } else { (0.0, TAB_STRIP_POINTS * inv_zoom) };
                    let grid_rect =
                        egui::Rect::from_min_max(rect.min + egui::vec2(0.0, top), rect.max - egui::vec2(0.0, bottom))
                            .shrink(GRAPH_THUMB_MARGIN);
                    let gap = 3.0 * inv_zoom;
                    let cell_size = egui::vec2((grid_rect.width() - gap) / 2.0, (grid_rect.height() - gap) / 2.0);
                    for (i, member_id) in member_ids.iter().take(4).enumerate() {
                        let (col, row) = (i as f32 % 2.0, (i / 2) as f32);
                        let cell_min = grid_rect.min + egui::vec2(col * (cell_size.x + gap), row * (cell_size.y + gap));
                        let cell_rect = egui::Rect::from_min_size(cell_min, cell_size);
                        match self.gpu.as_ref().and_then(|g| g.thumbnail_id(*member_id)) {
                            Some(tex_id) => {
                                let radius = 2.0 * inv_zoom;
                                rounded_image(ui.painter(), tex_id, cell_rect, FULL_UV, egui::Color32::WHITE, radius);
                            }
                            None => {
                                ui.painter().rect_filled(cell_rect, 2.0 * inv_zoom, palette(ui.visuals()).canvas);
                            }
                        }
                    }
                    if member_count > 4 && label_fits(rect, zoom_scale) {
                        let overflow_pos = grid_rect.right_bottom();
                        let overflow_text = format!("+{}", member_count - 4);
                        deferred_overlays.push(Box::new(move |overlay_painter, project| {
                            overlay_painter.text(
                                project(overflow_pos) + egui::vec2(-3.0, -3.0),
                                egui::Align2::RIGHT_BOTTOM,
                                overflow_text,
                                egui::FontId::proportional(12.0),
                                egui::Color32::WHITE,
                            );
                        }));
                    }
                }
            }
        }
        body
    }
}
