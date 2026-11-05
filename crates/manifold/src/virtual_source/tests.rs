use super::fake::{Rig, FAKE};
use super::*;
use crate::level::Origin;

#[test]
fn the_default_sources_have_distinct_tags() {
    let app = App::default();
    let names: Vec<&str> = app.virtual_sources.iter().map(|s| s.name()).collect();
    crate::card_id::assert_distinct_tags(&names);
}

#[test]
#[should_panic(expected = "share a tag")]
fn registering_a_source_twice_panics_in_debug() {
    let mut app = App::default();
    app.register_virtual_source(Rc::new(fake::FakeSource::default()));
    app.register_virtual_source(Rc::new(fake::FakeSource::default()));
}

#[test]
fn level_items_include_a_registered_virtual_source_after_the_stored_nodes() {
    let mut app = App::default();
    app.register_virtual_source(Rc::new(fake::FakeSource::default()));
    let items = app.level_items();
    assert!(items.iter().any(|i| i.id == fake::id('a') && matches!(i.origin, Origin::Virtual("fake"))));
    assert_eq!(items.last().unwrap().id, fake::id('c'), "virtual entries draw last, so on top");
}

#[test]
fn a_virtual_entry_comes_after_a_stored_node_and_takes_the_sources_name_and_kind() {
    let rig = Rig::new();
    let items = rig.app.level_items();
    assert!(matches!(items[0].origin, Origin::Stored), "the tab is first: {items:?}");
    assert_eq!(items.len(), 4);
    assert!(items[1..].iter().all(|i| i.kind.ty == FAKE && rig.app.virtual_source("fake").is_some()));
    let (a, b) = (rig.screen(fake::id('a')), rig.screen(fake::id('b')));
    assert!(a.width() > 0.0 && a.right() < b.left(), "{a:?} {b:?}");
}

const ALL: &str = "application/vnd.manifold.virtual.fake-all";
const MENU: &str = "application/vnd.manifold.virtual.fake-menu";

fn calls(rig: &Rig, name: &str) -> usize {
    rig.source.calls.borrow().iter().filter(|c| c.as_str() == name).count()
}

#[test]
fn the_card_loop_draws_virtual_entries_and_hovers_them() {
    let mut rig = Rig::with_kind(ALL);
    let on_b = rig.screen(fake::id('b')).center();
    rig.frame(vec![egui::Event::PointerMoved(on_b)], egui::Modifiers::NONE);
    rig.frame(vec![], egui::Modifiers::NONE);
    assert_eq!(rig.app.interaction.hovered, Some(fake::id('b')));
    assert!(calls(&rig, "paint_body") >= 3, "each of the three cards paints through the source");
}

#[test]
fn a_click_on_a_virtual_card_reaches_the_source_and_touches_no_node() {
    let mut rig = Rig::with_kind(ALL);
    let (nodes, selected) = (rig.app.nodes.len(), rig.app.interaction.selected.clone());
    rig.click(rig.screen(fake::id('b')).center());
    assert_eq!(calls(&rig, "on_click"), 1, "{:?}", rig.source.calls.borrow());
    assert_eq!((rig.app.nodes.len(), &rig.app.interaction.selected), (nodes, &selected));
    assert!(rig.app.menus.node_menu.open.is_none());
}

#[test]
fn a_right_click_on_a_virtual_card_opens_no_node_menu_and_calls_on_menu_only_with_the_ability() {
    let mut rig = Rig::with_kind(FAKE);
    rig.right_click(rig.screen(fake::id('b')).center());
    assert_eq!(calls(&rig, "on_menu"), 0);
    assert!(rig.app.menus.node_menu.open.is_none() && rig.app.menus.node_menu.page.is_none());

    let mut rig = Rig::with_kind(MENU);
    let at = rig.screen(fake::id('b')).center();
    rig.right_click(at);
    assert_eq!(calls(&rig, "on_menu"), 1, "{:?}", rig.source.calls.borrow());
    assert!(rig.app.menus.node_menu.open.is_none() && rig.app.menus.node_menu.page.is_none());
    assert_eq!(rig.source.menu_at.get(), Some(at));
}

#[test]
fn a_virtual_card_shows_no_stored_controls_and_the_tab_order_lists_it_last() {
    use crate::card::card_controls;
    let rig = Rig::new();
    let entry = rig.app.level_items().pop().unwrap();
    let view = rig.app.card_view(&entry, &Default::default()).unwrap();
    assert!(card_controls(&view).is_empty(), "{:?}", card_controls(&view));
    assert!(!view.closable && !view.in_group);
    let stored = rig.app.level_items().remove(0);
    assert!(!card_controls(&rig.app.card_view(&stored, &Default::default()).unwrap()).is_empty());
    let order = rig.app.card_tab_order();
    assert_eq!(order.last().copied(), Some(fake::id('c')));
    assert_eq!(order.len(), 4);
}

#[test]
fn a_virtual_card_is_not_in_eguis_own_tab_traversal() {
    let rig = Rig::new();
    let sense = |id: CardId| rig.ctx.read_response(id.widget()).expect("drawn").sense;
    assert!(!sense(fake::id('b')).is_focusable(), "only the first card or the focused one is focusable");
    let first = rig.app.card_tab_order()[0];
    assert!(sense(first).is_focusable());
}

#[test]
fn tab_onto_a_virtual_card_pans_to_its_rect() {
    let mut rig = Rig::new();
    let order = rig.app.card_tab_order();
    let from = rig.app.level_card_rect(order[0]).unwrap();
    rig.app.camera.jump_to(egui::Rect::from_center_size(from.center(), egui::vec2(120.0, 120.0)));
    rig.frame(vec![], egui::Modifiers::NONE);
    rig.ctx.memory_mut(|m| m.request_focus(order[0].widget()));
    rig.frame(vec![], egui::Modifiers::NONE);
    rig.app.camera.tween = None;
    let tab = egui::Event::Key {
        key: egui::Key::Tab,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    rig.frame(vec![tab], egui::Modifiers::NONE);
    assert!(rig.app.camera.tween.is_some(), "Tab to a card out of view pans to it");
    let want = rig.app.level_items().iter().find(|e| e.id == order[1]).unwrap().rect;
    assert_eq!(rig.app.level_card_rect(order[1]), Some(want));
}

const SELECTABLE: &str = "application/vnd.manifold.virtual.fake-selectable";

fn virtual_ids() -> std::collections::HashSet<CardId> {
    ['a', 'b', 'c'].map(fake::id).into_iter().collect()
}

#[test]
fn a_marquee_selects_virtual_cards_only_when_the_kind_is_selectable() {
    let mut rig = Rig::with_kind(SELECTABLE);
    rig.marquee(rig.around_virtuals());
    assert_eq!(rig.app.interaction.selected, virtual_ids());
    rig.app.end_stale_selection();
    assert_eq!(rig.app.interaction.selected, virtual_ids(), "a selectable virtual id is not stale");

    for ty in [FAKE, MENU] {
        let mut rig = Rig::with_kind(ty);
        rig.marquee(rig.around_virtuals());
        assert!(rig.app.interaction.selected.is_empty(), "{ty}: {:?}", rig.app.interaction.selected);
        rig.app.interaction.selected.insert(fake::id('a'));
        rig.app.end_stale_selection();
        assert!(rig.app.interaction.selected.is_empty(), "{ty}: a kind that cannot be selected is dropped");
    }
}

#[test]
fn space_on_a_focused_virtual_card_toggles_it_only_if_selectable() {
    let space = || egui::Event::Key {
        key: egui::Key::Space,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    for (ty, expect) in [(SELECTABLE, true), (FAKE, false)] {
        let mut rig = Rig::with_kind(ty);
        let first = rig.app.card_tab_order()[0];
        rig.ctx.memory_mut(|m| m.request_focus(first.widget()));
        rig.frame(vec![], egui::Modifiers::NONE);
        let tab = egui::Event::Key {
            key: egui::Key::Tab,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        rig.frame(vec![tab], egui::Modifiers::NONE);
        rig.frame(vec![], egui::Modifiers::NONE);
        let focused = rig.app.card_tab_order()[1];
        assert!(rig.ctx.memory(|m| m.has_focus(focused.widget())), "{focused}");
        rig.frame(vec![space()], egui::Modifiers::NONE);
        assert_eq!(rig.app.interaction.selected.contains(&focused), expect, "{ty}");
    }
}

#[test]
fn selection_actions_act_on_stored_cards_and_leave_a_virtual_id_alone() {
    let mut rig = Rig::with_kind(SELECTABLE);
    let (tab, virt) = (rig.tab(), fake::id('a'));
    let nodes = rig.app.nodes.len();
    rig.app.interaction.selected.extend([CardId::from(tab), virt]);
    rig.app.pin_shortcut();
    assert!(rig.app.doc.mirror.pinned(tab), "the stored tab is pinned");
    assert_eq!(rig.app.interaction.selected.len(), 2);

    let ctx = rig.ctx.clone();
    rig.app.apply_graph_action(&ctx, GraphAction::CloseSelected);
    assert!(!rig.app.nodes.contains_key(&tab), "the stored tab is closed");
    assert_eq!(rig.app.nodes.len(), nodes - 1);
    assert_eq!(rig.app.interaction.selected, std::collections::HashSet::from([virt]));

    let (mut rig, virt) = (Rig::with_kind(SELECTABLE), fake::id('a'));
    let tab = rig.tab();
    rig.app.interaction.selected.extend([CardId::from(tab), virt]);
    let ctx = rig.ctx.clone();
    rig.app.apply_graph_action(&ctx, GraphAction::GroupSelected);
    assert!(rig.app.doc.mirror.group(tab).is_some(), "the stored tab is grouped");
    assert_eq!(rig.app.interaction.selected, std::collections::HashSet::from([virt]));
}
