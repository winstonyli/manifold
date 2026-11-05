use super::*;
use crate::card_id::testutil::node;
use crate::trail::history::NavHistory;
use crate::virtual_source::fake::Rig;

/// /a -> /b -> /c and /a -> /d, cursor on /d, on the rig's tab, which is focused.
fn forked() -> Rig {
    let mut rig = Rig::new();
    let tab = rig.tab();
    rig.app.scoped.nav_history.insert(
        tab,
        NavHistory::with_tree(
            &[("/a", None, Some(3)), ("/b", Some(0), Some(2)), ("/c", Some(1), None), ("/d", Some(0), None)],
            3,
        ),
    );
    rig.app.focused = Some(tab);
    rig.app.view_mode = crate::ViewMode::Graph;
    rig.frame(vec![], egui::Modifiers::NONE);
    // The strip's arrival eases the camera (`frame_new_strip`); these tests aim at a still one.
    rig.app.camera.tween = None;
    rig.frame(vec![], egui::Modifiers::NONE);
    rig
}

/// The card id of the page at arena index `entry` of `tab`'s trail.
fn page_id(rig: &Rig, tab: NodeId, entry: usize) -> CardId {
    let history = &rig.app.scoped.nav_history[&tab];
    page_card(history, history.serial_of(entry).expect("the trail has that page"))
}

fn pages(rig: &Rig) -> Vec<crate::level::CardEntry> {
    rig.app.level_items().into_iter().filter(|e| is_page_id(e.id)).collect()
}

#[test]
fn a_strip_that_appears_off_screen_is_framed_once() {
    let mut rig = Rig::new();
    let tab = rig.tab();
    rig.app.focused = Some(tab);
    rig.app.view_mode = crate::ViewMode::Graph;
    rig.frame(vec![], egui::Modifiers::NONE);
    let far = egui::Rect::from_min_size(egui::pos2(9000.0, 9000.0), rig.app.camera.rect.size());
    rig.app.camera.jump_to(far);
    rig.app.scoped.nav_history.insert(tab, NavHistory::visited(&["/a", "/b"]));
    rig.frame(vec![], egui::Modifiers::NONE);
    assert!(rig.app.camera.tween.is_some(), "the strip appeared off screen: the camera eases to it");
    assert_eq!(rig.app.camera.strip_tab, Some(tab));
    rig.app.camera.jump_to(far);
    rig.frame(vec![], egui::Modifiers::NONE);
    assert!(rig.app.camera.tween.is_none(), "a strip already framed does not fight a later pan");
}

#[test]
fn a_strip_with_only_its_root_in_view_is_framed() {
    let mut rig = forked();
    let tab = rig.tab();
    let card = rig.app.card_rect(tab).expect("the tab's card");
    let all = pages(&rig);
    let root = all.iter().find(|e| e.origin == Origin::Virtual("trail")).expect("a root page").rect;
    let lowest = all.iter().map(|e| e.rect.bottom()).fold(f32::MIN, f32::max);
    assert!(lowest > root.bottom() + 1.0, "the strip reaches below its root");
    // A camera that holds the tab's card and the strip's root and stops short of the lowest page.
    let seen = card.union(root).expand2(egui::vec2(2.0, 0.0));
    rig.app.camera.jump_to(seen);
    rig.app.camera.tween = None;
    rig.app.camera.strip_tab = None;
    rig.frame(vec![], egui::Modifiers::NONE);
    assert!(rig.app.camera.tween.is_some(), "the lowest page is off screen: the camera eases to the strip");
}

#[test]
fn a_forked_trail_shows_one_card_per_page_under_the_tab_card() {
    let rig = forked();
    let tab = rig.tab();
    let cards = pages(&rig);
    assert_eq!(cards.len(), 4);
    assert!(cards.iter().all(|c| c.kind.ty == TYPE_TRAIL_PAGE && c.origin == Origin::Virtual("trail")));
    assert_eq!(cards[0].id, page_id(&rig, tab, 0));
    let below = rig.app.card_rect(tab).unwrap().left_bottom() + egui::vec2(0.0, STRIP_GAP);
    let top_left = cards.iter().fold(egui::Rect::NOTHING, |b, c| b.union(c.rect)).min;
    assert_eq!(top_left, below, "the whole strip hangs under the tab card, none of it above");
    assert_eq!(cards[0].rect.height(), PAGE.y);
    assert!(cards[0].rect.width() < PAGE.x, "a short label gets a narrow page");
    assert_eq!(cards[0].rect.min.x, below.x, "the root is the leftmost column");
    assert_eq!(cards[1].rect.min.x, below.x + cards[0].rect.width() + COLUMN_GAP, "children one column right");
    assert!(cards[3].rect.min.y > cards[1].rect.min.y, "the later branch sits on a lower lane");
}

#[test]
fn trail_cards_lay_out_any_tabs_trail_from_the_origin_in_any_view() {
    let mut rig = forked();
    let tab = rig.tab();
    rig.app.view_mode = crate::ViewMode::Item;
    rig.app.focused = None;
    assert!(pages(&rig).is_empty(), "no strip here");
    let origin = egui::pos2(5.0, 7.0);
    let cards = trail_cards(&rig.app, tab, origin);
    assert_eq!(cards.len(), 4);
    assert_eq!(cards.iter().fold(egui::Rect::NOTHING, |b, e| b.union(e.rect)).min, origin);
    let rect_of = |id: CardId| cards.iter().find(|e| e.id == id).map(|e| e.rect);
    assert_eq!(trail_edges(&rig.app, tab, &rect_of).len(), 3);
    assert!(trail_cards(&rig.app, node(9999), origin).is_empty());
}

#[test]
fn the_page_menu_stays_while_a_branch_popup_is_open_and_closes_when_neither_shows() {
    let mut rig = forked();
    let tab = rig.tab();
    let id = page_id(&rig, tab, 1);
    rig.app.view_mode = crate::ViewMode::Item;
    rig.app.menus.trail_menu = Some((id, egui::pos2(5.0, 5.0)));
    let ctx = rig.app.egui_ctx.clone();
    let run = |app: &mut App| {
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| app.build_trail_menu(ui));
        out.textures_delta.clear();
    };
    run(&mut rig.app);
    assert!(rig.app.menus.trail_menu.is_none(), "no strip and no popup: closed");
    rig.app.menus.trail_menu = Some((id, egui::pos2(5.0, 5.0)));
    rig.app.menus.open_popup = Some(crate::popups::AnchoredPopup::BackBranches);
    run(&mut rig.app);
    assert!(rig.app.menus.trail_menu.is_some(), "a branch popup is open: kept");
}

#[test]
fn no_strip_for_one_page_no_focus_tab_view_or_another_level() {
    let mut rig = forked();
    let tab = rig.tab();
    rig.app.focused = None;
    assert!(pages(&rig).is_empty(), "no focused tab");
    rig.app.focused = Some(tab);
    rig.app.view_mode = crate::ViewMode::Item;
    assert!(pages(&rig).is_empty(), "Tab view");
    rig.app.view_mode = crate::ViewMode::Graph;
    rig.app.current_group = Some(node(9999));
    assert!(pages(&rig).is_empty(), "the tab is on another level");
    rig.app.current_group = None;
    rig.app.scoped.nav_history.insert(tab, NavHistory::with_tree(&[("/a", None, None)], 0));
    assert!(pages(&rig).is_empty(), "one page");
}

#[test]
fn edges_join_parent_to_child_and_mark_the_path() {
    let rig = forked();
    let tab = rig.tab();
    let edges: Vec<_> = rig.app.level_edges().into_iter().filter(|e| is_page_id(e.from_id)).collect();
    let key = |e: &crate::level::LevelEdge| (e.from_id, e.to_id, e.highlight);
    let id = |i: usize| page_id(&rig, tab, i);
    assert_eq!(
        edges.iter().map(key).collect::<Vec<_>>(),
        [(id(0), id(1), false), (id(1), id(2), false), (id(0), id(3), true)]
    );
}

/// Runs `f` with a card pass at `zoom` (shapes at `1 / zoom`, text at `zoom` screen scale) and plays the queued
/// overlays on `ui`'s painter with the world-to-screen projection `p * zoom`.
fn with_pass(ui: &mut egui::Ui, zoom: f32, f: impl FnOnce(&mut egui::Ui, &mut CardPass)) {
    let mut overlays = crate::graph_ui::DeferredOverlays::default();
    let (mut actions, drop_preview, members) = (Vec::new(), None, std::collections::HashMap::new());
    let mut pass = CardPass {
        deferred_overlays: &mut overlays,
        actions: &mut actions,
        inv_zoom: 1.0 / zoom,
        zoom_scale: zoom,
        drop_preview: &drop_preview,
        pick_targets: &[],
        connect_target: None,
        connect_ring: None,
        show_order: false,
        order: &[],
        any_focused: false,
        members: &members,
        viewport: egui::Rect::EVERYTHING,
        pan_target: None,
        carry: true,
    };
    f(ui, &mut pass);
    overlays.play(ui.painter(), &|p| p * zoom);
}

fn text_shapes(output: &egui::FullOutput) -> Vec<(egui::Pos2, f32)> {
    output
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            egui::Shape::Text(t) => Some((t.pos, t.galley.size().y)),
            _ => None,
        })
        .collect()
}

#[test]
fn the_label_keeps_its_screen_size_at_every_zoom_and_hides_when_the_page_is_too_small_to_read() {
    let ctx = text_ctx();
    let page =
        TrailPage { serial: 0, parent: None, label: "Title".into(), url: String::new(), cursor: false, on_path: false };
    let rect = egui::Rect::from_min_size(egui::pos2(100.0, 50.0), PAGE);
    let heights = |zoom: f32, labelled: bool| {
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            with_pass(ui, zoom, |ui, pass| paint_trail_page(ui.painter(), pass, rect, &page, false, labelled));
        });
        output.textures_delta.clear();
        text_shapes(&output)
    };
    let (half, one, two) = (heights(0.5, true), heights(1.0, true), heights(2.0, true));
    assert_eq!((half.len(), one.len(), two.len()), (1, 1, 1), "one label each");
    assert_eq!((half[0].1, two[0].1), (one[0].1, one[0].1), "its height does not follow the zoom");
    assert_eq!(two[0].0.x, 200.0 + LABEL_LEFT, "it starts a screen-sized inset inside the page's left edge");
    assert!(heights(1.0, false).is_empty(), "an unlabelled page paints no text");
    assert!(heights(0.2, true).is_empty(), "a page under the label threshold paints no text");
}

#[test]
fn a_long_url_label_paints_clipped_to_the_page() {
    let ctx = crate::theme::testutil::ctx(true);
    let page = TrailPage {
        serial: 0,
        parent: None,
        label: "x".repeat(400),
        url: String::new(),
        cursor: false,
        on_path: false,
    };
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, PAGE);
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        let painter = ui.painter().with_clip_rect(rect);
        with_pass(ui, 1.0, |_, pass| paint_trail_page(&painter, pass, rect, &page, false, true));
    });
    output.textures_delta.clear();
}

#[test]
fn clicking_a_page_starts_a_plain_move_to_it_and_not_while_one_is_pending() {
    let mut rig = forked();
    let tab = rig.tab();
    let b = rig.screen(page_id(&rig, tab, 1)).center();
    rig.click(b);
    let now = std::time::Instant::now();
    assert!(rig.app.scoped.nav_history[&tab].move_pending(now), "the click started a move");
    // A second click on another page while that move is pending is ignored.
    let c = rig.screen(page_id(&rig, tab, 2)).center();
    rig.click(c);
    assert!(rig.app.scoped.nav_history[&tab].move_pending(now), "still the first move");
}

#[test]
fn clicking_the_cursor_page_or_an_evicted_one_does_nothing() {
    let mut rig = forked();
    let tab = rig.tab();
    let d = rig.screen(page_id(&rig, tab, 3)).center();
    rig.click(d);
    assert!(!rig.app.scoped.nav_history[&tab].move_pending(std::time::Instant::now()), "the cursor page");
    rig.app.trail_pick(tab, 99);
    assert!(!rig.app.scoped.nav_history[&tab].move_pending(std::time::Instant::now()), "an index past the end");
}

#[test]
fn dragging_a_page_onto_empty_canvas_opens_it_as_a_card_there() {
    let mut rig = forked();
    let tab = rig.tab();
    let from = rig.screen(page_id(&rig, tab, 1)).center();
    let to = from + egui::vec2(0.0, 300.0);
    rig.drag(from, to);
    let (spawn, _) = rig.app.test_spawns.last().expect("a tab was spawned");
    assert_eq!(spawn.url, "/b");
    assert_eq!(spawn.parent, Some(tab), "opened from the tab whose trail it came from");
    let world = rig.app.graph_screen_point_to_world(to);
    assert!((spawn.pos - world).length() < 1.0, "placed at the release point");
    assert_eq!(rig.app.scoped.nav_history[&tab].export()["entries"], 4, "the trail is unchanged");
}

#[test]
fn dropping_a_page_on_a_card_opens_it_the_same_way() {
    let mut rig = forked();
    let tab = rig.tab();
    let from = rig.screen(page_id(&rig, tab, 2)).center();
    let to = rig.app.graph_world_rect_to_screen(rig.app.card_rect(tab).unwrap()).center();
    rig.drag(from, to);
    let (spawn, _) = rig.app.test_spawns.last().expect("a tab was spawned");
    assert_eq!(spawn.url, "/c");
    assert_eq!(spawn.parent, Some(tab), "the card dropped on is not the opener");
}

fn text_ctx() -> egui::Context {
    let ctx = crate::theme::testutil::ctx(true);
    ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
    ctx
}

#[test]
fn a_page_is_as_wide_as_its_label_within_the_clamp() {
    let ctx = text_ctx();
    let page = |label: &str| TrailPage {
        serial: 0,
        parent: None,
        label: label.into(),
        url: String::new(),
        cursor: false,
        on_path: false,
    };
    let widths: Vec<f32> = page_sizes(&ctx, &[page("a"), page("a medium title"), page(&"w".repeat(400))], PAGE)
        .iter()
        .map(|s| s.x)
        .collect();
    assert_eq!(widths[0], MIN_PAGE_WIDTH, "a short label sits at the floor");
    assert!(widths[0] < widths[1] && widths[1] < PAGE.x, "{widths:?}");
    assert_eq!(widths[2], PAGE.x, "a long label is capped, and clipped when painted");
    assert!(page_sizes(&ctx, &[page("a")], PAGE).iter().all(|s| s.y == PAGE.y), "height is the cell's");
}

#[test]
fn the_strip_page_tooltip_is_the_full_url_and_fit_frames_the_strip() {
    let mut rig = forked();
    let tab = rig.tab();
    let h = NavHistory::visited(&["https://example.com/a", "https://example.com/b"]);
    rig.app.scoped.nav_history.insert(tab, h);
    rig.frame(vec![], egui::Modifiers::NONE);
    let entry = pages(&rig).remove(1);
    let mut tip = None;
    let ctx = rig.ctx.clone();
    ctx.run_ui(egui::RawInput::default(), |ui| {
        with_pass(ui, 1.0, |ui, pass| tip = TrailSource.paint_body(&rig.app, ui, pass, &entry).tooltip);
    })
    .textures_delta
    .clear();
    assert_eq!(tip.as_deref(), Some("https://example.com/b"));
    let strip = pages(&rig).iter().fold(egui::Rect::NOTHING, |b, c| b.union(c.rect));
    assert!(rig.app.fit_all_nodes_scene_rect().contains_rect(strip), "Reset View frames the strip too");
}

#[test]
fn right_clicking_a_page_opens_the_same_menu_as_the_popup_rows_and_open_as_card_opens_it() {
    let mut rig = forked();
    let tab = rig.tab();
    let at = rig.screen(page_id(&rig, tab, 2)).center();
    rig.right_click(at);
    let (id, pos) = rig.app.menus.trail_menu.expect("the page's menu is open");
    assert_eq!(id, page_id(&rig, tab, 2));
    assert_eq!(pos, at, "at the pointer");
    rig.frame(vec![], egui::Modifiers::NONE);
    let menu = rig.ctx.memory(|m| m.area_rect(egui::Id::new("trail_page_menu"))).expect("the menu's area");
    rig.click(menu.center());
    assert!(rig.app.menus.trail_menu.is_none(), "choosing closes it");
    let (spawn, _) = rig.app.test_spawns.last().expect("Open as card spawned a tab");
    assert_eq!(spawn.url, "/c");
    assert_eq!(spawn.parent, Some(tab));
}

#[test]
fn a_menu_for_a_page_that_is_gone_closes() {
    let mut rig = forked();
    let tab = rig.tab();
    let unknown = page_card(&rig.app.scoped.nav_history[&tab], 99);
    rig.app.menus.trail_menu = Some((unknown, egui::pos2(10.0, 10.0)));
    rig.frame(vec![], egui::Modifiers::NONE);
    assert!(rig.app.menus.trail_menu.is_none(), "a serial no page has names no page");
}

#[test]
fn a_column_is_as_wide_as_its_widest_page() {
    let pages = NavHistory::with_tree(&[("/a", None, None), ("/b", Some(0), None), ("/c", Some(0), None)], 0).tree();
    let sizes = [egui::vec2(60.0, 20.0), egui::vec2(100.0, 20.0), egui::vec2(70.0, 20.0)];
    let rects = page_rects(&pages, &sizes, 10.0, 4.0);
    assert_eq!(rects[1].min.x, 70.0, "children start after the root's column");
    assert_eq!(rects[2].min.x, rects[1].min.x, "siblings share the column's left edge");
    assert_eq!(rects[2].size(), sizes[2], "each page keeps its own width");
}

#[test]
fn a_page_evicted_at_the_cap_retires_only_its_own_id() {
    let mut rig = forked();
    let tab = rig.tab();
    let urls: Vec<String> = (0..50).map(|i| format!("/p{i}")).collect();
    let refs: Vec<&str> = urls.iter().map(String::as_str).collect();
    rig.app.scoped.nav_history.insert(tab, NavHistory::visited(&refs));
    rig.frame(vec![], egui::Modifiers::NONE);
    let before: Vec<CardId> = pages(&rig).into_iter().map(|c| c.id).collect();
    assert_eq!(before.len(), 50);
    let h = rig.app.scoped.nav_history.get_mut(&tab).unwrap();
    let now = std::time::Instant::now();
    h.observe("/new", "", true, false, now);
    h.observe("/new", "", false, false, now);
    rig.frame(vec![], egui::Modifiers::NONE);
    let after: Vec<CardId> = pages(&rig).into_iter().map(|c| c.id).collect();
    assert_eq!(after.len(), 50, "still 50 pages");
    assert!(live_page(&rig.app, before[0]).is_none(), "the evicted page's id names nothing (Review Focus 2)");
    assert_eq!(&after[..49], &before[1..], "every survivor keeps its id although its index dropped by one");
    assert!(!before.contains(&after[49]), "the new page has an id of its own");
    let (_, index) = live_page(&rig.app, before[1]).expect("the survivor resolves");
    assert_eq!(index, 0, "to its new index");
}

#[test]
fn a_page_leaves_the_frame_to_the_card_and_rings_the_cursor_outside_it() {
    let ctx = crate::theme::testutil::ctx(true);
    ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, PAGE);
    let page =
        TrailPage { parent: None, serial: 0, label: "a".into(), url: String::new(), cursor: true, on_path: true };
    let layer = egui::LayerId::new(egui::Order::Background, egui::Id::new("paint_probe"));
    let mut rects = Vec::new();
    ctx.run_ui(egui::RawInput::default(), |ui| {
        with_pass(ui, 1.0, |_, pass| paint_trail_page(&ctx.layer_painter(layer), pass, rect, &page, false, true));
        rects = ctx.graphics_mut(|g| {
            g.entry(layer)
                .all_entries()
                .filter_map(|c| match &c.shape {
                    egui::Shape::Rect(r) => Some((r.fill, r.stroke_kind)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        });
    })
    .textures_delta
    .clear();
    assert!(rects.iter().all(|(fill, _)| *fill == egui::Color32::TRANSPARENT), "no fill over the card frame");
    assert!(rects.iter().any(|(_, kind)| *kind == egui::StrokeKind::Outside), "the cursor ring clears its border");
}

/// One headless frame of the rig's graph, keeping the painted shapes (`Rig::frame` drops them).
fn frame_shapes(rig: &mut Rig) -> Vec<egui::epaint::ClippedShape> {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024.0, 768.0))),
        ..Default::default()
    };
    let app = &mut rig.app;
    let mut output = rig.ctx.run_ui(input, |ui| app.build_graph_ui(ui));
    output.textures_delta.clear();
    output.shapes
}

fn shared_area(a: egui::Rect, b: egui::Rect) -> f32 {
    let i = a.intersect(b);
    i.width().max(0.0) * i.height().max(0.0)
}

fn is_drop_hint(shown: &str, target: &str) -> bool {
    use crate::card::DropEffect::*;
    [Merge, Join, GroupWith, Reparent].iter().any(|effect| shown == effect.hint(target))
}

/// Each text shape reading `text`, as its index in `shapes` (the paint order), the part of it that shows (its
/// bounds cut by its clip) and its galley height. The drag's drop hint ("Move into <text>") is left out: it is
/// drawn above every card on purpose.
fn shown_text_at(shapes: &[egui::epaint::ClippedShape], text: &str) -> Vec<(usize, egui::Rect, f32)> {
    shapes
        .iter()
        .enumerate()
        .filter_map(|(k, s)| match &s.shape {
            egui::Shape::Text(t) if t.galley.text().contains(text) && !is_drop_hint(t.galley.text(), text) => {
                Some((k, t.visual_bounding_rect().intersect(s.clip_rect), t.galley.size().y))
            }
            _ => None,
        })
        .collect()
}

/// `shown_text_at` without the paint order.
fn shown_text(shapes: &[egui::epaint::ClippedShape], text: &str) -> Vec<(egui::Rect, f32)> {
    shown_text_at(shapes, text).into_iter().map(|(_, bounds, height)| (bounds, height)).collect()
}

/// Whether an opaque rectangle painted after shape `from` holds all of `bounds`, so what is there is not seen.
fn painted_over(shapes: &[egui::epaint::ClippedShape], from: usize, bounds: egui::Rect) -> bool {
    fn opaque_over(shape: &egui::Shape, bounds: egui::Rect) -> bool {
        match shape {
            egui::Shape::Rect(r) => r.fill.a() == 255 && r.rect.contains_rect(bounds),
            egui::Shape::Vec(inner) => inner.iter().any(|s| opaque_over(s, bounds)),
            _ => false,
        }
    }
    shapes[from + 1..].iter().any(|s| opaque_over(&s.shape, bounds.intersect(s.clip_rect)))
}

/// The bug reported 2026-10-21 (a card's title drawn across the strip over it, and the strip's own text not scaling
/// with the rest of the graph), made deterministic: a tab card placed under the strip's first page, viewed at
/// three zooms.
#[test]
fn text_of_a_card_under_the_strip_stays_behind_it_and_the_strip_label_keeps_its_size() {
    let mut rig = forked();
    let tab = rig.tab();
    let under = rig.app.alloc_node_id();
    let world_page = pages(&rig)[0].rect;
    rig.app.insert_placeholder_node(
        under,
        world_page.center().to_vec2() - egui::vec2(60.0, 40.0),
        crate::store::model::Kind::Tab,
        crate::session_sync::tests::sleeping_tab("https://under.test/", "UNDERNEATH"),
    );
    let label = rig.app.scoped.nav_history[&tab].tree()[0].label.clone();
    let (mut overlapped, mut label_heights) = (false, Vec::new());
    for zoom in [0.8_f32, 1.0, 2.0] {
        rig.app.graph.pending_zoom = Some(zoom);
        let mut shapes = Vec::new();
        for _ in 0..4 {
            shapes = frame_shapes(&mut rig);
            rig.app.camera.tween = None;
        }
        let strip: Vec<egui::Rect> = pages(&rig).iter().map(|p| rig.app.graph_world_rect_to_screen(p.rect)).collect();
        let card = rig.app.graph_world_rect_to_screen(rig.app.card_rect(under).expect("the placed card"));
        for (bounds, _) in shown_text(&shapes, "UNDERNEATH") {
            assert!(
                strip.iter().all(|page| shared_area(bounds, *page) < 1.0),
                "zoom {zoom}: the lower card's title shows through the strip: {bounds:?} over {strip:?}"
            );
        }
        overlapped |= strip.iter().any(|page| shared_area(card, *page) > 100.0);
        label_heights.extend(shown_text(&shapes, &label).iter().map(|(_, h)| (zoom, *h)));
    }
    assert!(overlapped, "the placed card really sits under the strip, or this test proves nothing");
    let at = |z: f32| label_heights.iter().filter(|(zoom, _)| *zoom == z).map(|(_, h)| *h).collect::<Vec<_>>();
    assert!(!at(1.0).is_empty(), "the strip's label shows at 100%: {label_heights:?}");
    assert_eq!(at(2.0), at(1.0), "its on-screen height does not follow the zoom: {label_heights:?}");
    assert_eq!(at(0.8), at(1.0), "nor at 80%: {label_heights:?}");
}

/// A small deterministic generator, so a failing layout can be rerun from its seed.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }

    fn between(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next()
    }
}

/// The forked trail's rig with nine cards (tab, download, group, note, image and PDF in turn) scattered around the strip's first page by
/// `rng`, and a random one selected half the time; each card's id maps to the title its text carries.
fn random_layout(rng: &mut Lcg) -> (Rig, std::collections::HashMap<CardId, String>) {
    let mut rig = forked();
    let anchor = pages(&rig)[0].rect.center().to_vec2();
    let mut titles = std::collections::HashMap::new();
    for n in 0..9 {
        let at = anchor + egui::vec2(rng.between(-250.0, 450.0), rng.between(-200.0, 250.0));
        let (title, id) = match n % 6 {
            0 => {
                let (id, title) = (rig.app.alloc_node_id(), format!("CARD{n}"));
                let tab = crate::session_sync::tests::sleeping_tab("https://stress.test/", &title);
                rig.app.insert_placeholder_node(id, at, crate::store::model::Kind::Tab, tab);
                (title, id)
            }
            1 => {
                let (id, title) = (rig.app.alloc_node_id(), format!("FILE{n}"));
                let download =
                    crate::Runtime::Download(crate::node::DownloadPayload::finished(format!("{title}.bin"), 0));
                rig.app.insert_placeholder_node(id, at, crate::store::model::Kind::Download, download);
                (title, id)
            }
            2 => {
                let title = format!("GROUP{n}");
                let placement = crate::fetched::Placement { group: None, pos: at };
                (title.clone(), rig.app.place_group(None, &title, placement))
            }
            3 => {
                let (id, title) = (rig.app.alloc_node_id(), format!("NOTE{n}"));
                let note = crate::Runtime::Note(crate::node::NotePayload { text: title.clone(), editing: false });
                rig.app.insert_placeholder_node(id, at, crate::store::model::Kind::Note, note);
                (title, id)
            }
            4 => {
                let (id, title) = (rig.app.alloc_node_id(), format!("IMG{n}"));
                let image =
                    crate::node::ImagePayload { path: format!("{title}.png"), width: 0, height: 0, error: None };
                rig.app.insert_placeholder_node(id, at, crate::store::model::Kind::Image, crate::Runtime::Image(image));
                (title, id)
            }
            _ => {
                let (id, title) = (rig.app.alloc_node_id(), format!("PDF{n}"));
                let pdf = crate::node::PdfPayload { path: format!("{title}.pdf"), outcome: None };
                rig.app.insert_placeholder_node(id, at, crate::store::model::Kind::Pdf, crate::Runtime::Pdf(pdf));
                (title, id)
            }
        };
        titles.insert(CardId::from(id), title);
    }
    if rng.next() < 0.5 {
        let mut ids: Vec<_> = titles.keys().cloned().collect();
        ids.sort_by_key(|id| titles[id].clone());
        rig.app.select_only(ids[(rng.next() * ids.len() as f32) as usize % ids.len()]);
    }
    (rig, titles)
}

/// Frames the rig a few times so the camera settles, and returns the last frame's shapes.
fn settled_shapes(rig: &mut Rig) -> Vec<egui::epaint::ClippedShape> {
    let mut shapes = Vec::new();
    for _ in 0..4 {
        shapes = frame_shapes(rig);
        rig.app.camera.tween = None;
    }
    shapes
}

/// How many shown titles `shapes` has (their kinds' first four letters added to `kinds`), or the first title that
/// shows inside a card drawn after its own.
fn titles_behind_later_cards(
    rig: &Rig,
    shapes: &[egui::epaint::ClippedShape],
    titles: &std::collections::HashMap<CardId, String>,
    kinds: &mut std::collections::HashSet<String>,
) -> Result<usize, String> {
    let order = rig.app.level_items();
    let screen = |e: &crate::level::CardEntry| rig.app.graph_world_rect_to_screen(e.rect);
    let mut checked = 0;
    for (i, lower) in order.iter().enumerate() {
        let Some(title) = titles.get(&lower.id) else { continue };
        for (at, bounds, _) in shown_text_at(shapes, title) {
            checked += 1;
            kinds.insert(title[..4].to_string());
            // `bounds` is the text's line box, taller than its ink: a card edge within a pixel of it is not a show-through.
            // Only the scene area counts: the card pane beside it names the selected card over cards it covers.
            let ink = bounds.intersect(rig.app.graph_scene_outer_rect()).shrink2(egui::vec2(0.0, 1.0));
            // Text that sticks out past a later card's edge is fine: only the part inside that card must be hidden.
            let shows = |h: &&crate::level::CardEntry| {
                shared_area(ink, screen(h)) >= 1.0 && !painted_over(shapes, at, bounds.intersect(screen(h)))
            };
            if let Some(higher) = order[i + 1..].iter().find(shows) {
                let over = titles.get(&higher.id).cloned().unwrap_or_else(|| higher.id.to_string());
                return Err(format!("{title} shows through {over} ({bounds:?} over {:?})", screen(higher)));
            }
        }
    }
    Ok(checked)
}

/// The heights of the strip's first label where it shows inside its page.
fn strip_label_heights(rig: &Rig, shapes: &[egui::epaint::ClippedShape]) -> Vec<f32> {
    let label = rig.app.scoped.nav_history[&rig.tab()].tree()[0].label.clone();
    let page0 = rig.app.graph_world_rect_to_screen(pages(rig)[0].rect);
    shown_text(shapes, &label)
        .into_iter()
        .filter(|(bounds, _)| page0.contains_rect(*bounds) && bounds.area() > 0.0)
        .map(|(_, height)| height)
        .collect()
}

/// Random layouts at random zooms: no card's text shows inside a card drawn after it (the strip's pages
/// included; text painted under an opaque fill does not show), and the strip's labels keep one on-screen height at every zoom.
#[test]
fn random_layouts_never_show_a_cards_text_through_a_later_card_or_rescale_the_strip_label() {
    let (mut checked, mut label_heights, mut kinds) = (0, Vec::new(), std::collections::HashSet::new());
    for seed in 0..40_u64 {
        let mut rng = Lcg(seed + 1);
        let (mut rig, titles) = random_layout(&mut rng);
        let zoom = rng.between(0.5, 3.0);
        rig.app.graph.pending_zoom = Some(zoom);
        let shapes = settled_shapes(&mut rig);
        match titles_behind_later_cards(&rig, &shapes, &titles, &mut kinds) {
            Ok(n) => checked += n,
            Err(why) => panic!("seed {seed}, zoom {zoom}: {why}"),
        }
        label_heights.extend(strip_label_heights(&rig, &shapes));
    }
    assert_eq!(kinds.len(), 6, "every kind of card had text on screen to check: {kinds:?}");
    assert!(checked > 20, "the layouts must put titles on screen to mean anything: {checked}");
    assert!(label_heights.len() > 10, "the strip label must show in enough layouts: {}", label_heights.len());
    let first = label_heights[0];
    assert!(
        label_heights.iter().all(|h| *h == first),
        "the strip label's height varied with the zoom: {label_heights:?}"
    );
}

/// The same invariants while a card is held mid-drag onto another: a stored card on even seeds, a strip page on
/// odd ones.
#[test]
fn a_card_held_mid_drag_keeps_the_layering_and_the_strip_label_size() {
    let (mut checked, mut drags, mut kinds) = (0, 0, std::collections::HashSet::new());
    for seed in 0..24_u64 {
        let mut rng = Lcg(1000 + seed);
        let (mut rig, titles) = random_layout(&mut rng);
        let zoom = rng.between(0.8, 2.0);
        rig.app.graph.pending_zoom = Some(zoom);
        settled_shapes(&mut rig);
        let ids: Vec<CardId> = if seed % 2 == 0 {
            let mut ids: Vec<CardId> = titles.keys().copied().collect();
            ids.sort_by_key(|id| titles[id].clone());
            ids
        } else {
            pages(&rig).into_iter().map(|p| p.id).collect()
        };
        let grabbed = ids[(rng.next() * ids.len() as f32) as usize % ids.len()];
        let from = rig.screen(grabbed).center();
        let to = egui::pos2(rng.between(150.0, 850.0), rng.between(150.0, 650.0));
        rig.hold_drag(from, to);
        let shapes = settled_shapes(&mut rig);
        drags += usize::from(rig.app.interaction.dragging.is_some());
        match titles_behind_later_cards(&rig, &shapes, &titles, &mut kinds) {
            Err(why) => panic!("seed {seed}, dragging {grabbed} at zoom {zoom}: {why}"),
            Ok(n) => checked += n,
        }
        let heights = strip_label_heights(&rig, &shapes);
        assert!(heights.windows(2).all(|w| w[0] == w[1]), "seed {seed}: strip label heights differ: {heights:?}");
        rig.release(to);
    }
    assert!(checked > 10, "titles must be on screen mid-drag for this to mean anything: {checked}");
    assert!(drags > 6, "most grabs must actually start a drag: {drags}");
}
