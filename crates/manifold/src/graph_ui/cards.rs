//! The Graph view's cards: drawing them, a virtual card's click, the new-node menu and the hover peek.

use super::*;

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
        self.card_hint = None;
        if let Some(rect) = card_tab_step(ui, &tab_order, |id| self.level_card_rect(id)) {
            self.camera.pan_to(rect);
        }
        self.handle_card_digits(ui, &tab_order);
        self.handle_undo_keys(ui);
        self.hints_revealed = {
            let (held, other_key) = ui.input(|i| {
                (
                    i.modifiers.ctrl && !i.modifiers.alt && !i.modifiers.shift,
                    i.events.iter().any(|e| matches!(e, egui::Event::Key { pressed: true, .. })),
                )
            });
            let now = ui.input(|i| i.time);
            let state: CtrlReveal = ui.data(|d| d.get_temp(egui::Id::new("ctrl_reveal"))).unwrap_or_default();
            let (state, shown) = ctrl_reveal_step(state, held, other_key, now);
            ui.data_mut(|d| d.insert_temp(egui::Id::new("ctrl_reveal"), state));
            if state.since.is_some() && !shown {
                ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(CTRL_REVEAL_AFTER));
            }
            shown
        };
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
            show_order: self.card_nav_active || self.hints_revealed,
            order: &tab_order,
            any_focused: false,
            members: &members,
            viewport: self.camera.rect,
            pan_target: None,
            carry: true,
        };
        for entry in self.level_items_from(at_level) {
            let kind = entry.kind;
            if let Origin::Virtual(name) = entry.origin {
                self.virtual_card_click(ui, &mut pass, &entry, name, pick_targets.is_some());
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
            // to open; a click tries its file again instead.
            let missing_file = self.nodes.get(&id).is_some_and(|n| {
                n.as_image().is_some_and(|i| i.error.is_some())
                    || n.as_unloaded().is_some_and(|t| matches!(t.state, UnloadedState::Failed(_)))
                    || n.as_json().is_some_and(|j| j.error.is_some())
                    || n.as_pdf().is_some_and(|p| matches!(p.outcome, Some(Err(_))))
            });
            // A double-click, or Enter on the keyboard-focused card: what opens a card. A plain click only selects it
            // (below); a double-click's first click selects it too, so nothing waits on the second.
            let activated = card.response.double_clicked()
                || (card.response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
            // Click (not drag -- egui doesn't report `clicked()` for a drag).
            if card.response.clicked() {
                self.select_only(entry.id);
            }
            let click = activated.then(|| click_action(kind)).flatten();
            match (kind.ty, click) {
                _ if missing_file => {
                    if card.response.clicked() {
                        pass.actions.push(GraphAction::Node(id, NodeAction::RetryMissingFile));
                    }
                }
                (_, Some(action)) => pass.actions.push(GraphAction::Node(id, action)),
                // Click on the rendered note enters editing in place.
                (TYPE_NOTE, _) if activated && !note_was_editing => {
                    if let Some(note) = self.nodes.get_mut(&id).and_then(|n| n.as_note_mut()) {
                        note.editing = true;
                    }
                    // The editor appears next frame; focus it so typing can start at once.
                    ui.memory_mut(|m| m.request_focus(entry.id.editor()));
                }
                // Enters the node's JSON level, unless its file failed to
                // load (plan ruling 10) or it's still being refetched (`JsonPayload::enterable`).
                (TYPE_JSON, _)
                    if activated && self.nodes.get(&id).and_then(|n| n.as_json()).is_some_and(|j| j.enterable()) =>
                {
                    // Entered from the keyboard: the level's root card takes focus
                    // (`draw_json_level`), sparing the toolbar's ~17 Tab stops.
                    if !card.response.double_clicked() {
                        ui.data_mut(|d| d.insert_temp(crate::json_level_ui::json_focus_root_key(), true));
                    }
                    pass.actions.push(GraphAction::EnterGroup(id))
                }
                // Drills into the group's own subgraph.
                (ty, _) if is_container(ty) && activated => pass.actions.push(GraphAction::EnterGroup(id)),
                _ => {}
            }
        }
        if let Some(rect) = pass.pan_target {
            self.camera.pan_to(rect);
        }
        self.card_nav_active = pass.any_focused;
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
        name: &str,
        picking: bool,
    ) -> Option<egui::Response> {
        let card = self.draw_graph_card(ui, pass, entry)?;
        if picking || !card.response.clicked() {
            return Some(card.response);
        }
        if ui.input(|i| i.modifiers.ctrl) && entry.caps().selectable {
            self.toggle_selected(entry.id);
        } else if let Some(source) = self.virtual_source(name) {
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
        let (mut tab, mut note, mut group, mut file) = (false, false, false, false);
        let id = egui::Id::new("pending_new_node_menu");
        // The right-click that opens the menu is still this frame's click: it must not dismiss it.
        let opened_now = ui.ctx().memory(|m| m.area_rect(id)).is_none();
        let area = menu_popup(ui.ctx(), egui::Area::new(id).fixed_pos(screen_pos), 1.0, |ui| {
            ui.set_min_width(150.0);
            tab = ui.button(format!("{} New Tab", egui_phosphor::light::PLUS)).clicked();
            note = ui.button(format!("{} New Note", egui_phosphor::light::NOTE_PENCIL)).clicked();
            group = ui.button(format!("{} New Group", egui_phosphor::light::FOLDER_PLUS)).clicked();
            // A real picker blocks on OS/human input, which headless mode has neither of.
            if !self.headless {
                file = ui.button(format!("{} Add File...", egui_phosphor::light::FILE_PLUS)).clicked();
            }
        });
        if tab {
            actions.push(GraphAction::NewTab(self.focused));
        } else if note {
            actions.push(GraphAction::NewNote(pos));
        } else if group {
            actions.push(GraphAction::NewGroup(pos));
        } else if file {
            actions.push(GraphAction::NewFile(pos));
        }
        if tab || note || group || file || !opened_now && clicked_outside_popup(ui, area.rect, None) {
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
        // Polled every frame regardless of `peek_target`, same
        // "continuous polling, not just while shown" reasoning the
        // dropdown popups' own fade needs -- a plain `if let Some`
        // gate here would only ever call `animate_bool` while a
        // peek is actually showing, and the real-elapsed-time gap
        // since the last poll (however long the pointer sat
        // elsewhere) would make it jump straight to fully visible
        // the instant it reappears instead of visibly fading in.
        let peek_fade = ui.ctx().animate_bool(egui::Id::new("peek_preview_fade"), peek_target.is_some());
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
                        painter.image(
                            tex_id,
                            rect,
                            crate::card::FULL_UV,
                            egui::Color32::WHITE.gamma_multiply(peek_fade),
                        );
                        painter.rect_stroke(rect, rounding, egui::Stroke::new(1.5, ring), egui::StrokeKind::Inside);
                    }));
                }
            }
        }
    }
}
