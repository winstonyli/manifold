use serde_json::{json, Value};

use super::*;
use crate::store::body::{tree_to_value, value_to_tree};
use crate::store::model::field;

/// An app with one inline card holding `value`, shown.
fn card(value: Value) -> (App, NodeId) {
    let mut app = App::default();
    let id = app.alloc_node_id();
    app.insert_inline_json_at(id, None);
    let mut mint = |_: &str| app.alloc_node_id();
    let tree = value_to_tree(&value, &mut mint).unwrap();
    app.set_body(id, tree);
    app.refresh_inline(id);
    (app, id)
}

fn doc(app: &App, id: NodeId) -> Value {
    tree_to_value(app.doc.mirror.tree(id, field::BODY).unwrap())
}

fn shown(app: &App, id: NodeId) -> Value {
    (*app.nodes[&id].as_json().unwrap().value).clone()
}

fn set(pointer: &str, value: Value) -> JsonOp {
    JsonOp::Set { pointer: pointer.into(), value }
}

fn remove(pointer: &str) -> JsonOp {
    JsonOp::Remove { pointer: pointer.into() }
}

fn insert(pointer: &str, index: usize, value: Value) -> JsonOp {
    JsonOp::Insert { pointer: pointer.into(), index, value }
}

fn undo(app: &mut App) {
    app.step_history(None, false);
}

#[test]
fn set_replaces_a_scalar_and_undo_restores_it() {
    let (mut app, id) = card(json!({"a": 1, "b": 2}));
    app.edit_json(id, set("/a", json!("x"))).unwrap();
    assert_eq!(doc(&app, id), json!({"a": "x", "b": 2}));
    assert_eq!(shown(&app, id), json!({"a": "x", "b": 2}));
    undo(&mut app);
    assert_eq!(doc(&app, id), json!({"a": 1, "b": 2}));
    assert_eq!(shown(&app, id), json!({"a": 1, "b": 2}));
}

#[test]
fn set_retypes_a_container_to_a_scalar_and_undo_restores_every_descendant() {
    let original = json!({"a": {"x": [1, {"y": 2}], "z": "s"}});
    let (mut app, id) = card(original.clone());
    app.edit_json(id, set("/a", json!(5))).unwrap();
    assert_eq!(doc(&app, id), json!({"a": 5}));
    undo(&mut app);
    assert_eq!(doc(&app, id), original);
    app.edit_json(id, set("/a/z", json!([1, 2]))).unwrap();
    assert_eq!(doc(&app, id), json!({"a": {"x": [1, {"y": 2}], "z": [1, 2]}}));
    undo(&mut app);
    assert_eq!(doc(&app, id), original);
}

#[test]
fn add_key_to_an_empty_object_and_to_a_populated_one() {
    let (mut app, id) = card(json!({}));
    let add =
        |pointer: &str, key: &str, value: Value| JsonOp::AddKey { pointer: pointer.into(), key: key.into(), value };
    app.edit_json(id, add("", "a", json!(1))).unwrap();
    app.edit_json(id, add("", "b", json!({"c": []}))).unwrap();
    assert_eq!(doc(&app, id), json!({"a": 1, "b": {"c": []}}));
    assert!(app.edit_json(id, add("", "a", json!(2))).is_err(), "a duplicate key is refused");
    assert_eq!(doc(&app, id), json!({"a": 1, "b": {"c": []}}));
    undo(&mut app);
    undo(&mut app);
    assert_eq!(doc(&app, id), json!({}));
}

#[test]
fn remove_the_last_member_leaves_an_empty_object() {
    let (mut app, id) = card(json!({"a": {"only": 1}, "b": 2}));
    app.edit_json(id, remove("/a/only")).unwrap();
    assert_eq!(doc(&app, id), json!({"a": {}, "b": 2}));
    app.edit_json(id, remove("/b")).unwrap();
    assert_eq!(doc(&app, id), json!({"a": {}}));
    undo(&mut app);
    undo(&mut app);
    assert_eq!(doc(&app, id), json!({"a": {"only": 1}, "b": 2}));
}

#[test]
fn remove_the_last_element_leaves_an_empty_array() {
    let (mut app, id) = card(json!({"l": [1, 2]}));
    app.edit_json(id, remove("/l/0")).unwrap();
    assert_eq!(doc(&app, id), json!({"l": [2]}));
    app.edit_json(id, remove("/l/0")).unwrap();
    assert_eq!(doc(&app, id), json!({"l": []}));
    undo(&mut app);
    undo(&mut app);
    assert_eq!(doc(&app, id), json!({"l": [1, 2]}));
}

#[test]
fn insert_at_each_end_and_between_orders_the_array() {
    let (mut app, id) = card(json!([2, 4]));
    app.edit_json(id, insert("", 0, json!(1))).unwrap();
    app.edit_json(id, insert("", 2, json!(3))).unwrap();
    app.edit_json(id, insert("", 4, json!(5))).unwrap();
    assert_eq!(doc(&app, id), json!([1, 2, 3, 4, 5]));
    let (mut empty, e) = card(json!([]));
    empty.edit_json(e, insert("", 0, json!("a"))).unwrap();
    assert_eq!(doc(&empty, e), json!(["a"]));
    undo(&mut app);
    assert_eq!(doc(&app, id), json!([1, 2, 3, 4]));
}

#[test]
fn move_reorders_one_register_and_undo_puts_it_back() {
    let (mut app, id) = card(json!(["a", "b", "c", "d"]));
    let mv = |pointer: &str, to: usize| JsonOp::Move { pointer: pointer.into(), to };
    let before = app.doc.mirror.tree(id, field::BODY).unwrap().clone();
    app.edit_json(id, mv("/0", 2)).unwrap();
    assert_eq!(doc(&app, id), json!(["b", "c", "a", "d"]));
    let after = app.doc.mirror.tree(id, field::BODY).unwrap();
    let changed = after.iter().filter(|(p, l)| before.get(*p) != Some(*l)).count();
    assert_eq!(changed, 1, "one order register moved");
    app.edit_json(id, mv("/3", 0)).unwrap();
    assert_eq!(doc(&app, id), json!(["d", "b", "c", "a"]));
    undo(&mut app);
    undo(&mut app);
    assert_eq!(*app.doc.mirror.tree(id, field::BODY).unwrap(), before);
}

#[test]
fn insert_between_equal_order_keys_renumbers_the_array() {
    let (mut app, id) = card(json!(["a", "b"]));
    let tree = app.doc.mirror.tree(id, field::BODY).unwrap().clone();
    let orders: Vec<TreePath> = elements(&tree, &body::root()).iter().map(|e| e.at.child(&field_key("o"))).collect();
    for p in &orders {
        app.edit_prop(id, field::BODY, p.clone(), Some(Leaf::Str("00005000".into())));
    }
    app.refresh_inline(id);
    app.edit_json(id, insert("", 1, json!("m"))).unwrap();
    let value = doc(&app, id);
    let items = value.as_array().unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(items[1], json!("m"), "the new element lands at the asked index");
    let sorted: Vec<String> =
        elements(app.doc.mirror.tree(id, field::BODY).unwrap(), &body::root()).into_iter().map(|e| e.order).collect();
    assert!(sorted.windows(2).all(|w| w[0] < w[1]), "renumbered keys are strictly ordered: {sorted:?}");
    undo(&mut app);
    assert_eq!(doc(&app, id).as_array().unwrap().len(), 2);
}

#[test]
fn rename_moves_the_subtree_and_refuses_an_existing_key() {
    let (mut app, id) = card(json!({"a": {"x": [1, 2]}, "b": 0}));
    let rename = |pointer: &str, to: &str| JsonOp::Rename { pointer: pointer.into(), to: to.into() };
    app.edit_json(id, rename("/a", "c")).unwrap();
    assert_eq!(doc(&app, id), json!({"c": {"x": [1, 2]}, "b": 0}));
    assert!(app.edit_json(id, rename("/c", "b")).is_err());
    assert_eq!(doc(&app, id), json!({"c": {"x": [1, 2]}, "b": 0}));
    undo(&mut app);
    assert_eq!(doc(&app, id), json!({"a": {"x": [1, 2]}, "b": 0}));
}

#[test]
fn edits_refuse_a_file_card_a_stale_pointer_and_the_root_delete() {
    let (mut app, id) = card(json!({"a": [1]}));
    for op in
        [set("/nope", json!(1)), remove("/a/7"), remove(""), insert("/a", 9, json!(1)), insert("/zzz", 0, json!(1))]
    {
        assert!(app.edit_json(id, op.clone()).is_err(), "{op:?}");
    }
    assert_eq!(doc(&app, id), json!({"a": [1]}));
    assert!(app.edit_json(NodeId { actor: crate::store::model::ActorId(9), counter: 99 }, set("", json!(1))).is_err());
}

#[test]
fn an_edit_that_changes_nothing_records_no_undo_step() {
    let (mut app, id) = card(json!({"a": 1}));
    assert!(app.toasts.undo.is_none());
    app.edit_json(id, set("/a", json!(1))).unwrap();
    assert!(app.toasts.undo.is_none(), "no step was offered");
}

#[test]
fn an_insert_past_the_leaf_cap_changes_nothing() {
    let (mut app, id) = card(Value::Array((0..4999).map(Value::from).collect()));
    let before = app.doc.mirror.tree(id, field::BODY).unwrap().clone();
    assert!(app.edit_json(id, insert("", 0, json!(1))).is_err());
    assert_eq!(*app.doc.mirror.tree(id, field::BODY).unwrap(), before);
}

#[test]
fn an_undo_is_skipped_when_a_remote_edit_touched_its_path() {
    let (mut app, id) = card(json!({"a": 1}));
    app.edit_json(id, set("/a", json!(2))).unwrap();
    let path = pointer_path(app.doc.mirror.tree(id, field::BODY).unwrap(), "/a").unwrap();
    app.doc.mirror.edit_tree(id, field::BODY, &path, Some(Leaf::Int(99)));
    undo(&mut app);
    assert_eq!(doc(&app, id), json!({"a": 99}));
}

#[test]
fn two_replicas_inserting_into_one_slot_converge() {
    let dir = crate::store::testutil::temp_dir("json-edit-converge");
    let (mut a, id) = card(json!([1, 3]));
    let mut b = crate::session_sync::tests::app_on(&dir.join("b.manifold"), 2);
    b.apply_remote(vec![(id, crate::session_sync::Remote::Shown(a.snapshot(id).unwrap()))]);
    a.edit_json(id, insert("", 1, json!("from a"))).unwrap();
    b.edit_json(id, insert("", 1, json!("from b"))).unwrap();
    let mut merged = a.doc.mirror.tree(id, field::BODY).unwrap().clone();
    merged.extend(b.doc.mirror.tree(id, field::BODY).unwrap().clone());
    let value = tree_to_value(&merged);
    let items = value.as_array().unwrap();
    assert_eq!(items.len(), 4, "{value}");
    assert!(items.contains(&json!("from a")) && items.contains(&json!("from b")));
    assert_eq!((&items[0], &items[3]), (&json!(1), &json!(3)));
}
