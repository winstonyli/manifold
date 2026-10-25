use serde_json::{json, Value};

use super::*;
use crate::json_level::JsonEdit;
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

fn s(text: &str) -> String {
    text.to_owned()
}

#[test]
fn remap_pointer_follows_each_structural_edit() {
    use PointerMap::*;
    let remove = [Drop(s("/a/1")), Shift { parent: s("/a"), from: 2, delta: -1 }];
    assert_eq!(remap_pointer(&remove, "/a/1/b"), None);
    assert_eq!(remap_pointer(&remove, "/a/0"), Some(s("/a/0")));
    assert_eq!(remap_pointer(&remove, "/a/3/b"), Some(s("/a/2/b")));
    assert_eq!(remap_pointer(&remove, "/c/3"), Some(s("/c/3")));
    let insert = [Shift { parent: s("/a"), from: 3, delta: 1 }];
    assert_eq!(remap_pointer(&insert, "/a/3/b"), Some(s("/a/4/b")));
    assert_eq!(remap_pointer(&insert, "/a/2"), Some(s("/a/2")));
    assert_eq!(remap_pointer(&insert, "/a"), Some(s("/a")));
    assert_eq!(remap_pointer(&insert, ""), Some(s("")));
    let down = [Permute { parent: s("/a"), from: 1, to: 3 }];
    let seen: Vec<_> = (0..5).map(|i| remap_pointer(&down, &format!("/a/{i}")).unwrap()).collect();
    assert_eq!(seen, ["/a/0", "/a/3", "/a/1", "/a/2", "/a/4"]);
    let up = [Permute { parent: s("/a"), from: 3, to: 1 }];
    let seen: Vec<_> = (0..5).map(|i| remap_pointer(&up, &format!("/a/{i}")).unwrap()).collect();
    assert_eq!(seen, ["/a/0", "/a/2", "/a/3", "/a/1", "/a/4"]);
    let rename = [Prefix { old: s("/a"), new: s("/c/d") }];
    assert_eq!(remap_pointer(&rename, "/a"), Some(s("/c/d")));
    assert_eq!(remap_pointer(&rename, "/a/x"), Some(s("/c/d/x")));
    assert_eq!(remap_pointer(&rename, "/ab"), Some(s("/ab")));
    let escaped = [Shift { parent: s("/a~1b"), from: 0, delta: 1 }];
    assert_eq!(remap_pointer(&escaped, "/a~1b/0/x~0y"), Some(s("/a~1b/1/x~0y")));
}

fn view_of(app: &App, id: NodeId, pointer: &str) -> Option<bool> {
    app.doc.mirror.view(id)?.get(pointer)?.collapsed
}

#[test]
fn inserting_an_element_keeps_a_later_cards_position() {
    let (mut app, id) = card(json!({"a": [{"k": 1}, {"k": 2}, {"k": 3}]}));
    app.edit_view(id, r#"{"/a/2":{"collapsed":true}}"#);
    app.edit_json(id, insert("/a", 0, json!({"k": 0}))).unwrap();
    assert_eq!(view_of(&app, id, "/a/3"), Some(true));
    assert_eq!(view_of(&app, id, "/a/2"), None);
}

#[test]
fn deleting_an_element_drops_its_card_override_and_selection() {
    let (mut app, id) = card(json!({"a": [1, 2, 3]}));
    app.edit_view(id, r#"{"/a/1":{"collapsed":true},"/a/2":{"collapsed":true}}"#);
    app.current_group = Some(id);
    app.sync_json_level();
    app.json_level.as_mut().unwrap().selected = Some(s("/a/1"));
    app.json_level.as_mut().unwrap().outline.open.insert(s("/a/2"));
    app.edit_json(id, remove("/a/1")).unwrap();
    assert_eq!(view_of(&app, id, "/a/1"), Some(true), "the old /a/2 is now /a/1");
    assert_eq!(view_of(&app, id, "/a/2"), None);
    let level = app.json_level.as_ref().unwrap();
    assert_eq!(level.selected, None);
    assert!(level.outline.open.contains("/a/1") && !level.outline.open.contains("/a/2"));
}

#[test]
fn rename_and_move_carry_the_view_and_the_selection() {
    let (mut app, id) = card(json!({"a": {"x": 1}, "l": [1, 2, 3]}));
    app.edit_view(id, r#"{"/a":{"collapsed":true},"/l/0":{"collapsed":true}}"#);
    app.current_group = Some(id);
    app.sync_json_level();
    app.json_level.as_mut().unwrap().selected = Some(s("/a/x"));
    app.edit_json(id, JsonOp::Rename { pointer: s("/a"), to: s("c") }).unwrap();
    assert_eq!(view_of(&app, id, "/c"), Some(true));
    assert_eq!(app.json_level.as_ref().unwrap().selected.as_deref(), Some("/c/x"));
    app.edit_json(id, JsonOp::Move { pointer: s("/l/0"), to: 2 }).unwrap();
    assert_eq!(view_of(&app, id, "/l/2"), Some(true));
}

#[test]
fn json_edit_verb_parses_every_form() {
    let ok = |line: &str| parse_json_edit(line);
    assert_eq!(ok(r#"set /a/0 {"x": [1, 2]}"#), Ok(set("/a/0", json!({"x": [1, 2]}))));
    assert_eq!(ok("set root 5"), Ok(set("", json!(5))));
    assert_eq!(ok(r#"add root k "v w""#), Ok(JsonOp::AddKey { pointer: s(""), key: s("k"), value: json!("v w") }));
    assert_eq!(ok("insert /b 2 null"), Ok(insert("/b", 2, Value::Null)));
    assert_eq!(ok("remove /a/b"), Ok(remove("/a/b")));
    assert_eq!(ok("rename /a c"), Ok(JsonOp::Rename { pointer: s("/a"), to: s("c") }));
    assert_eq!(ok("move /l/1 0"), Ok(JsonOp::Move { pointer: s("/l/1"), to: 0 }));
    for bad in
        ["", "set", "set /a", "set /a {nope", "insert /b x 1", "move /l/1 -1", "add root k", "frobnicate /a", "remove"]
    {
        assert!(ok(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn the_level_export_says_whether_it_is_editable() {
    let (mut app, id) = card(json!({"a": 1}));
    app.current_group = Some(id);
    app.sync_json_level();
    assert_eq!(app.json_level_state()["editable"], json!(true));
}

#[test]
fn parse_edit_takes_text_by_the_originals_kind() {
    assert_eq!(parse_edit(&json!("old"), "12 apples"), Ok(json!("12 apples")));
    assert_eq!(parse_edit(&json!("old"), ""), Ok(json!("")));
    assert_eq!(parse_edit(&json!(1), " 2.5 "), Ok(json!(2.5)));
    assert_eq!(parse_edit(&json!(1), "true"), Ok(json!(true)));
    assert_eq!(parse_edit(&json!(true), "null"), Ok(Value::Null));
    assert_eq!(parse_edit(&json!(null), "\"now a string\""), Ok(json!("now a string")));
    for bad in ["", "abc", "{\"a\":1}", "[1]", "1 2"] {
        assert_eq!(parse_edit(&json!(1), bad), Err("Not a JSON value".to_owned()), "{bad:?}");
    }
}

#[test]
fn type_default_carries_what_it_can() {
    use JsonType::*;
    assert_eq!(type_default(String, &json!(12)), json!("12"));
    assert_eq!(type_default(String, &json!(true)), json!("true"));
    assert_eq!(type_default(String, &json!({"a": 1})), json!(""));
    assert_eq!(type_default(Number, &json!("2.5")), json!(2.5));
    assert_eq!(type_default(Number, &json!("x")), json!(0));
    assert_eq!(type_default(Number, &json!(7)), json!(7));
    assert_eq!(type_default(Boolean, &json!(true)), json!(true));
    assert_eq!(type_default(Boolean, &json!("true")), json!(true));
    assert_eq!(type_default(Boolean, &json!(3)), json!(false));
    assert_eq!(type_default(Null, &json!(3)), Value::Null);
    assert_eq!(type_default(Object, &json!([1])), json!({}));
    assert_eq!(type_default(Array, &json!({"a": 1})), json!([]));
}

#[test]
fn the_menu_rows_depend_on_the_entry() {
    let object = json!({"a": 1});
    let array = json!([1]);
    let scalar = json!(1);
    assert_eq!(json_menu_rows(true, &object, true, false), ["Add key", "Type"]);
    assert_eq!(json_menu_rows(true, &object, false, false), ["Add key", "Type", "Delete"]);
    assert_eq!(json_menu_rows(true, &array, false, false), ["Add element", "Type", "Delete"]);
    assert_eq!(
        json_menu_rows(true, &scalar, false, true),
        ["Insert above", "Insert below", "Move up", "Move down", "Type", "Delete"]
    );
    assert_eq!(json_menu_rows(true, &array, false, true)[..2], ["Add element", "Insert above"]);
    assert!(json_menu_rows(false, &object, false, false).is_empty(), "a file level edits nothing");
}

/// An app on `id`'s level, opened the way a click does.
fn open_level(app: &mut App, id: NodeId) {
    app.current_group = Some(id);
    app.sync_json_level();
}

fn edit_of(app: &App) -> Option<(String, String)> {
    app.json_level.as_ref().unwrap().edit.as_ref().map(|e| (e.pointer.clone(), e.buffer.clone()))
}

#[test]
fn begin_opens_a_value_or_key_edit_on_the_right_entries() {
    let (mut app, id) = card(json!({"a": "x", "b": {"c": 1}, "l": [true]}));
    open_level(&mut app, id);
    for (pointer, what, expect) in [
        ("/a", EditWhat::Value, Some(("/a", "x"))),
        ("/l/0", EditWhat::Value, Some(("/l/0", "true"))),
        ("/a", EditWhat::Key, Some(("/a", "a"))),
        ("/b", EditWhat::Value, None),
        ("/l/0", EditWhat::Key, None),
        ("", EditWhat::Key, None),
        ("/nope", EditWhat::Value, None),
    ] {
        app.apply_json_edit(JsonEdit::CancelEdit);
        app.apply_json_edit(JsonEdit::Begin(pointer.into(), what));
        assert_eq!(edit_of(&app), expect.map(|(p, b)| (p.to_owned(), b.to_owned())), "{pointer} {what:?}");
    }
}

#[test]
fn commit_applies_one_set_and_clears_the_edit() {
    let (mut app, id) = card(json!({"a": 1}));
    open_level(&mut app, id);
    app.apply_json_edit(JsonEdit::Begin(s("/a"), EditWhat::Value));
    app.json_level.as_mut().unwrap().edit.as_mut().unwrap().buffer = s("42");
    app.apply_json_edit(JsonEdit::Commit);
    assert_eq!(doc(&app, id), json!({"a": 42}));
    assert_eq!(edit_of(&app), None);
    undo(&mut app);
    assert_eq!(doc(&app, id), json!({"a": 1}));
}

#[test]
fn a_bad_value_keeps_the_editor_open_and_writes_nothing() {
    let (mut app, id) = card(json!({"a": 1}));
    open_level(&mut app, id);
    app.apply_json_edit(JsonEdit::Begin(s("/a"), EditWhat::Value));
    app.json_level.as_mut().unwrap().edit.as_mut().unwrap().buffer = s("oops");
    app.apply_json_edit(JsonEdit::Commit);
    assert_eq!(doc(&app, id), json!({"a": 1}));
    assert_eq!(edit_of(&app), Some((s("/a"), s("oops"))));
}

#[test]
fn a_key_commit_renames_and_a_taken_name_changes_nothing() {
    let (mut app, id) = card(json!({"a": 1, "b": 2}));
    open_level(&mut app, id);
    app.apply_json_edit(JsonEdit::Begin(s("/a"), EditWhat::Key));
    app.json_level.as_mut().unwrap().edit.as_mut().unwrap().buffer = s("c");
    app.apply_json_edit(JsonEdit::Commit);
    assert_eq!(doc(&app, id), json!({"c": 1, "b": 2}));
    app.apply_json_edit(JsonEdit::Begin(s("/c"), EditWhat::Key));
    app.json_level.as_mut().unwrap().edit.as_mut().unwrap().buffer = s("b");
    app.apply_json_edit(JsonEdit::Commit);
    assert_eq!(doc(&app, id), json!({"c": 1, "b": 2}));
    assert_eq!(edit_of(&app), None);
}

#[test]
fn a_commit_after_the_entry_changed_underneath_is_cancelled() {
    let (mut app, id) = card(json!({"a": [10, 11, 12]}));
    open_level(&mut app, id);
    app.apply_json_edit(JsonEdit::Begin(s("/a/1"), EditWhat::Value));
    app.json_level.as_mut().unwrap().edit.as_mut().unwrap().buffer = s("99");
    app.edit_json(id, insert("/a", 0, json!(9))).unwrap();
    assert_eq!(edit_of(&app), None, "the swap cancelled it: /a/1 now holds another value");
    app.apply_json_edit(JsonEdit::Commit);
    assert_eq!(doc(&app, id), json!({"a": [9, 10, 11, 12]}), "no extra change");
}

#[test]
fn op_then_opens_the_next_edit_after_the_op_landed() {
    let (mut app, id) = card(json!({"a": 1}));
    open_level(&mut app, id);
    let add = JsonOp::AddKey { pointer: s(""), key: s("new"), value: Value::Null };
    app.apply_json_edit(JsonEdit::Op(add, Some((s("/new"), EditWhat::Key))));
    assert_eq!(doc(&app, id), json!({"a": 1, "new": null}));
    assert_eq!(edit_of(&app), Some((s("/new"), s("new"))));
}

#[test]
fn menu_rows_become_ops_with_the_edit_to_open_next() {
    use crate::json_edit::{menu_op, EditWhat};
    let doc = json!({"a": 1, "new": 2, "l": [10, 20]});
    let row = |r: &str, p: &str| menu_op(r, &doc, p);
    // The first free name, then a key edit on it.
    let (op, next) = row("Add key", "").unwrap();
    assert_eq!(op, JsonOp::AddKey { pointer: "".into(), key: "new2".into(), value: Value::Null });
    assert_eq!(next, Some(("/new2".into(), EditWhat::Key)));
    // Appending edits the new element's value.
    let (op, next) = row("Add element", "/l").unwrap();
    assert_eq!(op, JsonOp::Insert { pointer: "/l".into(), index: 2, value: Value::Null });
    assert_eq!(next, Some(("/l/2".into(), EditWhat::Value)));
    assert_eq!(row("Insert above", "/l/1").unwrap().1, Some(("/l/1".into(), EditWhat::Value)));
    assert_eq!(row("Insert below", "/l/1").unwrap().1, Some(("/l/2".into(), EditWhat::Value)));
    // The ends of an array cannot move further.
    assert!(row("Move up", "/l/0").is_none() && row("Move down", "/l/1").is_none());
    assert_eq!(row("Move down", "/l/0").unwrap().0, JsonOp::Move { pointer: "/l/0".into(), to: 1 });
    assert_eq!(row("Delete", "/a").unwrap().0, JsonOp::Remove { pointer: "/a".into() });
    assert!(row("Add key", "/a").is_none(), "a scalar takes no key");
}

/// An inline card `{"x": 0, "l": [0]}` whose `/x` and `/l/0` hold `Ref`s to `targets`, shown as a level.
fn card_with_refs(targets: [NodeId; 2]) -> (App, NodeId) {
    let (mut app, id) = card(json!({"x": 0, "l": [0]}));
    for (pointer, target) in ["/x", "/l/0"].into_iter().zip(targets) {
        let tree = app.doc.mirror.tree(id, field::BODY).cloned().unwrap();
        let path = crate::store::body::pointer_path(&tree, pointer).unwrap();
        app.edit_prop(id, field::BODY, path, Some(crate::store::tree::Leaf::Ref(target)));
    }
    app.refresh_inline(id);
    app.current_group = Some(id);
    app.sync_json_level();
    (app, id)
}

#[test]
fn chips_show_the_target_title_or_missing() {
    let probe = card(json!(null)).0;
    let gone = NodeId { actor: crate::store::model::ActorId(9), counter: 9 };
    assert!(!probe.nodes.contains_key(&gone));
    let (mut app, _) = card_with_refs([gone, gone]);
    let note = crate::session_sync::tests::add_note(&mut app, "n");
    app.edit_field(note, field::NAME, crate::store::model::FieldValue::Str("N".into()));
    let id = app.json_level.as_ref().unwrap().node_id;
    let tree = app.doc.mirror.tree(id, field::BODY).cloned().unwrap();
    let path = crate::store::body::pointer_path(&tree, "/l/0").unwrap();
    app.edit_prop(id, field::BODY, path, Some(crate::store::tree::Leaf::Ref(note)));
    app.refresh_inline(id);
    let level = app.json_level.as_ref().unwrap();
    assert_eq!(level.value["x"], json!(gone.to_string()), "the level shows the id string");
    let root = &level.cards[0];
    let chips = app.ref_chips(&level.refs, &level.value, root);
    let shown: Vec<(usize, &str, bool)> = chips.iter().map(|(r, c)| (*r, c.text.as_str(), c.missing)).collect();
    assert_eq!(shown.len(), 1, "only the root card's own rows: {shown:?}");
    assert_eq!(shown[0], (1, "missing", true), "/x is entry 1 of the root ({{\"l\", \"x\"}} sorted)");
    let l = level.cards.iter().find(|c| c.pointer == "/l").unwrap();
    let chips = app.ref_chips(&level.refs, level.value.pointer("/l").unwrap(), l);
    assert_eq!((chips[0].0, chips[0].1.text.as_str(), chips[0].1.missing), (0, "N", false));
}

#[test]
fn a_ref_row_does_not_open_a_value_edit_and_a_chip_opens_its_target() {
    let target = NodeId { actor: crate::store::model::ActorId(1), counter: 77 };
    let (mut app, _) = card_with_refs([target, target]);
    app.begin_json_edit("/x".into(), EditWhat::Value);
    assert!(app.json_level.as_ref().unwrap().edit.is_none(), "a link is not typed over");
    app.begin_json_edit("/x".into(), EditWhat::Key);
    assert!(app.json_level.as_ref().unwrap().edit.is_some(), "its key still renames");
    // A click on a chip whose target is gone does nothing; on a live one it leaves the level and selects it.
    app.apply_json_edit(JsonEdit::OpenRef(target));
    assert!(app.json_level.is_some());
    let note = crate::session_sync::tests::add_note(&mut app, "n");
    app.apply_json_edit(JsonEdit::OpenRef(note));
    assert!(app.json_level.is_none());
    assert!(app.interaction.selected.contains(&crate::card_id::CardId::from(note)));
}

#[test]
fn export_inline_json_matches_the_document_and_round_trips_through_a_file_card() {
    let source = json!({"a": [1, {"b": null}], "s": "x"});
    let (mut app, id) = card(source.clone());
    assert_eq!(app.export_inline_json(id), Some(source.clone()));
    let path = crate::store::testutil::temp_dir("export-inline").join("out.json");
    crate::export::write_json_export(&app.export_inline_json(id).unwrap(), &path, "json");
    let imported = app.alloc_node_id();
    app.insert_json_node_with_id(imported, path.to_string_lossy().into_owned(), egui::Vec2::ZERO, None).unwrap();
    assert_eq!(*app.nodes[&imported].as_json().unwrap().value, source);
    assert_eq!(app.export_inline_json(imported), None, "a file card has no inline document");
}

#[test]
fn a_box_over_a_repeated_value_is_cancelled_when_the_array_shifts() {
    let (mut app, id) = card(json!({"a": [null, null, null]}));
    open_level(&mut app, id);
    app.apply_json_edit(JsonEdit::Begin(s("/a/1"), EditWhat::Value));
    app.edit_json(id, insert("/a", 0, json!(null))).unwrap();
    assert_eq!(edit_of(&app), None, "/a/1 still reads null but is another element");
}

#[test]
fn a_menu_is_closed_when_the_array_shifts_under_it() {
    let (mut app, id) = card(json!({"a": [10, 11, 12]}));
    open_level(&mut app, id);
    app.open_json_menu(s("/a/2"), egui::Pos2::ZERO);
    assert!(app.json_level.as_ref().unwrap().menu.is_some());
    app.edit_json(id, insert("/a", 0, json!(9))).unwrap();
    assert!(app.json_level.as_ref().unwrap().menu.is_none(), "/a/2 is now the old /a/1");
}

#[test]
fn a_remote_body_change_swaps_the_value_of_the_open_level() {
    let (mut a, id) = card(json!({"a": 1}));
    let dir = crate::store::testutil::temp_dir("json-open-level-remote");
    let mut b = crate::session_sync::tests::app_on(&dir.join("b.manifold"), 2);
    b.apply_remote(vec![(id, crate::session_sync::Remote::Shown(a.snapshot(id).unwrap()))]);
    open_level(&mut b, id);
    a.edit_json(id, set("/a", json!(2))).unwrap();
    b.apply_remote(vec![(id, crate::session_sync::Remote::Shown(a.snapshot(id).unwrap()))]);
    assert_eq!(*b.json_level.as_ref().unwrap().value, json!({"a": 2}));
}
