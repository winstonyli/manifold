//! A PDF card's body and header.

use super::*;
use crate::store::model::NodeId;

impl App {
    /// Pdf: same card shape as an image node (drag, hover-revealed
    /// close button, drop-onto-group), but with three visual states
    /// instead of one -- still rendering in the background
    /// (`outcome: None`, a plain spinner, same fixed
    /// `PDF_PENDING_NODE_SIZE` box a tab respawn placeholder uses
    /// `Spinner` for too), failed (`Some(Err(_))`, a real error
    /// glyph plus the message as a hover tooltip -- unlike an
    /// image's "somehow outlived its own texture" broken-image
    /// case, this is an *expected*, real outcome worth explaining,
    /// not a rare bug), or rendered (`Some(Ok(_))`, the page's own
    /// texture in `GpuState::images` -- the exact same map an
    /// `Image` node's texture lives in, since a rendered PDF page
    /// is just another RGBA bitmap by the time it's on the GPU).
    pub(crate) fn paint_pdf_body(
        &mut self,
        ui: &mut egui::Ui,
        pass: &mut CardPass,
        id: NodeId,
        _view: &CardView,
        rect: egui::Rect,
    ) -> CardBody {
        let (inv_zoom, zoom_scale) = (pass.inv_zoom, pass.zoom_scale);
        let deferred_overlays = &mut *pass.deferred_overlays;
        let mut body = CardBody { interact_rect: rect, tooltip: None };
        let Some(state) = self.nodes.get(&id).and_then(|n| n.as_pdf()).map(|pdf| match &pdf.outcome {
            None => None,
            Some(Err(err)) => Some(Err(err.clone())),
            Some(Ok(outcome)) => Some(Ok((self.gpu.as_ref().and_then(|g| g.image_id(id)), outcome.page_count))),
        }) else {
            return body;
        };
        // A failed render's error message shown as a hover
        // tooltip on the card itself -- the only place there's
        // room for real text without permanently occupying part
        // of the card the way a label would.
        if let Some(Err(err)) = &state {
            body.tooltip = Some(err.clone());
        }
        // LOD: same "plain solid box instead of the real
        // content" simplification as `paint_image_body` -- see
        // `paint_tab_body`'s own LOD comment.
        if self.graph.detail.content() {
            match state {
                Some(Ok((Some(tex_id), page_count))) => {
                    rounded_image(ui.painter(), tex_id, rect, FULL_UV, egui::Color32::WHITE, CARD_RADIUS * inv_zoom);
                    self.paint_pdf_header(ui, deferred_overlays, id, rect, inv_zoom, zoom_scale, false);
                    // Shown even for a one-page PDF -- a glanceable
                    // "this is a PDF, not a plain image" cue on top
                    // of the file-type distinction the icon in the
                    // context menu/toolbar already gives.
                    if page_count > 0 && label_fits(rect, zoom_scale) {
                        let page_count_pos = rect.right_bottom();
                        let page_count_text = format!("{page_count} page{}", if page_count == 1 { "" } else { "s" });
                        deferred_overlays.push(Box::new(move |overlay_painter, project| {
                            overlay_painter.text(
                                project(page_count_pos) + egui::vec2(-4.0, -4.0),
                                egui::Align2::RIGHT_BOTTOM,
                                page_count_text,
                                egui::FontId::proportional(11.0),
                                egui::Color32::WHITE,
                            );
                        }));
                    }
                }
                // Reachable the same rare way an `Image` node's own
                // `None` case is -- the node outlived its texture.
                Some(Ok((None, _))) => {
                    push_centered_text(
                        deferred_overlays,
                        rect.center(),
                        egui_phosphor::light::IMAGE_BROKEN,
                        24.0,
                        ui.visuals().weak_text_color(),
                    );
                }
                Some(Err(_)) => {
                    push_centered_text(
                        deferred_overlays,
                        rect.center(),
                        egui_phosphor::light::FILE_X,
                        24.0,
                        danger(ui.visuals()),
                    );
                }
                // Still rendering: the header's icon spins, over the plain fill.
                None => self.paint_pdf_header(ui, deferred_overlays, id, rect, inv_zoom, zoom_scale, true),
            }
        }
        body
    }

    /// A PDF card's header; `busy` while the first render is pending.
    #[allow(clippy::too_many_arguments, reason = "the card header's context, as `paint_card_header` takes it")]
    fn paint_pdf_header(
        &mut self,
        ui: &mut egui::Ui,
        deferred_overlays: &mut DeferredOverlays,
        id: NodeId,
        rect: egui::Rect,
        inv_zoom: f32,
        zoom_scale: f32,
        busy: bool,
    ) {
        let Some(pdf) = self.nodes.get(&id).and_then(|n| n.as_pdf()) else { return };
        let (name, source, source_is_path) = file_header_text(&pdf.path, self.source_url(id));
        // The page count sits at the bottom right, so the fallback strip stops short of it.
        let header = CardHeader {
            identity: Identity::Glyph { glyph: kind_caps(Kind::Pdf).glyph, color: None }.busy(busy),
            name,
            user_name: None,
            source,
            source_is_path,
            scrim: !busy,
            right_margin: 60.0,
        };
        self.paint_card_header(ui, deferred_overlays, id, rect, inv_zoom, zoom_scale, header);
    }
}
