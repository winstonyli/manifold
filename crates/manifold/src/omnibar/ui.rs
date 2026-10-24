//! Drawing the omnibar: the field every host shares, its dropdown, and the Ctrl+K popup.

use super::*;

/// The icon of a row that is not a card.
fn row_icon(row: &Row) -> Identity {
    let glyph = match row {
        Row::Card { .. } => egui_phosphor::light::BROWSER,
        Row::History { .. } => egui_phosphor::light::CLOCK_COUNTER_CLOCKWISE,
        Row::Setting { .. } => egui_phosphor::light::GEAR,
        Row::Typed { text } if looks_like_url(text) => egui_phosphor::light::ARROW_RIGHT,
        Row::Typed { .. } | Row::Suggestion { .. } => egui_phosphor::light::MAGNIFYING_GLASS,
        Row::Command(_) => egui_phosphor::light::SPARKLE,
    };
    Identity::Glyph { glyph, color: None }
}

/// What a row says.
pub(crate) fn row_text(row: &Row) -> String {
    match row {
        Row::Card { label, .. } => label.clone(),
        Row::History { url, title } if title.is_empty() => url.clone(),
        Row::History { url, title } => format!("{title} \u{2014} {url}"),
        Row::Setting { title, section, .. } => format!("{section} \u{203a} {title}"),
        Row::Typed { text } if looks_like_url(text) => format!("Go to {text}"),
        Row::Typed { text } => format!("Search for \u{201c}{text}\u{201d}"),
        Row::Suggestion { text } => text.clone(),
        Row::Command(command) => command.title().to_owned(),
    }
}

impl App {
    /// A host that changed since the bar last drew restarts it: the Query hosts empty, the dropdown closed. The Item
    /// view's text is the tab's address, which its builder syncs.
    pub(crate) fn omni_sync_host(&mut self, host: Host) {
        let bar = self.bar_mut(host);
        if bar.host == Some(host) {
            return;
        }
        bar.host = Some(host);
        bar.mode = host.mode();
        bar.focused = false;
        bar.dismiss();
        if host != Host::Item {
            bar.text.clear();
        }
    }

    /// The host the showing view draws its bar in, if any.
    pub(crate) fn view_host(&self) -> Option<Host> {
        match self.view_mode {
            ViewMode::Item if self.showing_tab_view() => Some(Host::Item),
            ViewMode::Item => None,
            ViewMode::Graph => Some(Host::Graph),
            ViewMode::Settings => Some(Host::Settings),
        }
    }

    /// Leaving a view ends the edit, as Escape does. Otherwise the dropdown and typed text came back over whichever
    /// view was shown next.
    pub(crate) fn omni_leave_other_views(&mut self) {
        if self.omni.host.is_some() && self.omni.host != self.view_host() {
            self.omni.dismiss();
            self.omni.focused = false;
            self.omni.host = None;
        }
    }

    /// The field of an inline host (`width` points wide) with its dropdown below. Enter, Up and Down act on the
    /// dropdown; the Item view keeps Escape for its own layered chain.
    pub(crate) fn omni_ui(&mut self, ui: &mut egui::Ui, host: Host, width: f32) -> egui::Response {
        self.omni_sync_host(host);
        let bar = self.bar_mut(host);
        let (left, right, hint) = match host {
            // Room for the favicon at the left and the Ctrl L keycaps at the right.
            Host::Item => (32, 56, "Search or enter address"),
            _ => (8, 8, "Search tabs, history, settings..."),
        };
        let response = toolbar_field(ui, |ui| {
            ui.add(field_edit_inset(&mut bar.text, left, right).desired_width(width).hint_text(hint))
        });
        if take_focus(&mut self.pending_focus, FocusTarget::OmniBar) {
            response.request_focus();
        }
        // Egui's own focus, not `response.has_focus()`: that is also false while the OS window is unfocused, yet the
        // field keeps its edit, so switching windows mid-edit would close the dropdown and resync the typed text
        // away. A click into the page goes through egui and does move focus.
        let has_focus = ui.memory(|m| m.has_focus(response.id));
        self.omni.note_focus(has_focus);
        if let Some(results) = self.omni.fetcher.tick() {
            self.omni.search_suggestions = results;
        }
        if response.changed() {
            self.omni_text_changed(host);
        }
        let (enter, escape) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
        if response.lost_focus() && enter {
            self.omni_confirm(host);
        } else if response.lost_focus() && escape && host != Host::Item {
            self.omni_escape(host);
        } else if self.omni.editing() {
            // The arrows are the field's only while it has focus (or is in its one-frame blip).
            let delta = if self.omni.unfocused_frames <= 1 { ui.input(popups::arrow_delta) } else { 0 };
            self.omni_move(host, delta);
            let rows = self.omni_rows(host);
            if self.omni.open && !rows.is_empty() {
                let width = response.rect.width().max(DROPDOWN_MIN_WIDTH);
                if let Some(clicked) = self.omni_dropdown(ui, &rows, response.rect.left_bottom(), width) {
                    self.omni_activate(rows[clicked].clone(), host);
                }
            }
        }
        response
    }

    /// The dropdown under an inline field; returns the row clicked.
    fn omni_dropdown(&self, ui: &mut egui::Ui, rows: &[Row], at: egui::Pos2, width: f32) -> Option<usize> {
        let mut clicked = None;
        egui::Area::new(egui::Id::new("omni_dropdown")).fixed_pos(at).order(egui::Order::Foreground).show(
            ui.ctx(),
            |ui| {
                // Inner width minus the frame's padding and border, so the dropdown lines up with the field's edges.
                let frame = popup_frame(ui);
                let inner_width = width - frame.total_margin().sum().x;
                frame.show(ui, |ui| {
                    menu_item_style(ui);
                    ui.set_width(inner_width);
                    clicked = self.omni_rows_ui(ui, rows, self.omni.highlighted, true);
                });
            },
        );
        clicked
    }

    /// The rows as full-width menu rows with the highlighted one filled; returns the row clicked (`live`: they are
    /// interactive).
    fn omni_rows_ui(&self, ui: &mut egui::Ui, rows: &[Row], highlighted: Option<usize>, live: bool) -> Option<usize> {
        let qualifiers = self.listed_tab_qualifiers();
        let full_width = ui.available_width();
        let mut clicked = None;
        for (idx, row) in rows.iter().enumerate() {
            let selected = highlighted == Some(idx);
            let (identity, qualifier) = match row {
                Row::Card { id, url, .. } => (self.row_identity(*id, url), qualifiers.get(id).map(String::as_str)),
                Row::History { url, .. } => (Self::page_row_identity(self.gpu.as_ref(), url), None),
                other => (row_icon(other), None),
            };
            let text = row_text(row);
            let mut button = egui::Button::new(row_label(ui, &text, qualifier, 0.0))
                .truncate()
                .min_size(egui::vec2(full_width, 0.0));
            if selected {
                button = button.fill(accent(ui.visuals()));
            }
            let response = if live { ui.add(button) } else { ui.add_enabled(false, button) };
            paint_row_icon(ui, response.rect, ui.spacing().button_padding.x, &text, selected, identity);
            if live && response.clicked() {
                clicked = Some(idx);
            }
            if let Row::Card { label, url, jar, .. } = row {
                let mut tip = if url == label { String::new() } else { url.clone() };
                if let Some(jar) = jar {
                    tip = format!("{tip}\nJar: {jar}").trim().to_owned();
                }
                if !tip.is_empty() {
                    response.on_hover_text(tip);
                }
            }
        }
        clicked
    }

    /// `Ctrl+K`: a centered omnibar with its rows listed under it. Arrow keys move, Enter activates, Escape clears and
    /// then closes. Called from `build_ui` itself rather than a view's builder so it works the same from any view.
    pub(crate) fn build_omni_popup(&mut self, ui: &mut egui::Ui) {
        let is_live = self.menus.omni_popup.open.is_some();
        let Some(((), fade)) = self.menus.omni_popup.state(ui.ctx(), egui::Id::new("omni_popup_fade")) else { return };
        if let Some(results) = self.omni_popup.fetcher.tick() {
            self.omni_popup.search_suggestions = results;
        }
        if is_live {
            // Read once, outside the `Area`, so arrow/Enter/Escape work regardless of whether the field inside
            // currently has egui focus. Gated on `is_live`: once closing there is no live bar left to move or confirm.
            let (delta, enter, escape) = ui
                .input(|i| (popups::arrow_delta(i), i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
            self.omni_move(Host::Popup, delta);
            if escape {
                self.omni_escape(Host::Popup);
            } else if enter {
                self.omni_confirm(Host::Popup);
            }
        }
        let rows = self.omni_rows(Host::Popup);
        let live = self.menus.omni_popup.open.is_some() && is_live;
        let mut clicked = None;
        egui::Area::new(egui::Id::new("omni_popup"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 80.0))
            .order(egui::Order::Foreground)
            .interactable(live)
            .show(ui.ctx(), |ui| {
                ui.set_opacity(fade);
                dialog_frame(ui).show(ui, |ui| {
                    ui.set_width(360.0);
                    let response = toolbar_field(ui, |ui| {
                        ui.add_enabled(
                            live,
                            field_edit(&mut self.omni_popup.text)
                                .desired_width(f32::INFINITY)
                                // Tab would otherwise walk focus out to the toolbar behind the popup; arrows and
                                // Enter pick a row.
                                .lock_focus(true)
                                .hint_text("Search tabs, history, settings..."),
                        )
                    });
                    if take_focus(&mut self.pending_focus, FocusTarget::OmniPopup) {
                        response.request_focus();
                    }
                    if response.changed() {
                        self.omni_text_changed(Host::Popup);
                    }
                    ui.separator();
                    if rows.is_empty() {
                        ui.label("Nothing open");
                    }
                    key_hint_row(ui, [("Up / Down", "move"), ("Enter", "open"), ("Esc", "close")].into_iter());
                    egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                        // Rows are menu rows: the same padding, height and corners as every other list.
                        menu_item_style(ui);
                        clicked = self.omni_rows_ui(ui, &rows, self.omni_popup.highlighted, live);
                    });
                    // The highlighted card's full address, for a keyboard user who never hovers.
                    let shown = self.omni_popup.highlighted.and_then(|i| rows.get(i));
                    if let Some(Row::Card { label, url, .. }) = shown.filter(|_| !rows.is_empty()) {
                        if url != label {
                            ui.add(egui::Label::new(egui::RichText::new(url).small().weak()).truncate());
                        }
                    }
                });
            });
        if let Some(i) = clicked {
            self.omni_activate(rows[i].clone(), Host::Popup);
        }
    }
}

/// The dropdown is at least this wide, so a short field (the Graph's) still shows readable rows.
const DROPDOWN_MIN_WIDTH: f32 = 340.0;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_sync::tests::{add, app_on, sleeping_tab};
    use crate::store::model::Kind;

    /// The texts one frame of the popup paints, in order.
    fn popup_texts(app: &mut App) -> Vec<String> {
        let ctx = crate::theme::testutil::ctx(true);
        // An `Area` is hidden for its sizing frame, and the popup fades in over a moment: run both out.
        let mut output = Default::default();
        for frame in 0..4 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
                time: Some(f64::from(frame)),
                ..Default::default()
            };
            output = ctx.run_ui(input, |ui| app.build_omni_popup(ui));
            output.textures_delta.clear();
        }
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            fn walk(shape: &egui::Shape, texts: &mut Vec<String>) {
                match shape {
                    egui::Shape::Text(t) => texts.push(t.galley.text().to_string()),
                    egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, texts)),
                    _ => {}
                }
            }
            walk(&clipped.shape, &mut texts);
        }
        texts
    }

    /// The highlighted row's full address shows under the list when the label is not already the address.
    #[test]
    fn the_popup_shows_the_highlighted_rows_address_unless_it_is_the_label() {
        let mut app = app_on(&crate::store::testutil::temp_dir("omni-address").join("session.mnf"), 1);
        add(&mut app, Kind::Tab, sleeping_tab("https://a.test/page", "Alpha"));
        app.open_omni_popup();
        let texts = popup_texts(&mut app);
        assert!(texts.iter().any(|t| t == "https://a.test/page"), "{texts:?}");
        // An untitled tab's label is its address: said once, in the row.
        let mut app = app_on(&crate::store::testutil::temp_dir("omni-address2").join("session.mnf"), 1);
        add(&mut app, Kind::Tab, sleeping_tab("https://b.test/", ""));
        app.open_omni_popup();
        let texts = popup_texts(&mut app);
        assert_eq!(texts.iter().filter(|t| t.contains("https://b.test/")).count(), 1, "{texts:?}");
    }
}
