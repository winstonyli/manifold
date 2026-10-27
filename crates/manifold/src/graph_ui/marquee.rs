//! The Graph view's right button: rectangle-select on empty space, the card menus on a card.

use super::*;

impl App {
    /// Rectangle-select: a right-button drag over the graph panel
    /// selects every tab node its screen-space rectangle overlaps.
    /// Deliberately the secondary button, not primary -- primary is
    /// Scene's own pan gesture, and `.drag_pan_buttons(PRIMARY)` in `build_graph_ui`
    /// restricts Scene to just that button so the two never compete for
    /// the same drag. Read directly off `ui.input`'s raw pointer state
    /// rather than through a `Response`: this needs to work anywhere
    /// over the panel, including empty space no widget claims, and
    /// egui's own hit-testing/consumption (which a `Response` reflects)
    /// is irrelevant to a plain "where is the mouse" query.
    pub(crate) fn handle_rect_select(
        &mut self,
        ui: &egui::Ui,
        overlay_painter: &egui::Painter,
        scene_outer_rect: egui::Rect,
        transform: egui::emath::TSTransform,
    ) {
        let to_world = transform.inverse();
        let secondary_down = ui.input(|i| i.pointer.button_down(egui::PointerButton::Secondary));
        let pointer_pos = ui.input(|i| i.pointer.interact_pos());
        // Not while a card drag or a connect-drag holds the primary button: a right-press then would open a menu
        // under the drag (whose release still drops or links) or start a marquee beside it.
        let secondary_pressed = ui.input(|i| i.pointer.button_pressed(egui::PointerButton::Secondary))
            && self.interaction.dragging.is_none()
            && self.link.connect_drag.is_none();
        if secondary_pressed {
            // A fresh right-click press always cancels any still-open
            // "New Tab/Note/Group/File" card from a previous one -- same
            // "starting a new gesture dismisses the old one" rule every
            // other popup here already follows.
            self.interaction.pending_new_node = None;
            if let Some(pos) = pointer_pos.filter(|p| scene_outer_rect.contains(*p)) {
                self.right_press(ui, pos, to_world * pos);
            }
        }
        if let Some(start) = self.interaction.rect_select_start {
            let current = pointer_pos.unwrap_or(start);
            let screen_rect = egui::Rect::from_two_pos(start, current);
            let accent = accent(ui.visuals());
            overlay_painter.rect_filled(screen_rect, 0.0, with_alpha(accent, 35));
            overlay_painter.rect_stroke(screen_rect, 0.0, egui::Stroke::new(1.5, accent), egui::StrokeKind::Inside);
            if !secondary_down {
                self.interaction.rect_select_start = None;
                // Barely moved -- a plain right-click, not a real
                // rectangle-select drag. The level items'
                // hit-testing above already ruled out landing on any real
                // node, so this is genuinely empty background: show the
                // "New Tab/Note/Group/File" card here instead of computing
                // a select-nothing zero-area marquee.
                if (current - start).length() < 4.0 {
                    // A JSON level is read-only: no creation card there.
                    if self.json_level.is_none() {
                        let world_pos = to_world * start;
                        self.interaction.pending_new_node = Some(egui::Vec2::new(world_pos.x, world_pos.y));
                    }
                } else {
                    let world_rect = to_world * screen_rect;
                    self.interaction.selected = self
                        .level_items()
                        .into_iter()
                        .filter(|item| item.caps().selectable && world_rect.intersects(item.rect))
                        .map(|item| item.id)
                        .collect();
                }
            }
        }
    }

    /// A right-press at `pos` (`world_pos` in the scene): opens the menu of the card under it, or starts a marquee on
    /// empty space.
    fn right_press(&mut self, ui: &egui::Ui, pos: egui::Pos2, world_pos: egui::Pos2) {
        // A right-press landing on any card opens that card's
        // context menu instead of starting a marquee -- groups
        // specifically aren't marquee-selectable (only tabs are,
        // see `nearest_tab_in_direction`'s own doc comment on why
        // `self.focused` stays tab-only), so a right-click on one
        // would otherwise just compute an empty selection.
        // Reverse draw order, `level_items`, so the card on top
        // takes the right-click, the same card a left-click there
        // would reach.
        let hit = self.level_items().into_iter().rev().find(|item| item.rect.contains(world_pos));
        match hit {
            Some(CardEntry { id, origin: Origin::Stored, .. }) => {
                // A right-click on the jar chip opens the menu on its Jar page.
                if let Some(key) = id.node() {
                    let on_chip = crate::card::on_jar_chip(ui.ctx(), key, pos);
                    self.menus.node_menu.open(key, pos);
                    if on_chip {
                        self.menus.node_menu.show_page(Some(crate::node_menu::MenuPage::Jar));
                    }
                }
                self.interaction.rect_select_start = None;
            }
            // A virtual card has no node menu and no jar chip: its source opens whatever menu the kind's
            // `menu` ability grants.
            Some(entry @ CardEntry { origin: Origin::Virtual(_), .. }) => {
                self.interaction.rect_select_start = None;
                if entry.caps().menu {
                    if let Some(source) = self.entry_source(&entry) {
                        source.on_menu(self, &entry, pos);
                    }
                }
            }
            Some(CardEntry { origin: Origin::Json, .. }) => {
                self.interaction.rect_select_start = None;
                let target = self.json_level.as_ref().and_then(|level| {
                    let i = level.card_at(world_pos)?;
                    Some(level.row_at(i, world_pos).unwrap_or_else(|| level.cards[i].pointer.clone()))
                });
                if let Some(target) = target {
                    self.open_json_menu(target, pos);
                }
            }
            None => {
                self.menus.node_menu.close();
                if let Some(level) = self.json_level.as_mut() {
                    level.menu = None;
                }
                self.interaction.rect_select_start = Some(pos);
            }
        }
    }
}
