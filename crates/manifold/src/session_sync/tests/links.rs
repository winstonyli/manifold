//! Session-sync tests: links between cards: connect, edges, pick-target mode and their menu rows.

use super::*;

/// Deleting an endpoint deletes the links naming it (the save also runs `check_graph_matches_document`).
#[test]
fn removing_a_card_deletes_its_links() {
    let mut app = app_on(&temp_dir("link-delete").join("session.mnf"), 1);
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    let link = app.create_link(a, b);
    app.remove_tab(a);
    assert!(app.links_naming(b).is_empty());
    assert!(app.doc.mirror.link_ids().all(|id| id != link));
    assert!(app.save_session());
}

#[test]
fn connect_link_makes_one_link_and_refuses_a_repeat_in_either_order() {
    let mut app = app_on(&temp_dir("connect-link").join("session.mnf"), 1);
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    assert_eq!(app.connect(LINK, a, b), Ok(()));
    assert_eq!(app.connect(LINK, a, b), Err(ConnectError::Exists));
    assert_eq!(app.connect(LINK, b, a), Err(ConnectError::Exists));
    assert_eq!(app.links_naming(a).len(), 1);
    app.disconnect(LINK, a, b);
    assert!(app.links_naming(a).is_empty());
}

#[test]
fn connect_opened_from_sets_the_parent_and_refuses_a_cycle() {
    let mut app = app_on(&temp_dir("connect-opener").join("session.mnf"), 1);
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    assert_eq!(app.connect(OPENED_FROM, b, a), Ok(()));
    assert_eq!(app.doc.mirror.parent(b), Some(a));
    assert_eq!(app.connect(OPENED_FROM, a, b), Err(ConnectError::Cycle));
    app.disconnect(OPENED_FROM, b, a);
    assert_eq!(app.doc.mirror.parent(b), None);
}

#[test]
fn a_link_is_a_level_edge_only_while_both_ends_are_live_at_this_level() {
    use crate::level::EdgeKind;
    let mut app = app_on(&temp_dir("link-level").join("session.mnf"), 1);
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    app.connect(LINK, a, b).unwrap();
    let edges = app.level_edges();
    assert_eq!(edges.iter().filter(|e| e.kind == EdgeKind::Link).count(), 1);
    assert!(edges.iter().all(|e| e.curve == crate::level::EdgeCurve::Bow), "every edge between stored nodes bows");
    assert!(edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Link)
        .all(|e| (e.from_id.node(), e.to_id.node()) == (Some(a), Some(b)) && e.link.is_some()));
    let elsewhere = add_group(&mut app, "elsewhere");
    app.edit_group(b, Some(elsewhere));
    assert!(app.level_edges().iter().all(|e| e.kind != EdgeKind::Link), "another level: not shown");
}

#[test]
fn links_never_feed_the_layout_springs() {
    let mut app = app_on(&temp_dir("link-springs").join("session.mnf"), 1);
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    app.connect(LINK, a, b).unwrap();
    assert!(app.spring_edges().is_empty());
    app.connect(OPENED_FROM, b, a).unwrap();
    assert_eq!(app.spring_edges(), vec![(a, b)]);
}

#[test]
fn connect_refuses_another_level_and_a_missing_card() {
    let mut app = app_on(&temp_dir("connect-level").join("session.mnf"), 1);
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    let g = add_group(&mut app, "g");
    app.edit_group(b, Some(g));
    assert_eq!(app.connect(LINK, a, b), Err(ConnectError::DifferentLevel));
    assert_eq!(app.connect(LINK, a, id(99)), Err(ConnectError::NotLive));
    assert_eq!(app.connect("contains", a, b), Err(ConnectError::NotAllowed), "not a user-drawn edge");
}

#[test]
fn edges_of_lists_the_opener_the_openees_and_the_links() {
    let mut app = app_on(&temp_dir("edges-of").join("session.mnf"), 1);
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    let c = succeed_spawn(&mut app, "https://c/");
    app.connect(OPENED_FROM, b, a).unwrap();
    app.connect(LINK, a, c).unwrap();
    let of_a = app.edges_of(a);
    assert!(of_a.iter().any(|e| e.edge == OPENED_FROM && e.other == b && !e.outgoing));
    assert!(of_a.iter().any(|e| e.edge == LINK && e.other == c && e.link.is_some()));
    let of_b = app.edges_of(b);
    assert!(of_b.iter().any(|e| e.edge == OPENED_FROM && e.other == a && e.outgoing));
}

#[test]
fn valid_targets_are_the_cards_connect_would_accept() {
    let mut app = app_on(&temp_dir("valid-targets").join("session.mnf"), 1);
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    let c = succeed_spawn(&mut app, "https://c/");
    app.connect(LINK, a, b).unwrap();
    assert_eq!(app.valid_targets(LINK, a), vec![c], "not itself, not one it is linked to");
    app.connect(OPENED_FROM, b, a).unwrap();
    assert!(!app.valid_targets(OPENED_FROM, a).contains(&b), "that would close a loop");
}

/// A running download is placed but not stored until it completes (`snapshot`), so it is no end for a link or an
/// opener yet: the save would write that end as none (a half-made link, a cleared parent).
#[test]
fn a_running_download_is_no_link_or_opener_end_until_it_completes() {
    let mut app = app_on(&temp_dir("connect-running-download").join("session.mnf"), 1);
    let tab = succeed_spawn(&mut app, "https://a/");
    let download = |completed| {
        Runtime::Download(DownloadPayload {
            path: "f.bin".into(),
            bytes_received: 1,
            total_bytes: Some(2),
            completed,
            failed: false,
        })
    };
    let dl = add(&mut app, Kind::Download, download(false));
    for (edge, from, to) in [(LINK, tab, dl), (LINK, dl, tab), (OPENED_FROM, tab, dl), (OPENED_FROM, dl, tab)] {
        assert_eq!(app.connect(edge, from, to), Err(ConnectError::NotLive), "{edge} {from} -> {to}");
    }
    assert!(app.valid_targets(LINK, tab).is_empty() && app.valid_targets(OPENED_FROM, tab).is_empty());
    assert!(app.valid_targets(LINK, dl).is_empty(), "nor a source");
    assert!(app.doc.mirror.link_ids().next().is_none() && app.doc.mirror.parent(tab).is_none());

    app.nodes.get_mut(&dl).unwrap().runtime = download(true);
    assert_eq!(app.valid_targets(LINK, tab), vec![dl], "complete, it is a card like any other");
    assert_eq!(app.connect(LINK, tab, dl), Ok(()));
    assert!(app.save_session());
    let link = app.links_naming(tab).pop().expect("the link");
    assert_eq!(stored(&app, link, field::TO), FieldValue::Id(Some(dl)));
}

/// A tab that becomes a content card keeps its links: each now names the new card.
#[test]
fn a_tab_that_becomes_content_keeps_its_links() {
    let dir = temp_dir("link-redirect");
    let mut app = app_on(&dir.join("session.mnf"), 1);
    let tab = succeed_spawn(&mut app, "https://x.test/a.json");
    let other = succeed_spawn(&mut app, "https://b/");
    app.create_link(tab, other);
    app.create_link(other, tab);
    let node = app.alloc_node_id();
    let body = crate::redirect_fetch::save(&dir, node, crate::content::ContentKind::Json, b"[1]".to_vec()).unwrap();
    app.redirect_tab_to_content(tab, node, "https://x.test/a.json".into(), body);
    assert!(app.links_naming(tab).is_empty());
    let links = app.links_naming(node);
    assert_eq!(links.len(), 2);
    let ends: Vec<_> = links.iter().map(|l| app.doc.mirror.link_ends(*l)).collect();
    let (n, o) = (Some(node), Some(other));
    assert!(ends.contains(&(n, o)) && ends.contains(&(o, n)), "direction kept");
    assert!(app.save_session());
}

/// Links final review M2: merging S into T moves S's links onto T, keeping their direction, except one that would join T
/// to itself (S-T) and one to a card T is already linked to (S-c beside T-c). Restored as saved.
#[test]
fn merging_a_group_retargets_its_links_onto_the_target() {
    let path = temp_dir("link-merge").join("session.mnf");
    let mut app = app_on(&path, 1);
    let s = add_group(&mut app, "S");
    let t = add_group(&mut app, "T");
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    let c = succeed_spawn(&mut app, "https://c/");
    app.create_link(s, a);
    app.create_link(b, s);
    app.create_link(s, t);
    app.create_link(s, c);
    let tc = app.create_link(t, c);
    assert!(app.save_session());
    app.merge_groups(s, t);
    assert!(app.links_naming(s).is_empty());
    let ends = |app: &App| {
        let mut ends: Vec<_> = app.links_naming(t).iter().map(|l| app.doc.mirror.link_ends(*l)).collect();
        ends.sort();
        ends
    };
    let mut want = vec![(Some(t), Some(a)), (Some(b), Some(t)), (Some(t), Some(c))];
    want.sort();
    assert_eq!(ends(&app), want);
    assert_eq!(app.links_naming(c), [tc], "T-c kept as it was");
    assert!(app.save_session());
    app.close_store();
    let mut restored = app_on(&path, 2);
    assert!(restored.restore_session());
    assert_eq!(ends(&restored), want);
}

#[test]
fn edges_of_leaves_out_a_dangling_link_and_one_at_another_level() {
    let mut app = app_on(&temp_dir("edges-live").join("session.mnf"), 1);
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    let c = succeed_spawn(&mut app, "https://c/");
    let ghost = app.alloc_node_id();
    app.create_link(a, ghost);
    app.connect(LINK, a, b).unwrap();
    app.connect(LINK, a, c).unwrap();
    let g = add_group(&mut app, "g");
    app.edit_group(b, Some(g));
    app.edit_parent(a, Some(ghost));
    let of_a = app.edges_of(a);
    assert_eq!(of_a.len(), 1, "only the link to c is live and at a's level");
    assert_eq!(of_a[0].other, c);
}

#[test]
fn connecting_opened_from_again_replaces_the_opener() {
    let mut app = app_on(&temp_dir("connect-replace").join("session.mnf"), 1);
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    let c = succeed_spawn(&mut app, "https://c/");
    app.connect(OPENED_FROM, c, a).unwrap();
    assert_eq!(app.connect(OPENED_FROM, c, b), Ok(()));
    assert_eq!(app.doc.mirror.parent(c), Some(b));
}

#[test]
fn disconnect_removes_a_link_from_the_opposite_end() {
    let mut app = app_on(&temp_dir("disconnect-reverse").join("session.mnf"), 1);
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    app.connect(LINK, a, b).unwrap();
    app.disconnect(LINK, b, a);
    assert!(app.links_naming(a).is_empty());
}

/// EXPORTSTATE lists every link with its ends and whether both ends are shown.
#[test]
fn export_state_lists_links() {
    let mut app = app_on(&temp_dir("link-state").join("session.mnf"), 1);
    let (a, b) = (succeed_spawn(&mut app, "https://a/"), succeed_spawn(&mut app, "https://b/"));
    let link = app.create_link(a, b);
    let state = app.export_state();
    assert_eq!(
        state["links"],
        serde_json::json!([{ "id": link.to_string(), "from": a.to_string(), "to": b.to_string(), "shown": true }])
    );
}

#[test]
fn the_menu_offers_a_verb_per_edge_with_a_valid_target_and_a_disconnect_per_live_edge() {
    let mut app = app_on(&temp_dir("link-menu").join("session.mnf"), 1);
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    app.connect(LINK, a, b).unwrap();
    let rows = app.node_menu_view(a).unwrap().edges;
    let actions: Vec<&NodeAction> = rows.iter().map(|r| &r.action).collect();
    assert!(!actions.contains(&&NodeAction::Connect(LINK)), "b is the only other card and is linked already");
    assert!(actions.contains(&&NodeAction::Connect(OPENED_FROM)));
    assert!(rows.iter().any(|r| matches!(&r.action, NodeAction::Disconnect { edge, .. } if *edge == LINK)));
}

#[test]
fn pick_target_mode_enters_completes_and_cancels() {
    let mut app = app_on(&temp_dir("pick-target").join("session.mnf"), 1);
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    app.apply_node_action(&egui::Context::default(), a, NodeAction::Connect(LINK));
    assert_eq!(app.link.connecting, Some((a, LINK)));
    app.finish_connect(b);
    assert!(app.link.connecting.is_none());
    assert_eq!(app.links_naming(a).len(), 1);
    app.apply_node_action(&egui::Context::default(), a, NodeAction::Connect(OPENED_FROM));
    app.cancel_connect();
    assert!(app.link.connecting.is_none());
}

/// Pick-target mode ends on any other node action, a level change, and its source card closing.
#[test]
fn pick_target_mode_ends_on_another_action_a_level_change_or_its_source_closing() {
    let mut app = app_on(&temp_dir("pick-ends").join("session.mnf"), 1);
    let ctx = egui::Context::default();
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    app.apply_node_action(&ctx, a, NodeAction::Connect(LINK));
    app.apply_node_action(&ctx, b, NodeAction::TogglePin);
    assert!(app.link.connecting.is_none(), "another action");
    app.apply_node_action(&ctx, a, NodeAction::Connect(LINK));
    let g = add_group(&mut app, "g");
    app.switch_group_level(Some(g));
    assert!(app.link.connecting.is_none(), "a level change");
    app.switch_group_level(None);
    app.apply_node_action(&ctx, a, NodeAction::Connect(LINK));
    app.close_node(a);
    assert!(app.link.connecting.is_none(), "its source closed");
    // A stale mode found by the per-frame check: the Graph view not shown.
    app.apply_node_action(&ctx, b, NodeAction::Connect(LINK));
    app.end_stale_connect(true);
    assert!(app.link.connecting.is_some());
    app.end_stale_connect(false);
    assert!(app.link.connecting.is_none());
}

/// "Disconnect from X" removes the edge its row names, either way round, and its label says which edge it is.
#[test]
fn disconnect_rows_name_their_edge_and_remove_it() {
    let mut app = app_on(&temp_dir("disconnect-rows").join("session.mnf"), 1);
    let ctx = egui::Context::default();
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    app.connect(OPENED_FROM, b, a).unwrap();
    app.connect(LINK, a, b).unwrap();
    let rows = |app: &App, id: NodeId| app.node_menu_view(id).unwrap().edges;
    let disconnects = |app: &App, id: NodeId| -> Vec<(String, NodeAction)> {
        rows(app, id)
            .into_iter()
            .filter(|r| matches!(r.action, NodeAction::Disconnect { .. }))
            .map(|r| (r.label, r.action))
            .collect()
    };
    let of_a = disconnects(&app, a);
    assert_eq!(of_a.len(), 2, "{of_a:?}");
    assert!(of_a.iter().any(|(label, _)| label.starts_with("Disconnect from Tab") && label.ends_with("(link)")));
    assert!(of_a.iter().any(|(label, _)| label.ends_with("(opened from here)")), "{of_a:?}");
    assert!(disconnects(&app, b).iter().any(|(label, _)| label.ends_with("(opened from it)")));
    // a's row for the opened-from edge: the parent field is b's.
    let (_, opener) = of_a.iter().find(|(label, _)| label.ends_with("(opened from here)")).unwrap().clone();
    app.apply_node_action(&ctx, a, opener);
    assert_eq!(app.doc.mirror.parent(b), None);
    let (_, link) = disconnects(&app, b).into_iter().find(|(label, _)| label.ends_with("(link)")).unwrap();
    app.apply_node_action(&ctx, b, link);
    assert!(app.links_naming(a).is_empty());
    assert!(disconnects(&app, a).is_empty());
    assert!(app.save_session());
}

/// EXPORTSTATE shows pick-target mode, with its targets.
#[test]
fn export_state_shows_pick_target_mode() {
    let mut app = app_on(&temp_dir("pick-state").join("session.mnf"), 1);
    let (a, b) = (succeed_spawn(&mut app, "https://a/"), succeed_spawn(&mut app, "https://b/"));
    assert_eq!(app.export_state()["connecting"], serde_json::Value::Null);
    app.apply_node_action(&egui::Context::default(), a, NodeAction::Connect(LINK));
    assert_eq!(
        app.export_state()["connecting"],
        serde_json::json!({ "from": a.to_string(), "edge": LINK, "targets": [b.to_string()] })
    );
}
