//! The Graph view's hints: the card and JSON gesture lists, the Ctrl reveal, the empty-canvas line and the drop hint.

use super::*;

/// Painted on the canvas when the level has no nodes (the bindings are in `shortcuts.rs`).
pub(super) const EMPTY_HINT: &str = "Ctrl+T for a new tab \u{b7} drop a file or URL \u{b7} F1 for shortcuts";

/// A small label beside the pointer on the topmost layer, over the cards and the toolbar alike.
pub(super) fn paint_drop_hint(ctx: &egui::Context, at: egui::Pos2, text: &str) {
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("drop_hint")));
    let galley = painter.layout_no_wrap(text.to_owned(), egui::FontId::proportional(13.0), crate::theme::TEXT_PRIMARY);
    let rect = egui::Rect::from_min_size(at + egui::vec2(16.0, 18.0), galley.size()).expand2(egui::vec2(8.0, 4.0));
    painter.rect(
        rect,
        crate::theme::RADIUS,
        crate::theme::SURFACE_RAISED,
        egui::Stroke::new(1.0, crate::theme::OUTLINE),
        egui::StrokeKind::Inside,
    );
    painter.galley(rect.min + egui::vec2(8.0, 4.0), galley, crate::theme::TEXT_PRIMARY);
}

/// The mouse and key pairs the hint strip shows for a `kind` of card: each gesture beside its keyboard twin.
fn card_hint_pairs(kind: CardKind) -> Vec<(&'static str, &'static str)> {
    let select = if kind.ty == TYPE_TAB { ("Click / Space", "select") } else { ("Click", "select") };
    let open = match kind.ty {
        TYPE_TAB | TYPE_IMAGE => Some("open"),
        TYPE_DOWNLOAD | TYPE_PDF => Some("reveal"),
        TYPE_NOTE => Some("edit"),
        TYPE_JSON => Some("enter"),
        ty if is_container(ty) => Some("enter"),
        _ => None,
    };
    let mut pairs = vec![select];
    pairs.extend(open.map(|verb| ("Dbl-click / Enter", verb)));
    pairs.extend([("Right-click / Shift+F10", "menu"), ("Tab", "next"), ("Del", del_verb(kind))]);
    pairs
}

impl App {
    /// While a graph card has keyboard focus or the pointer, a strip of keycaps under it (above, near the bottom
    /// edge) lists the mouse and keys that act on it. Not while its menu is open.
    pub(super) fn build_card_hint(&self, ui: &egui::Ui) {
        let Some((kind, rect)) =
            self.card_hint.filter(|_| self.menus.node_menu.open.is_none() && self.json_level.is_none())
        else {
            return;
        };
        let screen = ui.ctx().content_rect();
        let hints = card_hint_pairs(kind);
        let below = rect.bottom() + 40.0 < screen.bottom();
        let (anchor_y, pivot) = if below {
            (rect.bottom() + 8.0, egui::Align2::CENTER_TOP)
        } else {
            (rect.top() - 8.0, egui::Align2::CENTER_BOTTOM)
        };
        let x = rect.center().x.clamp(screen.left() + 190.0, (screen.right() - 190.0).max(screen.left() + 190.0));
        let area = egui::Area::new(egui::Id::new("card_key_hint")).pivot(pivot).fixed_pos(egui::pos2(x, anchor_y));
        key_hint_strip(ui.ctx(), area, hints.iter().copied());
    }

    /// While Ctrl is held on its own (`ctrl_reveal_step`), a strip of the Ctrl chords at the bottom of
    /// the Graph view; the cards show their number badges too.
    pub(super) fn build_ctrl_hint(&self, ui: &egui::Ui) {
        if !self.hints_revealed || self.json_level.is_some() || self.menus.node_menu.open.is_some() {
            return;
        }
        let area = egui::Area::new(egui::Id::new("ctrl_key_hint")).anchor(egui::Align2::CENTER_BOTTOM, [0.0, -12.0]);
        let hints = [
            ("Ctrl+T", "new tab"),
            ("Ctrl+Shift+T", "reopen"),
            ("Ctrl+K", "switch"),
            ("Ctrl+G", "view"),
            ("Ctrl+L", "address"),
            ("Ctrl+W", "close"),
            ("0 - 9", "jump to card"),
        ];
        key_hint_strip(ui.ctx(), area, hints.into_iter());
    }

    /// In a JSON level with a row or card selected, a strip of keycaps at the bottom of the window
    /// lists the keys the level takes.
    pub(super) fn build_json_hint(&self, ui: &egui::Ui) {
        if self.json_level.as_ref().is_none_or(|l| l.selected.is_none() || l.menu.is_some()) {
            return;
        }
        let area = egui::Area::new(egui::Id::new("json_key_hint")).anchor(egui::Align2::CENTER_BOTTOM, [0.0, -12.0]);
        let hints =
            [("Up / Down", "select"), ("Enter", "expand"), ("Right / Left", "open / close row"), ("Ctrl+C", "copy")];
        key_hint_strip(ui.ctx(), area, hints.into_iter());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_card_hint_pairs_each_gesture_with_its_key_and_matches_the_click_action() {
        use super::card_hint_pairs;
        use crate::card::click_action;
        use crate::card_kind::fixtures::plain;
        use crate::store::model::TYPE_GROUP;
        let verb = |ty| card_hint_pairs(plain(ty)).iter().find(|(keys, _)| keys.starts_with("Dbl-click")).map(|p| p.1);
        assert_eq!(verb(TYPE_TAB), Some("open"));
        assert_eq!(verb(TYPE_IMAGE), Some("open"));
        assert_eq!(verb(TYPE_DOWNLOAD), Some("reveal"));
        assert_eq!(verb(TYPE_PDF), Some("reveal"));
        assert_eq!(verb(TYPE_NOTE), Some("edit"));
        assert_eq!(verb(TYPE_JSON), Some("enter"));
        assert_eq!(verb(TYPE_GROUP), Some("enter"));
        // A kind whose double-click is a `NodeAction` is never labelled otherwise.
        for ty in [TYPE_TAB, TYPE_IMAGE, TYPE_DOWNLOAD, TYPE_PDF] {
            assert!(click_action(plain(ty)).is_some() && verb(ty).is_some());
        }
        let tab = card_hint_pairs(plain(TYPE_TAB));
        assert_eq!(tab[0], ("Click / Space", "select"));
        assert_eq!(tab.last(), Some(&("Del", "close")));
    }

    /// An empty level keeps its canvas: the whole of `build_graph_ui` runs without panicking and paints the hint.
    #[test]
    fn an_empty_level_paints_the_hint_on_its_canvas() {
        let ctx = egui::Context::default();
        crate::theme::apply_fonts(&ctx);
        let mut app = App::default();
        assert!(app.nodes.is_empty());
        let mut hints = Vec::new();
        for _ in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| app.build_graph_ui(ui));
            output.textures_delta.clear();
            hints = output
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Text(t) if t.galley.text() == EMPTY_HINT => Some(t.pos),
                    _ => None,
                })
                .collect();
        }
        assert_eq!(hints.len(), 1, "the hint is painted once");
    }

    // Real node ids: cards are keyed by `CardId`, which parses stored ids as `NodeId`s.
    const FULL: &str = "0000000000000001-1";
    const EMPTY: &str = "0000000000000001-2";
    const N: &str = "0000000000000001-3";

    #[test]
    fn the_hint_shows_on_every_empty_level_not_only_an_empty_graph() {
        use crate::fetched::Placement;
        let ctx = egui::Context::default();
        crate::theme::apply_fonts(&ctx);
        let mut app = App::default();
        let at = Placement { group: None, pos: egui::Vec2::ZERO };
        app.place_group(Some(FULL.into()), "Full", at);
        app.place_group(Some(EMPTY.into()), "Empty", Placement { group: Some(FULL.into()), pos: egui::Vec2::ZERO });
        let note = Runtime::Note(NotePayload { text: "n".into(), editing: false });
        app.insert_placeholder_node(N.into(), egui::Vec2::ZERO, crate::store::model::Kind::Note, note);
        app.edit_group(N, Some(FULL.into()));
        let hints_at = |app: &mut App, level: Option<&str>| {
            app.current_group = level.map(str::to_owned);
            let mut hints = 0;
            for _ in 0..3 {
                let input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
                    ..Default::default()
                };
                let mut output = ctx.run_ui(input, |ui| app.build_graph_ui(ui));
                output.textures_delta.clear();
                hints = output
                    .shapes
                    .iter()
                    .filter(|s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.text() == EMPTY_HINT))
                    .count();
            }
            hints
        };
        assert_eq!(hints_at(&mut app, Some(EMPTY)), 1, "an empty group shows the hint");
        assert_eq!(hints_at(&mut app, Some(FULL)), 0, "a level with a node does not");
        assert_eq!(hints_at(&mut app, None), 0, "the top level holds the group card");
    }
}
