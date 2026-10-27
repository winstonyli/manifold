//! Tree fields (`props`) in the session: the mirror holds the document's leaves, a remote change reaches it, and
//! edits still waiting for a save survive a remote refresh.

use super::*;
use crate::store::tree::Seg;

fn key_path(segs: &[&str]) -> TreePath {
    TreePath::of(&segs.iter().map(|s| Seg::Key((*s).to_owned())).collect::<Vec<_>>())
}

/// What the document shows for `card`'s trees.
pub(super) fn doc_trees(app: &App, card: NodeId) -> Trees {
    shown(app.doc.journal.as_ref().unwrap().doc())[&card].trees.clone()
}

#[test]
fn a_remote_tree_edit_reaches_the_mirror_and_equals_the_document() {
    let path = temp_dir("props-remote").join("session.mnf");
    let mut app = app_on(&path, 1);
    let card = add_note(&mut app, "n");
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set_tree(&mut other, card, field::PROPS, key_path(&["k"]), Some(Leaf::Int(7)));
    assert!(app.save_session());
    assert_eq!(app.doc.mirror.tree(card, field::PROPS).unwrap().len(), 1);
    assert_eq!(snapshots(&app)[&card].trees, doc_trees(&app, card));
}

#[test]
fn a_remote_refresh_keeps_a_queued_edit_and_takes_the_rest() {
    let path = temp_dir("props-refresh").join("session.mnf");
    let mut app = app_on(&path, 1);
    let card = add_note(&mut app, "n");
    assert!(app.save_session());
    // A local edit waits for the next save.
    let mine = key_path(&["mine"]);
    app.doc.mirror.edit_tree(card, field::PROPS, &mine, Some(Leaf::Int(1)));
    app.doc.writes.queue_tree(card, field::PROPS, mine.clone(), Leaf::Int(1).encode());
    // Another replica writes /theirs and /mine.
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set_tree(&mut other, card, field::PROPS, key_path(&["theirs"]), Some(Leaf::Int(2)));
    remote_set_tree(&mut other, card, field::PROPS, mine.clone(), Some(Leaf::Int(3)));
    app.apply_remote(vec![(card, Remote::Shown(shown(other.doc())[&card].clone()))]);
    let tree = app.doc.mirror.tree(card, field::PROPS).unwrap();
    assert_eq!(tree[&mine], Leaf::Int(1), "the waiting local edit still wins");
    assert_eq!(tree[&key_path(&["theirs"])], Leaf::Int(2), "the rest of theirs shows now");
}

#[test]
fn a_queued_ancestor_edit_evicts_queued_descendants() {
    let mut writes = PendingWrites::default();
    let card = node(1);
    let (a, ab) = (key_path(&["a"]), key_path(&["a", "b"]));
    writes.queue_tree(card, field::PROPS, ab.clone(), vec![1]);
    writes.queue_tree(card, field::PROPS, a.clone(), vec![2]);
    assert_eq!(writes.tree_edits(card, field::PROPS).map(|(p, _)| p.clone()).collect::<Vec<_>>(), vec![a.clone()]);
    writes.queue_tree(card, field::PROPS, ab.clone(), vec![3]);
    assert_eq!(writes.tree_edits(card, field::PROPS).map(|(p, _)| p.clone()).collect::<Vec<_>>(), vec![a, ab]);
    assert!(writes.tree_queued(card, field::PROPS) && !writes.tree_queued(node(2), field::PROPS));
}

#[test]
fn an_edit_shows_at_once_and_commits_as_a_path_set() {
    let path = temp_dir("props-edit").join("session.mnf");
    let mut app = app_on(&path, 1);
    let card = add_note(&mut app, "n");
    assert!(app.save_session());
    let p = key_path(&["a", "b"]);
    app.edit_prop(card, field::PROPS, p.clone(), Some(Leaf::Int(1))).unwrap();
    assert_eq!(app.doc.mirror.tree(card, field::PROPS).unwrap().len(), 1);
    assert!(app.edit_prop(card, field::PROPS, p, Some(Leaf::Int(1))).is_none(), "a repeat queues nothing");
    assert!(app.doc.writes.tree_queued(card, field::PROPS));
    assert!(app.save_session());
    assert_eq!(doc_trees(&app, card), app.doc.mirror.trees_of(card));
    assert!(!app.doc.writes.tree_queued(card, field::PROPS), "the commit clears the queue");
}

#[test]
fn a_subtree_delete_is_one_tombstone() {
    let path = temp_dir("props-subtree").join("session.mnf");
    let mut app = app_on(&path, 1);
    let card = add_note(&mut app, "n");
    for segs in [&["a", "b"][..], &["a", "c"]] {
        app.edit_prop(card, field::PROPS, key_path(segs), Some(Leaf::Int(1)));
    }
    assert!(app.save_session());
    assert_eq!(doc_trees(&app, card)[&field::PROPS].len(), 2);
    let edit = app.edit_prop(card, field::PROPS, key_path(&["a"]), None).unwrap();
    assert_eq!(edit.before.len(), 2, "both leaves are reported for undo");
    assert!(app.doc.mirror.tree(card, field::PROPS).is_none());
    assert!(app.save_session());
    assert!(doc_trees(&app, card).is_empty());
}

#[test]
fn a_card_created_with_a_tree_commits_it_in_the_same_save() {
    let path = temp_dir("props-create").join("session.mnf");
    let mut app = app_on(&path, 1);
    let card = add_note(&mut app, "n"); // queued Create, not committed
    app.edit_prop(card, field::PROPS, key_path(&["label"]), Some(Leaf::Str("x".into()))).unwrap();
    assert!(app.save_session());
    assert_eq!(doc_trees(&app, card)[&field::PROPS].len(), 1);
    assert_eq!(snapshots(&app)[&card].trees, doc_trees(&app, card));
}

#[test]
fn a_queued_edit_on_a_node_deleted_in_the_same_save_is_dropped() {
    let path = temp_dir("props-deleted").join("session.mnf");
    let mut app = app_on(&path, 1);
    let card = add_note(&mut app, "n");
    assert!(app.save_session());
    app.edit_prop(card, field::PROPS, key_path(&["k"]), Some(Leaf::Int(1))).unwrap();
    app.remove_node(card);
    let before = ops_of(&app);
    assert!(app.save_session());
    assert!(!app.doc.journal.as_ref().unwrap().doc().nodes[&card].is_live());
    assert!(!app.doc.writes.tree_queued(card, field::PROPS));
    assert_eq!(ops_of(&app) - before, 1, "only the Delete is committed");
}

#[test]
fn a_duplicate_carries_the_tree_and_remaps_refs_inside_the_copy() {
    let path = temp_dir("props-duplicate").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (a, b) = (add_note(&mut app, "a"), add_note(&mut app, "b"));
    app.edit_prop(a, field::PROPS, key_path(&["label"]), Some(Leaf::Str("x".into()))).unwrap();
    app.edit_prop(a, field::PROPS, key_path(&["link"]), Some(Leaf::Ref(b))).unwrap();
    assert!(app.save_session());
    app.duplicate_nodes(&[a, b]);
    let copy_of = |text: &str| {
        *app.nodes.keys().find(|&&id| ![a, b].contains(&id) && app.doc.mirror.text(id) == text).expect("a copy")
    };
    let (copy_a, copy_b) = (copy_of("a"), copy_of("b"));
    let tree = app.doc.mirror.tree(copy_a, field::PROPS).expect("the copy has a tree");
    assert_eq!(tree[&key_path(&["label"])], Leaf::Str("x".into()));
    assert_eq!(tree[&key_path(&["link"])], Leaf::Ref(copy_b), "the ref names the copy of b, not b");
    assert_eq!(doc_trees(&app, copy_a), app.doc.mirror.trees_of(copy_a), "and the document agrees");
}

#[test]
fn a_tab_duplicate_keeps_the_tree() {
    let path = temp_dir("props-tab-duplicate").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = succeed_spawn(&mut app, "https://a.test/");
    app.edit_prop(tab, field::PROPS, key_path(&["label"]), Some(Leaf::Str("x".into()))).unwrap();
    assert!(app.save_session());
    app.duplicate_tab(tab);
    let (spawn, generation) = app.test_spawns.pop().expect("the copy's spawn is parked");
    let copy = spawn.id;
    assert_eq!(app.spawns.spawning[&copy].trees, app.doc.mirror.trees_of(tab), "the spawn starts with the tree");
    let (view, _peer) = crate::process_manager::ViewTab::fake(spawn.kind);
    app.handle_tab_spawn_result(crate::tabs::TabSpawnOutcome {
        spawn,
        generation,
        result: Ok(view),
        seeded: Default::default(),
        jar: Default::default(),
        seed_clears: None,
    });
    assert_eq!(app.doc.mirror.trees_of(copy), app.doc.mirror.trees_of(tab), "and lands with it");
    assert!(app.save_session());
    assert_eq!(doc_trees(&app, copy), app.doc.mirror.trees_of(copy), "and the save commits it");
}

/// A remote change parked for a waking tab predates a tree edit committed after it; the landing
/// must not drop that edit from the mirror.
#[test]
fn a_committed_tree_edit_survives_the_landing_of_a_parked_remote_change() {
    let path = temp_dir("props-parked").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://a/", "A"));
    assert!(app.save_session());
    app.focus_tab(tab);
    assert!(app.spawns.spawning.contains_key(&tab), "waking");
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, tab, field::NAME, two());
    let mine = key_path(&["mine"]);
    app.edit_prop(tab, field::PROPS, mine.clone(), Some(Leaf::Int(1))).unwrap();
    assert!(app.save_session());
    assert!(app.spawns.remote_in_flight.contains_key(&tab), "the remote change is parked");
    fail_spawns(&mut app);
    assert_eq!(app.doc.mirror.tree(tab, field::PROPS).unwrap()[&mine], Leaf::Int(1));
    assert_eq!(doc_trees(&app, tab), app.doc.mirror.trees_of(tab));
}

#[test]
fn set_body_replaces_the_tree_and_queues_only_the_difference() {
    let path = temp_dir("set-body").join("session.mnf");
    let mut app = app_on(&path, 1);
    let id = add(&mut app, Kind::Json, Runtime::Json(JsonPayload::inline(0, Default::default(), Default::default())));
    let tree = |app: &mut App, v: serde_json::Value| {
        let mut ids = Vec::new();
        crate::store::body::value_to_tree(&v, &mut |_| {
            let id = app.alloc_node_id();
            ids.push(id);
            id
        })
        .unwrap()
    };
    let a = tree(&mut app, serde_json::json!({"a": 1, "b": 2}));
    let v1 = app.set_body(id, a);
    let b = tree(&mut app, serde_json::json!({"a": 1, "c": 3}));
    let v2 = app.set_body(id, b);
    assert_ne!(v1, v2);
    assert_eq!(app.doc.writes.tree_edits(id, field::BODY).count(), 3, "a once, b as a tombstone, c");
    assert!(app.save_session());
    assert_eq!(
        crate::store::body::tree_to_value(&doc_trees(&app, id)[&field::BODY]),
        serde_json::json!({"a": 1, "c": 3})
    );
}

#[test]
fn duplicating_an_inline_json_card_copies_the_body_with_fresh_element_ids() {
    let path = temp_dir("body-duplicate").join("session.mnf");
    let mut app = app_on(&path, 1);
    let a = app.alloc_node_id();
    app.insert_inline_json_at(a, None);
    let tree =
        crate::store::body::value_to_tree(&serde_json::json!({"list": [1, 2]}), &mut |_| app.alloc_node_id()).unwrap();
    app.set_body(a, tree);
    assert!(app.save_session());
    app.duplicate_nodes(&[a]);
    let b = *app.nodes.keys().find(|&&id| id != a && app.doc.mirror.tree(id, field::BODY).is_some()).expect("a copy");
    let (ta, tb) = (app.doc.mirror.tree(a, field::BODY).unwrap(), app.doc.mirror.tree(b, field::BODY).unwrap());
    assert_eq!(crate::store::body::tree_to_value(ta), crate::store::body::tree_to_value(tb));
    let ids = |t: &crate::store::tree::Tree| {
        t.keys()
            .flat_map(|p| p.segments())
            .filter_map(|s| if let Seg::Id(i) = s { Some(i) } else { None })
            .collect::<std::collections::BTreeSet<_>>()
    };
    assert!(ids(ta).is_disjoint(&ids(tb)), "the copy's elements are new ids");
    assert!(
        matches!(&app.nodes[&b].runtime, Runtime::Json(p) if p.is_inline() && p.value["list"] == serde_json::json!([1, 2]))
    );
}
