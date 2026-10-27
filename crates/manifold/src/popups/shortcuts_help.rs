//! The keyboard-shortcut reference popup (`F1`).

use super::*;

impl App {
    /// `F1` / headless `TOGGLESHORTCUTSHELP` -- a centered reference popup
    /// listing every global keyboard shortcut `handle_global_shortcuts`
    /// handles, so a shortcut added to that function doesn't just live in a
    /// code comment nobody using the app ever sees. Static content (no
    /// state to stage/commit), so unlike the quick-switcher this is just a
    /// plain `Area`/`Frame` with a scrollable list and a Close button.
    pub(crate) fn build_shortcuts_help(&mut self, ui: &mut egui::Ui, anchor_rect: Option<egui::Rect>) {
        if !self.menus.shortcuts_help_open {
            return;
        }
        // Consumed, so a page that reads Escape later in the frame does not also act on it.
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.menus.shortcuts_help_open = false;
            return;
        }

        // A key of "#" starts a section; its text is the heading. Everywhere is generated from `CHORDS`.
        const SHORTCUTS: &[(&str, &str)] = &[
            ("#", "Graph canvas"),
            ("1 - 9", "Focus the card with that number (two digits for 10 and up); Enter then opens it"),
            ("Hold Ctrl", "Reveal the card numbers and the Ctrl shortcuts (after a short hold, with no other key)"),
            ("Arrows", "Move focus to the nearest node"),
            ("Enter", "Open the focused node"),
            ("Escape", "Clear the selection, close a menu, dismiss a preview"),
            ("#", "Graph cards (Tab onto the canvas)"),
            ("Tab / Shift+Tab", "Next / previous card, in creation order (numbered badges)"),
            ("Enter", "Open the focused card; enters a group or JSON level"),
            ("Space", "Select / deselect a tab card"),
            ("Delete", "Close the focused card"),
            ("Shift+F10", "The card's menu: color, move to a group, ..."),
            ("Escape", "Close a menu or popup, back to its card"),
            ("#", "JSON level"),
            ("Up / Down", "Select the previous / next outline row"),
            ("Enter", "Expand / collapse the selected card"),
            ("Right / Left", "Open / close the row (Left on a closed row selects its parent)"),
            ("Ctrl+C", "Copy the selected value"),
            ("Ctrl+Shift+C", "Copy the selected path"),
            ("#", "Tab view and pages"),
            ("Escape", "Back to Graph (closes an open popup first)"),
            ("#", "Address bar, find bar, quick switcher"),
            ("Up / Down", "Address bar: move through the suggestions (wraps)"),
            ("Enter", "Address bar: take the highlighted suggestion, else go to the typed text"),
            ("Escape", "Address bar: dismiss the suggestions; again for Graph"),
            ("Enter", "Find bar: next match; Shift+Enter previous"),
            ("Escape", "Find bar / quick switcher: close"),
            ("#", "Lists and popups"),
            ("Up / Down", "Move the highlight (Recently Closed, Bookmarks, History, Downloads, quick switcher)"),
            ("Enter", "Pick the highlighted row"),
            ("Escape", "Close and return to the button that opened it"),
        ];

        let area = egui::Area::new(egui::Id::new("shortcuts_help"))
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                dialog_frame(ui).show(ui, |ui| {
                    // Narrower than 360 in a narrow window; the descriptions wrap to the room left of the keys.
                    let width = 360.0_f32.min(ui.ctx().content_rect().width() - 60.0);
                    ui.set_width(width);
                    ui.label(heading_text(ui.visuals(), "Keyboard Shortcuts"));
                    ui.separator();
                    // Leaves room for the title above and the Close button below on a short window.
                    let list_height = (ui.ctx().content_rect().height() - 170.0).clamp(120.0, 520.0);
                    egui::ScrollArea::vertical().max_height(list_height).show(ui, |ui| {
                        egui::Grid::new("shortcuts_help_grid")
                            .num_columns(2)
                            .spacing(egui::vec2(16.0, 6.0))
                            .max_col_width(width * 0.6)
                            .show(ui, |ui| {
                                let everywhere = [("#", "Everywhere")].into_iter().chain(crate::shortcuts::help_rows());
                                for (keys, desc) in everywhere.chain(SHORTCUTS.iter().copied()) {
                                    if keys == "#" {
                                        ui.label(egui::RichText::new(desc).color(accent(ui.visuals())));
                                        ui.label("");
                                    } else {
                                        ui.horizontal(|ui| kbd_keys(ui, keys));
                                        ui.label(desc);
                                    }
                                    ui.end_row();
                                }
                            });
                    });
                    ui.separator();
                    if ui.button("Close").clicked() {
                        self.menus.shortcuts_help_open = false;
                    }
                });
            });

        if clicked_outside_popup(ui, area.response.rect, anchor_rect) {
            self.menus.shortcuts_help_open = false;
        }
    }
}
