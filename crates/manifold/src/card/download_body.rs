//! A download card's body.

use super::*;

impl App {
    /// Download: no thumbnail/title-strip/audio-dot machinery (no
    /// live content behind it), just the filename, an icon, and a
    /// live progress bar while it's still downloading.
    pub(crate) fn paint_download_body(
        &mut self,
        ui: &mut egui::Ui,
        deferred_overlays: &mut DeferredOverlays,
        id: &str,
        rect: egui::Rect,
        inv_zoom: f32,
        zoom_scale: f32,
    ) -> CardBody {
        let body = CardBody { interact_rect: rect, tooltip: None };
        let Some((download_path, completed, failed, total_bytes, bytes_received)) = self
            .nodes
            .get(id)
            .and_then(|n| n.as_download())
            .map(|d| (d.path.clone(), d.completed, d.failed, d.total_bytes, d.bytes_received))
        else {
            return body;
        };
        let download_url = self.copyable_url(id).map(str::to_owned);
        let filename = download_filename(&download_path);
        // LOD: the header text is skipped once simplified -- see `paint_tab_body`'s own LOD comment.
        if !self.graph_lod_simplified {
            let header = CardHeader {
                identity: Identity::Glyph { glyph: egui_phosphor::light::DOWNLOAD_SIMPLE, color: None },
                name: filename.to_owned(),
                user_name: None,
                source_is_path: download_url.is_none(),
                source: download_url.unwrap_or_else(|| download_path.clone()),
                scrim: false,
                right_margin: 0.0,
            };
            let tall = self.paint_card_header(ui, deferred_overlays, id, rect, inv_zoom, zoom_scale, header);
            // The size (or how much has arrived) in the middle of the body below the header.
            if tall {
                let size = match (completed, total_bytes) {
                    _ if failed => "Download failed".to_owned(),
                    (true, total) => crate::human_bytes(total.unwrap_or(bytes_received)),
                    (false, Some(total)) => {
                        format!("{} of {}", crate::human_bytes(bytes_received), crate::human_bytes(total))
                    }
                    (false, None) => crate::human_bytes(bytes_received),
                };
                let middle = card_body_rect(rect, true, inv_zoom).center();
                let color = if failed { DANGER_COLOR } else { ui.visuals().weak_text_color() };
                push_centered_text(deferred_overlays, middle, size, 13.0, color);
            }
        }
        // Live progress bar, only while not yet complete
        // and only when the server reported a real total
        // (an unknown-total download still shows as a
        // download via the icon/filename above; the
        // Downloads popup's own byte-count-with-no-bar
        // fallback is where that case gets a number). A
        // plain `ui.painter()` world-space rect, same
        // coordinate space `rect`/the card's own fill and
        // border already draw in -- not `overlay_painter`
        // (screen-space, used for text specifically to
        // avoid blurring at non-100% zoom), and not scaled
        // by `inv_zoom` for the same reason `GRAPH_THUMB_
        // MARGIN` isn't: this is card-layout geometry
        // already inside the zoom-scaled Scene transform,
        // not chrome that needs a constant on-screen size.
        if !completed && !failed {
            if let Some(total) = total_bytes.filter(|t| *t > 0) {
                let fraction = (bytes_received as f32 / total as f32).clamp(0.0, 1.0);
                let track_rect = egui::Rect::from_min_max(
                    egui::pos2(rect.left() + 8.0, rect.bottom() - 26.0),
                    egui::pos2(rect.right() - 8.0, rect.bottom() - 22.0),
                );
                ui.painter().rect_filled(track_rect, 2.0, ui.visuals().weak_text_color().gamma_multiply(0.4));
                let fill_rect = egui::Rect::from_min_max(
                    track_rect.min,
                    egui::pos2(track_rect.min.x + track_rect.width() * fraction, track_rect.max.y),
                );
                ui.painter().rect_filled(fill_rect, 2.0, ACCENT);
            }
        }
        body
    }
}
