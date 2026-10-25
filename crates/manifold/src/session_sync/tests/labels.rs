//! Session-sync tests: labels, the stored-only nodes cards can carry.

use super::*;

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
