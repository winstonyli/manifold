//! The trail strip (spec `2026-10-16-trail-strip-design.md`): the focused tab's page tree as virtual cards under the
//! tab's card, and the painter and geometry the Back/Forward popups share.

use crate::card::{CardBody, DropEffect};
use crate::level::{CardEntry, EdgeCurve, EdgeKind, LevelEdge, Origin};
use crate::nav_history::TrailPage;
use crate::store::model::TYPE_TRAIL_PAGE;
use crate::theme::{menu_item_style, popup_frame, ACCENT, SURFACE_HOVER, TEXT_PRIMARY, TEXT_WEAK};
use crate::tree_layout::{tree_layout, TreeNode};
use crate::virtual_source::VirtualSource;
use crate::{App, CardKind, GraphAction};

/// The most a strip page can be (a long label is clipped to it); a shorter label gets a narrower page.
pub(crate) const PAGE: egui::Vec2 = egui::vec2(160.0, 44.0);
/// The least a page is wide, so a one-letter label still reads as a button.
pub(crate) const MIN_PAGE_WIDTH: f32 = 56.0;
/// Text inset on each side of a label.
const LABEL_INSET: f32 = 8.0;
pub(crate) const COLUMN_GAP: f32 = 24.0;
pub(crate) const ROW_GAP: f32 = 12.0;
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

fn label_font() -> egui::FontId {
    egui::FontId::proportional(12.0)
}

/// Each page's size: its label's width plus the insets, within `MIN_PAGE_WIDTH` and `cell.x`, at `cell.y` high.
pub(crate) fn page_sizes(ctx: &egui::Context, pages: &[TrailPage], cell: egui::Vec2) -> Vec<egui::Vec2> {
    pages
        .iter()
        .map(|p| {
            let text =
                ctx.fonts_mut(|f| f.layout_no_wrap(p.label.clone(), label_font(), egui::Color32::WHITE)).size().x;
            egui::vec2((text + 2.0 * LABEL_INSET).max(MIN_PAGE_WIDTH).min(cell.x), cell.y)
        })
        .collect()
}

/// Every page's rect (`sizes[i]` is page `i`'s size), rebased so the strip's bounding box starts at the origin (a parent
/// centres on its children, so the tree's first lane can sit above the root).
pub(crate) fn page_rects(pages: &[TrailPage], sizes: &[egui::Vec2], column_gap: f32, row_gap: f32) -> Vec<egui::Rect> {
    let nodes: Vec<TreeNode> = pages.iter().zip(sizes).map(|(p, &size)| TreeNode { parent: p.parent, size }).collect();
    let rects: Vec<egui::Rect> = tree_layout(&nodes, column_gap, row_gap)
        .into_iter()
        .zip(sizes)
        .map(|(min, &size)| egui::Rect::from_min_size(min, size))
        .collect();
    let top_left = rects.iter().fold(egui::Rect::NOTHING, |b, r| b.union(*r)).min;
    rects.into_iter().map(|r| r.translate(-top_left.to_vec2())).collect()
}

/// One page's body: the card loop already drew the frame and its border over it, so only a hover fills it and the
/// cursor's ring is drawn outside the page, clear of that border; the label is clipped to the page, dimmed off the
/// path.
pub(crate) fn paint_trail_page(painter: &egui::Painter, rect: egui::Rect, page: &TrailPage, hovered: bool) {
    if hovered {
        painter.rect_filled(rect, 6.0, SURFACE_HOVER);
    }
    if page.cursor {
        painter.rect_stroke(rect, 6.0, egui::Stroke::new(2.0, ACCENT), egui::StrokeKind::Outside);
    }
    let color = if page.cursor || page.on_path { TEXT_PRIMARY } else { TEXT_WEAK };
    // One line, cut with an ellipsis at the page's inner width.
    let mut job = egui::text::LayoutJob::simple_singleline(page.label.clone(), label_font(), color);
    job.wrap = egui::text::TextWrapping {
        max_width: (rect.width() - 2.0 * LABEL_INSET).max(0.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let galley = painter.layout_job(job);
    let at = rect.left_center() + egui::vec2(LABEL_INSET, -galley.size().y / 2.0);
    painter.with_clip_rect(rect.intersect(painter.clip_rect())).galley(at, galley, color);
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

/// A page's menu items, shared by the popup's rows and the strip's menu: whether "Open as card" was chosen.
pub(crate) fn page_menu_items(ui: &mut egui::Ui) -> bool {
    ui.button("Open as card").clicked()
}

impl App {
    /// The strip page menu (`menus.trail_menu`), at the pointer. Closes on a choice, a click elsewhere, Escape, or when
    /// its page is gone or nothing hosts it: the strip, or an open Back/Forward branch popup (Tab view has no strip).
    pub(crate) fn build_trail_menu(&mut self, ui: &mut egui::Ui) {
        let Some((id, pos)) = self.menus.trail_menu.clone() else { return };
        let frame = ui.ctx().cumulative_frame_nr();
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("trail_menu_frame"), frame));
        let hosted = self.trail_strip_tab().is_some()
            || matches!(
                self.menus.open_popup,
                Some(crate::popups::AnchoredPopup::BackBranches | crate::popups::AnchoredPopup::ForwardBranches)
            );
        let page = live_page(self, &id).map(|(tab, page)| (tab.to_owned(), page));
        let Some((tab, page)) = page.filter(|_| hosted) else {
            self.menus.trail_menu = None;
            return;
        };
        let mut chosen = false;
        let area = egui::Area::new(egui::Id::new("trail_page_menu"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(ui.ctx(), |ui| {
                popup_frame(ui).show(ui, |ui| {
                    menu_item_style(ui);
                    chosen = page_menu_items(ui);
                });
            });
        if chosen {
            self.open_branch_as_card(&tab, page, None);
        }
        // The right-click that opened the menu is, to a menu not yet laid out, a click elsewhere.
        let opening_click = ui.input(|i| i.pointer.interact_pos()) == Some(pos);
        let elsewhere = area.response.clicked_elsewhere() && !opening_click;
        if chosen || elsewhere || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.menus.trail_menu = None;
        }
    }
}

impl TrailSource {
    fn page_id(tab: &str, epoch: u32, entry: usize) -> String {
        format!("{ID_PREFIX}{tab}:{epoch}:{entry}")
    }
}

/// Tab `tab`'s page cards, their bounding box's top-left at `origin`: the strip hangs it under the tab's card, the Back
/// and Forward popups use their own origin. Empty when the tab has no trail.
pub(crate) fn trail_cards(app: &App, tab: &str, origin: egui::Pos2) -> Vec<CardEntry> {
    let Some(history) = app.nav_history.get(tab) else { return Vec::new() };
    let (pages, epoch) = (history.tree(), history.epoch());
    let kind = CardKind { ty: TYPE_TRAIL_PAGE, tags: crate::caps::TagSet::EMPTY };
    pages
        .iter()
        .zip(page_rects(&pages, &page_sizes(&app.egui_ctx, &pages, PAGE), COLUMN_GAP, ROW_GAP))
        .map(|(page, rect)| CardEntry {
            id: TrailSource::page_id(tab, epoch, page.entry),
            kind,
            origin: Origin::Virtual("trail"),
            rect: rect.translate(origin.to_vec2()),
        })
        .collect()
}

/// The edges between `tab`'s pages (`rect_of` finds a card's rect by id); those on the path to the cursor are
/// highlighted.
pub(crate) fn trail_edges(app: &App, tab: &str, rect_of: &dyn Fn(&str) -> Option<egui::Rect>) -> Vec<LevelEdge> {
    let Some(history) = app.nav_history.get(tab) else { return Vec::new() };
    let (pages, epoch) = (history.tree(), history.epoch());
    pages
        .iter()
        .filter_map(|page| {
            let parent = &pages[page.parent?];
            let (from_id, to_id) =
                (TrailSource::page_id(tab, epoch, parent.entry), TrailSource::page_id(tab, epoch, page.entry));
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

impl VirtualSource for TrailSource {
    fn name(&self) -> &'static str {
        "trail"
    }

    fn entries(&self, app: &App) -> Vec<CardEntry> {
        let Some(tab) = app.trail_strip_tab() else { return Vec::new() };
        let Some(card) = app.card_rect(&tab) else { return Vec::new() };
        trail_cards(app, &tab, card.left_bottom() + egui::vec2(0.0, STRIP_GAP))
    }

    fn edges(&self, app: &App, rect_of: &dyn Fn(&str) -> Option<egui::Rect>) -> Vec<LevelEdge> {
        app.trail_strip_tab().map_or_else(Vec::new, |tab| trail_edges(app, &tab, rect_of))
    }

    fn paint_body(&self, app: &App, ui: &mut egui::Ui, entry: &CardEntry) -> CardBody {
        let page = live_page(app, &entry.id).and_then(|(tab, i)| app.nav_history.get(tab)?.tree().into_iter().nth(i));
        if let Some(page) = &page {
            paint_trail_page(ui.painter(), entry.rect, page, ui.rect_contains_pointer(entry.rect));
        }
        CardBody { interact_rect: entry.rect, tooltip: page.map(|p| p.url) }
    }

    fn on_click(&self, app: &mut App, entry: &CardEntry) -> Option<GraphAction> {
        let (tab, page) = live_page(app, &entry.id)?;
        app.trail_pick(tab, page);
        None
    }

    fn on_menu(&self, app: &mut App, entry: &CardEntry, at: egui::Pos2) {
        app.menus.trail_menu = Some((entry.id.clone(), at));
    }

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
        let cards = trail_cards(&rig.app, &tab, origin);
        assert_eq!(cards.len(), 4);
        assert_eq!(cards.iter().fold(egui::Rect::NOTHING, |b, e| b.union(e.rect)).min, origin);
        let rect_of = |id: &str| cards.iter().find(|e| e.id == id).map(|e| e.rect);
        assert_eq!(trail_edges(&rig.app, &tab, &rect_of).len(), 3);
        assert!(trail_cards(&rig.app, "no-such-tab", origin).is_empty());
    }

    #[test]
    fn the_page_menu_stays_while_a_branch_popup_is_open_and_closes_when_neither_shows() {
        let mut rig = forked();
        let tab = rig.tab();
        let id = TrailSource::page_id(&tab, rig.app.nav_history[&tab].epoch(), 1);
        rig.app.view_mode = crate::ViewMode::Item;
        rig.app.menus.trail_menu = Some((id.clone(), egui::pos2(5.0, 5.0)));
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
        let page = TrailPage {
            entry: 0,
            parent: None,
            label: "x".repeat(400),
            url: String::new(),
            cursor: false,
            on_path: false,
        };
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, PAGE);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let painter = ui.painter().with_clip_rect(rect);
            paint_trail_page(&painter, rect, &page, false);
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

    fn text_ctx() -> egui::Context {
        let ctx = egui::Context::default();
        crate::theme::apply_fonts(&ctx);
        ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
        ctx
    }

    #[test]
    fn a_page_is_as_wide_as_its_label_within_the_clamp() {
        let ctx = text_ctx();
        let page = |label: &str| TrailPage {
            entry: 0,
            parent: None,
            label: label.into(),
            url: String::new(),
            cursor: false,
            on_path: false,
        };
        let widths: Vec<f32> =
            page_sizes(&ctx, &[page("a"), page("a medium length title"), page(&"w".repeat(400))], PAGE)
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
        rig.app.nav_history.insert(tab.clone(), h);
        rig.frame(vec![], egui::Modifiers::NONE);
        let entry = pages(&rig).remove(1);
        let mut tip = None;
        let ctx = rig.ctx.clone();
        ctx.run_ui(egui::RawInput::default(), |ui| {
            tip = TrailSource.paint_body(&rig.app, ui, &entry).tooltip;
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
        let at = rig.screen(&format!("virtual:trail:{tab}:0:2")).center();
        rig.right_click(at);
        let (id, pos) = rig.app.menus.trail_menu.clone().expect("the page's menu is open");
        assert_eq!(id, format!("virtual:trail:{tab}:0:2"));
        assert_eq!(pos, at, "at the pointer");
        rig.frame(vec![], egui::Modifiers::NONE);
        let menu = rig.ctx.memory(|m| m.area_rect(egui::Id::new("trail_page_menu"))).expect("the menu's area");
        rig.click(menu.center());
        assert!(rig.app.menus.trail_menu.is_none(), "choosing closes it");
        let (spawn, _) = rig.app.test_spawns.last().expect("Open as card spawned a tab");
        assert_eq!(spawn.url, "/c");
        assert_eq!(spawn.parent.as_deref(), Some(tab.as_str()));
    }

    #[test]
    fn a_menu_for_a_page_that_is_gone_closes() {
        let mut rig = forked();
        let tab = rig.tab();
        rig.app.menus.trail_menu = Some((format!("virtual:trail:{tab}:7:2"), egui::pos2(10.0, 10.0)));
        rig.frame(vec![], egui::Modifiers::NONE);
        assert!(rig.app.menus.trail_menu.is_none(), "a stale epoch names no page");
    }

    #[test]
    fn a_column_is_as_wide_as_its_widest_page() {
        let pages =
            NavHistory::with_tree(&[("/a", None, None), ("/b", Some(0), None), ("/c", Some(0), None)], 0).tree();
        let sizes = [egui::vec2(60.0, 20.0), egui::vec2(100.0, 20.0), egui::vec2(70.0, 20.0)];
        let rects = page_rects(&pages, &sizes, 10.0, 4.0);
        assert_eq!(rects[1].min.x, 70.0, "children start after the root's column");
        assert_eq!(rects[2].min.x, rects[1].min.x, "siblings share the column's left edge");
        assert_eq!(rects[2].size(), sizes[2], "each page keeps its own width");
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
    fn a_page_leaves_the_frame_to_the_card_and_rings_the_cursor_outside_it() {
        let ctx = egui::Context::default();
        crate::theme::apply_fonts(&ctx);
        ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, PAGE);
        let page =
            TrailPage { entry: 0, parent: None, label: "a".into(), url: String::new(), cursor: true, on_path: true };
        let layer = egui::LayerId::new(egui::Order::Background, egui::Id::new("paint_probe"));
        paint_trail_page(&ctx.layer_painter(layer), rect, &page, false);
        let rects = ctx.graphics_mut(|g| {
            g.entry(layer)
                .all_entries()
                .filter_map(|c| match &c.shape {
                    egui::Shape::Rect(r) => Some((r.fill, r.stroke_kind)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        });
        assert!(rects.iter().all(|(fill, _)| *fill == egui::Color32::TRANSPARENT), "no fill over the card frame");
        assert!(rects.iter().any(|(_, kind)| *kind == egui::StrokeKind::Outside), "the cursor ring clears its border");
    }
}
