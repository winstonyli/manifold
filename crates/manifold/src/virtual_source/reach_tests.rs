use super::fake::{Rig, FAKE};
use super::*;
use crate::level::Origin;
use crate::store::model::NodeId;

const SELECTABLE: &str = "application/vnd.manifold.virtual.fake-selectable";

/// Everything a stored write could touch, for a virtual id `id` and the stored tab and group beside it: the
/// mirror, layout, node count, queued Sets, Moves and Deletes, and the serialized store document.
fn footprint(app: &mut App, others: &[NodeId]) -> String {
    let doc = app.export_session_json();
    let others: Vec<_> =
        others.iter().map(|&o| (app.doc.mirror.group(o), app.doc.mirror.parent(o), app.doc.mirror.pinned(o))).collect();
    format!(
        "{} {:?} {:?} {:?} {:?} {:?}",
        app.nodes.len(),
        app.doc.writes.fields().collect::<Vec<_>>(),
        app.doc.writes.moves().collect::<Vec<_>>(),
        others,
        app.layout.keys().collect::<std::collections::BTreeSet<_>>(),
        doc,
    )
}

#[test]
fn every_write_entry_point_leaves_a_virtual_id_out_of_the_store_and_the_layout() {
    let mut rig = Rig::with_kind("application/vnd.manifold.virtual.fake-all");
    let placement = crate::fetched::Placement { group: None, pos: egui::vec2(700.0, 300.0) };
    let group = crate::card_id::testutil::node(9);
    rig.app.place_group(Some(group), "G", placement);
    let (virt, tab) = (fake::id('a'), rig.tab());
    let ctx = rig.ctx.clone();
    let others = [tab, group];
    let before = footprint(&mut rig.app, &others);
    let virtual_b = fake::id('b');

    // A release of a live virtual drag (it must really run).
    rig.app.interaction.dragging = Some(virt);
    rig.app.interaction.drag_moved = vec![virt];
    assert!(rig.app.is_being_dragged(virt), "the release below is not vacuous");
    rig.app.release_drag();

    // Every drop effect: the virtual id dragged onto a stored or virtual target, and the stored tab and group
    // dragged onto a virtual target.
    for effect in [DropEffect::Join, DropEffect::GroupWith, DropEffect::Reparent, DropEffect::Merge, DropEffect::Eject]
    {
        for target in [CardId::from(group), CardId::from(tab), virtual_b] {
            rig.app.apply_drop(virt, Some((effect, Some(target))));
        }
        for dragged in [CardId::from(tab), CardId::from(group)] {
            rig.app.apply_drop(dragged, Some((effect, Some(virt))));
        }
    }
    assert_eq!(footprint(&mut rig.app, &others), before);

    // Selection actions with the virtual id alone selected.
    rig.app.interaction.selected = std::collections::HashSet::from([virt]);
    rig.app.apply_graph_action(&ctx, GraphAction::CloseSelected);
    rig.app.apply_graph_action(&ctx, GraphAction::GroupSelected);
    rig.app.pin_shortcut();
    assert_eq!(footprint(&mut rig.app, &others), before);
}

#[test]
fn a_virtual_card_far_from_the_stored_ones_enlarges_the_fit() {
    struct Far;
    impl VirtualSource for Far {
        fn name(&self) -> &'static str {
            "far"
        }
        fn entries(&self, _: &App) -> Vec<CardEntry> {
            vec![CardEntry {
                id: CardId::virt("far", 0x78),
                kind: crate::CardKind { ty: FAKE, tags: crate::caps::TagSet::EMPTY },
                origin: Origin::Virtual("far"),
                rect: egui::Rect::from_min_size(egui::pos2(9000.0, 9000.0), egui::vec2(100.0, 60.0)),
            }]
        }
        fn edges(&self, _: &App, _: &dyn Fn(CardId) -> Option<egui::Rect>) -> Vec<LevelEdge> {
            Vec::new()
        }
        fn paint_body(&self, _: &App, _: &mut egui::Ui, _: &mut CardPass, entry: &CardEntry) -> CardBody {
            CardBody { interact_rect: entry.rect, tooltip: None }
        }
        fn on_click(&self, _: &mut App, _: &CardEntry) -> Option<GraphAction> {
            None
        }
        fn on_menu(&self, _: &mut App, _: &CardEntry, _: egui::Pos2) {}
        fn on_drop(
            &self,
            _: &mut App,
            _: &CardEntry,
            _: Option<(DropEffect, Option<CardId>)>,
            _: egui::Pos2,
        ) -> Option<GraphAction> {
            None
        }
    }
    let mut rig = Rig::new();
    let fit = rig.app.fit_all_nodes_scene_rect();
    rig.app.register_virtual_source(Rc::new(Far));
    assert!(rig.app.fit_all_nodes_scene_rect().max.x > 9000.0 && fit.max.x < 1000.0);
}

#[test]
fn export_state_lists_a_virtual_card_with_its_source_and_abilities() {
    let rig = Rig::with_kind(SELECTABLE);
    let state = rig.app.export_state();
    let card =
        state["cards"].as_array().unwrap().iter().find(|c| c["id"] == fake::id('a').to_string()).expect("listed");
    assert_eq!(card["virtual"], "fake");
    assert_eq!(card["type"], SELECTABLE);
    assert_eq!(card["json_card"], false);
    assert_eq!(card["abilities"]["selectable"], true);
    assert_eq!(card["abilities"]["menu"], false);
    assert!(card["rect"].is_array() && card["screen"].is_array());
    let stored = state["cards"].as_array().unwrap().iter().find(|c| c["id"] == rig.tab().to_string()).unwrap();
    assert!(stored.get("virtual").is_none());
}

/// EXPORTSTATE prints a card's id as `Display` and `FromStr` reads it back to the same id, for
/// stored and virtual cards alike (a script drives the app with the text it was given).
#[test]
fn exportstate_prints_a_card_id_that_parses_back() {
    let rig = Rig::with_kind(SELECTABLE);
    let state = rig.app.export_state();
    let printed: Vec<CardId> = state["cards"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap().parse().expect("a printed id parses"))
        .collect();
    let mut drawn: Vec<CardId> = rig.app.level_items().into_iter().map(|e| e.id).collect();
    drawn.sort();
    let mut printed_sorted = printed;
    printed_sorted.sort();
    assert_eq!(printed_sorted, drawn);
    assert!(drawn.iter().any(|id| id.is_virtual()) && drawn.iter().any(|id| id.node().is_some()));
}

#[test]
fn a_filter_dims_a_virtual_card_only_when_its_source_gives_searchable_text() {
    let mut rig = Rig::new();
    let entry = rig.app.level_items().pop().unwrap();
    rig.app.set_graph_filter("zzz");
    assert!(!rig.app.virtual_card_dimmed(&entry), "no searchable text: never dimmed");
    rig.app.virtual_sources.clear();
    let source = Rc::new(fake::FakeSource::default());
    source.searchable.set(Some("alpha"));
    rig.app.register_virtual_source(source);
    assert!(rig.app.virtual_card_dimmed(&entry), "text that does not match dims");
    rig.app.set_graph_filter("ALP");
    assert!(!rig.app.virtual_card_dimmed(&entry), "text that matches does not");
}
