//! An image card's body, and the card a file kind shows when its file is missing.

use super::*;
use crate::store::model::NodeId;

impl App {
    /// A restored image/JSON card whose file couldn't be loaded: header F (a `FILE_X` glyph in the
    /// danger colour, the file name, its path) with the same glyph a failed PDF card shows in the
    /// middle of the body below it (see `ImagePayload::error`).
    #[allow(
        clippy::too_many_arguments,
        reason = "the card's id, rect and zoom travel beside the header, as at every sibling painter"
    )]
    pub(crate) fn paint_missing_file_card(
        &mut self,
        ui: &egui::Ui,
        deferred_overlays: &mut DeferredOverlays,
        id: NodeId,
        rect: egui::Rect,
        inv_zoom: f32,
        zoom_scale: f32,
        (name, source, source_is_path): (String, String, bool),
    ) {
        if !self.graph.detail.content() {
            return;
        }
        let header = CardHeader {
            identity: Identity::Glyph { glyph: egui_phosphor::light::FILE_X, color: Some(danger(ui.visuals())) },
            name,
            user_name: None,
            source,
            source_is_path,
            scrim: false,
            right_margin: 0.0,
        };
        let tall = self.paint_card_header(ui, deferred_overlays, id, rect, inv_zoom, zoom_scale, header);
        let icon_pos = card_body_rect(rect, tall, inv_zoom).center();
        push_centered_text(deferred_overlays, icon_pos, egui_phosphor::light::FILE_X, 24.0, danger(ui.visuals()));
    }

    /// Image: a locally-added picture, no engine/IPC behind it --
    /// see `ImagePayload`'s own doc comment. Unlike a note, nothing
    /// inside the card needs its own clicks (no embedded text
    /// editor), so the whole card is drag-sensitive via one plain
    /// `Sense::click_and_drag()`, same as a tab or download card.
    /// The card's own footprint comes from `image_node_card_size`
    /// (the decoded picture's real aspect ratio, capped at
    /// `IMAGE_NODE_MAX_SIZE`), not a fixed size the way every other
    /// kind's card is.
    pub(crate) fn paint_image_body(
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
        let error = self.nodes.get(&id).and_then(|n| n.as_image()).and_then(|i| i.error.clone());
        if let Some(error) = error {
            if let Some(path) = self.nodes.get(&id).and_then(|n| n.as_image()).map(|i| i.path.clone()) {
                let text = file_header_text(&path, self.source_url(id));
                self.paint_missing_file_card(ui, deferred_overlays, id, rect, inv_zoom, zoom_scale, text);
            }
            body.tooltip = Some(error);
            return body;
        }
        let tex_id = self.gpu.as_ref().and_then(|g| g.image_id(id));
        // LOD: the real texture blit (and the broken-image
        // glyph text) skipped once simplified -- see
        // `paint_tab_body`'s own LOD comment. Unlike a tab card, an image
        // node has no separate background fill of its own (the
        // picture *is* the fill). The simplified state shows the
        // frame's plain fill, matching every other kind's
        // "background + border" simplified look.
        if self.graph.detail.content() {
            // Still decoding (`ImageDecoder`): the header's icon spins, over the plain fill.
            let decoding =
                tex_id.is_none() && self.nodes.get(&id).and_then(|n| n.as_image()).is_some_and(|i| i.decoding());
            match tex_id {
                Some(tex_id) => {
                    rounded_image(ui.painter(), tex_id, rect, FULL_UV, egui::Color32::WHITE, CARD_RADIUS * inv_zoom);
                }
                None if decoding => {}
                // Only reachable if the node somehow outlived its own texture (freed but not removed): a
                // broken-image glyph, since this picture isn't about to arrive.
                None => {
                    push_centered_text(
                        deferred_overlays,
                        rect.center(),
                        egui_phosphor::light::IMAGE_BROKEN,
                        24.0,
                        ui.visuals().weak_text_color(),
                    );
                }
            }
            if tex_id.is_some() || decoding {
                if let Some(image) = self.nodes.get(&id).and_then(|n| n.as_image()) {
                    let (name, source, source_is_path) = file_header_text(&image.path, self.source_url(id));
                    let header = CardHeader {
                        identity: Identity::Glyph { glyph: kind_caps(Kind::Image).glyph, color: None }.busy(decoding),
                        name,
                        user_name: None,
                        source,
                        source_is_path,
                        scrim: !decoding,
                        right_margin: 0.0,
                    };
                    self.paint_card_header(ui, deferred_overlays, id, rect, inv_zoom, zoom_scale, header);
                }
            }
        }
        body
    }
}
