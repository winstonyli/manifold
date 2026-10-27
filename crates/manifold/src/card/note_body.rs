//! A note card's body.

use super::*;
use crate::store::model::NodeId;

impl App {
    /// Note: a plain user-authored text card, no engine process
    /// behind it -- see `NotePayload`'s own doc comment. Unlike
    /// every other kind, most of the card is a real editable
    /// `TextEdit`, not overlay-painted static text -- editing
    /// happens directly in place, on the graph. The header strip
    /// (not the text body) is the drag handle only while editing,
    /// via `CardBody::interact_rect`, so a click inside the text
    /// placing a cursor/selecting text is never mistaken for the
    /// start of a card drag; the frame's own hover-only interact
    /// covers the whole card regardless, so hovering the text body
    /// still counts even though it isn't the drag handle.
    pub(crate) fn paint_note_body(
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
        let tall = tab_has_header(rect, inv_zoom);
        let paper = tab_thumb_rect(rect, inv_zoom);
        let text_rect = paper.shrink(6.0 * inv_zoom);
        // While editing, only the header is the drag handle (a short card has the strip instead).
        let handle = if tall {
            egui::Rect::from_min_max(rect.min, egui::pos2(rect.max.x, paper.min.y))
        } else {
            egui::Rect::from_min_max(egui::pos2(rect.min.x, rect.max.y - TAB_STRIP_POINTS * inv_zoom), rect.max)
        };
        let is_editing = self.nodes.get(&id).and_then(|n| n.as_note()).is_some_and(|n| n.editing);

        ui.painter().rect_filled(paper, CARD_RADIUS * inv_zoom, NOTE_CARD_COLOR);
        if self.graph.detail.content() {
            let words = self.note_text(id).split_whitespace().count();
            let header = CardHeader {
                identity: Identity::Glyph { glyph: kind_caps(Kind::Note).glyph, color: None },
                name: "Note".to_owned(),
                user_name: None,
                source: format!("{words} word{}", if words == 1 { "" } else { "s" }),
                source_is_path: false,
                scrim: false,
                right_margin: 0.0,
            };
            self.paint_card_header(ui, deferred_overlays, id, rect, inv_zoom, zoom_scale, header);
        }

        if is_editing {
            body.interact_rect = handle;
            let mut lost_focus = false;
            if let Some(note) = self.nodes.get_mut(&id).and_then(|n| n.as_note_mut()) {
                let edit = ui.put(
                    text_rect,
                    egui::TextEdit::multiline(&mut note.text)
                        .id(CardId::from(id).editor())
                        .frame(egui::Frame::NONE)
                        .font(egui::FontId::proportional(13.0))
                        .text_color(NOTE_TEXT)
                        .hint_text("Type a note..."),
                );
                if self.interaction.focus_new_note.as_ref().is_some_and(|(n, _)| *n == CardId::from(id)) {
                    self.interaction.focus_new_note = None;
                    edit.request_focus();
                }
                lost_focus = edit.lost_focus();
            }
            // The box edits the payload, the edit buffer; the mirror gets it on blur (and at each save), not on
            // every keystroke -- a note can reach 1 MB, and `save_session` is real file I/O.
            if lost_focus {
                // Escape ends the edit; focus goes back to the card so keyboard users keep their place
                // (a click elsewhere keeps the focus it took).
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    ui.memory_mut(|m| m.request_focus(CardId::from(id).widget()));
                }
                self.blur_note(id);
            }
        } else if self.graph.detail.content() {
            // LOD: skip the markdown render (real parsing +
            // rich text layout, one of the pricier per-node
            // widgets in this whole view) once simplified --
            // see `paint_tab_body`'s own LOD comment. Never
            // applies to the `is_editing` branch above: a
            // note actively being typed into must never have
            // its live `TextEdit` yanked out from under the
            // user just because the rest of the graph got
            // busy.
            // Borrowed, not cloned: `text` borrows `self.doc.mirror` and `cache` `self.commonmark_cache`,
            // two disjoint fields. Not being typed in, the note shows the mirror's text (`App::note_text`).
            let text = self.doc.mirror.text(id);
            let cache = &mut self.commonmark_cache;
            ui.scope_builder(
                egui::UiBuilder::new().max_rect(text_rect).layout(egui::Layout::top_down(egui::Align::LEFT)),
                |ui| {
                    ui.style_mut().visuals.override_text_color = Some(NOTE_TEXT);
                    egui_commonmark::CommonMarkViewer::new().show(ui, cache, text);
                },
            );
        }
        body
    }
}
