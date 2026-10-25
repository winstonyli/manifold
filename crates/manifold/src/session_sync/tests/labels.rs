//! Session-sync tests: labels, the stored-only nodes cards can carry.

use super::*;

impl App {
    /// A label made the way `create_link` makes a link (Task 3's `create_label` replaces this).
    fn test_stored_label(&mut self, name: &str) -> NodeId {
        let id = self.alloc_node_id();
        self.doc.mirror.add_stored(id, Kind::Label);
        self.queue_create(id);
        self.edit_field(id, field::NAME, FieldValue::Str(name.to_owned()));
        id
    }
}

#[test]
fn a_label_syncs_as_a_stored_node_and_is_never_a_card() {
    let path = temp_dir("label-sync").join("session.mnf");
    let mut app = app_on(&path, 1);
    let label = app.test_stored_label("red things");
    assert!(app.save_session());
    assert!(app.doc.mirror.is_stored(label) && !app.nodes.contains_key(&label), "mirror only");
    let mut other = app_on(&path, 2);
    other.restore_session();
    assert_eq!(other.doc.mirror.label_def(label).name, "red things");
    assert!(other.doc.mirror.is_stored(label) && !other.nodes.contains_key(&label));
    assert!(other.doc.mirror.link_ids().next().is_none(), "a label is not a link");
}
