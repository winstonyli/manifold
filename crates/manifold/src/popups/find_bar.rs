//! The find bar (`Ctrl+F`): its state and the messages that start, step and stop a search in the focused tab.

use super::*;

/// Find-in-page state: tied to the focused tab only, not persisted per-tab,
/// and closed (with `FindStop`) on tab switch or close, the same "not a full
/// session" scope as the address bar's suggestions. See
/// `ViewToHost::FindResult`'s doc comment for why `active_index` is always
/// 0 on a WebKit tab and Servo tabs never populate `result`.
#[derive(Default)]
pub(crate) struct FindState {
    pub(crate) open: bool,
    /// Mirrors the last `FindStart`. Kept across a close, so reopening
    /// searches for it again, as a real browser's find bar does.
    pub(crate) text: String,
    pub(crate) match_case: bool,
    /// `(count, active_index)`; `None` until the first `FindResult` for the
    /// current search arrives, so the bar shows nothing rather than a stale
    /// count while a fresh search is in flight.
    pub(crate) result: Option<(u32, u32)>,
}

impl FindState {
    /// The UI side of closing the bar: `App::close_find_bar` also sends
    /// `FindStop`, which a destroyed tab can't receive. The next session
    /// starts case-insensitive, as a real browser's find bar does.
    pub(crate) fn close(&mut self) {
        self.open = false;
        self.result = None;
        self.match_case = false;
    }
}

impl App {
    /// Sends `FindStart` for `self.find.text` to the focused tab, or
    /// `FindStop` if it's empty (clearing any existing highlight) — shared
    /// by the find bar's own text-changed handling and `open_find_bar_shortcut`
    /// re-opening with a remembered term.
    pub(crate) fn send_find_start(&mut self) {
        self.find.result = None;
        // Tab view has no continuous redraw loop of its own (unlike Graph
        // view's physics) — without this, a headless `FINDTEXT` (or a
        // remembered-term re-open) can land with nothing scheduled to ever
        // paint the bar's updated count, the same gap `focus_address_bar_
        // shortcut` already has its own note about.
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        let Some(id) = self.focused else { return };
        let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) else { return };
        if self.find.text.is_empty() {
            node.tab.send(&HostToView::FindStop);
        } else {
            node.tab.send(&HostToView::FindStart { text: self.find.text.clone(), match_case: self.find.match_case });
        }
    }

    /// Closes the find bar: sends `FindStop` (clears highlighted matches)
    /// and resets its result and match case (`FindState::close`) — called on `Escape`, its own close
    /// button, headless `CLOSEFINDBAR`, and whenever focus leaves the tab
    /// it was searching (tab switch/close), so a stale search never keeps
    /// running silently in the background.
    pub(crate) fn close_find_bar(&mut self) {
        if !self.find.open {
            return;
        }
        self.find.close();
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        if let Some(id) = self.focused {
            if let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
                node.tab.send(&HostToView::FindStop);
            }
        }
    }

    /// `Enter`/`Shift+Enter` in the find bar, or its Prev/Next buttons.
    pub(crate) fn send_find_navigate(&mut self, forward: bool) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        let Some(id) = self.focused else { return };
        let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) else { return };
        node.tab.send(&if forward { HostToView::FindNext } else { HostToView::FindPrevious });
    }

    /// `Ctrl+F`'s floating find bar: a text field, prev/next, a match count,
    /// and a close button. Anchored top-right of the content area in
    /// screen space rather than off some toolbar button's rect — unlike
    /// Settings, there's no persistent visible button to anchor from (this
    /// only ever opens via `Ctrl+F`/headless `OPENFINDBAR`).
    pub(crate) fn build_find_bar(&mut self, ui: &mut egui::Ui) {
        let mut close = false;
        let mut text_changed = false;
        let mut go_next = false;
        let mut go_prev = false;
        egui::Area::new(egui::Id::new("find_bar"))
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-12.0, CHROME_HEIGHT_POINTS + 6.0))
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                bar_frame(ui).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let response = toolbar_field(ui, |ui| {
                            ui.add(field_edit(&mut self.find.text).desired_width(160.0).hint_text("Find in page"))
                        })
                        .on_hover_text("Enter: next match, Shift+Enter: previous, Esc: close");
                        if take_focus(&mut self.pending_focus, FocusTarget::FindBar) {
                            response.request_focus();
                        }
                        if response.changed() {
                            text_changed = true;
                        }
                        if rest_borderless(ui, |ui| ui.toggle_value(&mut self.find.match_case, "Aa"))
                            .on_hover_text("Match case")
                            .changed()
                        {
                            text_changed = true;
                        }
                        if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            if ui.input(|i| i.modifiers.shift) {
                                go_prev = true;
                            } else {
                                go_next = true;
                            }
                        }

                        match self.find.result {
                            Some((0, _)) => {
                                ui.label("No results");
                            }
                            // WebKit never reports `active_index` (see
                            // `ViewToHost::FindResult`'s doc comment)
                            // — show just the count, not "0 of N".
                            Some((count, 0)) => {
                                ui.label(format!("{count}"));
                            }
                            Some((count, active)) => {
                                ui.label(format!("{active}/{count}"));
                            }
                            None => {}
                        }

                        // Prev/Next stay plain ASCII labels, not Unicode
                        // arrow glyphs — `▲`/`▼` render as empty tofu
                        // boxes with this app's own font (unlike
                        // `◀`/`▶`/`⚙` elsewhere in this same toolbar,
                        // which do render).
                        let has_matches = matches!(self.find.result, Some((count, _)) if count > 0);
                        if ui.add_enabled(has_matches, egui::Button::new("Prev")).clicked() {
                            go_prev = true;
                        }
                        if ui.add_enabled(has_matches, egui::Button::new("Next")).clicked() {
                            go_next = true;
                        }
                        // The real `X` icon, as on the other close buttons in
                        // this app's chrome (a Unicode `✕` tofu-boxes here,
                        // same as the arrows above).
                        if ui.button(egui_phosphor::light::X).named_tip("Close (Esc)").clicked() {
                            close = true;
                        }
                    });
                });
            });

        if text_changed {
            self.send_find_start();
        }
        if go_next {
            self.send_find_navigate(true);
        }
        if go_prev {
            self.send_find_navigate(false);
        }
        if close {
            self.close_find_bar();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_find_keeps_the_text_and_resets_the_rest() {
        let mut find = FindState { open: true, text: "abc".into(), match_case: true, result: Some((3, 1)) };
        find.close();
        assert!(!find.open);
        assert_eq!(find.text, "abc");
        assert!(!find.match_case);
        assert_eq!(find.result, None);
    }
}
