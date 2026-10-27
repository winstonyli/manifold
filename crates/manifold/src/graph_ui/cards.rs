//! The Graph view's cards: drawing them, a virtual card's click, the new-node menu and the hover peek.

use super::*;
use crate::card_kind::kind_caps;
use crate::store::model::Kind;

impl App {
    /// Every card at this level, through the shared frame
    /// (`draw_graph_card`), plus each kind's own click action. Runs inside
    /// `build_graph_ui`'s `Scene`, after the grid and edges, so cards paint
    /// on top of them.
    #[allow(
        clippy::too_many_arguments,
        reason = "one call site, in `build_graph_ui`'s Scene closure: these are that frame's locals, and a struct would only rename them"
    )]
    pub(crate) fn draw_graph_cards(
        &mut self,
        ui: &mut egui::Ui,
        deferred_overlays: &mut DeferredOverlays,
        actions: &mut Vec<GraphAction>,
        at_level: &[Vec<NodeId>],
        inv_zoom: f32,
        zoom_scale: f32,
        leave_group_button_rect: Option<egui::Rect>,
        connect_target: Option<NodeId>,
    ) {
        // One drop target per frame, shared by the ring and the
        // release (see `drop_target`).
        let pointer = ui.input(|i| i.pointer.interact_pos());
        let drop_preview =
            self.interaction.dragging.and_then(|dragged| self.drop_target(dragged, pointer, leave_group_button_rect));
        // The ring says where a drop would land; this says what it would do, beside the pointer.
        if let (Some((effect, target)), Some(at)) = (&drop_preview, pointer) {
            let name = target.and_then(|id| id.node()).map(|node| self.drop_target_name(node)).unwrap_or_default();
            paint_drop_hint(ui.ctx(), at, &effect.hint(&name));
        }

        // Every card: one shared frame each (`draw_graph_card`),
        // layer by layer (`level_scan`, groups last), which is
        // also their stacking order: a later layer draws, and takes
        // the pointer, on top of an earlier one. Each kind's own
        // click action is applied here.
        let tab_order = self.card_tab_order();
        self.graph.card_hint = None;
        if let Some(rect) = card_tab_step(ui, &tab_order, |id| self.level_card_rect(id)) {
            self.camera.pan_to(rect);
        }
        self.handle_card_digits(ui, &tab_order);
        self.handle_undo_keys(ui);
        self.graph.hints_revealed = ctrl_hints_revealed(ui);
        let members = self.group_members();
        // Pick-target mode: the cards a click would connect to, ringed.
        let pick_targets: Option<Vec<CardId>> =
            self.link.connecting.as_ref().map(|(from, edge)| {
                self.valid_targets(edge, *from).iter().map(|&target| CardId::from(target)).collect()
            });
        let mut pass = CardPass {
            deferred_overlays,
            actions,
            inv_zoom,
            zoom_scale,
            drop_preview: &drop_preview,
            pick_targets: pick_targets.as_deref().unwrap_or_default(),
            connect_target: connect_target.map(CardId::from),
            connect_ring: self
                .link
                .retarget_preview
                .map(|outcome| crate::link_gestures::retarget_look(outcome, ui.visuals()).ring),
            show_order: self.graph.card_nav_active || self.graph.hints_revealed,
            order: &tab_order,
            any_focused: false,
            members: &members,
            viewport: self.camera.rect,
            pan_target: None,
            carry: true,
        };
        for entry in self.level_items_from(at_level) {
            let kind = entry.kind;
            if let Origin::Virtual(_) = entry.origin {
                self.virtual_card_click(ui, &mut pass, &entry, pick_targets.is_some());
                continue;
            }
            let Some(id) = entry.id.node() else { continue };
            // Read before the frame: a Note's `TextEdit` can lose
            // focus to a click on its own header this very frame,
            // clearing `editing`, and that click must not
            // immediately re-enter editing.
            let note_was_editing = self.nodes.get(&id).and_then(|n| n.as_note()).is_some_and(|n| n.editing);
            let Some(card) = self.draw_graph_card(ui, &mut pass, &entry) else {
                continue;
            };
            if let Some(action) = card.control {
                // A control consumed this frame's click, so
                // the kind's own click action below (e.g.
                // Image maximize) is skipped.
                pass.actions.push(GraphAction::Node(id, action));
                continue;
            }
            // Pick-target mode takes the card's click: a valid target finishes it, any other card ends it (as every
            // click does, `build_graph_ui`), and neither does the card's own click action.
            if pick_targets.is_some() {
                if card.response.clicked() && pass.pick_targets.contains(&entry.id) {
                    pass.actions.push(GraphAction::FinishConnect(id));
                }
                continue;
            }
            // Ctrl-click on a card toggles it in the selection and opens nothing.
            if card.response.clicked() && ui.input(|i| i.modifiers.ctrl) {
                self.toggle_selected(entry.id);
                continue;
            }
            // A placeholder (see `ImagePayload::error`) has nothing
            // to open; a click or Enter tries its file again instead.
            let missing_file = self.nodes.get(&id).is_some_and(|n| n.file_failed());
            // A double-click, or Enter on the keyboard-focused card: what opens a card. A plain click only selects it
            // (below); a double-click's first click selects it too, so nothing waits on the second.
            let activated = card.response.double_clicked()
                || (card.response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
            // Click (not drag -- egui doesn't report `clicked()` for a drag).
            if card.response.clicked() {
                self.select_only(entry.id);
            }
            if missing_file {
                // egui reports Enter on the focused card as a click too.
                if card.response.clicked() {
                    pass.actions.push(GraphAction::Node(id, NodeAction::RetryMissingFile));
                }
            } else if let Some(click) = kind.caps().click.filter(|_| activated) {
                match click {
                    Click::Open | Click::Reveal => {
                        pass.actions.extend(click_action(kind).map(|a| GraphAction::Node(id, a)))
                    }
                    // Click on the rendered note enters editing in place.
                    Click::Edit if !note_was_editing => {
                        if let Some(note) = self.nodes.get_mut(&id).and_then(|n| n.as_note_mut()) {
                            note.editing = true;
                        }
                        // The editor appears next frame; focus it so typing can start at once.
                        ui.memory_mut(|m| m.request_focus(entry.id.editor()));
                    }
                    // Enters the group's own subgraph or the JSON node's level, unless its file is still being
                    // refetched (`JsonPayload::enterable`).
                    Click::Enter if self.nodes.get(&id).and_then(|n| n.as_json()).is_none_or(|j| j.enterable()) => {
                        // A JSON level entered from the keyboard: its root card takes focus (`draw_json_level`),
                        // sparing the toolbar's ~17 Tab stops.
                        if kind.ty == TYPE_JSON && !card.response.double_clicked() {
                            ui.data_mut(|d| d.insert_temp(crate::json_level_ui::json_focus_root_key(), true));
                        }
                        pass.actions.push(GraphAction::EnterGroup(id))
                    }
                    Click::Edit | Click::Enter => {}
                }
            }
        }
        if let Some(rect) = pass.pan_target {
            self.camera.pan_to(rect);
        }
        self.graph.card_nav_active = pass.any_focused;
        self.draw_connect_rubber_band(ui, inv_zoom, connect_target);
    }

    /// A virtual card's frame, then its click: a click in pick-target mode only ends it (as every click does,
    /// `build_graph_ui`); a Ctrl-click toggles a `selectable` kind in the selection and opens nothing; any other
    /// click goes to the source's `on_click`, whose action is applied like a stored card's.
    /// Returns the card's response, `None` when it was culled.
    pub(crate) fn virtual_card_click(
        &mut self,
        ui: &mut egui::Ui,
        pass: &mut CardPass,
        entry: &CardEntry,
        picking: bool,
    ) -> Option<egui::Response> {
        let card = self.draw_graph_card(ui, pass, entry)?;
        if picking || !card.response.clicked() {
            return Some(card.response);
        }
        if ui.input(|i| i.modifiers.ctrl) && entry.caps().selectable {
            self.toggle_selected(entry.id);
        } else if let Some(source) = self.entry_source(entry) {
            pass.actions.extend(source.on_click(self, entry));
        }
        Some(card.response)
    }

    /// World rect of Tab-order card `id`: a stored node's, or a virtual entry's.
    pub(crate) fn level_card_rect(&self, id: CardId) -> Option<egui::Rect> {
        id.node().and_then(|node| self.card_rect(node)).or_else(|| {
            self.virtual_sources.iter().flat_map(|source| source.entries(self)).find(|e| e.id == id).map(|e| e.rect)
        })
    }

    /// The empty-canvas "New Tab/Note/Group/File" menu -- see `pending_new_node`'s own doc comment for what
    /// spawns/dismisses it. A screen-space `Foreground` `Area` at the clicked point, like every other context menu:
    /// drawn inside the Scene it sat under the cards' overlay text and chips, and its label size followed the zoom.
    pub(crate) fn draw_pending_new_node_menu(
        &mut self,
        ui: &mut egui::Ui,
        actions: &mut Vec<GraphAction>,
        project: &impl Fn(egui::Pos2) -> egui::Pos2,
    ) {
        let Some(pos) = self.interaction.pending_new_node else { return };
        let screen_pos = project(egui::Pos2::new(pos.x, pos.y));
        let mut chosen = None;
        let id = egui::Id::new("pending_new_node_menu");
        // The right-click that opens the menu is still this frame's click: it must not dismiss it.
        let opened_now = ui.ctx().memory(|m| m.area_rect(id)).is_none();
        let area = menu_popup(ui.ctx(), egui::Area::new(id).fixed_pos(screen_pos), 1.0, |ui| {
            ui.set_min_width(150.0);
            let tab = ui.button(format!("{} New Tab", egui_phosphor::light::PLUS)).clicked();
            let note = ui.button(format!("{} New Note", kind_caps(Kind::Note).glyph)).clicked();
            let group = ui.button(format!("{} New Group", egui_phosphor::light::FOLDER_PLUS)).clicked();
            let json = ui.button(format!("{} New JSON", kind_caps(Kind::Json).glyph)).clicked();
            // A real picker blocks on OS/human input, which headless mode has neither of.
            let file =
                !self.headless && ui.button(format!("{} Add File...", egui_phosphor::light::FILE_PLUS)).clicked();
            chosen = tab
                .then_some(GraphAction::NewTab(self.focused))
                .or(note.then_some(GraphAction::NewNote(pos)))
                .or(group.then_some(GraphAction::NewGroup(pos)))
                .or(json.then_some(GraphAction::NewJson(pos)))
                .or(file.then_some(GraphAction::NewFile(pos)));
        });
        let dismissed = chosen.is_some() || !opened_now && clicked_outside_popup(ui, area.rect, None);
        actions.extend(chosen);
        if dismissed {
            self.interaction.pending_new_node = None;
        }
    }

    /// Peek preview: an enlarged view of the hovered node's cached
    /// thumbnail, floating near it, without changing focus. Queued as a
    /// deferred overlay (not a separate `egui::Area`) so it is projected
    /// with the frame's final pan/zoom and paints after every card's
    /// overlay text and chips (`draw_graph_cards`), which would
    /// otherwise cover it. Skipped while `peek_suppressed` matches -- an
    /// Escape press dismissing this exact node's peek early,
    /// see that field's own doc comment.
    pub(crate) fn draw_peek_preview(&mut self, ui: &mut egui::Ui, deferred_overlays: &mut DeferredOverlays) {
        if self.interaction.peek_suppressed.is_some() && self.interaction.peek_suppressed != self.interaction.hovered {
            self.interaction.peek_suppressed = None;
        }
        let peek_target = self.interaction.hovered.filter(|id| self.interaction.peek_suppressed != Some(*id));
        // Polled every frame regardless of `peek_target`, as the dropdown popups' fades are: polling only
        // while a peek shows would let the elapsed time since the last poll make it jump to fully visible
        // when it reappears instead of fading in.
        let peek_fade = crate::theme::animate(
            ui.ctx(),
            egui::Id::new("peek_preview_fade"),
            peek_target.is_some(),
            egui::emath::easing::linear,
        );
        if peek_fade > 0.0 {
            if let Some(hovered_id) = peek_target {
                let key = hovered_id.node();
                let node_pos = key.filter(|key| self.nodes.contains_key(key)).map(|key| self.layout_pos(key));
                let thumb_id = key.and_then(|key| self.gpu.as_ref()?.thumbnail_id(key));
                if let (Some(node_pos), Some(tex_id)) = (node_pos, thumb_id) {
                    let rect = egui::Rect::from_min_size(
                        egui::Pos2::new(
                            node_pos.x + GRAPH_NODE_SIZE.x / 2.0 + 12.0,
                            node_pos.y - GRAPH_PEEK_SIZE.y / 2.0,
                        ),
                        GRAPH_PEEK_SIZE,
                    );
                    // Queued, not painted here: card labels and chips paint on the overlay layer after the
                    // scene, so a peek drawn in the scene would sit under a neighbour's text.
                    let bg = ui.visuals().extreme_bg_color.gamma_multiply(peek_fade);
                    let ring = accent(ui.visuals()).gamma_multiply(peek_fade);
                    deferred_overlays.push(Box::new(move |painter, project| {
                        let (min, max) = (project(rect.min), project(rect.max));
                        let rounding = CARD_RADIUS * (max.x - min.x) / rect.width();
                        // Slid back inside the graph panel when the node is near its edge.
                        let view = painter.clip_rect();
                        let shift = (view.max - max).min(egui::Vec2::ZERO) + (view.min - min).max(egui::Vec2::ZERO);
                        let rect = egui::Rect::from_min_max(min + shift, max + shift);
                        painter.rect_filled(rect, rounding, bg);
                        crate::card::rounded_image(
                            painter,
                            tex_id,
                            rect,
                            crate::card::FULL_UV,
                            egui::Color32::WHITE.gamma_multiply(peek_fade),
                            rounding,
                        );
                        painter.rect_stroke(rect, rounding, egui::Stroke::new(1.5, ring), egui::StrokeKind::Inside);
                    }));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card_id::CardId;

    fn frame(ctx: &egui::Context, app: &mut App, events: Vec<egui::Event>) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024.0, 768.0))),
            events,
            ..Default::default()
        };
        ctx.run_ui(input, |ui| app.build_graph_ui(ui)).textures_delta.clear();
    }

    /// Enter on a keyboard-focused card whose file failed retries it, as a click on it does.
    #[test]
    fn enter_on_a_failed_image_card_retries_it() {
        let ctx = crate::theme::testutil::ctx(true);
        let mut app = App::default();
        let id = app.alloc_node_id();
        let image = ImagePayload { path: "gone.png".into(), width: 0, height: 0, error: Some("missing".into()) };
        app.insert_placeholder_node(id, egui::Vec2::ZERO, Kind::Image, Runtime::Image(image));
        for _ in 0..3 {
            frame(&ctx, &mut app, vec![]);
        }
        app.camera.jump_to(app.fit_all_nodes_scene_rect());
        frame(&ctx, &mut app, vec![]);
        ctx.memory_mut(|m| m.request_focus(CardId::from(id).widget()));
        frame(&ctx, &mut app, vec![]);
        assert!(app.nodes[&id].file_failed());
        let enter = |pressed| egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&ctx, &mut app, vec![enter(true), enter(false)]);
        assert!(!app.nodes[&id].file_failed(), "Enter retried the image: its error was taken");
    }
}
