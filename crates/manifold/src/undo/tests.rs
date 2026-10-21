use super::*;
use crate::session_sync::tests::{add_group, add_note, app_on, closed_ids, group_with_note, stored};
use crate::store::model::{field, FieldValue};

fn fold(tag: &str) -> UndoKind {
    UndoKind::Fold(vec![(tag.to_owned(), None)])
}

/// A new record empties `redo`; undoing and redoing only move an entry between the lists.
#[test]
fn a_record_empties_redo_but_a_step_does_not() {
    let mut h = UndoHistory::default();
    let a = h.record("a".into(), fold("a"));
    let b = h.record("b".into(), fold("b"));
    let entry = h.take(None, false).expect("newest");
    assert_eq!(entry.serial, b);
    h.put(entry, true);
    assert_eq!(h.redo.len(), 1);
    let entry = h.take(Some(a), false).expect("by serial");
    h.put(entry, true);
    assert_eq!(h.redo.len(), 2, "a second undo keeps the first redo");
    h.record("c".into(), fold("c"));
    assert!(h.redo.is_empty(), "a fresh record clears redo");
}

/// The oldest entry goes past the cap, and serials are never reused.
#[test]
fn the_history_is_capped_and_serials_never_repeat() {
    let mut h = UndoHistory::default();
    let serials: Vec<u64> = (0..UNDO_DEPTH + 3).map(|i| h.record(i.to_string(), fold("x"))).collect();
    assert_eq!(h.undo.len(), UNDO_DEPTH);
    assert!(h.take(Some(serials[0]), false).is_none(), "the oldest is gone");
    assert!(serials.windows(2).all(|w| w[0] < w[1]));
}

/// "Dissolve Group" closes the emptied group holding its members' ids: Undo (or the Recently Closed row) reopens
/// the same group, with its name and color, around the members still at its level.
#[test]
fn dissolve_group_is_undone_by_reopening_the_same_group() {
    use crate::node_menu::NodeAction;
    let ctx = egui::Context::default();
    let color = session_sync::color_value(Some(egui::Color32::from_rgb(1, 2, 3)));
    for from_toast in [true, false] {
        let path = store::testutil::temp_dir("dissolve-undo").join("session.mnf");
        let mut app = app_on(&path, 1);
        let (g, inner) = group_with_note(&mut app);
        let moved_on = add_note(&mut app, "moved on");
        app.edit_group(&moved_on, Some(g.clone()));
        app.edit_name(&g, "Research");
        app.edit_field(&g, field::COLOR, color.clone());
        assert!(app.save_session());
        app.apply_node_action(&ctx, &g, NodeAction::Dissolve);
        let (message, serial, _) = app.toasts.undo.clone().expect("the dissolve offers an undo");
        assert!(message.starts_with("Dissolved"), "{message}");
        // A member the user put elsewhere meanwhile stays there.
        let elsewhere = add_group(&mut app, "Elsewhere");
        app.edit_group(&moved_on, Some(elsewhere.clone()));
        if from_toast {
            app.step_history(Some(serial), false);
        } else {
            app.reopen_node(g.parse().unwrap());
        }
        assert_eq!(app.doc.mirror.group(&inner), Some(g.as_str()), "back in the same group (toast: {from_toast})");
        assert_eq!(app.doc.mirror.group(&moved_on), Some(elsewhere.as_str()));
        assert_eq!(app.doc.mirror.name(&g), "Research");
        assert_eq!(app.doc.mirror.get(&g, field::COLOR), color);
        assert!(closed_ids(&app).is_empty());
        assert_eq!(stored(&app, &g, field::HELD), FieldValue::Str(String::new()), "forgotten once restored");
    }
}

/// A merge closes the source group holding its members: Undo reopens it around them, and a member the user moved
/// on stays where it went. The target keeps the members that stay.
#[test]
fn a_merge_is_undone_by_reopening_the_source_group() {
    let path = store::testutil::temp_dir("merge-undo").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (source, kept) = group_with_note(&mut app);
    let (_, left) = group_with_note(&mut app);
    app.edit_group(&left, Some(source.clone()));
    let target = add_group(&mut app, "T");
    assert!(app.save_session());
    app.merge_groups(&source, &target);
    assert_eq!(app.doc.mirror.group(&kept), Some(target.as_str()));
    assert!(closed_ids(&app).contains(&source));
    let (message, serial, _) = app.toasts.undo.clone().expect("the merge offers an undo");
    assert!(message.starts_with("Merged"), "{message}");
    app.edit_group(&left, None);
    app.step_history(Some(serial), false);
    assert_eq!(app.doc.mirror.group(&kept), Some(source.as_str()), "back in the source group");
    assert_eq!(app.doc.mirror.group(&left), None, "a member moved on stays");
    assert!(app.nodes.contains_key(&source) && closed_ids(&app).is_empty());
    app.step_history(None, true);
    assert_eq!(app.doc.mirror.group(&kept), Some(target.as_str()), "redo merges again");
    assert!(closed_ids(&app).contains(&source));
}

/// Ungroup All offers one Undo for every group it dissolved.
#[test]
fn ungroup_all_is_undone_in_one_step() {
    let path = store::testutil::temp_dir("ungroup-undo").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (g1, n1) = group_with_note(&mut app);
    let (g2, n2) = group_with_note(&mut app);
    app.ungroup(&[g1.clone(), g2.clone()]);
    assert!(app.doc.mirror.group(&n1).is_none() && app.doc.mirror.group(&n2).is_none());
    let (message, serial, _) = app.toasts.undo.clone().expect("an undo");
    assert_eq!(message, "Ungrouped 2 groups");
    app.step_history(Some(serial), false);
    assert_eq!(app.doc.mirror.group(&n1), Some(g1.as_str()));
    assert_eq!(app.doc.mirror.group(&n2), Some(g2.as_str()));
    app.step_history(None, true);
    assert!(app.doc.mirror.group(&n1).is_none() && app.doc.mirror.group(&n2).is_none(), "redo ungroups both again");
}

/// Reopening a folded group from Recently Closed goes round the history, so there is nothing to redo.
#[test]
fn a_reopen_from_recently_closed_leaves_nothing_to_redo() {
    let path = store::testutil::temp_dir("redo-after-reopen").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (g, inner) = group_with_note(&mut app);
    app.ungroup(std::slice::from_ref(&g));
    app.reopen_node(g.parse().unwrap());
    assert_eq!(app.doc.mirror.group(&inner), Some(g.as_str()));
    app.step_history(None, true);
    assert_eq!(
        app.doc.mirror.group(&inner),
        Some(g.as_str()),
        "nothing to redo: the reopen did not go through the history"
    );
}

/// Ctrl+Z on a fold whose group is already reopened changes nothing and does not offer a redo.
#[test]
fn an_undo_of_an_already_reopened_fold_does_nothing() {
    let path = store::testutil::temp_dir("undo-after-reopen").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (g, inner) = group_with_note(&mut app);
    app.ungroup(std::slice::from_ref(&g));
    app.reopen_node(g.parse().unwrap());
    app.step_history(None, false);
    app.step_history(None, true);
    assert_eq!(app.doc.mirror.group(&inner), Some(g.as_str()), "the group was not dissolved again");
}

/// Join: a tab dropped into a group is back at the top level, at its old position, by Undo; Redo puts it in again.
#[test]
fn a_join_is_undone_and_redone() {
    let path = store::testutil::temp_dir("join-undo").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (g, _) = group_with_note(&mut app);
    let n = add_note(&mut app, "loose");
    app.edit_pos(&n, Some(egui::vec2(40.0, 50.0)));
    app.interaction.drag_origin = vec![(n.clone(), app.doc.mirror.pos(&n))];
    app.edit_pos(&n, Some(egui::vec2(300.0, 300.0)));
    app.snap_layout(&n, egui::vec2(300.0, 300.0));
    app.apply_drop(&n, Some((DropEffect::Join, Some(g.clone()))));
    assert_eq!(app.doc.mirror.group(&n), Some(g.as_str()));
    app.step_history(None, false);
    assert_eq!(app.doc.mirror.group(&n), None);
    assert_eq!(app.doc.mirror.pos(&n), Some(egui::vec2(40.0, 50.0)), "back where the drag began");
    assert_eq!(app.layout_pos(&n), egui::vec2(40.0, 50.0), "and drawn there");
    app.step_history(None, true);
    assert_eq!(app.doc.mirror.group(&n), Some(g.as_str()));
    assert_eq!(app.doc.mirror.pos(&n), Some(egui::vec2(300.0, 300.0)));
    assert_eq!(app.layout_pos(&n), egui::vec2(300.0, 300.0));
}

/// Undo never writes a card into a group that is closed: it would vanish from every level.
#[test]
fn an_undo_into_a_closed_group_is_refused() {
    let path = store::testutil::temp_dir("undo-closed-dest").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (g, inner) = group_with_note(&mut app);
    let other = add_note(&mut app, "other");
    app.edit_group(&other, Some(g.clone()));
    app.apply_drop(&inner, Some((DropEffect::Eject, None)));
    assert_eq!(app.doc.mirror.group(&inner), None);
    assert!(app.toasts.undo.is_some(), "recorded");
    app.close_node(&g);
    assert!(!app.nodes.contains_key(&g), "closed");
    app.step_history(None, false);
    assert_eq!(app.doc.mirror.group(&inner), None, "the group is closed, so the card stays out");
    assert!(app.toasts.undo.as_ref().is_none_or(|(_, _, redo)| !redo), "nothing was undone, so no Redo is offered");
}

/// A menu "Move into" on a selection has no drag, yet every card it moved is on the step.
#[test]
fn a_menu_join_of_a_selection_records_every_card() {
    let path = store::testutil::temp_dir("menu-join-undo").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (g, _) = group_with_note(&mut app);
    let (a, b) = (add_note(&mut app, "a"), add_note(&mut app, "b"));
    app.interaction.selected = [a.clone(), b.clone()].into_iter().collect();
    app.apply_drop(&a, Some((DropEffect::Join, Some(g.clone()))));
    assert_eq!(app.doc.mirror.group(&b), Some(g.as_str()));
    app.step_history(None, false);
    assert_eq!(app.doc.mirror.group(&a), None);
    assert_eq!(app.doc.mirror.group(&b), None, "the other selected card comes back too");
}

/// A release that no drop follows (the window lost focus mid-drag) leaves no origin for a later drop to find.
#[test]
fn a_focus_loss_mid_drag_leaves_no_stale_origin() {
    let path = store::testutil::temp_dir("focus-origin").join("session.mnf");
    let mut app = app_on(&path, 1);
    let n = add_note(&mut app, "n");
    app.interaction.dragging = Some(n.clone());
    app.interaction.drag_moved = vec![n.clone()];
    app.on_window_focus(false);
    assert!(app.interaction.drag_origin.is_empty());
}

/// A selection dropped together comes back together, and a card moved on since is left alone.
#[test]
fn a_selection_join_reverts_each_card_and_skips_one_that_moved_on() {
    let path = store::testutil::temp_dir("join-selection-undo").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (g, _) = group_with_note(&mut app);
    let (a, b) = (add_note(&mut app, "a"), add_note(&mut app, "b"));
    app.interaction.selected = [a.clone(), b.clone()].into_iter().collect();
    app.interaction.drag_origin = vec![(a.clone(), None), (b.clone(), None)];
    app.apply_drop(&a, Some((DropEffect::Join, Some(g.clone()))));
    assert_eq!(app.doc.mirror.group(&b), Some(g.as_str()));
    let elsewhere = add_group(&mut app, "Elsewhere");
    app.edit_group(&b, Some(elsewhere.clone()));
    app.step_history(None, false);
    assert_eq!(app.doc.mirror.group(&a), None, "restored");
    assert_eq!(app.doc.mirror.group(&b), Some(elsewhere.as_str()), "moved on, so left alone");
}

/// GroupWith: Undo ungroups both and closes the group it made; Redo brings back the same group id.
#[test]
fn a_group_with_is_undone_and_redone_with_the_same_group() {
    let path = store::testutil::temp_dir("groupwith-undo").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (a, b) = (add_note(&mut app, "a"), add_note(&mut app, "b"));
    app.apply_drop(&a, Some((DropEffect::GroupWith, Some(b.clone()))));
    let made = app.doc.mirror.group(&a).map(str::to_owned).expect("grouped");
    assert_eq!(app.doc.mirror.group(&b), Some(made.as_str()));
    app.step_history(None, false);
    assert!(app.doc.mirror.group(&a).is_none() && app.doc.mirror.group(&b).is_none());
    assert!(!app.nodes.contains_key(&made), "the made group is closed");
    app.step_history(None, true);
    assert_eq!(app.doc.mirror.group(&a), Some(made.as_str()), "the same group id");
    assert_eq!(app.doc.mirror.group(&b), Some(made.as_str()));
}

/// Eject from a nested group goes up one level; Undo puts it back, Redo ejects again.
#[test]
fn an_eject_is_undone_and_redone() {
    let path = store::testutil::temp_dir("eject-undo").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (g, inner) = group_with_note(&mut app);
    app.apply_drop(&inner, Some((DropEffect::Eject, None)));
    assert_eq!(app.doc.mirror.group(&inner), None);
    app.step_history(None, false);
    assert_eq!(app.doc.mirror.group(&inner), Some(g.as_str()));
    app.step_history(None, true);
    assert_eq!(app.doc.mirror.group(&inner), None);
}

/// Reparent is an edge: Undo restores the old parent.
#[test]
fn a_reparent_is_undone() {
    let path = store::testutil::temp_dir("reparent-undo").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (g, a) = group_with_note(&mut app);
    let b = add_note(&mut app, "b");
    app.edit_group(&b, Some(g.clone()));
    let before = app.doc.mirror.parent(&a).map(str::to_owned);
    app.apply_drop(&a, Some((DropEffect::Reparent, Some(b.clone()))));
    assert_eq!(app.doc.mirror.parent(&a), Some(b.as_str()));
    app.step_history(None, false);
    assert_eq!(app.doc.mirror.parent(&a).map(str::to_owned), before);
}

/// A plain move (no effect) adds no entry; a fresh drop empties redo.
#[test]
fn only_effect_drops_are_recorded_and_a_drop_clears_redo() {
    let path = store::testutil::temp_dir("drop-record").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (g, _) = group_with_note(&mut app);
    let (a, b) = (add_note(&mut app, "a"), add_note(&mut app, "b"));
    app.apply_drop(&a, None);
    assert!(app.toasts.undo.is_none(), "a plain move is not recorded");
    app.apply_drop(&a, Some((DropEffect::Join, Some(g.clone()))));
    app.step_history(None, false);
    app.apply_drop(&b, Some((DropEffect::Join, Some(g.clone()))));
    app.step_history(None, true);
    assert_eq!(app.doc.mirror.group(&a), None, "the redo list was emptied by the second drop");
}

/// A merge drops the source group from the selection with the rest of its UI state (`forget_id`).
#[test]
fn a_merged_away_group_leaves_the_selection() {
    let mut app = App::default();
    let (source, target) = (add_group(&mut app, "S"), add_group(&mut app, "T"));
    app.interaction.selected.extend([source.clone(), target.clone()]);
    app.merge_groups(&source, &target);
    assert!(!app.nodes.contains_key(&source));
    assert_eq!(app.interaction.selected, HashSet::from([target]));
}

/// A Join (or a menu's Move into) onto a group another replica closed after the menu opened is a no-op: the node
/// stays where it is rather than vanishing into the hidden group. A merge onto it is refused the same way.
#[test]
fn a_join_onto_a_group_closed_remotely_does_nothing() {
    use crate::card::DropEffect;
    let path = store::testutil::temp_dir("closed-join-hidden").join("session.mnf");
    let mut app = app_on(&path, 1);
    let g = add_group(&mut app, "G");
    let (n, other_group) = (add_note(&mut app, "n"), add_group(&mut app, "H"));
    assert!(app.save_session());
    let mut other = store::journal::Journal::open(&path, store::model::ActorId(2)).unwrap();
    crate::session_sync::tests::remote_set(&mut other, &g, field::CLOSED, FieldValue::Replica(99));
    assert!(app.save_session());
    assert!(!app.nodes.contains_key(&g), "closed remotely, so not shown");
    app.apply_drop(&n, Some((DropEffect::Join, Some(g.clone()))));
    app.apply_node_action(&egui::Context::default(), &n, crate::node_menu::NodeAction::MoveToGroup(g.clone()));
    assert_eq!(app.doc.mirror.group(&n), None, "still at the top level");
    assert!(app.nodes.contains_key(&n));
    app.apply_drop(&other_group, Some((DropEffect::Merge, Some(g.clone()))));
    assert!(app.nodes.contains_key(&other_group), "the merge into a hidden group is refused");
}
