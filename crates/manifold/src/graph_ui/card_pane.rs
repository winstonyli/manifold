//! The card pane (card-pane spec): what the selected cards are, in the right panel beside the canvas. It shows state
//! and edits it with the actions the menus send (`GraphAction`, one card's `NodeAction`s as `GraphAction::Node`);
//! commands stay in the right-click menu and the selection bar.

use super::*;
use crate::node_menu::{NodeAction, NodeMenuItem};

/// The Text section's height before it scrolls.
const TEXT_MAX_HEIGHT: f32 = 240.0;

impl App {
    /// The one stored card the pane describes: the stored selection when it is a single card still shown, on a
    /// level that is not a JSON level.
    pub(crate) fn pane_card(&self) -> Option<NodeId> {
        let mut stored = self.interaction.selected.iter().filter_map(|c| c.node());
        match (stored.next(), stored.next()) {
            (Some(id), None) if self.json_level.is_none() && self.nodes.contains_key(&id) => Some(id),
            _ => None,
        }
    }

    /// R2: when the pane starts describing another card, the camera brings it into view (`camera.reveal`, eased by
    /// `pan_to` only when it is not fully visible; never during a drag, `apply_camera_requests`).
    pub(super) fn track_pane_card(&mut self) {
        let card = self.pane_card();
        if card == self.interaction.pane_card {
            return;
        }
        self.interaction.pane_card = card;
        if let Some(id) = card {
            self.camera.reveal = Some(id);
        }
    }

    /// The pane. One card: header, labels, place, links, text. Several: the count and the labels. Its rows step with
    /// Up/Down while one has focus, and never take focus themselves (R5).
    pub(super) fn card_pane_contents(&mut self, ui: &mut egui::Ui, actions: &mut Vec<GraphAction>) {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match self.pane_card() {
            Some(id) => {
                self.pane_header(ui, actions, id);
                self.pane_labels(ui, actions);
                self.pane_place(ui, actions, id);
                self.pane_text_section(ui, id);
            }
            None => {
                ui.label(format!("{} selected", self.selected_stored_count()));
                self.pane_labels(ui, actions);
            }
        });
        let rows = crate::popups::take_list_rows(ui);
        crate::popups::step_focused_row(ui, &rows.rows, rows.partial);
    }

    /// Whether the pane shows the rename field for `id`: the one draft (`renaming_group`) is this card's and its
    /// menu, which would show the same field, is not open on it.
    pub(crate) fn pane_renaming(&self, id: NodeId) -> bool {
        self.renaming_group.as_ref().is_some_and(|(of, _)| *of == id)
            && self.menus.node_menu.open.as_ref().map(|(of, _)| *of) != Some(id)
    }

    /// The card's name as a screen reader says it, a pencil that starts a rename (the menu's `Rename`), and its URL
    /// or path, selectable for copying.
    fn pane_header(&mut self, ui: &mut egui::Ui, actions: &mut Vec<GraphAction>, id: NodeId) {
        if self.pane_renaming(id) {
            let focus = crate::popups::take_focus(&mut self.pending_focus, crate::popups::FocusTarget::GroupRename);
            let Some((_, draft)) = self.renaming_group.as_mut() else { return };
            let field = crate::theme::toolbar_field(ui, |ui| {
                ui.add(crate::theme::field_edit(draft).desired_width(f32::INFINITY))
            });
            if focus {
                field.request_focus();
            }
            if field.lost_focus() {
                let action = if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    NodeAction::CancelRename
                } else {
                    NodeAction::CommitRename(draft.clone())
                };
                actions.push(GraphAction::Node(id, action));
            }
        } else {
            ui.horizontal(|ui| {
                ui.add(
                    egui::Label::new(egui::RichText::new(self.accessible_name(id).unwrap_or_default()).strong()).wrap(),
                );
                if ui.small_button(egui_phosphor::light::PENCIL_SIMPLE).on_hover_text("Rename").clicked() {
                    actions.push(GraphAction::Node(id, NodeAction::Rename));
                }
            });
        }
        if let Some(url) = self.copyable_url(id) {
            ui.add(egui::Label::new(egui::RichText::new(url).small().weak()).selectable(true).wrap());
        }
    }

    /// Where the card sits: its group (with Remove from Group), a pinned mark, its jar rows, its links rows. The jar
    /// and links rows are the menu's own items (`draw_menu_item`), so the two never list them differently.
    fn pane_place(&mut self, ui: &mut egui::Ui, actions: &mut Vec<GraphAction>, id: NodeId) {
        let mut clicked = None;
        if let Some(group) = self.doc.mirror.group(id) {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let name = self.accessible_name(group).unwrap_or_default();
                ui.label(egui::RichText::new(format!("{} {name}", egui_phosphor::light::FOLDER)).weak());
                if ui.small_button(egui_phosphor::light::X).on_hover_text("Remove from Group").clicked() {
                    clicked = Some(NodeAction::RemoveFromGroup);
                }
            });
        }
        if self.doc.mirror.pinned(id) {
            ui.label(egui::RichText::new(format!("{} Pinned", egui_phosphor::light::PUSH_PIN)).weak());
        }
        if let Some(jars) = self.jar_menu(id) {
            crate::node_menu::draw_menu_item(ui, &NodeMenuItem::Jars { heading: "Jar", rows: jars.rows }, &mut clicked);
        }
        let edges = self.edge_menu(id);
        if !edges.is_empty() {
            crate::node_menu::draw_menu_item(ui, &NodeMenuItem::Edges(edges), &mut clicked);
        }
        if let Some(action) = clicked {
            actions.push(GraphAction::Node(id, action));
        }
    }

    /// The text the pane shows for `id`: a note's text (`note_text`), else the page text last stored for it
    /// (`search::text_store`); none when both are empty.
    pub(crate) fn pane_text(&self, id: NodeId) -> Option<&str> {
        let stored = || self.search.texts.get(id).map_or("", |r| r.text.as_str());
        Some(self.note_text(id)).filter(|t| !t.trim().is_empty()).or(Some(stored())).filter(|t| !t.trim().is_empty())
    }

    /// The Text section: read-only, capped in height, scrolled.
    fn pane_text_section(&self, ui: &mut egui::Ui, id: NodeId) {
        let Some(text) = self.pane_text(id) else { return };
        ui.add_space(8.0);
        ui.label(egui::RichText::new("Text").small().weak());
        egui::ScrollArea::vertical().id_salt("card_pane_text").max_height(TEXT_MAX_HEIGHT).show(ui, |ui| {
            ui.add(egui::Label::new(text).selectable(true).wrap());
        });
    }

    /// The Labels section's rows: every live label, its stored name and colour, and how much of the selection carries
    /// it.
    pub(crate) fn pane_label_rows(&self) -> Vec<(NodeId, String, Option<egui::Color32>, crate::labels::LabelState)> {
        // name rule: the rows draw the name through the display rule and edit the stored one
        self.label_defs().into_iter().map(|(id, def)| (id, def.name, def.color, self.label_state(id))).collect()
    }

    /// The Labels section: a row per label (tick, rename and recolour, delete on the second press), then the field
    /// that makes a new label and puts it on the selection.
    fn pane_labels(&mut self, ui: &mut egui::Ui, actions: &mut Vec<GraphAction>) {
        ui.add_space(8.0);
        ui.label(egui::RichText::new("Labels").small().weak());
        let rows = self.pane_label_rows();
        let section = &mut self.interaction.label_section;
        for (id, name, color, state) in rows {
            label_row(ui, actions, section, id, &name, color, state);
        }
        let field = ui.add(egui::TextEdit::singleline(&mut section.draft).hint_text("New label").desired_width(160.0));
        if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !section.draft.trim().is_empty() {
            actions.push(GraphAction::NewLabel(std::mem::take(&mut section.draft)));
        }
    }
}

#[allow(clippy::too_many_arguments, reason = "one row of the section, its parts already split out of the label")]
fn label_row(
    ui: &mut egui::Ui,
    actions: &mut Vec<GraphAction>,
    section: &mut crate::labels::LabelSection,
    id: NodeId,
    name: &str,
    color: Option<egui::Color32>,
    state: crate::labels::LabelState,
) {
    use crate::labels::LabelState;
    let shown = crate::names::to_plain(&crate::names::display_name(name, "?"));
    if let Some((_, draft)) = section.editing.as_mut().filter(|(edited, _)| *edited == id) {
        let edit = ui.vertical(|ui| {
            let field = ui.add(egui::TextEdit::singleline(draft).desired_width(160.0));
            if std::mem::take(&mut section.focus_pending) {
                field.request_focus();
            }
            if let Some(picked) = crate::node_menu::color_swatch_row(ui, color) {
                actions.push(GraphAction::RecolorLabel { label: id, color: picked });
            }
            field
        });
        let field = edit.inner;
        let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if enter {
            actions.push(GraphAction::RenameLabel { label: id, name: std::mem::take(draft) });
        }
        // Escape, Tab or a click outside the field and its swatches cancels: the pane stays open, so a rename that
        // outlived its focus would keep the keys from the graph. A click inside (a swatch) hands the keys back. egui
        // surrenders focus on the click's release, so test the pointer's position, not the press origin (gone by then).
        let click_inside = ui.input(|i| {
            (i.pointer.any_pressed() || i.pointer.any_released())
                && i.pointer.interact_pos().is_some_and(|p| edit.response.rect.contains(p))
        });
        if enter {
            section.editing = None;
        } else if field.lost_focus() {
            if click_inside {
                section.focus_pending = true;
            } else {
                section.editing = None;
            }
        }
        return;
    }
    ui.horizontal(|ui| {
        let mark = match state {
            LabelState::All => egui_phosphor::light::CHECK_SQUARE,
            LabelState::Some => egui_phosphor::light::MINUS_SQUARE,
            LabelState::None => egui_phosphor::light::SQUARE,
        };
        let dot = color.map_or(ui.visuals().weak_text_color(), |c| crate::theme::tag_shown(c, ui.visuals().dark_mode));
        let row = ui.button(format!("{mark} {shown}"));
        crate::popups::nav_row(ui, &row);
        ui.painter().circle_filled(row.rect.right_center() - egui::vec2(6.0, 0.0), 4.0, dot);
        if row.clicked() {
            section.confirm_delete = None;
            actions.push(GraphAction::ToggleLabel { label: id, on: state != LabelState::All });
        }
        if ui.small_button(egui_phosphor::light::PENCIL_SIMPLE).on_hover_text("Rename or recolour").clicked() {
            section.start_editing(id, name); // name rule: the draft starts as the stored name
        }
        let armed = section.confirm_delete == Some(id);
        let trash = if armed {
            egui::Button::new(egui::RichText::new("Delete?").color(ui.visuals().error_fg_color)).small()
        } else {
            egui::Button::new(egui_phosphor::light::TRASH).small()
        };
        if ui.add(trash).on_hover_text("Delete this label (click twice)").clicked() && section.press_trash(id) {
            actions.push(GraphAction::DeleteLabel(id));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card_id::CardId;

    /// R2: the card the pane starts describing is revealed once; a second selected card is not. (A drag refuses the
    /// ease itself: `a_drag_stops_and_refuses_camera_eases`.)
    #[test]
    fn the_pane_reveals_the_card_it_starts_describing() {
        let mut app = App::default();
        let a = crate::session_sync::tests::add_note(&mut app, "a");
        let b = crate::session_sync::tests::add_note(&mut app, "b");
        app.interaction.selected.insert(CardId::from(a));
        app.track_pane_card();
        assert_eq!(app.camera.reveal, Some(a));
        app.camera.reveal = None;
        app.track_pane_card();
        assert_eq!(app.camera.reveal, None, "only when it changes");
        app.interaction.selected.insert(CardId::from(b));
        app.track_pane_card();
        assert_eq!((app.pane_card(), app.camera.reveal), (None, None), "two cards: nothing to reveal");
        app.interaction.selected.clear();
        app.track_pane_card();
        app.interaction.selected.insert(CardId::from(b));
        app.track_pane_card();
        assert_eq!((app.pane_card(), app.camera.reveal), (Some(b), Some(b)), "back to one card: revealed");
    }

    /// The Labels section lists every label with how much of the selection carries it.
    #[test]
    fn the_label_rows_show_how_much_of_the_selection_carries_each() {
        use crate::labels::LabelState;
        let mut app = App::default();
        let a = crate::session_sync::tests::add_note(&mut app, "a");
        let b = crate::session_sync::tests::add_note(&mut app, "b");
        let urgent = app.create_label("urgent", None).unwrap();
        let later = app.create_label("later", None).unwrap();
        app.interaction.selected.extend([CardId::from(a), CardId::from(b)]);
        app.label_selection(urgent, true);
        app.interaction.selected.remove(&CardId::from(b));
        let states = |app: &App| -> Vec<(NodeId, LabelState)> {
            app.pane_label_rows().into_iter().map(|(id, _, _, state)| (id, state)).collect()
        };
        assert!(states(&app).contains(&(urgent, LabelState::All)));
        app.interaction.selected.insert(CardId::from(b));
        app.set_label(&[b], urgent, false);
        assert!(states(&app).contains(&(urgent, LabelState::Some)));
        assert!(states(&app).contains(&(later, LabelState::None)));
    }

    /// The Text section: a note's text, a page's stored text, else nothing.
    #[test]
    fn the_pane_text_is_a_notes_text_or_a_pages_stored_text() {
        use crate::session_sync::tests::sleeping_tab;
        use crate::store::model::Kind;
        let mut app = App::default();
        let note = crate::session_sync::tests::add_note(&mut app, "milk and eggs");
        assert_eq!(app.pane_text(note), Some("milk and eggs"));
        let tab = app.alloc_node_id();
        app.insert_placeholder_node(tab, egui::Vec2::ZERO, Kind::Tab, sleeping_tab("https://a.test/", "a"));
        assert_eq!(app.pane_text(tab), None);
        app.search.texts.put(crate::search::text_store::TextRecord {
            id: tab,
            url: "https://a.test/".into(),
            title: "a".into(),
            text: "page words".into(),
            at: 0,
        });
        assert_eq!(app.pane_text(tab), Some("page words"));
    }

    /// One rename draft: the pane shows its field unless the card's menu is open on the card (the menu shows it).
    #[test]
    fn the_pane_shows_the_rename_field_only_when_the_menu_does_not() {
        let mut app = App::default();
        let note = crate::session_sync::tests::add_note(&mut app, "n");
        assert!(!app.pane_renaming(note));
        app.apply_node_action(&egui::Context::default(), note, crate::node_menu::NodeAction::Rename);
        assert!(app.pane_renaming(note));
        app.menus.node_menu.open(note, egui::Pos2::ZERO);
        assert!(!app.pane_renaming(note), "the menu's field has it");
    }

    /// R5: opening the pane takes no focus, and Down with nothing focused stays the graph's.
    #[test]
    fn the_pane_takes_no_focus_when_it_opens() {
        let mut app = App::default();
        let group = app.alloc_node_id();
        app.insert_group_at(group, None);
        app.create_label("urgent", None).unwrap();
        app.interaction.selected.insert(CardId::from(group));
        let ctx = crate::theme::testutil::ctx(true);
        let mut down_left = false;
        for events in [
            vec![],
            vec![],
            vec![egui::Event::Key {
                key: egui::Key::ArrowDown,
                physical_key: Some(egui::Key::ArrowDown),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        ] {
            let mut actions = Vec::new();
            let input = egui::RawInput { events, ..Default::default() };
            let mut output = ctx.run_ui(input, |ui| {
                app.build_right_panel(ui, &mut actions);
                down_left = ui.input(|i| i.key_pressed(egui::Key::ArrowDown));
            });
            output.textures_delta.clear();
        }
        assert_eq!(ctx.memory(|m| m.focused()), None);
        assert!(down_left, "the pane left Down to the graph");
    }

    /// Review I2: a label rename takes the keys once; a click away cancels it and leaves the keys to the graph.
    #[test]
    fn a_click_away_cancels_a_label_rename() {
        let mut app = App::default();
        let a = crate::session_sync::tests::add_note(&mut app, "a");
        let urgent = app.create_label("urgent", None).unwrap();
        app.interaction.selected.insert(CardId::from(a));
        let ctx = crate::theme::testutil::ctx(true);
        let frame = |app: &mut App, events: Vec<egui::Event>| {
            let mut actions = Vec::new();
            let input = egui::RawInput { events, ..Default::default() };
            let mut output = ctx.run_ui(input, |ui| app.build_right_panel(ui, &mut actions));
            output.textures_delta.clear(); // nothing uploads the font atlas here
            actions
        };
        frame(&mut app, vec![]);
        app.interaction.label_section.start_editing(urgent, "urgent");
        frame(&mut app, vec![]);
        frame(&mut app, vec![]);
        assert!(ctx.memory(|m| m.focused()).is_some(), "the field took the keys");
        let at = egui::pos2(10.0, 10.0); // the canvas side, left of the panel
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&mut app, vec![egui::Event::PointerMoved(at), button(true)]);
        let actions = frame(&mut app, vec![button(false)]);
        frame(&mut app, vec![]);
        assert!(app.interaction.label_section.editing.is_none(), "the click cancelled it");
        assert!(!actions.iter().any(|a| matches!(a, GraphAction::RenameLabel { .. })), "without renaming");
        assert_eq!(ctx.memory(|m| m.focused()), None, "and the field did not take the keys back");
    }

    /// A swatch click during a label rename recolours the label and keeps the rename and its draft. egui surrenders
    /// the field's focus on the click's release, when the press is already over.
    #[test]
    fn a_swatch_click_keeps_a_label_rename() {
        let mut app = App::default();
        let a = crate::session_sync::tests::add_note(&mut app, "a");
        let urgent = app.create_label("urgent", None).unwrap();
        app.interaction.selected.insert(CardId::from(a));
        let ctx = crate::theme::testutil::ctx(true);
        let frame = |app: &mut App, events: Vec<egui::Event>| {
            let mut actions = Vec::new();
            let input = egui::RawInput { events, ..Default::default() };
            let mut output = ctx.run_ui(input, |ui| app.build_right_panel(ui, &mut actions));
            output.textures_delta.clear(); // nothing uploads the font atlas here
            actions
        };
        frame(&mut app, vec![]);
        app.interaction.label_section.start_editing(urgent, "urgent");
        frame(&mut app, vec![]);
        frame(&mut app, vec![egui::Event::Text("x".into())]);
        let field = ctx.memory(|m| m.focused()).expect("the field took the keys");
        let below = ctx.read_response(field).unwrap().rect.bottom();
        // The first swatch: the first small square widget under the field.
        let swatch = ctx.viewport(|v| {
            v.prev_pass
                .widgets
                .layers()
                .flat_map(|(_, rects)| rects)
                .map(|w| w.rect)
                .find(|r| r.top() > below && (r.width() - r.height()).abs() < 0.5 && r.width() < 30.0)
        });
        let at = swatch.expect("a swatch under the field").center();
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&mut app, vec![egui::Event::PointerMoved(at)]);
        frame(&mut app, vec![button(true)]);
        let actions = frame(&mut app, vec![button(false)]);
        frame(&mut app, vec![]);
        assert!(actions.iter().any(|a| matches!(a, GraphAction::RecolorLabel { .. })), "the swatch recoloured");
        assert_eq!(
            app.interaction.label_section.editing.as_ref().map(|(id, draft)| (*id, draft.as_str())),
            Some((urgent, "urgentx")),
            "the rename and its draft stay"
        );
        assert_eq!(ctx.memory(|m| m.focused()), Some(field), "and the field has the keys back");
    }

    /// A label rename in progress is dropped with the pane, not carried to the next selection.
    #[test]
    fn the_label_section_resets_when_the_pane_closes() {
        let mut app = App::default();
        let a = crate::session_sync::tests::add_note(&mut app, "a");
        let urgent = app.create_label("urgent", None).unwrap();
        app.interaction.selected.insert(CardId::from(a));
        app.interaction.label_section.editing = Some((urgent, "half-typed".into()));
        app.interaction.label_section.draft = "new".into();
        let ctx = crate::theme::testutil::ctx(true);
        let frame = |app: &mut App| {
            let mut actions = Vec::new();
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| app.build_right_panel(ui, &mut actions));
            output.textures_delta.clear(); // nothing uploads the font atlas here
        };
        frame(&mut app);
        assert!(app.interaction.label_section.editing.is_some(), "kept while the pane shows");
        app.interaction.selected.clear();
        frame(&mut app);
        assert!(app.interaction.label_section.editing.is_none() && app.interaction.label_section.draft.is_empty());
    }
}
