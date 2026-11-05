//! Virtual cards (spec `2026-10-16-virtual-cards-design.md`): cards that no stored node backs. A `VirtualSource`
//! supplies a level's extra `CardEntry`s, which draw after the stored ones, so on top. The trait holds only
//! handlers something calls (the pre-commit clippy rejects dead code).

use crate::card::{CardBody, CardPass, DropEffect};
use crate::card_id::CardId;
use crate::level::{CardEntry, LevelEdge, Origin};
use crate::{App, GraphAction};
use std::rc::Rc;

/// Something that puts virtual cards on the current level. Registered in `App::virtual_sources`.
///
/// `on_click` and `on_drop` run with `&mut App` inside the card loop, whereas a stored card's drop goes through
/// `GraphAction::Drop` and is applied after it. A handler should therefore return a `GraphAction` for anything that
/// changes nodes, and touch only its own state directly.
pub(crate) trait VirtualSource {
    /// The source's name: the `Origin::Virtual` of every entry it yields.
    fn name(&self) -> &'static str;
    /// The cards this source shows on the level being viewed, in draw order.
    fn entries(&self, app: &App) -> Vec<CardEntry>;
    /// The edges this source adds to the level; `rect_of` gives any entry's rect on the level (`None` off it).
    fn edges(&self, app: &App, rect_of: &dyn Fn(CardId) -> Option<egui::Rect>) -> Vec<LevelEdge>;
    /// Paints `entry`'s content inside the shared card frame; the body says which part of the card takes the pointer.
    fn paint_body(&self, app: &App, ui: &mut egui::Ui, pass: &mut CardPass, entry: &CardEntry) -> CardBody;
    /// A click on `entry` (the shared card loop calls it for every virtual card, whatever its abilities).
    fn on_click(&self, app: &mut App, entry: &CardEntry) -> Option<GraphAction>;
    /// A right-click on `entry` at screen point `at`, only when its kind grants `menu`.
    fn on_menu(&self, app: &mut App, entry: &CardEntry, at: egui::Pos2);
    /// `entry`, a `drag_source` kind, was released at world point `at` (where the pointer was, so a source can put
    /// what it carried where it was dropped). `target` is what `App::drop_target` found under the pointer (the
    /// effect and the card dropped on), or `None` for a release over empty canvas. A release on the back button is
    /// `Some((DropEffect::Eject, None))`: `drop_action` ignores the dragged kind for it, so it reaches the source as
    /// an `Eject` too. The source decides what any of these means; a returned action is applied like a click's.
    fn on_drop(
        &self,
        app: &mut App,
        entry: &CardEntry,
        target: Option<(DropEffect, Option<CardId>)>,
        at: egui::Pos2,
    ) -> Option<GraphAction>;
    /// The text the Graph toolbar filter matches `entry` against; `None` (the default) means the filter never dims it.
    fn searchable_text(&self, _entry: &CardEntry) -> Option<String> {
        None
    }
}

impl App {
    /// Whether the toolbar filter dims virtual card `entry`: its source gives searchable text and the filter misses it.
    pub(crate) fn virtual_card_dimmed(&self, entry: &CardEntry) -> bool {
        let text = self.entry_source(entry).and_then(|source| source.searchable_text(entry));
        text.is_some_and(|text| !crate::text::filter_matches(self.omni_filter(), &[&text]))
    }

    /// The source that yielded `entry`: `None` for a stored or JSON entry, or a virtual one whose source is gone.
    pub(crate) fn entry_source(&self, entry: &CardEntry) -> Option<Rc<dyn VirtualSource>> {
        match entry.origin {
            Origin::Virtual(name) => self.virtual_source(name),
            _ => None,
        }
    }

    /// Registers `source` after the sources already there. Each name hashes to its cards' `CardId` tag, so a clash (or
    /// the JSON levels' name) panics.
    #[cfg(test)]
    pub(crate) fn register_virtual_source(&mut self, source: Rc<dyn VirtualSource>) {
        self.virtual_sources.push(source);
        let names: Vec<&str> = self.virtual_sources.iter().map(|s| s.name()).collect(); // name rule: a source's identifier, never shown
        crate::card_id::assert_distinct_tags(&names);
    }

    /// The registered source named `name` (the `Origin::Virtual` of the entries it yields). Shared, so a handler can
    /// take `&mut App` while the caller still holds the source.
    pub(crate) fn virtual_source(&self, name: &str) -> Option<Rc<dyn VirtualSource>> {
        self.virtual_sources.iter().find(|s| s.name() == name).cloned() // name rule: a source's identifier, never shown
    }
}

#[cfg(test)]
pub(crate) mod fake;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod reach_tests;

#[cfg(test)]
mod drag_tests;

#[cfg(test)]
mod rig_tests {
    use super::fake::{Rig, FAKE};
    use crate::card_id::testutil::node;

    /// A HashMap-order `tab()` picked the group about half the time once one was added; 40 rigs make that certain.
    #[test]
    fn the_rigs_tab_is_the_tab_even_with_a_group_beside_it() {
        for _ in 0..40 {
            let mut rig = Rig::with_kind(FAKE);
            let placement = crate::fetched::Placement { group: None, pos: egui::vec2(700.0, 300.0) };
            let group = node(9);
            rig.app.place_group(Some(group), "G", placement);
            assert_ne!(rig.tab(), group);
        }
    }
}

#[cfg(test)]
mod settle_tests {
    use super::fake::Rig;

    /// A source whose entry count changes (the trail grows with each navigation) must not look like a changed level:
    /// that would re-settle, and so move, the stored cards. Both settle-key sites (the per-frame check and
    /// `current_level_settle_key`) count stored entries only.
    #[test]
    fn a_virtual_entry_does_not_change_the_settle_key() {
        let rig = Rig::new();
        assert_eq!(rig.app.level_items().len(), 4, "the tab and three virtual cards");
        let key = rig.app.current_level_settle_key();
        assert_eq!(key.1.count, 1, "the stored tab only");
        assert_eq!(rig.app.graph.settled_key, Some(key), "the per-frame check keyed the same way");
    }
}

#[cfg(test)]
mod keyboard_menu_tests {
    use super::fake::Rig;

    /// Shift+F10 on a focused card opens its menu: a stored card's node menu, and for a virtual card whose kind
    /// grants `menu` the source's `on_menu`, at the card's bottom-left corner. A kind without `menu` has none.
    #[test]
    fn shift_f10_on_a_focused_virtual_card_calls_on_menu_only_with_the_ability() {
        const MENU: &str = "application/vnd.manifold.virtual.fake-menu";
        for (ty, calls) in [(MENU, 1), (super::fake::FAKE, 0)] {
            let mut rig = Rig::with_kind(ty);
            let first = rig.app.card_tab_order()[0];
            rig.ctx.memory_mut(|m| m.request_focus(first.widget()));
            rig.frame(vec![], egui::Modifiers::NONE);
            let key =
                |key, modifiers| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers };
            rig.frame(vec![key(egui::Key::Tab, egui::Modifiers::NONE)], egui::Modifiers::NONE);
            rig.frame(vec![], egui::Modifiers::NONE);
            let focused = rig.app.card_tab_order()[1];
            assert!(rig.ctx.memory(|m| m.has_focus(focused.widget())), "{focused}");
            rig.frame(vec![key(egui::Key::F10, egui::Modifiers::SHIFT)], egui::Modifiers::SHIFT);
            let seen = rig.source.calls.borrow().iter().filter(|c| *c == "on_menu").count();
            assert_eq!(seen, calls, "{ty}");
            if calls == 1 {
                let corner = rig.screen(focused).left_bottom();
                let at = rig.source.menu_at.get().expect("asked to open");
                assert!((at - corner).length() < 1.0, "{at:?} vs {corner:?}");
            }
            assert!(rig.app.menus.node_menu.open.is_none(), "a virtual card opens no node menu");
        }
    }
}

#[cfg(test)]
mod selection_rule_tests {
    use crate::card_id::{testutil::node, CardId};
    use crate::App;

    /// "Virtual" in a selection action is the id's kind (`CardId::node`), the persistence guard's too: a virtual id
    /// whose source no longer lists it stays out of the stored count and the stored take.
    #[test]
    fn the_selection_actions_split_on_the_id_kind_not_on_what_sources_list() {
        let mut app = App::default();
        let (n, gone) = (node(1), CardId::virt("gone", 0x78));
        app.test_node(n, crate::store::model::Kind::Jar, crate::node::Runtime::Jar);
        app.interaction.selected.extend([CardId::from(n), gone]);
        assert_eq!(app.selected_stored_count(), 1);
        let taken = app.take_selected_stored();
        assert_eq!(taken, std::collections::HashSet::from([n]));
        assert_eq!(app.interaction.selected, std::collections::HashSet::from([gone]));
    }

    /// A selected virtual id whose source is not registered is pruned with the rest of a stale
    /// selection, so no action ever sees a card nothing draws.
    #[test]
    fn selection_prunes_a_virtual_id_whose_source_is_unregistered() {
        let mut app = App::default();
        app.interaction.selected.insert(CardId::virt("gone", 0x78));
        app.end_stale_selection();
        assert!(app.interaction.selected.is_empty());
    }
}
