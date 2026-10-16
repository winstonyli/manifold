//! The trail strip (spec `2026-10-16-trail-strip-design.md`): the focused tab's page tree as virtual cards under the
//! tab's card, and the painter and geometry the Back/Forward popups share.

use crate::card::{CardBody, DropEffect};
use crate::level::{CardEntry, EdgeCurve, EdgeKind, LevelEdge, Origin};
use crate::nav_history::TrailPage;
use crate::store::model::TYPE_TRAIL_PAGE;
use crate::theme::{ACCENT, SURFACE_HOVER, SURFACE_RAISED, TEXT_PRIMARY, TEXT_WEAK};
use crate::tree_layout::{tree_layout, TreeNode};
use crate::virtual_source::VirtualSource;
use crate::{App, CardKind, GraphAction};

pub(crate) const PAGE: egui::Vec2 = egui::vec2(160.0, 44.0);
pub(crate) const COLUMN_GAP: f32 = 24.0;
pub(crate) const ROW_GAP: f32 = 12.0;
/// The Back/Forward popups draw the same tree at a smaller cell.
pub(crate) const POPUP_PAGE: egui::Vec2 = egui::vec2(120.0, 28.0);
pub(crate) const POPUP_COLUMN_GAP: f32 = 16.0;
pub(crate) const POPUP_ROW_GAP: f32 = 6.0;
/// Between the tab card's bottom edge and the root page.
const STRIP_GAP: f32 = 24.0;

const ID_PREFIX: &str = "virtual:trail:";

/// `virtual:trail:<tab>:<epoch>:<entry>` -> (tab, epoch, entry).
fn page_ids(id: &str) -> Option<(&str, u32, usize)> {
    let (rest, entry) = id.strip_prefix(ID_PREFIX)?.rsplit_once(':')?;
    let (tab, epoch) = rest.rsplit_once(':')?;
    Some((tab, epoch.parse().ok()?, entry.parse().ok()?))
}

/// The tab and entry a page id names, only while that tab's trail has not evicted anything since the id was made (an
/// eviction shifts the indices after it, so an old id could now mean another page).
fn live_page<'a>(app: &App, id: &'a str) -> Option<(&'a str, usize)> {
    let (tab, epoch, entry) = page_ids(id)?;
    (app.nav_history.get(tab)?.epoch() == epoch).then_some((tab, entry))
}

/// Every page's rect, rebased so the strip's bounding box starts at the origin (a parent centres on its children, so
/// the tree's first lane can sit above the root).
pub(crate) fn page_rects(pages: &[TrailPage], cell: egui::Vec2, column_gap: f32, row_gap: f32) -> Vec<egui::Rect> {
    let nodes: Vec<TreeNode> = pages.iter().map(|p| TreeNode { parent: p.parent, size: cell }).collect();
    let rects: Vec<egui::Rect> =
        tree_layout(&nodes, column_gap, row_gap).into_iter().map(|min| egui::Rect::from_min_size(min, cell)).collect();
    let top_left = rects.iter().fold(egui::Rect::NOTHING, |b, r| b.union(*r)).min;
    rects.into_iter().map(|r| r.translate(-top_left.to_vec2())).collect()
}

/// One page: the frame, the label clipped to the page, dimmed off the path. On the card path (`card`) the card loop
/// already drew the frame and its border over this body, so no fill goes down and the cursor's ring is drawn outside
/// the page, clear of that border; the popup (not a card) paints its own fill and an inside ring.
pub(crate) fn paint_trail_page(painter: &egui::Painter, rect: egui::Rect, page: &TrailPage, hovered: bool, card: bool) {
    if !card {
        painter.rect_filled(rect, 6.0, if hovered { SURFACE_HOVER } else { SURFACE_RAISED });
    }
    if page.cursor {
        let kind = if card { egui::StrokeKind::Outside } else { egui::StrokeKind::Inside };
        painter.rect_stroke(rect, 6.0, egui::Stroke::new(2.0, ACCENT), kind);
    }
    let color = if page.cursor || page.on_path { TEXT_PRIMARY } else { TEXT_WEAK };
    let clip = painter.with_clip_rect(rect.shrink2(egui::vec2(6.0, 0.0)).intersect(painter.clip_rect()));
    clip.text(
        rect.left_center() + egui::vec2(8.0, 0.0),
        egui::Align2::LEFT_CENTER,
        &page.label,
        egui::FontId::proportional(12.0),
        color,
    );
}

pub(crate) struct TrailSource;

impl App {
    /// The tab the strip hangs under: focused, in Graph view, its card on the level shown, and a trail of two or more
    /// pages.
    fn trail_strip_tab(&self) -> Option<String> {
        if self.view_mode != crate::ViewMode::Graph {
            return None;
        }
        let tab = self.focused.clone()?;
        let on_level = self.card_rect(&tab).is_some() && self.in_level(&tab);
        (on_level && self.nav_history.get(&tab).is_some_and(|h| h.tree().len() > 1)).then_some(tab)
    }
}

impl TrailSource {
    fn page_id(tab: &str, epoch: u32, entry: usize) -> String {
        format!("{ID_PREFIX}{tab}:{epoch}:{entry}")
    }
}

impl VirtualSource for TrailSource {
    fn name(&self) -> &'static str {
        "trail"
    }

    fn entries(&self, app: &App) -> Vec<CardEntry> {
        let Some(tab) = app.trail_strip_tab() else { return Vec::new() };
        let (Some(history), Some(card)) = (app.nav_history.get(&tab), app.card_rect(&tab)) else { return Vec::new() };
        let (pages, epoch) = (history.tree(), history.epoch());
        let origin = card.left_bottom() + egui::vec2(0.0, STRIP_GAP);
        let kind = CardKind { ty: TYPE_TRAIL_PAGE, tags: crate::caps::TagSet::EMPTY };
        pages
            .iter()
            .zip(page_rects(&pages, PAGE, COLUMN_GAP, ROW_GAP))
            .map(|(page, rect)| CardEntry {
                id: Self::page_id(&tab, epoch, page.entry),
                kind,
                origin: Origin::Virtual("trail"),
                rect: rect.translate(origin.to_vec2()),
            })
            .collect()
    }

    fn edges(&self, app: &App, rect_of: &dyn Fn(&str) -> Option<egui::Rect>) -> Vec<LevelEdge> {
        let Some(tab) = app.trail_strip_tab() else { return Vec::new() };
        let Some(history) = app.nav_history.get(&tab) else { return Vec::new() };
        let (pages, epoch) = (history.tree(), history.epoch());
        pages
            .iter()
            .filter_map(|page| {
                let parent = &pages[page.parent?];
                let (from_id, to_id) =
                    (Self::page_id(&tab, epoch, parent.entry), Self::page_id(&tab, epoch, page.entry));
                Some(LevelEdge {
                    from: rect_of(&from_id)?.right_center(),
                    to: rect_of(&to_id)?.left_center(),
                    highlight: parent.on_path && page.on_path,
                    from_id,
                    to_id,
                    curve: EdgeCurve::Horizontal,
                    kind: EdgeKind::Opened,
                    link: None,
                })
            })
            .collect()
    }

    fn paint_body(&self, app: &App, ui: &mut egui::Ui, entry: &CardEntry) -> CardBody {
        let page = live_page(app, &entry.id).and_then(|(tab, i)| app.nav_history.get(tab)?.tree().into_iter().nth(i));
        if let Some(page) = page {
            paint_trail_page(ui.painter(), entry.rect, &page, false, true);
        }
        CardBody { interact_rect: entry.rect, tooltip: None }
    }

    fn on_click(&self, app: &mut App, entry: &CardEntry) -> Option<GraphAction> {
        let (tab, page) = live_page(app, &entry.id)?;
        app.trail_pick(tab, page);
        None
    }

    fn on_menu(&self, _app: &mut App, _entry: &CardEntry, _at: egui::Pos2) {} // never called: the kind has no `menu`

    /// Every release (on a card, empty canvas, or the back button's `Eject`) opens the page as a card at `at`, from the
    /// tab whose trail it came from (spec decision 5), so the target is ignored.
    fn on_drop(
        &self,
        app: &mut App,
        entry: &CardEntry,
        _target: Option<(DropEffect, Option<String>)>,
        at: egui::Pos2,
    ) -> Option<GraphAction> {
        let (tab, page) = live_page(app, &entry.id)?;
        app.open_branch_as_card(tab, page, Some(at.to_vec2()));
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nav_history::NavHistory;
    use crate::virtual_source::fake::Rig;

    /// /a -> /b -> /c and /a -> /d, cursor on /d, on the rig's tab, which is focused.
    fn forked() -> Rig {
        let mut rig = Rig::new();
        let tab = rig.tab();
        rig.app.nav_history.insert(
            tab.clone(),
            NavHistory::with_tree(
                &[("/a", None, Some(3)), ("/b", Some(0), Some(2)), ("/c", Some(1), None), ("/d", Some(0), None)],
                3,
            ),
        );
        rig.app.focused = Some(tab);
        rig.app.view_mode = crate::ViewMode::Graph;
        rig.frame(vec![], egui::Modifiers::NONE);
        rig
    }

    fn pages(rig: &Rig) -> Vec<crate::level::CardEntry> {
        rig.app.level_items().into_iter().filter(|e| e.id.starts_with("virtual:trail:")).collect()
    }

    #[test]
    fn a_forked_trail_shows_one_card_per_page_under_the_tab_card() {
        let rig = forked();
        let tab = rig.tab();
        let cards = pages(&rig);
        assert_eq!(cards.len(), 4);
        assert!(cards.iter().all(|c| c.kind.ty == TYPE_TRAIL_PAGE && c.origin == Origin::Virtual("trail")));
        assert_eq!(cards[0].id, format!("virtual:trail:{tab}:0:0"));
        let below = rig.app.card_rect(&tab).unwrap().left_bottom() + egui::vec2(0.0, STRIP_GAP);
        let top_left = cards.iter().fold(egui::Rect::NOTHING, |b, c| b.union(c.rect)).min;
        assert_eq!(top_left, below, "the whole strip hangs under the tab card, none of it above");
        assert_eq!(cards[0].rect.size(), PAGE);
        assert_eq!(cards[0].rect.min.x, below.x, "the root is the leftmost column");
        assert_eq!(cards[1].rect.min.x, below.x + PAGE.x + COLUMN_GAP, "children one column right");
        assert!(cards[3].rect.min.y > cards[1].rect.min.y, "the later branch sits on a lower lane");
    }

    #[test]
    fn no_strip_for_one_page_no_focus_tab_view_or_another_level() {
        let mut rig = forked();
        let tab = rig.tab();
        rig.app.focused = None;
        assert!(pages(&rig).is_empty(), "no focused tab");
        rig.app.focused = Some(tab.clone());
        rig.app.view_mode = crate::ViewMode::Item;
        assert!(pages(&rig).is_empty(), "Tab view");
        rig.app.view_mode = crate::ViewMode::Graph;
        rig.app.current_group = Some("elsewhere".into());
        assert!(pages(&rig).is_empty(), "the tab is on another level");
        rig.app.current_group = None;
        rig.app.nav_history.insert(tab, NavHistory::with_tree(&[("/a", None, None)], 0));
        assert!(pages(&rig).is_empty(), "one page");
    }

    #[test]
    fn edges_join_parent_to_child_and_mark_the_path() {
        let rig = forked();
        let tab = rig.tab();
        let edges: Vec<_> =
            rig.app.level_edges().into_iter().filter(|e| e.from_id.starts_with("virtual:trail:")).collect();
        let key = |e: &crate::level::LevelEdge| (e.from_id.clone(), e.to_id.clone(), e.highlight);
        let id = |i: usize| format!("virtual:trail:{tab}:0:{i}");
        assert_eq!(
            edges.iter().map(key).collect::<Vec<_>>(),
            [(id(0), id(1), false), (id(1), id(2), false), (id(0), id(3), true)]
        );
    }

    #[test]
    fn a_long_url_label_paints_clipped_to_the_page() {
        let ctx = egui::Context::default();
        crate::theme::apply_fonts(&ctx);
        let page = TrailPage { entry: 0, parent: None, label: "x".repeat(400), cursor: false, on_path: false };
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, PAGE);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let painter = ui.painter().with_clip_rect(rect);
            paint_trail_page(&painter, rect, &page, false, true);
        });
        output.textures_delta.clear();
    }

    #[test]
    fn clicking_a_page_starts_a_plain_move_to_it_and_not_while_one_is_pending() {
        let mut rig = forked();
        let tab = rig.tab();
        let b = rig.screen(&format!("virtual:trail:{tab}:0:1")).center();
        rig.click(b);
        let now = std::time::Instant::now();
        assert!(rig.app.nav_history[&tab].move_pending(now), "the click started a move");
        // A second click on another page while that move is pending is ignored.
        let c = rig.screen(&format!("virtual:trail:{tab}:0:2")).center();
        rig.click(c);
        assert!(rig.app.nav_history[&tab].move_pending(now), "still the first move");
    }

    #[test]
    fn clicking_the_cursor_page_or_an_evicted_one_does_nothing() {
        let mut rig = forked();
        let tab = rig.tab();
        let d = rig.screen(&format!("virtual:trail:{tab}:0:3")).center();
        rig.click(d);
        assert!(!rig.app.nav_history[&tab].move_pending(std::time::Instant::now()), "the cursor page");
        rig.app.trail_pick(&tab, 99);
        assert!(!rig.app.nav_history[&tab].move_pending(std::time::Instant::now()), "an index past the end");
    }

    #[test]
    fn dragging_a_page_onto_empty_canvas_opens_it_as_a_card_there() {
        let mut rig = forked();
        let tab = rig.tab();
        let from = rig.screen(&format!("virtual:trail:{tab}:0:1")).center();
        let to = from + egui::vec2(0.0, 300.0);
        rig.drag(from, to);
        let (spawn, _) = rig.app.test_spawns.last().expect("a tab was spawned");
        assert_eq!(spawn.url, "/b");
        assert_eq!(spawn.parent.as_deref(), Some(tab.as_str()), "opened from the tab whose trail it came from");
        let world = rig.app.graph_screen_point_to_world(to);
        assert!((spawn.pos - world).length() < 1.0, "placed at the release point");
        assert_eq!(rig.app.nav_history[&tab].export()["entries"], 4, "the trail is unchanged");
    }

    #[test]
    fn dropping_a_page_on_a_card_opens_it_the_same_way() {
        let mut rig = forked();
        let tab = rig.tab();
        let from = rig.screen(&format!("virtual:trail:{tab}:0:2")).center();
        let to = rig.app.graph_world_rect_to_screen(rig.app.card_rect(&tab).unwrap()).center();
        rig.drag(from, to);
        let (spawn, _) = rig.app.test_spawns.last().expect("a tab was spawned");
        assert_eq!(spawn.url, "/c");
        assert_eq!(spawn.parent.as_deref(), Some(tab.as_str()), "the card dropped on is not the opener");
    }

    #[test]
    fn popup_geometry_is_the_same_tree_at_a_smaller_cell() {
        let pages =
            NavHistory::with_tree(&[("/a", None, None), ("/b", Some(0), None), ("/c", Some(0), None)], 0).tree();
        let rects = page_rects(&pages, POPUP_PAGE, POPUP_COLUMN_GAP, POPUP_ROW_GAP);
        assert_eq!(rects.len(), 3);
        assert_eq!(rects[1].min.x, POPUP_PAGE.x + POPUP_COLUMN_GAP, "second column");
        assert!(rects[2].min.y > rects[1].min.y, "second branch below the first");
        assert_eq!(rects[0].size(), POPUP_PAGE);
    }

    #[test]
    fn a_page_evicted_at_the_cap_retires_every_old_page_id() {
        let mut rig = forked();
        let tab = rig.tab();
        let urls: Vec<String> = (0..50).map(|i| format!("/p{i}")).collect();
        let refs: Vec<&str> = urls.iter().map(String::as_str).collect();
        rig.app.nav_history.insert(tab.clone(), NavHistory::visited(&refs));
        rig.frame(vec![], egui::Modifiers::NONE);
        let before: Vec<String> = pages(&rig).into_iter().map(|c| c.id).collect();
        assert_eq!(before.len(), 50);
        let h = rig.app.nav_history.get_mut(&tab).unwrap();
        let now = std::time::Instant::now();
        h.observe("/new", "", true, false, now);
        h.observe("/new", "", false, false, now);
        rig.frame(vec![], egui::Modifiers::NONE);
        let after: Vec<String> = pages(&rig).into_iter().map(|c| c.id).collect();
        assert_eq!(after.len(), 50, "still 50 pages");
        assert!(after.iter().all(|id| !before.contains(id)), "an index that now means another page has a new id");
    }

    #[test]
    fn the_strip_does_not_stretch_fit_or_the_level_of_detail_count() {
        let mut rig = forked();
        let with = rig.app.fit_all_nodes_scene_rect();
        rig.app.focused = None;
        assert_eq!(with, rig.app.fit_all_nodes_scene_rect(), "fit frames the cards, not the strip");
    }

    #[test]
    fn the_card_path_leaves_the_frame_to_the_card_and_the_popup_paints_its_own() {
        let ctx = egui::Context::default();
        crate::theme::apply_fonts(&ctx);
        ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, PAGE);
        let page = TrailPage { entry: 0, parent: None, label: "a".into(), cursor: true, on_path: true };
        let rects = |card: bool| {
            let layer = egui::LayerId::new(egui::Order::Background, egui::Id::new(("paint_probe", card)));
            paint_trail_page(&ctx.layer_painter(layer), rect, &page, false, card);
            ctx.graphics_mut(|g| {
                g.entry(layer)
                    .all_entries()
                    .filter_map(|c| match &c.shape {
                        egui::Shape::Rect(r) => Some((r.fill, r.stroke_kind)),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            })
        };
        let card = rects(true);
        assert!(card.iter().all(|(fill, _)| *fill == egui::Color32::TRANSPARENT), "no fill over the card's frame");
        assert!(card.iter().any(|(_, kind)| *kind == egui::StrokeKind::Outside), "the cursor ring clears its border");
        assert!(rects(false).iter().any(|(fill, _)| *fill != egui::Color32::TRANSPARENT), "the popup has a fill");
    }
}
