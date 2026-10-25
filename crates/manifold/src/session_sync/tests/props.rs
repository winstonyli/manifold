//! Tree fields (`props`) in the session: the mirror holds the document's leaves, a remote change reaches it, and
//! edits still waiting for a save survive a remote refresh.

use super::*;
use crate::store::tree::Seg;

fn key_path(segs: &[&str]) -> TreePath {
    TreePath::of(&segs.iter().map(|s| Seg::Key((*s).to_owned())).collect::<Vec<_>>())
}

/// What the document shows for `card`'s trees.
fn doc_trees(app: &App, card: NodeId) -> Trees {
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
