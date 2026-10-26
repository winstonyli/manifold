//! Session-sync tests: labels, the stored-only nodes cards can carry.

use super::*;
use crate::labels::labels_path;
use crate::store::tree::Leaf;

#[test]
fn a_label_syncs_as_a_stored_node_and_is_never_a_card() {
    let path = temp_dir("label-sync").join("session.mnf");
    let mut app = app_on(&path, 1);
    let label = app.create_label("red things", None).unwrap();
    assert!(app.save_session());
    assert!(app.doc.mirror.is_stored(label) && !app.nodes.contains_key(&label), "mirror only");
    let mut other = app_on(&path, 2);
    other.restore_session();
    assert_eq!(other.doc.mirror.label_def(label).name, "red things");
    assert!(other.doc.mirror.is_stored(label) && !other.nodes.contains_key(&label));
    assert!(other.doc.mirror.link_ids().next().is_none(), "a label is not a link");
}

#[test]
fn a_label_name_is_cleaned_and_unique() {
    let path = temp_dir("label-names").join("session.mnf");
    let mut app = app_on(&path, 1);
    assert_eq!(app.create_label("   ", None), None, "an empty name makes nothing");
    let a = app
        .create_label(
            "  Urgent 
",
            None,
        )
        .unwrap();
    assert_eq!(app.doc.mirror.label_def(a).name, "Urgent");
    assert_eq!(app.create_label("urgent", None), Some(a), "same name key: the existing label");
    assert_eq!(app.label_defs().len(), 1);
    assert_ne!(app.create_label("other", None), Some(a));
    assert_eq!(app.label_defs().len(), 2);
}

#[test]
fn new_labels_take_free_palette_colours() {
    let mut app = app_on(&temp_dir("label-colours").join("session.mnf"), 1);
    let (a, b) = (app.create_label("a", None).unwrap(), app.create_label("b", None).unwrap());
    let (ca, cb) = (app.doc.mirror.label_def(a).color, app.doc.mirror.label_def(b).color);
    assert!(ca.is_some() && cb.is_some() && ca != cb);
}

#[test]
fn a_refused_menu_rename_says_why() {
    let mut app = app_on(&temp_dir("label-refused").join("session.mnf"), 1);
    let a = app.create_label("a", None).unwrap();
    app.create_label("b", None).unwrap();
    app.rename_label_or_toast(a, "B");
    app.rename_label_or_toast(a, " ");
    app.rename_label_or_toast(a, "fine");
    let toasts: Vec<_> = app.toasts.save_toast_queue.iter().map(String::as_str).collect();
    assert_eq!(toasts, ["A label named \"B\" already exists", "A label needs a name"]);
    assert_eq!(app.doc.mirror.label_def(a).name, "fine");
}

#[test]
fn the_trash_button_deletes_on_the_second_press_of_the_same_label() {
    let mut app = app_on(&temp_dir("label-trash").join("session.mnf"), 1);
    let (a, b) = (app.create_label("a", None).unwrap(), app.create_label("b", None).unwrap());
    let mut menu = crate::labels::LabelSection::default();
    assert!(!menu.press_trash(a), "the first press only arms it");
    assert!(!menu.press_trash(b), "another label's press moves the arming");
    assert!(menu.press_trash(b) && menu.confirm_delete.is_none(), "the second press on the same label deletes");
}

/// A group with one labelled note in `app`, exported.
fn labelled_group_export(app: &mut App, label: &str, color: egui::Color32) -> crate::store::json::JsonDoc {
    let g = add_group(app, "G");
    let n = add_note(app, "n");
    app.edit_group(n, Some(g));
    let l = app.create_label(label, Some(color)).unwrap();
    app.set_label(&[n], l, true);
    assert!(app.save_session());
    app.export_group_json(g).unwrap()
}

fn label_names(app: &App) -> Vec<String> {
    let mut names: Vec<_> = app.label_defs().values().map(|d| d.name.clone()).collect();
    names.sort();
    names
}

#[test]
fn a_group_export_brings_its_label_to_a_document_without_it() {
    let mut a = app_on(&temp_dir("label-export-a").join("session.mnf"), 1);
    let json = labelled_group_export(&mut a, "urgent", egui::Color32::RED);
    let mut b = app_on(&temp_dir("label-export-b").join("session.mnf"), 2);
    b.import_group_json(&json);
    assert_eq!(label_names(&b), ["urgent"]);
    let (id, def) = b.label_defs().into_iter().next().unwrap();
    assert_eq!(def.color, Some(egui::Color32::RED));
    assert!(b.nodes.keys().any(|n| b.doc.mirror.card_labels(*n) == [id]), "the imported note carries the label");
}

#[test]
fn a_group_export_reuses_a_same_named_label_and_keeps_the_targets_colour() {
    let mut a = app_on(&temp_dir("label-merge-a").join("session.mnf"), 1);
    let json = labelled_group_export(&mut a, "Urgent", egui::Color32::RED);
    let mut b = app_on(&temp_dir("label-merge-b").join("session.mnf"), 2);
    let mine = b.create_label("urgent", Some(egui::Color32::BLUE)).unwrap();
    b.import_group_json(&json);
    b.import_group_json(&json);
    assert_eq!(label_names(&b), ["urgent"], "one label after two imports, named as the target has it");
    assert_eq!(b.doc.mirror.label_def(mine).color, Some(egui::Color32::BLUE));
    let carriers = b.nodes.keys().filter(|n| b.doc.mirror.card_labels(**n) == [mine]).count();
    assert_eq!(carriers, 2, "both imported notes carry the target's label");
}

#[test]
fn a_file_label_with_an_unusable_name_imports_as_its_own_label() {
    let mut b = app_on(&temp_dir("label-blank").join("session.mnf"), 2);
    b.create_label("x", None).unwrap();
    let json: crate::store::json::JsonDoc = serde_json::from_str(
        r#"{"nodes": [{"id": "g", "type": "application/vnd.manifold.group", "name": "G"},
            {"id": "l", "type": "application/vnd.manifold.label", "name": "\u200b"}]}"#,
    )
    .unwrap();
    b.import_group_json(&json);
    assert!(b.label_defs().len() <= 2, "no false match with the live label");
}

/// `app` (actor 1) with a label "urgent" and, from a replica that never saw it (actor 2), a duplicate. Returns
/// (canonical, duplicate): the lower id first.
fn collided(path: &std::path::Path) -> (App, NodeId, NodeId) {
    let mut app = app_on(path, 1);
    let mut other = app_on(path, 2);
    let b = other.create_label("urgent", None).unwrap();
    assert!(other.save_session());
    let a = app.create_label("urgent", None).unwrap();
    assert!(app.save_session());
    assert!(app.doc.mirror.is_stored(b), "the save pulled the other replica's label");
    assert!(a < b);
    (app, a, b)
}

#[test]
fn a_collision_shows_one_row_and_unlabel_clears_a_card_that_carried_only_the_duplicate() {
    let (mut app, a, b) = collided(&temp_dir("label-collide-a").join("session.mnf"));
    assert_eq!(app.label_defs().keys().copied().collect::<Vec<_>>(), [a]);
    let n = add_note(&mut app, "n");
    app.edit_prop(n, field::PROPS, labels_path(b), Some(Leaf::Int(5)));
    assert_eq!(app.doc.mirror.card_labels(n), [a], "one chip, as the canonical label");
    assert!(app.set_label(&[n], a, true).is_empty(), "the card already shows it");
    assert!(!app.set_label(&[n], a, false).is_empty());
    assert!(app.doc.mirror.card_labels(n).is_empty());
}

#[test]
fn renaming_recolouring_and_deleting_reach_every_member() {
    let (mut app, a, b) = collided(&temp_dir("label-collide-b").join("session.mnf"));
    assert!(app.rename_label(a, "URGENT"), "the duplicate is the same label, not another");
    assert!(app.rename_label(a, "later"));
    assert_eq!(app.doc.mirror.label_def(b).name, "later");
    assert_eq!(app.label_defs().keys().copied().collect::<Vec<_>>(), [a], "the duplicate is not promoted");
    app.recolor_label(a, Some(egui::Color32::RED));
    assert_eq!(app.doc.mirror.label_def(b).color, Some(egui::Color32::RED));
    app.delete_label(a);
    assert!(app.label_defs().is_empty() && !app.doc.mirror.is_stored(b));
}

#[test]
fn a_rename_on_another_replica_that_never_saw_the_duplicate_splits_the_group_here() {
    let path = temp_dir("label-collide-split").join("session.mnf");
    let mut app = app_on(&path, 1);
    let mut other = app_on(&path, 2);
    let b = other.create_label("urgent", None).unwrap();
    assert!(other.save_session());
    let a = app.create_label("urgent", None).unwrap();
    assert!(app.save_session());
    assert_eq!(app.label_defs().len(), 1, "grouped here");
    assert!(other.rename_label(b, "later"), "the other replica has not seen a, so b is alone there");
    assert!(other.save_session());
    assert!(app.save_session(), "pulls the rename");
    assert_eq!(app.label_defs().keys().copied().collect::<Vec<_>>(), [a, b], "two labels again");
    assert_eq!(app.doc.mirror.label_def(b).name, "later");
}

#[test]
fn undoing_a_label_delete_brings_back_the_label_and_its_cards_and_redo_removes_it_again() {
    let mut app = app_on(&temp_dir("label-undo").join("session.mnf"), 1);
    let (n1, n2, n3) = (add_note(&mut app, "1"), add_note(&mut app, "2"), add_note(&mut app, "3"));
    let label = app.create_label("urgent", Some(egui::Color32::RED)).unwrap();
    app.set_label(&[n1, n2], label, true);
    let stamp = app.doc.mirror.tree(n1, field::PROPS).unwrap().clone();
    app.delete_label(label);
    assert!(app.label_defs().is_empty());
    app.step_history(None, false);
    let defs = app.label_defs();
    let (id, def) = defs.iter().next().expect("the label is back");
    assert_eq!((def.name.as_str(), def.color), ("urgent", Some(egui::Color32::RED)));
    assert_eq!(app.doc.mirror.card_labels(n1), [*id]);
    assert_eq!(app.doc.mirror.card_labels(n2), [*id]);
    assert!(app.doc.mirror.card_labels(n3).is_empty(), "a card that never had it still does not");
    let restored = app.doc.mirror.tree(n1, field::PROPS).unwrap();
    let stamp_of = |t: &crate::store::tree::Tree, l: NodeId| t.get(&labels_path(l)).cloned();
    assert_eq!(stamp_of(restored, *id), stamp_of(&stamp, label), "the membership keeps its stamp");
    app.step_history(None, true);
    assert!(app.label_defs().is_empty(), "redo deletes it again");
    app.step_history(None, false);
    assert_eq!(app.label_defs().len(), 1, "and undo brings it back once more");
}

#[test]
fn import_group_reuses_the_canonical_label_when_the_target_holds_duplicates() {
    let mut a = app_on(&temp_dir("label-collide-src").join("session.mnf"), 3);
    let json = labelled_group_export(&mut a, "urgent", egui::Color32::RED);
    let (mut b, canonical, _) = collided(&temp_dir("label-collide-d").join("session.mnf"));
    b.import_group_json(&json);
    assert_eq!(b.label_defs().keys().copied().collect::<Vec<_>>(), [canonical]);
    assert!(b.nodes.keys().any(|n| b.doc.mirror.card_labels(*n) == [canonical]), "the imported note carries it");
}

#[test]
fn rename_recolor_and_delete_reach_the_document() {
    let path = temp_dir("label-edit").join("session.mnf");
    let mut app = app_on(&path, 1);
    let a = app.create_label("a", None).unwrap();
    let b = app.create_label("b", None).unwrap();
    assert!(app.rename_label(a, "  c ") && app.save_session());
    assert_eq!(app.doc.mirror.label_def(a).name, "c");
    assert!(!app.rename_label(a, "B"), "another label already has that name key");
    assert!(!app.rename_label(a, " "), "an empty name is refused");
    app.recolor_label(a, Some(egui::Color32::BLUE));
    assert!(app.save_session());
    assert_eq!(app.doc.mirror.label_def(a).color, Some(egui::Color32::BLUE));
    app.delete_label(a);
    assert!(app.save_session());
    assert_eq!(app.label_defs().into_keys().collect::<Vec<_>>(), vec![b]);
    assert!(!app.doc.journal.as_ref().unwrap().doc().nodes[&a].is_live());
}

fn labels_of(app: &App, card: NodeId) -> Vec<NodeId> {
    app.doc.mirror.card_labels(card)
}

#[test]
fn labels_show_in_the_order_they_were_added_and_re_adding_goes_last() {
    let path = temp_dir("label-order").join("session.mnf");
    let mut app = app_on(&path, 1);
    let card = add_note(&mut app, "n");
    let (a, b) = (app.create_label("a", None).unwrap(), app.create_label("b", None).unwrap());
    app.set_label(&[card], b, true);
    app.set_label(&[card], a, true);
    assert_eq!(labels_of(&app, card), vec![b, a]);
    app.set_label(&[card], b, false);
    app.set_label(&[card], b, true);
    assert_eq!(labels_of(&app, card), vec![a, b]);
}

#[test]
fn a_deleted_label_is_not_shown_and_a_repeat_apply_writes_nothing() {
    let path = temp_dir("label-dead").join("session.mnf");
    let mut app = app_on(&path, 1);
    let card = add_note(&mut app, "n");
    let a = app.create_label("a", None).unwrap();
    assert_eq!(app.set_label(&[card], a, true).len(), 1);
    assert!(app.set_label(&[card], a, true).is_empty(), "already applied");
    assert!(app.set_label(&[card], a, false).len() == 1 && app.set_label(&[card], a, false).is_empty());
    app.set_label(&[card], a, true);
    app.delete_label(a);
    assert!(labels_of(&app, card).is_empty());
}

#[test]
fn two_replicas_adding_one_label_converge_to_one_entry() {
    let path = temp_dir("label-merge").join("session.mnf");
    let mut app = app_on(&path, 1);
    let card = add_note(&mut app, "n");
    let a = app.create_label("a", None).unwrap();
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set_tree(&mut other, card, field::PROPS, labels_path(a), Some(Leaf::Int(5)));
    app.set_label(&[card], a, true);
    assert!(app.save_session());
    assert_eq!(labels_of(&app, card), vec![a]);
    assert_eq!(snapshots(&app)[&card].trees, super::props::doc_trees(&app, card));
}

#[test]
fn a_duplicate_of_a_labelled_card_keeps_its_labels() {
    let path = temp_dir("label-copy").join("session.mnf");
    let mut app = app_on(&path, 1);
    let card = add_note(&mut app, "n");
    let a = app.create_label("a", None).unwrap();
    app.set_label(&[card], a, true);
    app.duplicate_nodes(&[card]);
    let copy =
        *app.nodes.keys().find(|&&id| id != card && labels_of(&app, id) == vec![a]).expect("the copy has the label");
    assert_ne!(copy, card);
}

#[test]
fn a_label_edit_undoes_and_redoes_as_one_step() {
    let path = temp_dir("label-undo").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (c1, c2) = (add_note(&mut app, "1"), add_note(&mut app, "2"));
    let a = app.create_label("a", None).unwrap();
    let edits = app.set_label(&[c1, c2], a, true);
    app.record_label_undo("Label 2 cards", edits);
    app.step_history(None, false);
    assert!(labels_of(&app, c1).is_empty() && labels_of(&app, c2).is_empty());
    app.step_history(None, true);
    assert_eq!((labels_of(&app, c1), labels_of(&app, c2)), (vec![a], vec![a]));
}

#[test]
fn a_labelled_cards_view_carries_the_first_labels_colour_and_a_deleted_label_disappears() {
    let mut app = App::default();
    let note = add_note(&mut app, "n");
    let (red, blue) = (egui::Color32::RED, egui::Color32::BLUE);
    let a = app.create_label("a", Some(red)).unwrap();
    let b = app.create_label("b", Some(blue)).unwrap();
    app.set_label(&[note], a, true);
    app.set_label(&[note], b, true);
    let entry = crate::level::CardEntry {
        id: note.into(),
        kind: app.card_kind(note).unwrap(),
        origin: crate::level::Origin::Stored,
        rect: egui::Rect::ZERO,
    };
    let view = app.card_view(&entry, &HashMap::new()).unwrap();
    assert_eq!(view.color, Some(red));
    assert_eq!(view.labels.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(), ["a", "b"]);
    app.delete_label(a);
    let view = app.card_view(&entry, &HashMap::new()).unwrap();
    assert_eq!((view.color, view.labels.len(), view.labels_more), (Some(blue), 1, 0));
}

fn select(app: &mut App, ids: &[NodeId]) {
    app.interaction.selected = ids.iter().map(|&id| crate::card_id::CardId::from(id)).collect();
}

fn act(app: &mut App, action: crate::graph_ui::GraphAction) {
    app.apply_graph_action(&egui::Context::default(), action);
}

#[test]
fn applying_a_label_to_the_selection_is_one_undo_step() {
    use crate::graph_ui::GraphAction;
    let mut app = app_on(&temp_dir("label-bar").join("session.mnf"), 1);
    let ids: Vec<NodeId> = (0..3).map(|i| add_note(&mut app, &i.to_string())).collect();
    select(&mut app, &ids);
    act(&mut app, GraphAction::NewLabel("urgent".into()));
    let a = *app.label_defs().keys().next().unwrap();
    assert!(ids.iter().all(|&c| labels_of(&app, c) == vec![a]), "a new label applies to the selection");
    app.step_history(None, false);
    assert!(ids.iter().all(|&c| labels_of(&app, c).is_empty()), "one undo clears all three");
    assert!(app.label_defs().contains_key(&a), "and leaves the label");
    app.step_history(None, true);
    assert!(ids.iter().all(|&c| labels_of(&app, c) == vec![a]), "one redo puts it back");
    let toast = app.toasts.undo.as_ref().map(|(m, ..)| m.clone());
    act(&mut app, GraphAction::ToggleLabel { label: a, on: true });
    assert_eq!(app.toasts.undo.as_ref().map(|(m, ..)| m.clone()), toast, "a no-op records nothing");
}

#[test]
fn the_menu_row_state_is_all_some_or_none_of_the_selection() {
    use crate::labels::LabelState;
    let mut app = app_on(&temp_dir("label-state").join("session.mnf"), 1);
    let (c1, c2) = (add_note(&mut app, "1"), add_note(&mut app, "2"));
    let a = app.create_label("a", None).unwrap();
    select(&mut app, &[c1, c2]);
    assert_eq!(app.label_state(a), LabelState::None);
    app.set_label(&[c1], a, true);
    assert_eq!(app.label_state(a), LabelState::Some);
    app.set_label(&[c2], a, true);
    assert_eq!(app.label_state(a), LabelState::All);
    select(&mut app, &[]);
    assert_eq!(app.label_state(a), LabelState::None, "nothing selected: nothing is checked");
}

#[test]
fn the_menu_renames_recolours_and_deletes_a_label() {
    use crate::graph_ui::GraphAction;
    let mut app = app_on(&temp_dir("label-menu-edit").join("session.mnf"), 1);
    let card = add_note(&mut app, "n");
    let a = app.create_label("a", Some(egui::Color32::RED)).unwrap();
    app.set_label(&[card], a, true);
    act(&mut app, GraphAction::RenameLabel { label: a, name: "renamed".into() });
    act(&mut app, GraphAction::RecolorLabel { label: a, color: Some(egui::Color32::BLUE) });
    let def = app.doc.mirror.label_def(a);
    assert_eq!((def.name.as_str(), def.color), ("renamed", Some(egui::Color32::BLUE)));
    act(&mut app, GraphAction::DeleteLabel(a));
    assert!(labels_of(&app, card).is_empty(), "a deleted label leaves its card without a chip");
}

#[test]
fn a_jar_card_keeps_its_own_colour_for_its_border() {
    let mut app = App::default();
    let jar = add_jar(&mut app, "Work");
    app.edit_field(jar, field::COLOR, super::super::color_value(Some(egui::Color32::GREEN)));
    let entry = crate::level::CardEntry {
        id: jar.into(),
        kind: app.card_kind(jar).unwrap(),
        origin: crate::level::Origin::Stored,
        rect: egui::Rect::ZERO,
    };
    let view = app.card_view(&entry, &HashMap::new()).unwrap();
    assert_eq!(view.color, Some(egui::Color32::GREEN));
}

#[test]
fn a_labelled_card_keeps_its_labels_through_close_and_reopen() {
    let path = temp_dir("label-close").join("session.mnf");
    let mut app = app_on(&path, 1);
    let card = add_note(&mut app, "n");
    let a = app.create_label("a", None).unwrap();
    app.set_label(&[card], a, true);
    app.close_node(card);
    app.reopen_node(card);
    assert_eq!(labels_of(&app, card), vec![a]);
    assert!(app.save_session());
    assert_eq!(snapshots(&app)[&card].trees, super::props::doc_trees(&app, card));
}

#[test]
fn a_remote_remove_and_a_local_add_of_one_label_converge_to_the_document() {
    let path = temp_dir("label-add-remove").join("session.mnf");
    let mut app = app_on(&path, 1);
    let card = add_note(&mut app, "n");
    let a = app.create_label("a", None).unwrap();
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set_tree(&mut other, card, field::PROPS, labels_path(a), None);
    app.set_label(&[card], a, true);
    assert!(app.save_session());
    assert_eq!(snapshots(&app)[&card].trees, super::props::doc_trees(&app, card));
}
