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
