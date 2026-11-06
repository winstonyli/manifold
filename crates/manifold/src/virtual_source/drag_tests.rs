use super::fake::{self, Dropped, Rig, FAKE};
use crate::card::DropEffect;
use crate::card_id::CardId;
use crate::store::model::NodeId;

const DRAG: &str = "application/vnd.manifold.virtual.fake-drag";
const ALL: &str = "application/vnd.manifold.virtual.fake-all";

/// A rig with a stored group `g` right of the cards, the camera framing everything.
fn rig_with_group(ty: &'static str) -> (Rig, NodeId) {
    let mut rig = Rig::with_kind(ty);
    let placement = crate::node_store::Placement { group: None, pos: egui::vec2(700.0, 300.0) };
    let group = crate::card_id::testutil::node(9);
    rig.app.place_group(Some(group), "G", placement);
    rig.app.camera.jump_to(egui::Rect::from_min_max(egui::pos2(-100.0, -100.0), egui::pos2(1100.0, 600.0)));
    for _ in 0..3 {
        rig.frame(vec![], egui::Modifiers::NONE);
    }
    (rig, group)
}

fn world_to_screen(rig: &Rig, world: egui::Pos2) -> egui::Pos2 {
    rig.app.graph_world_rect_to_screen(egui::Rect::from_min_size(world, egui::Vec2::ZERO)).min
}

fn rects(rig: &Rig) -> Vec<(CardId, egui::Rect)> {
    rig.app.level_items().into_iter().map(|e| (e.id, e.rect)).collect()
}

fn drops(rig: &Rig) -> Vec<Dropped> {
    rig.source.drops.borrow().clone()
}

#[test]
fn dragging_a_virtual_card_moves_nothing_and_the_drag_stays_live_while_held() {
    let (mut rig, _) = rig_with_group(DRAG);
    let before = rects(&rig);
    let from = rig.screen(fake::id('a')).center();
    rig.hold_drag(from, from + egui::vec2(40.0, 70.0));
    let layout_len = rig.app.layout.len();
    assert_eq!(rig.app.interaction.dragging, Some(fake::id('a')));
    assert!(rig.app.interaction.drag_moved.is_empty());
    assert_eq!(rects(&rig), before, "no rect moved");
    assert_eq!(rig.app.layout.len(), layout_len, "no layout entry");
    rig.frame(vec![], egui::Modifiers::NONE);
    assert_eq!(rig.app.interaction.dragging, Some(fake::id('a')), "end_stale_drag keeps it");
    rig.release(from);
    assert!(rig.app.interaction.dragging.is_none());
    assert_eq!(rects(&rig), before);
    assert_eq!(rig.app.layout.len(), layout_len);
}

#[test]
fn a_kind_without_drag_source_is_not_dragged() {
    let (mut rig, _) = rig_with_group(FAKE);
    let from = rig.screen(fake::id('a')).center();
    rig.hold_drag(from, from + egui::vec2(40.0, 70.0));
    assert!(rig.app.interaction.dragging.is_none());
}

#[test]
fn a_release_over_empty_canvas_calls_on_drop_once_with_no_target() {
    let (mut rig, _) = rig_with_group(DRAG);
    let empty = world_to_screen(&rig, egui::pos2(500.0, 150.0));
    assert!(rig
        .app
        .level_items()
        .iter()
        .all(|e| !e.rect.contains(rig.app.graph_screen_point_to_world(empty).to_pos2())));
    let from = rig.screen(fake::id('a')).center();
    rig.drag(from, empty);
    assert_eq!(drops(&rig), [(fake::id('a'), None)]);
    assert_eq!(rig.source.calls.borrow().iter().filter(|c| *c == "on_drop").count(), 1);
    assert!(rig.app.interaction.dragging.is_none());
}

/// The point a source is handed is the pointer's world position at the release, so a card dragged out can be
/// placed where it was dropped.
#[test]
fn on_drop_gets_the_release_point_in_world_space() {
    let (mut rig, _) = rig_with_group(DRAG);
    let empty = world_to_screen(&rig, egui::pos2(500.0, 150.0));
    let from = rig.screen(fake::id('a')).center();
    rig.drag(from, empty);
    let want = rig.app.graph_screen_point_to_world(empty).to_pos2();
    let at = rig.source.drop_at.borrow().clone();
    assert_eq!(at.len(), 1);
    assert!((at[0] - want).length() < 0.5, "{:?} vs {want:?}", at[0]);
    assert!((want - egui::pos2(500.0, 150.0)).length() < 0.5, "the test's own world point round-trips");
}

#[test]
fn a_release_over_a_stored_group_calls_on_drop_with_the_effect_and_the_group() {
    let (mut rig, group) = rig_with_group(DRAG);
    let over = rig.app.graph_world_rect_to_screen(rig.app.card_rect(group).unwrap()).center();
    let from = rig.screen(fake::id('a')).center();
    rig.drag(from, over);
    assert_eq!(drops(&rig), [(fake::id('a'), Some((DropEffect::Join, Some(CardId::from(group)))))]);
    assert_eq!(rig.app.members_of(group).count(), 0, "nothing joined the group");
}

#[test]
fn a_virtual_card_is_a_drop_target_only_with_the_ability() {
    for (ty, expect) in [(ALL, true), (DRAG, false)] {
        let mut rig = Rig::with_kind(ty);
        let (a, b) = (rig.screen(fake::id('a')).center(), rig.screen(fake::id('b')).center());
        rig.drag(a, b);
        let want = expect.then(|| (DropEffect::Join, Some(fake::id('b'))));
        assert_eq!(drops(&rig), [(fake::id('a'), want)], "{ty}");
    }
}

#[test]
fn a_virtual_drop_never_takes_the_stored_selection_with_it() {
    let (mut rig, group) = rig_with_group(ALL);
    let tab = rig.tab();
    rig.app.interaction.selected.extend([CardId::from(tab), fake::id('a')]);
    let over = rig.app.graph_world_rect_to_screen(rig.app.card_rect(group).unwrap()).center();
    let from = rig.screen(fake::id('a')).center();
    rig.drag(from, over);
    assert_eq!(drops(&rig).len(), 1, "{:?}", drops(&rig));
    assert!(rig.app.doc.mirror.group(tab).is_none(), "the selected stored tab did not join the group");
    assert!(rig.app.interaction.selected.contains(&CardId::from(tab)), "the selection is untouched");
}

#[test]
fn release_drag_for_a_virtual_id_writes_nothing() {
    let (mut rig, _) = rig_with_group(DRAG);
    rig.app.interaction.dragging = Some(fake::id('a'));
    rig.app.interaction.drag_moved = vec![];
    let (nodes, layout_len) = (rig.app.nodes.len(), rig.app.layout.len());
    rig.app.release_drag();
    assert!(rig.app.interaction.dragging.is_none());
    assert_eq!((rig.app.nodes.len(), rig.app.layout.len()), (nodes, layout_len));
}

#[test]
fn a_stored_tab_drag_is_unchanged_and_never_reaches_a_source() {
    let (mut rig, group) = rig_with_group(DRAG);
    let tab = rig.tab();
    let over = rig.app.graph_world_rect_to_screen(rig.app.card_rect(group).unwrap()).center();
    let from = rig.app.graph_world_rect_to_screen(rig.app.card_rect(tab).unwrap()).center();
    rig.drag(from, over);
    assert_eq!(rig.app.doc.mirror.group(tab), Some(group), "the tab joined the group");
    assert!(drops(&rig).is_empty());
}
