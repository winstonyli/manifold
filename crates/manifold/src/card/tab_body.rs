//! A tab card's body: the unloaded placeholder, the thumbnail view, and the header and title they share.

use super::*;
use crate::store::model::NodeId;

/// A card title, left-centred at `pos`: the styled `rich` job when there is one, else the plain `title`.
fn paint_card_title(
    painter: &egui::Painter,
    pos: egui::Pos2,
    rich: Option<egui::text::LayoutJob>,
    title: String,
    font: egui::FontId,
    color: egui::Color32,
) {
    match rich {
        Some(job) => {
            let galley = painter.layout_job(job);
            let y = pos.y - galley.size().y / 2.0;
            painter.galley(egui::pos2(pos.x, y), galley, color);
        }
        None => {
            painter.text(pos, egui::Align2::LEFT_CENTER, title, font, color);
        }
    }
}

impl App {
    /// Unloaded (see `UnloadedTabPayload`), by state: **Sleeping** draws the
    /// title (or url) and host dimmed, no icon; **Waking** adds the spinner
    /// over the same dimmed labels; **Failed** draws a
    /// warning icon, the url at the bottom, and which engine failed and why
    /// as the tooltip. LOD: skipped once simplified, as for
    /// `paint_download_body`.
    pub(crate) fn paint_unloaded_tab_body(
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
        let Some(tab) = self.nodes.get(&id).and_then(|n| n.as_unloaded()) else {
            return body;
        };
        let (url, title, state) = (tab.url.clone(), tab.title.clone(), tab.state.clone());
        let kind = self.doc.mirror.view_kind(id);
        // A failed card whose engine can't run offers what a click on it does: its Download, or why it is unavailable.
        let missing_view_kind = matches!(state, UnloadedState::Failed(_))
            && !matches!(pick_outcome(kind, &self.view_status(kind)), PickOutcome::Switch(_));
        body.tooltip = Some(match &state {
            UnloadedState::Failed(_) if missing_view_kind => match self.view_status(kind) {
                crate::view_install::ViewStatus::Unavailable(reason) => PickOutcome::unavailable_toast(kind, &reason),
                state => format!(
                    "{} {}\nClick to download it ({}).",
                    view_label(kind),
                    missing_view_phrases(&state).0,
                    crate::view_info::megabytes(self.view_download_size(kind))
                ),
            },
            UnloadedState::Failed(error) => {
                format!("Couldn't start {}: {error}\nClick to try again.", view_label(kind))
            }
            UnloadedState::Sleeping | UnloadedState::Waking => sleeping_or_waking_tooltip(&state).to_string(),
        });
        let chip = self.jar_chip(id);
        body.tooltip = chain_tooltip(body.tooltip, chip.as_ref().and_then(|c| c.tooltip.clone()));
        if !self.graph.detail.content() {
            return body;
        }
        // Favicons aren't kept across restarts and no engine loads a sleeping tab's url, so the
        // card asks for its own. Only on-screen cards drawn in full detail get here (culled and
        // LOD-simplified ones don't), so a big restored session fetches for what's in view, once
        // per origin.
        self.request_favicons([url.as_str()]);
        let favicon = self.favicon_texture(&url);
        let failed = matches!(state, UnloadedState::Failed(_));
        let header = CardHeader {
            identity: match (failed, favicon) {
                (false, Some(tex)) => Identity::Image(tex),
                (true, _) => {
                    Identity::Glyph { glyph: egui_phosphor::light::WARNING, color: Some(danger(ui.visuals())) }
                }
                (false, None) => Identity::Glyph { glyph: egui_phosphor::light::MOON, color: None },
            }
            .busy(matches!(state, UnloadedState::Waking)),
            name: if title.is_empty() { url.clone() } else { title },
            user_name: None,
            source: url,
            source_is_path: false,
            scrim: false,
            right_margin: 0.0,
        };
        let tall = self.paint_card_header(ui, deferred_overlays, id, rect, inv_zoom, zoom_scale, header);
        // The middle of the body below the header: the failure icon, the waking spinner, or the
        // moon of a sleeping tab.
        let middle = card_body_rect(rect, tall, inv_zoom).center();
        match &state {
            UnloadedState::Failed(_) if missing_view_kind => {
                let up = egui::vec2(0.0, 8.0 * inv_zoom);
                push_centered_text(
                    deferred_overlays,
                    middle - up,
                    egui_phosphor::light::WARNING_CIRCLE,
                    24.0,
                    danger(ui.visuals()),
                );
                let text = missing_view_text(kind, &self.view_status(kind));
                push_centered_text(deferred_overlays, middle + up * 2.5, text, 11.0, danger(ui.visuals()));
            }
            UnloadedState::Failed(_) => {
                push_centered_text(
                    deferred_overlays,
                    middle,
                    egui_phosphor::light::WARNING_CIRCLE,
                    24.0,
                    danger(ui.visuals()),
                );
            }
            UnloadedState::Waking => {}
            UnloadedState::Sleeping => {
                push_centered_text(
                    deferred_overlays,
                    middle,
                    egui_phosphor::light::MOON,
                    24.0,
                    ui.visuals().weak_text_color(),
                );
            }
        }
        // The jar chip just below the title: under the header, or at the top of a short card (whose title is on
        // the bottom strip).
        if let Some(chip) = chip.filter(|_| label_fits(rect, zoom_scale)) {
            let body_rect = card_body_rect(rect, tall, inv_zoom);
            let max_width = body_rect.width() * zoom_scale - 8.0;
            let warn = ui.visuals().warn_fg_color;
            push_jar_chip(deferred_overlays, id, body_rect.left_top(), egui::vec2(4.0, 4.0), max_width, chip, warn);
        }
        body
    }

    /// A card's header, drawn over the top of its rect: glyph (or favicon), name, and the source
    /// line, 40 screen points tall. A card under `TAB_HEADER_MIN_CARD_POINTS` tall gets the name on
    /// a strip along the bottom instead. Returns whether the full header was drawn, so a caller
    /// can place its own content below it.
    #[allow(
        clippy::too_many_arguments,
        reason = "the card's id, rect and zoom travel beside the header, as at every sibling painter"
    )]
    pub(crate) fn paint_card_header(
        &mut self,
        ui: &egui::Ui,
        deferred_overlays: &mut DeferredOverlays,
        id: NodeId,
        rect: egui::Rect,
        inv_zoom: f32,
        zoom_scale: f32,
        header: CardHeader<'_>,
    ) -> bool {
        let tall = tab_has_header(rect, inv_zoom);
        if !label_fits(rect, zoom_scale) {
            return tall;
        }
        let CardHeader { identity, name, user_name, source, source_is_path, scrim, right_margin } = header;
        // A stored `name` (any kind's rename) takes the title's place, drawn by the display rule like a group's.
        let stored = self.doc.mirror.name(id).to_owned(); // name rule: drawn through display_name
        let own_segs = crate::names::display_name(&stored, crate::names::UNTITLED);
        let user_name = user_name.or((!stored.is_empty()).then_some(own_segs.as_slice()));
        let band_height = if tall { TAB_HEADER_POINTS } else { TAB_STRIP_POINTS } * inv_zoom;
        let band = if tall {
            egui::Rect::from_min_size(rect.left_top(), egui::vec2(rect.width(), band_height))
        } else {
            egui::Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - band_height), rect.right_bottom())
        };
        if scrim || !tall {
            let radius = (CARD_RADIUS * inv_zoom).min(255.0) as u8;
            let corners = if tall {
                egui::CornerRadius { nw: radius, ne: radius, sw: 0, se: 0 }
            } else {
                egui::CornerRadius { nw: 0, ne: 0, sw: radius, se: radius }
            };
            ui.painter().rect_filled(band, corners, if tall { IMAGE_TEXT_SCRIM } else { TITLE_STRIP_SCRIM });
        }
        let light = scrim || !tall;
        let title_color = if light { crate::theme::SCRIM_TEXT } else { ui.visuals().text_color() };
        let source_color = if light { crate::theme::SCRIM_TEXT_WEAK } else { ui.visuals().weak_text_color() };
        // One size for measuring and drawing the title, so a strip's plain and rich (hidden-character) titles match.
        let title_size = if tall { HEADER_TITLE_SIZE } else { 12.0 };
        let title_font = egui::FontId::proportional(title_size);
        let source_font = egui::FontId::proportional(HEADER_SOURCE_SIZE);
        let text_left = if tall { HEADER_TEXT_X } else { 8.0 };
        let max_width = (rect.width() * zoom_scale - text_left - HEADER_RIGHT_RESERVE - right_margin).max(0.0);
        // A stored group name with a hidden character is a styled job (placeholders in the marker style, truncated
        // by the layout); any other title is cut text, as before (`truncate_to_width`'s cache).
        let rich = user_name.filter(|segs| crate::names::has_hidden(segs)).map(|segs| {
            let mut job = egui::text::LayoutJob::default();
            let format = egui::TextFormat::simple(title_font.clone(), title_color);
            crate::names::append_name(&mut job, segs, &format, source_color);
            job.wrap = egui::text::TextWrapping::truncate_at_width(max_width);
            job
        });
        let title = match (&rich, user_name) {
            (Some(_), _) => String::new(),
            // A clean name is one borrowed segment: cut it without copying (spaces collapsed need `to_plain`).
            (None, Some([crate::names::NameSeg::Plain(p)])) => {
                truncate_to_width(&mut self.graph.truncate_cache, ui.ctx(), p, &title_font, max_width)
            }
            (None, Some(segs)) => {
                let plain = crate::names::to_plain(segs);
                truncate_to_width(&mut self.graph.truncate_cache, ui.ctx(), &plain, &title_font, max_width)
            }
            (None, None) => truncate_to_width(&mut self.graph.truncate_cache, ui.ctx(), &name, &title_font, max_width),
        };
        let source = match (tall, source_is_path) {
            (false, _) => String::new(),
            (true, true) => {
                truncate_start_to_width(&mut self.graph.truncate_cache, ui.ctx(), &source, &source_font, max_width)
            }
            (true, false) => {
                truncate_to_width(&mut self.graph.truncate_cache, ui.ctx(), &source, &source_font, max_width)
            }
        };
        let icon_text = if tall {
            let center = band.left_top() + egui::vec2(HEADER_ICON_X, HEADER_CENTER_Y) * inv_zoom;
            identity.paint(ui, center, HEADER_ICON_SIZE, inv_zoom, source_color)
        } else {
            None
        };
        let origin = band.left_top();
        let band_screen_height = band_height * zoom_scale;
        deferred_overlays.push(Box::new(move |overlay_painter, project| {
            let base = project(origin);
            if tall {
                if let Some(text) = icon_text {
                    text.draw(overlay_painter, project(text.center));
                }
                paint_card_title(
                    overlay_painter,
                    base + egui::vec2(text_left, HEADER_TITLE_Y),
                    rich,
                    title,
                    title_font,
                    title_color,
                );
                overlay_painter.text(
                    base + egui::vec2(text_left, HEADER_SOURCE_Y),
                    egui::Align2::LEFT_CENTER,
                    source,
                    source_font,
                    source_color,
                );
            } else {
                let pos = base + egui::vec2(text_left, band_screen_height / 2.0);
                paint_card_title(overlay_painter, pos, rich, title, title_font, title_color);
            }
        }));
        tall
    }

    /// A live thumbnail (or a spinner until the first frame) under a header of favicon, title and
    /// source URL; a card too short for the header (`tab_has_header`) gets the old title strip
    /// over the thumbnail, with the engine's color dot and the favicon.
    pub(crate) fn paint_tab_body(
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
        // Busy while the page loads, and until the loaded page has settled into a frame (`App::tab_visible`).
        let Some((title, url, tab_view_kind, unresponsive, busy)) =
            self.nodes.get(&id).and_then(|n| n.as_tab()).map(|t| {
                (
                    t.title.clone(),
                    t.url.clone(),
                    t.tab.kind,
                    t.tab.is_unresponsive(std::time::Instant::now()),
                    t.loading || !t.tab.first_frame.framed(),
                )
            })
        else {
            return body;
        };
        let chip = self.jar_chip(id);
        body.tooltip = chip.as_ref().and_then(|c| c.tooltip.clone());
        // LOD: the thumbnail texture blit and the title/
        // favicon text below are this card's two genuinely
        // expensive parts (a real GPU texture sample, and
        // glyph rasterization queued on `overlay_painter`) --
        // skipped once `GraphUi::detail` is `Simple` (see its
        // own doc comment), leaving the frame's plain card
        // and controls as the simplified card's whole
        // appearance.
        if self.graph.detail.content() {
            let header = tab_has_header(rect, inv_zoom);
            let thumb_rect = tab_thumb_rect(rect, inv_zoom);
            let thumb_id = self.gpu.as_ref().and_then(|g| g.thumbnail_id(id));
            if let Some(tex_id) = thumb_id {
                let uv = self
                    .gpu
                    .as_ref()
                    .and_then(|g| g.thumbnail_size(id))
                    .map_or(FULL_UV, |size| cover_uv(size, thumb_rect.size()));
                // The bottom corners follow the card's; the top ones too unless the header sits above.
                let (top, bottom) = (if header { 0.0 } else { inset_radius(inv_zoom) }, inset_radius(inv_zoom));
                let radii = egui::CornerRadius {
                    nw: top.round().min(255.0) as u8,
                    ne: top.round().min(255.0) as u8,
                    sw: bottom.round().min(255.0) as u8,
                    se: bottom.round().min(255.0) as u8,
                };
                rounded_image(ui.painter(), tex_id, thumb_rect, uv, egui::Color32::WHITE, radii);
            }

            let label = if title.is_empty() { url.as_str() } else { title.as_str() };
            let identity = self.tab_identity(&url, busy, tab_view_kind);
            let show_text = label_fits(rect, zoom_scale);
            if header {
                let card_header = CardHeader {
                    identity,
                    name: label.to_owned(),
                    user_name: None,
                    source: url.clone(),
                    source_is_path: false,
                    scrim: false,
                    right_margin: 0.0,
                };
                self.paint_card_header(ui, deferred_overlays, id, rect, inv_zoom, zoom_scale, card_header);
            } else {
                // A flat scrim behind the title strip so it stays legible over a light thumbnail.
                let title_strip = egui::Rect::from_min_max(
                    egui::Pos2::new(thumb_rect.left(), thumb_rect.bottom() - TAB_STRIP_POINTS * inv_zoom),
                    thumb_rect.right_bottom(),
                );
                // Its bottom corners follow the thumbnail's.
                ui.painter().rect_filled(
                    title_strip,
                    egui::CornerRadius {
                        sw: inset_radius(inv_zoom).round().min(255.0) as u8,
                        se: inset_radius(inv_zoom).round().min(255.0) as u8,
                        ..egui::CornerRadius::ZERO
                    },
                    TITLE_STRIP_SCRIM,
                );
                // The same icon as the header's, small enough for the strip.
                let icon_text = identity.paint(
                    ui,
                    egui::Pos2::new(title_strip.left() + 8.0 * inv_zoom, title_strip.center().y),
                    ICON_SIZE_SMALL,
                    inv_zoom,
                    egui::Color32::WHITE,
                );
                if let Some(text) = icon_text {
                    deferred_overlays.push(Box::new(move |overlay_painter, project| {
                        text.draw(overlay_painter, project(text.center));
                    }));
                }
                // Below `MIN_LABEL_CARD_SCREEN_WIDTH` (`label_fits`) the label would be a sliver; the icon alone remains.
                if show_text {
                    let label_font = egui::FontId::proportional(14.0);
                    // Screen width left for the label: the strip's, less the 16px left offset and some room on
                    // the right.
                    let label_max_width = (title_strip.width() * zoom_scale - 16.0 - 6.0).max(0.0);
                    let label = truncate_to_width(
                        &mut self.graph.truncate_cache,
                        ui.ctx(),
                        label,
                        &label_font,
                        label_max_width,
                    );
                    let label_pos = egui::Pos2::new(title_strip.left(), title_strip.center().y);
                    deferred_overlays.push(Box::new(move |overlay_painter, project| {
                        overlay_painter.text(
                            project(label_pos) + egui::vec2(16.0, 0.0),
                            egui::Align2::LEFT_CENTER,
                            label,
                            label_font,
                            egui::Color32::WHITE,
                        );
                    }));
                }
            }
            // The jar chip at the thumbnail's bottom-left, above a short card's title strip.
            if let Some(chip) = chip.filter(|_| show_text) {
                let strip = if header { 0.0 } else { TAB_STRIP_POINTS * inv_zoom };
                let corner = thumb_rect.left_bottom() + egui::vec2(0.0, -strip);
                let offset = egui::vec2(4.0, -4.0 - JAR_CHIP_HEIGHT);
                let max_width = thumb_rect.width() * zoom_scale - 8.0;
                push_jar_chip(deferred_overlays, id, corner, offset, max_width, chip, ui.visuals().warn_fg_color);
            }
        }
        // At every level of detail: a stuck engine is the one thing on this card that needs the user.
        if unresponsive {
            let middle = tab_thumb_rect(rect, inv_zoom).center();
            push_centered_text(
                deferred_overlays,
                middle,
                egui_phosphor::light::WARNING_CIRCLE,
                24.0,
                danger(ui.visuals()),
            );
            body.tooltip = chain_tooltip(
                Some("Not responding. Right-click for Restart View or End View.".to_owned()),
                body.tooltip.take(),
            );
        }
        body
    }
}
