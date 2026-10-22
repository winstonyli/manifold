//! `Ctrl+K`'s tab quick switcher: its state, the matching, and the window.

use super::*;
use crate::store::model::NodeId;

/// `Ctrl+K`'s tab quick-switcher: substring-search-by-title-or-url across
/// *every* open tab regardless of level/group (unlike the graph views'
/// own level-scoped listings — jumping to a tab buried in a different
/// group is the whole point of this), then jump straight to whichever one
/// is selected. `selected` indexes into whatever `App::quick_switcher_
/// matches` currently returns for `query` -- clamped on every keystroke
/// since the match list can shrink out from under a previously valid
/// index (e.g. a query that used to match 5 tabs now matches 2).
#[derive(Clone)]
pub(crate) struct QuickSwitcher {
    pub(crate) query: String,
    pub(crate) selected: usize,
}

impl App {
    /// A tab list row's label for `id`: its title, or its url when the
    /// title is empty (live or unloaded alike, via `listed_tab`) -- factored
    /// out so `build_quick_switcher`'s row loop and its own test agree on
    /// exactly what a sleeping tab's row shows.
    pub(crate) fn quick_switcher_row_url(&self, id: NodeId) -> Option<String> {
        self.nodes.get(&id)?.runtime.listed_tab().map(|(_, url)| url.to_owned())
    }

    /// The qualifier each listed tab needs beside its label to be told from a tab that reads the same
    /// (`disambiguate::qualifiers`), over all listed tabs so a row's qualifier does not change as the query narrows.
    pub(crate) fn listed_tab_qualifiers(&self) -> std::collections::HashMap<NodeId, String> {
        let ids: Vec<NodeId> =
            self.node_order.iter().copied().filter(|&id| self.quick_switcher_row_label(id).is_some()).collect();
        let labels: Vec<(String, String)> = ids
            .iter()
            .map(|&id| {
                (
                    self.quick_switcher_row_label(id).unwrap_or_default(),
                    self.quick_switcher_row_url(id).unwrap_or_default(),
                )
            })
            .collect();
        let rows: Vec<(&str, &str)> = labels.iter().map(|(l, u)| (l.as_str(), u.as_str())).collect();
        let qualifiers = crate::disambiguate::qualifiers(&rows, true);
        ids.into_iter().zip(qualifiers).filter_map(|(id, q)| Some((id, q?))).collect()
    }

    pub(crate) fn quick_switcher_row_label(&self, id: NodeId) -> Option<String> {
        let (title, url) = self.nodes.get(&id)?.runtime.listed_tab()?;
        Some(if title.is_empty() { url.to_string() } else { title.to_string() })
    }

    /// `self.node_order`, filtered by a case-insensitive substring match
    /// against title-or-url — factored out so the quick-switcher's render
    /// function and its Enter-key jump always agree on the exact same list
    /// (computed once per frame, not separately in each place that needs
    /// it).
    pub(crate) fn quick_switcher_matches(&self, query: &str) -> Vec<NodeId> {
        let query = query.to_lowercase();
        self.node_order
            .iter()
            .filter(|id| {
                self.nodes.get(id).and_then(|n| n.runtime.listed_tab()).is_some_and(|(title, url)| {
                    query.is_empty() || title.to_lowercase().contains(&query) || url.to_lowercase().contains(&query)
                })
            })
            .copied()
            .collect()
    }

    /// Opens the quick switcher, empty, with its field focused (Ctrl+K and `QUICKSWITCHOPEN`).
    pub(crate) fn open_quick_switcher(&mut self) {
        self.menus.quick_switcher.open = Some(QuickSwitcher { query: String::new(), selected: 0 });
        self.pending_focus = Some(FocusTarget::QuickSwitcher);
    }

    /// Moves the quick-switcher's selection by `delta` (+1/-1), clamped to
    /// the current match list — shared by the real ArrowUp/ArrowDown
    /// handling in `build_quick_switcher` and the headless `QUICKSWITCHUP`/
    /// `QUICKSWITCHDOWN` debug commands (see `debug_control`'s own doc
    /// comment on why a real `KeyEvent` can't be synthesized).
    pub(crate) fn quick_switcher_move(&mut self, delta: i32) {
        let Some(query) = self.menus.quick_switcher.open.as_ref().map(|s| s.query.clone()) else { return };
        let len = self.quick_switcher_matches(&query).len();
        let Some(switcher) = &mut self.menus.quick_switcher.open else { return };
        if len == 0 {
            switcher.selected = 0;
        } else {
            switcher.selected = step_index(switcher.selected.min(len - 1), delta as isize, len, false);
        }
    }

    /// Jumps to whichever match is currently selected and closes the
    /// switcher — shared by the real Enter handling in
    /// `build_quick_switcher` and headless `QUICKSWITCHENTER`. See
    /// `build_quick_switcher`'s own doc comment for why this calls
    /// `focus_tab` directly rather than the graph-node expand animation.
    pub(crate) fn quick_switcher_confirm(&mut self) {
        let Some(switcher) = &self.menus.quick_switcher.open else { return };
        let matches = self.quick_switcher_matches(&switcher.query);
        // Clamped: a query narrowed after moving down can leave `selected` past the end.
        if let Some(id) = matches.get(switcher.selected.min(matches.len().saturating_sub(1))).copied() {
            self.show_level_of(id);
            self.focus_tab(id);
        }
        self.menus.quick_switcher.open = None;
    }

    /// `Ctrl+K`'s tab quick-switcher: a centered search box plus a
    /// scrollable list of matches (see `quick_switcher_matches`), arrow
    /// keys move the selection, Enter jumps to it, Escape closes. Called
    /// from `build_ui` itself rather than `build_item_ui`/`build_graph_ui`
    /// so it works the same from either view.
    pub(crate) fn build_quick_switcher(&mut self, ui: &mut egui::Ui) {
        let current = self.menus.quick_switcher.open.clone();
        let is_live = current.is_some();
        let Some((switcher, fade)) = self.menus.quick_switcher.state(ui.ctx(), egui::Id::new("quick_switcher_fade"))
        else {
            return;
        };

        if is_live {
            // Read once, outside the `Area`, so arrow/Enter/Escape work
            // regardless of whether the search field inside currently has
            // egui focus — same reasoning the address-bar dropdown's own
            // arrow-key handling already documents. Gated on `is_live`:
            // once actually closing (rendering the frozen `cache` snapshot
            // below instead), there's no live switcher left to move a
            // selection in or confirm.
            let (delta, enter, escape) =
                ui.input(|i| (arrow_delta(i), i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
            if delta != 0 {
                self.quick_switcher_move(delta as i32);
            }
            if escape {
                self.menus.quick_switcher.open = None;
            }
            if enter {
                self.quick_switcher_confirm();
            }
        }

        let query = switcher.query.clone();
        let matches = self.quick_switcher_matches(&query);
        // The row Enter would pick (`quick_switcher_confirm` clamps the same way).
        let selected_index = switcher.selected.min(matches.len().saturating_sub(1));
        let mut jump_to: Option<NodeId> = None;

        egui::Area::new(egui::Id::new("quick_switcher"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 80.0))
            .order(egui::Order::Foreground)
            .interactable(is_live)
            .show(ui.ctx(), |ui| {
                ui.set_opacity(fade);
                dialog_frame(ui).show(ui, |ui| {
                    control_font(ui);
                    ui.set_width(360.0);
                    if is_live {
                        if let Some(switcher) = &mut self.menus.quick_switcher.open {
                            let response = ui.add(
                                egui::TextEdit::singleline(&mut switcher.query)
                                    .desired_width(f32::INFINITY)
                                    // Tab would otherwise walk focus out to the toolbar behind
                                    // the switcher; arrows and Enter pick a row.
                                    .lock_focus(true)
                                    .hint_text("Search open tabs..."),
                            );
                            if response.changed() {
                                switcher.selected = 0; // a new query starts at its first match
                            }
                            if take_focus(&mut self.pending_focus, FocusTarget::QuickSwitcher) {
                                response.request_focus();
                            }
                        }
                    } else {
                        // Closing: a live `TextEdit` needs `&mut self.
                        // quick_switcher`, which no longer exists once
                        // it's actually `None` -- the fade past this
                        // point is purely a visual echo of the last
                        // real state, not something still editable.
                        let placeholder = if query.is_empty() { "Search open tabs..." } else { &query };
                        ui.label(placeholder);
                    }
                    ui.separator();
                    if matches.is_empty() {
                        ui.label("No matching tabs");
                    }
                    key_hint_row(ui, [("Up / Down", "move"), ("Enter", "jump"), ("Esc", "close")].into_iter());
                    let qualifiers = self.listed_tab_qualifiers();
                    egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                        for (i, &id) in matches.iter().enumerate() {
                            let Some(label) = self.quick_switcher_row_label(id) else { continue };
                            let identity =
                                self.row_identity(id, self.quick_switcher_row_url(id).as_deref().unwrap_or(""));
                            let widget_label = row_label(ui, &label, qualifiers.get(&id).map(String::as_str), 0.0);
                            // Full width, so the highlight spans the row, not just its text.
                            let button = egui::Button::selectable(i == selected_index, widget_label)
                                .min_size(egui::vec2(ui.available_width(), 0.0));
                            let row = if is_live {
                                let row = rest_borderless(ui, |ui| ui.add(button));
                                if row.clicked() {
                                    jump_to = Some(id);
                                }
                                row
                            } else {
                                // Non-interactive while closing --
                                // clicking a row already fading out
                                // would be a confusing "did that even
                                // do anything" moment.
                                ui.add_enabled(false, button)
                            };
                            paint_row_icon(
                                ui,
                                row.rect,
                                ui.spacing().button_padding.x,
                                &label,
                                i == selected_index,
                                identity,
                            );
                            // A title alone doesn't tell two tabs of one site apart; the address does.
                            if let Some(url) = self.quick_switcher_row_url(id).filter(|url| *url != label) {
                                row.on_hover_text(url);
                            }
                        }
                    });
                    // The selected row's full address, for a keyboard user who never hovers.
                    let selected_url = matches.get(selected_index).and_then(|&id| {
                        let label = self.quick_switcher_row_label(id)?;
                        self.quick_switcher_row_url(id).filter(|url| *url != label)
                    });
                    if let Some(url) = selected_url {
                        ui.add(egui::Label::new(egui::RichText::new(url).small().weak()).truncate());
                    }
                });
            });

        if let Some(id) = jump_to {
            self.show_level_of(id);
            self.focus_tab(id);
            self.menus.quick_switcher.open = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The texts one frame of the quick switcher paints, in order.
    fn quick_switcher_texts(app: &mut App) -> Vec<String> {
        let ctx = egui::Context::default();
        crate::theme::apply_fonts(&ctx);
        // An `Area` is hidden for its sizing frame, and the popup fades in over a second: run both out.
        let mut output = Default::default();
        for frame in 0..4 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
                time: Some(f64::from(frame)),
                ..Default::default()
            };
            output = ctx.run_ui(input, |ui| app.build_quick_switcher(ui));
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

    /// The selected row's full address shows under the list when the label is not already the address.
    #[test]
    fn the_quick_switcher_shows_the_selected_rows_address_unless_it_is_the_label() {
        use crate::session_sync::tests::{add, app_on, sleeping_tab};
        let mut app = app_on(&crate::store::testutil::temp_dir("qs-address").join("session.mnf"), 1);
        add(&mut app, Kind::Tab, sleeping_tab("https://a.test/page", "Alpha"));
        app.open_quick_switcher();
        let texts = quick_switcher_texts(&mut app);
        assert!(texts.iter().any(|t| t == "https://a.test/page"), "{texts:?}");
        // An untitled tab's label is its address: said once, in the row.
        let mut app = app_on(&crate::store::testutil::temp_dir("qs-address2").join("session.mnf"), 1);
        add(&mut app, Kind::Tab, sleeping_tab("https://b.test/", ""));
        app.open_quick_switcher();
        let texts = quick_switcher_texts(&mut app);
        assert_eq!(texts.iter().filter(|t| t.contains("https://b.test/")).count(), 1, "{texts:?}");
    }

    /// Tabs that read the same are told apart by what differs; a tab of its own label gets nothing.
    #[test]
    fn tabs_that_read_the_same_get_a_qualifier_and_others_none() {
        use crate::session_sync::tests::{add, app_on, sleeping_tab};
        let mut app = app_on(&crate::store::testutil::temp_dir("qs-qualifiers").join("session.mnf"), 1);
        let one = add(&mut app, Kind::Tab, sleeping_tab("https://one.test/", "Home"));
        let two = add(&mut app, Kind::Tab, sleeping_tab("https://two.test/", "Home"));
        let other = add(&mut app, Kind::Tab, sleeping_tab("https://one.test/x", "Docs"));
        let q = app.listed_tab_qualifiers();
        assert_eq!(q.get(&one).map(String::as_str), Some("one.test"));
        assert_eq!(q.get(&two).map(String::as_str), Some("two.test"));
        assert_eq!(q.get(&other), None);
    }

    /// A sleeping tab's Quick Switcher row shows its title, or its url when
    /// untitled -- same as a live tab's row, unlike the raw `as_tab` lookup
    /// this used to skip an unloaded match with (Fix 1, final-review).
    #[test]
    fn quick_switcher_label_uses_title_or_url_for_a_sleeping_tab() {
        use crate::session_sync::tests::{add, app_on, sleeping_tab};
        let mut app = app_on(&crate::store::testutil::temp_dir("qs-label").join("session.mnf"), 1);
        let titled = add(&mut app, Kind::Tab, sleeping_tab("https://a/", "Alpha"));
        let untitled = add(&mut app, Kind::Tab, sleeping_tab("https://b/", ""));
        assert_eq!(app.quick_switcher_row_label(titled).as_deref(), Some("Alpha"));
        assert_eq!(app.quick_switcher_row_label(untitled).as_deref(), Some("https://b/"));
    }

    /// Review C-B2: a query narrowed after moving down confirms its first match, not nothing.
    #[test]
    fn a_narrowed_query_confirms_its_first_match() {
        use crate::session_sync::tests::{add, app_on, sleeping_tab};
        let mut app = app_on(&crate::store::testutil::temp_dir("qs-narrowed").join("session.mnf"), 1);
        let ids: Vec<crate::store::model::NodeId> = (0..6)
            .map(|i| add(&mut app, Kind::Tab, sleeping_tab(&format!("https://{i}/"), &format!("Tab {i}"))))
            .collect();
        app.open_quick_switcher();
        for _ in 0..5 {
            app.quick_switcher_move(1);
        }
        assert_eq!(app.menus.quick_switcher.open.as_ref().unwrap().selected, 5);
        app.menus.quick_switcher.open.as_mut().unwrap().query = "tab 2".into();
        app.quick_switcher_confirm();
        assert!(app.menus.quick_switcher.open.is_none());
        assert_eq!(app.focused, Some(ids[2]), "jumped to the one match");

        app.open_quick_switcher();
        app.menus.quick_switcher.open.as_mut().unwrap().query = "no such tab".into();
        app.quick_switcher_confirm(); // nothing matches: closes, no panic
        assert!(app.menus.quick_switcher.open.is_none());
        assert_eq!(app.focused, Some(ids[2]), "focus unchanged");
    }
}
