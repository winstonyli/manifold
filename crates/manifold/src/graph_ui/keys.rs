//! The Graph view's keyboard: arrow and Tab navigation, digit jumps, undo and redo and the Ctrl reveal.

use super::*;

/// Tab / Shift+Tab from a focused card moves focus to the next / previous card in `order` and returns that card's
/// rect (`rect_of`) for the caller to pan to, since it may be culled. Past either end Tab is left to egui, which moves
/// on to the next widget outside the cards.
pub(crate) fn card_tab_step(
    ui: &egui::Ui,
    order: &[CardId],
    rect_of: impl Fn(CardId) -> Option<egui::Rect>,
) -> Option<egui::Rect> {
    let card_id = |id: &CardId| id.widget();
    let at = order.iter().position(|id| ui.memory(|m| m.has_focus(card_id(id))))?;
    // The shift state comes from the key event itself, which is also how the debug `TABKEY shift` sends it.
    let (tab, shift) = ui.input(|i| {
        let shifted = i.events.iter().any(
            |e| matches!(e, egui::Event::Key { key: egui::Key::Tab, pressed: true, modifiers, .. } if modifiers.shift),
        );
        (i.key_pressed(egui::Key::Tab), shifted)
    });
    let target = if shift { at.checked_sub(1) } else { Some(at + 1).filter(|&n| n < order.len()) };
    ui.memory_mut(|m| {
        m.set_focus_lock_filter(card_id(&order[at]), egui::EventFilter { tab: target.is_some(), ..Default::default() })
    });
    let target = target.filter(|_| tab)?;
    ui.memory_mut(|m| m.request_focus(card_id(&order[target])));
    rect_of(order[target])
}

/// Ctrl held alone this long reveals the key hints.
const CTRL_REVEAL_AFTER: f64 = 0.35;

/// The hold-Ctrl-to-reveal state: when a solo Ctrl hold began, and whether another key was pressed
/// during it (Ctrl+C and the like), which cancels the reveal until Ctrl is released.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
struct CtrlReveal {
    since: Option<f64>,
    cancelled: bool,
}

/// One frame of the reveal: `held` is Ctrl alone down, `other_key` a key press this frame. Returns the
/// new state and whether the hints show.
fn ctrl_reveal_step(mut state: CtrlReveal, held: bool, other_key: bool, now: f64) -> (CtrlReveal, bool) {
    if !held {
        return (CtrlReveal::default(), false);
    }
    let since = *state.since.get_or_insert(now);
    state.cancelled |= other_key;
    (state, !state.cancelled && now - since >= CTRL_REVEAL_AFTER)
}

/// Runs one frame of the reveal on this frame's input, keeping its state in `ui.data`, and returns whether the
/// hints show. Asks for a repaint when a hold is under way, to show them once `CTRL_REVEAL_AFTER` passes.
pub(super) fn ctrl_hints_revealed(ui: &egui::Ui) -> bool {
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
}

/// The undo chord pressed this frame, consumed: `Some(false)` for Ctrl+Z, `Some(true)` for Ctrl+Shift+Z or Ctrl+Y.
/// Shift is tried first, as the plain chord would match it too.
fn undo_chord(i: &mut egui::InputState) -> Option<bool> {
    use egui::{Key, Modifiers};
    if i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z) || i.consume_key(Modifiers::COMMAND, Key::Y) {
        Some(true)
    } else {
        i.consume_key(Modifiers::COMMAND, Key::Z).then_some(false)
    }
}

/// How long a digit waits for a second one before the number is taken as typed.
const DIGIT_WINDOW: f64 = 0.6;

/// The plain digit key (no modifiers) pressed this frame.
fn pressed_digit(i: &egui::InputState) -> Option<char> {
    use egui::Key::*;
    [
        (Num0, '0'),
        (Num1, '1'),
        (Num2, '2'),
        (Num3, '3'),
        (Num4, '4'),
        (Num5, '5'),
        (Num6, '6'),
        (Num7, '7'),
        (Num8, '8'),
        (Num9, '9'),
    ]
    .into_iter()
    .find(|(k, _)| i.key_pressed(*k) && !i.modifiers.any())
    .map(|(_, c)| c)
}

/// The card number to jump to once the typed `digits` are final: after two digits, once `idle`
/// passes `DIGIT_WINDOW`, or when no longer number (appending a digit) could name one of `count`
/// cards. A leading 0 names nothing and is dropped.
fn digits_ready(digits: &str, count: usize, idle: f64) -> Option<usize> {
    let n: usize = digits.parse().ok()?;
    let final_ = digits.len() >= 2 || idle > DIGIT_WINDOW || n * 10 > count;
    final_.then_some(n)
}

impl App {
    /// Arrow-key graph navigation: moves `self.focused` to the nearest
    /// tab node in the pressed direction, reusing the same field the
    /// ACCENT card border already highlights and every other
    /// "act on the focused tab" path (Ctrl+W, "+ New Tab"'s parent,
    /// Enter/click-to-open) already reads -- so a keyboard-selected node
    /// behaves exactly like a clicked one, just without the view
    /// transition. Gated on the group-rename text field and quick-
    /// switcher both being closed so arrow keys still do their normal
    /// job there instead of hijacking focus underneath an open widget --
    /// the Settings page itself needs no such check here, since it's now
    /// a separate `ViewMode` `build_graph_ui` never runs under at all.
    /// `typing`: a text field had focus at frame start; its keys are its own
    /// (`shortcuts::text_field_has_keys`).
    pub(crate) fn handle_graph_keys(&mut self, ui: &egui::Ui, typing: bool) {
        // Esc cancels a connect-drag even while a text field has the keys: the handle press never takes focus from
        // it (egui surrenders focus on clicks only), so a note being edited would otherwise eat that Esc and the
        // release would still link. That Esc does nothing else here, though egui itself drops keyboard focus on Esc,
        // so the same Esc also ends a note edit or rename that had the keys.
        if self.link.connect_drag.is_some() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            // egui aborts the Scene background's drag on this same Esc, so a retarget that began on it does not
            // turn into a pan with the button still down (`escape_cancels_a_dragged_curve`).
            self.cancel_connect();
            return;
        }
        if self.json_level.is_some() {
            if !typing {
                self.handle_undo_keys(ui);
            }
            self.handle_json_keys(ui, typing);
            return;
        }
        if !typing && self.renaming_group.is_none() && self.menus.omni_popup.open.is_none() {
            // Each press moves focus one node, so presses that land in one frame are each a move.
            let presses = ui.input(|i| {
                [
                    (egui::Key::ArrowRight, egui::vec2(1.0, 0.0)),
                    (egui::Key::ArrowLeft, egui::vec2(-1.0, 0.0)),
                    (egui::Key::ArrowDown, egui::vec2(0.0, 1.0)),
                    (egui::Key::ArrowUp, egui::vec2(0.0, -1.0)),
                ]
                .map(|(key, dir)| (dir, i.num_presses(key)))
            });
            // An open node menu, or a focused widget that is not a card (a card pane row), keeps the arrows and Enter
            // for itself. egui gives focus by keyboard or code only, never by a click, so a focused widget here was
            // stepped to. The menu needs its own check: it can be open with no row focused (its first frame, or after a
            // page swap), and Down then enters it (`list_row_nav`).
            let card_focused = self.graph.card_nav_active;
            let widget_keys =
                self.menus.node_menu.open.is_some() || (!card_focused && ui.memory(|m| m.focused()).is_some());
            if !widget_keys {
                for (dir, n) in presses {
                    for _ in 0..n {
                        self.move_graph_focus(dir);
                    }
                }
            }
            // Enter opens whatever arrow-key navigation landed on, the same
            // way clicking the node's card does. A Tab-focused card handles its own
            // Enter in the card pass, so this skips it (else it would open the
            // app-focused tab instead of the card).
            if !card_focused && !widget_keys && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                self.open_focused_graph_node();
            }
            // Escape clears a rectangle-select selection, dismisses any
            // node's context menu, and dismisses a hover-peek preview
            // early -- `self.interaction.hovered` itself is left alone (clearing it
            // here would just get overwritten later this same frame by the
            // card loop (`draw_graph_cards`) re-detecting the pointer still sitting over
            // it) and `peek_suppressed` is set instead, checked by the peek
            // preview (`draw_peek_preview`) so it skips rendering for this
            // specific id without needing the pointer to actually move
            // away first. Graph view has no other use for Escape (unlike
            // Tab view, where it steps back to Graph), so this is a plain
            // no-op the rest of the time. Pick-target mode comes first: that
            // Escape ends it and does nothing else.
            if self.link.connecting.is_some() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.cancel_connect();
            } else if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.interaction.selected.clear();
                if let Some((id, _)) = self.menus.node_menu.open.take() {
                    ui.memory_mut(|m| m.request_focus(CardId::from(id).widget()));
                }
                self.interaction.pending_new_node = None;
                self.dismiss_hover_peek();
            }
        }
    }

    /// The current level's cards in Tab order: the stored ones in creation order, then the virtual ones.
    pub(crate) fn card_tab_order(&self) -> Vec<CardId> {
        let stored = self
            .node_order
            .iter()
            .filter(|id| self.nodes.contains_key(*id) && self.in_level(**id))
            .map(|id| CardId::from(*id));
        stored.chain(self.virtual_sources.iter().flat_map(|source| source.entries(self)).map(|e| e.id)).collect()
    }

    /// Whether the Graph view's own plain keys (undo, card digits) may act: no text field, popup, menu, JSON level or
    /// Tab view has the keyboard.
    fn graph_keys_free(&self, ui: &egui::Ui) -> bool {
        self.json_level.is_none() && self.undo_keys_free(ui)
    }

    /// Whether the undo chord may act: no text field, popup, menu or Tab view has the keyboard. A JSON level
    /// does not block it (its inline edits are undoable).
    fn undo_keys_free(&self, ui: &egui::Ui) -> bool {
        !self.modal_open() && !self.showing_tab_view() && !ui.ctx().egui_wants_keyboard_input()
    }

    /// Ctrl+Z undoes and Ctrl+Shift+Z or Ctrl+Y redoes the newest step (`step_history`). Ignored where the digit keys
    /// are, except on a JSON level: a text field, popup, menu or Tab view has the keyboard.
    pub(super) fn handle_undo_keys(&mut self, ui: &egui::Ui) {
        let free = self.undo_keys_free(ui);
        if let Some(redo) = free.then(|| ui.input_mut(undo_chord)).flatten() {
            self.step_history(None, redo);
        }
    }

    /// Plain digit keys jump to the card with that Tab-order number (the badge's number): focus and
    /// pan, never open. Two digits within `DIGIT_WINDOW` make a longer number; a lone digit that no
    /// longer number could extend jumps at once. Ignored while a text field, popup, menu or JSON
    /// level has the keyboard.
    pub(super) fn handle_card_digits(&mut self, ui: &egui::Ui, order: &[CardId]) {
        let key = egui::Id::new("card_digits");
        let now = ui.input(|i| i.time);
        let (mut buf, at): (String, f64) = ui.data(|d| d.get_temp(key)).unwrap_or_default();
        let free = self.graph_keys_free(ui);
        let digit = free.then(|| ui.input(pressed_digit)).flatten();
        if let Some(d) = digit {
            if now - at > DIGIT_WINDOW {
                buf.clear();
            }
            buf.push(d);
            ui.data_mut(|d| d.insert_temp(key, (buf.clone(), now)));
        }
        if !free && !buf.is_empty() {
            ui.data_mut(|d| d.remove_temp::<(String, f64)>(key));
            return;
        }
        let Some(number) = digits_ready(&buf, order.len(), now - at) else {
            if !buf.is_empty() {
                ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(DIGIT_WINDOW));
            }
            return;
        };
        ui.data_mut(|d| d.remove_temp::<(String, f64)>(key));
        if let Some(id) = number.checked_sub(1).and_then(|n| order.get(n)) {
            ui.memory_mut(|m| m.request_focus(id.widget()));
            if let Some(rect) = self.level_card_rect(*id) {
                self.camera.pan_to(rect);
            }
        }
    }
}

#[cfg(test)]
mod tests;
