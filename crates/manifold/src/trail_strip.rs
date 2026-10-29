//! The trail strip (spec `2026-10-16-trail-strip-design.md`): the focused tab's page tree as virtual cards under the
//! tab's card, and the painter and geometry the Back/Forward popups share.

use crate::card::{label_fits, CardBody, CardPass, DropEffect};
use crate::card_id::{fnv64, CardId};
use crate::identity::{Identity, ICON_SIZE_SMALL};
use crate::level::{CardEntry, EdgeCurve, EdgeKind, LevelEdge, Origin};
use crate::nav_history::{NavHistory, TrailPage};
use crate::popups::menu_popup;
use crate::store::model::{NodeId, TYPE_TRAIL_PAGE};
use crate::theme::{accent, palette};
use crate::tree_layout::{tree_layout, TreeNode};
use crate::virtual_source::VirtualSource;
use crate::{App, CardKind, GraphAction};

/// The most a strip page can be (a long label is clipped to it); a shorter label gets a narrower page.
pub(crate) const PAGE: egui::Vec2 = egui::vec2(160.0, 44.0);
/// The least a page is wide, so a one-letter label still reads as a button.
pub(crate) const MIN_PAGE_WIDTH: f32 = 56.0;
/// Text inset on each side of a label.
const LABEL_INSET: f32 = 8.0;
/// The label's left edge: inset, icon, gap.
const LABEL_LEFT: f32 = LABEL_INSET + ICON_SIZE_SMALL + 6.0;
pub(crate) const COLUMN_GAP: f32 = 24.0;
pub(crate) const ROW_GAP: f32 = 12.0;
/// Between the tab card's bottom edge and the root page.
const STRIP_GAP: f32 = 24.0;

/// The source name of the strip's cards (`VirtualSource::name`).
const SOURCE: &str = "trail";

/// A page's card id: the tab's slot above the page's serial (both below 2^31, so the key's top bit stays free).
fn page_card(history: &NavHistory, serial: u32) -> CardId {
    CardId::virt(SOURCE, u64::from(history.slot().get()) << 32 | u64::from(serial))
}

/// Whether `id` is a strip page's card id.
pub(crate) fn is_page_id(id: CardId) -> bool {
    id.source_tag() == Some(fnv64(&[SOURCE]))
}

/// The tab and arena index a page id names, for as long as that page is in its tab's trail: the serial in the id
/// outlives an eviction of an earlier page and resolves to nothing once its own page is evicted.
fn live_page(app: &App, id: CardId) -> Option<(NodeId, usize)> {
    let key = id.key().filter(|_| is_page_id(id))?;
    let (slot, serial) = ((key >> 32) as u32, key as u32);
    let (tab, history) = app.nav_history.iter().find(|(_, h)| h.slot().get() == slot)?;
    Some((*tab, history.index_of(serial)?))
}

fn label_font() -> egui::FontId {
    egui::FontId::proportional(12.0)
}

/// Each page's size: its label's width plus the icon slot and insets, within `MIN_PAGE_WIDTH` and `cell.x`, at `cell.y` high.
pub(crate) fn page_sizes(ctx: &egui::Context, pages: &[TrailPage], cell: egui::Vec2) -> Vec<egui::Vec2> {
    pages
        .iter()
        .map(|p| {
            let text =
                ctx.fonts_mut(|f| f.layout_no_wrap(p.label.clone(), label_font(), egui::Color32::WHITE)).size().x;
            egui::vec2((text + LABEL_LEFT + LABEL_INSET).max(MIN_PAGE_WIDTH).min(cell.x), cell.y)
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

/// One page's body, under the same convention as every card: shapes are sized in screen points times `pass.inv_zoom`,
/// and the label is queued on the overlay layer at its real screen size, cut with an ellipsis at the page's inner screen
/// width and skipped (`labelled` false, or the page narrower than a card label may be) when the graph is zoomed out. The
/// card loop already drew the frame and its border over the page, so only a hover fills it and the cursor's ring is
/// drawn outside the page, clear of that border; the label is dimmed off the path.
pub(crate) fn paint_trail_page(
    painter: &egui::Painter,
    pass: &mut CardPass,
    rect: egui::Rect,
    page: &TrailPage,
    hovered: bool,
    labelled: bool,
) {
    let (inv_zoom, zoom_scale) = (pass.inv_zoom, pass.zoom_scale);
    let radius = (6.0 * inv_zoom).min(255.0);
    let visuals = painter.ctx().global_style().visuals.clone();
    if hovered {
        painter.rect_filled(rect, radius, visuals.widgets.hovered.bg_fill);
    }
    if page.cursor {
        painter.rect_stroke(
            rect,
            radius,
            egui::Stroke::new(2.0 * inv_zoom, accent(&visuals)),
            egui::StrokeKind::Outside,
        );
    }
    if !labelled || !label_fits(rect, zoom_scale) {
        return;
    }
    let color = if page.cursor || page.on_path { palette(&visuals).text } else { palette(&visuals).text_weak };
    let mut job = egui::text::LayoutJob::simple_singleline(page.label.clone(), label_font(), color);
    job.wrap = egui::text::TextWrapping {
        max_width: (rect.width() * zoom_scale - LABEL_LEFT - LABEL_INSET).max(0.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let (origin, screen_height) = (rect.left_top(), rect.height() * zoom_scale);
    pass.deferred_overlays.push(Box::new(move |overlay_painter, project| {
        let galley = overlay_painter.layout_job(job);
        let at = project(origin) + egui::vec2(LABEL_LEFT, (screen_height - galley.size().y) / 2.0);
        overlay_painter.galley(at, galley, color);
    }));
}

/// A page's icon at its left: the page's favicon once fetched, else its host's tile in the tab's engine color. A tile's
/// letter is queued on the overlay layer with the label.
fn paint_page_icon(app: &App, ui: &egui::Ui, pass: &mut CardPass, rect: egui::Rect, tab: NodeId, url: &str) {
    let Some(identity) = app.page_identity(tab, url) else { return };
    let center = rect.left_center() + egui::vec2(LABEL_INSET + ICON_SIZE_SMALL / 2.0, 0.0) * pass.inv_zoom;
    if let Some(text) = identity.paint(ui, center, ICON_SIZE_SMALL, pass.inv_zoom, palette(ui.visuals()).text_weak) {
        pass.deferred_overlays.push(Box::new(move |overlay_painter, project| {
            text.draw(overlay_painter, project(text.center));
        }));
    }
}

pub(crate) struct TrailSource;

impl App {
    /// What `url` looks like as a page of `tab`'s trail (`Identity::of_tab`, never loading); `None` if `tab` is gone.
    fn page_identity(&self, tab: NodeId, url: &str) -> Option<Identity> {
        let kind = self.nodes.get(&tab)?.as_tab()?.tab.kind;
        Some(self.tab_identity(url, false, kind))
    }
}

impl App {
    /// The tab the strip hangs under: focused, in Graph view, its card on the level shown, and a trail of two or more
    /// pages.
    fn trail_strip_tab(&self) -> Option<NodeId> {
        if self.view_mode != crate::ViewMode::Graph {
            return None;
        }
        let tab = self.focused?;
        let on_level = self.card_rect(tab).is_some() && self.in_level(tab);
        (on_level && self.nav_history.get(&tab).is_some_and(|h| h.tree().len() > 1)).then_some(tab)
    }
}

/// A page's menu items, shared by the popup's rows and the strip's menu: whether "Open as card" was chosen.
pub(crate) fn page_menu_items(ui: &mut egui::Ui) -> bool {
    ui.button("Open as card").clicked()
}

impl App {
    /// Eases the camera to a trail strip that has just appeared (Graph view entered, another tab focused, a first
    /// branch): the strip hangs under its tab's card wherever the layout put that, and nothing else moves the camera,
    /// so it could sit off screen. Frames the card and the whole strip, unless they are in view already. Not on
    /// every frame, so it never fights a pan, and a strip that grows while you browse does not move the view.
    pub(crate) fn frame_new_strip(&mut self) {
        let tab = self.trail_strip_tab();
        if tab == self.camera.strip_tab {
            return;
        }
        self.camera.strip_tab = tab;
        let Some(card) = tab.and_then(|tab| self.card_rect(tab)) else { return };
        let strip = self.level_items().into_iter().filter(|e| e.origin == Origin::Virtual("trail"));
        let view = strip.fold(None, |acc: Option<egui::Rect>, e| Some(acc.map_or(e.rect, |r| r.union(e.rect))));
        if let Some(strip) = view {
            self.camera.pan_to(card.union(strip));
        }
    }
}

impl App {
    /// The strip page menu (`menus.trail_menu`), at the pointer. Closes on a choice, a click elsewhere, Escape, or when
    /// its page is gone or nothing hosts it: the strip, or an open Back/Forward branch popup (Tab view has no strip).
    pub(crate) fn build_trail_menu(&mut self, ui: &mut egui::Ui) {
        let Some((id, pos)) = self.menus.trail_menu else { return };
        let frame = ui.ctx().cumulative_frame_nr();
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("trail_menu_frame"), frame));
        let hosted = self.trail_strip_tab().is_some()
            || matches!(
                self.menus.open_popup,
                Some(crate::popups::AnchoredPopup::BackBranches | crate::popups::AnchoredPopup::ForwardBranches)
            );
        let page = live_page(self, id);
        let Some((tab, page)) = page.filter(|_| hosted) else {
            self.menus.trail_menu = None;
            return;
        };
        let mut chosen = false;
        let area = egui::Area::new(egui::Id::new("trail_page_menu")).fixed_pos(pos);
        let area = menu_popup(ui.ctx(), area, 1.0, |ui| chosen = page_menu_items(ui));
        if chosen {
            self.open_branch_as_card(tab, page, None);
        }
        // The right-click that opened the menu is, to a menu not yet laid out, a click elsewhere.
        let opening_click = ui.input(|i| i.pointer.interact_pos()) == Some(pos);
        let elsewhere = area.clicked_elsewhere() && !opening_click;
        if chosen || elsewhere || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.menus.trail_menu = None;
        }
    }
}

/// Tab `tab`'s page cards, their bounding box's top-left at `origin`: the strip hangs it under the tab's card, the Back
/// and Forward popups use their own origin. Empty when the tab has no trail.
pub(crate) fn trail_cards(app: &App, tab: NodeId, origin: egui::Pos2) -> Vec<CardEntry> {
    let Some(history) = app.nav_history.get(&tab) else { return Vec::new() };
    let pages = history.tree();
    let kind = CardKind { ty: TYPE_TRAIL_PAGE, tags: crate::caps::TagSet::EMPTY };
    pages
        .iter()
        .zip(page_rects(&pages, &page_sizes(&app.egui_ctx, &pages, PAGE), COLUMN_GAP, ROW_GAP))
        .map(|(page, rect)| CardEntry {
            id: page_card(history, page.serial),
            kind,
            origin: Origin::Virtual(SOURCE),
            rect: rect.translate(origin.to_vec2()),
        })
        .collect()
}

/// The edges between `tab`'s pages (`rect_of` finds a card's rect by id); those on the path to the cursor are
/// highlighted.
pub(crate) fn trail_edges(app: &App, tab: NodeId, rect_of: &dyn Fn(CardId) -> Option<egui::Rect>) -> Vec<LevelEdge> {
    let Some(history) = app.nav_history.get(&tab) else { return Vec::new() };
    let pages = history.tree();
    pages
        .iter()
        .filter_map(|page| {
            let parent = &pages[page.parent?];
            let (from_id, to_id) = (page_card(history, parent.serial), page_card(history, page.serial));
            Some(LevelEdge {
                from: rect_of(from_id)?.right_center(),
                to: rect_of(to_id)?.left_center(),
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
        let Some(card) = app.card_rect(tab) else { return Vec::new() };
        trail_cards(app, tab, card.left_bottom() + egui::vec2(0.0, STRIP_GAP))
    }

    fn edges(&self, app: &App, rect_of: &dyn Fn(CardId) -> Option<egui::Rect>) -> Vec<LevelEdge> {
        app.trail_strip_tab().map_or_else(Vec::new, |tab| trail_edges(app, tab, rect_of))
    }

    fn paint_body(&self, app: &App, ui: &mut egui::Ui, pass: &mut CardPass, entry: &CardEntry) -> CardBody {
        let page = live_page(app, entry.id).and_then(|(tab, i)| app.nav_history.get(&tab)?.tree().into_iter().nth(i));
        if let Some(page) = &page {
            let full = app.graph.detail.content();
            paint_trail_page(ui.painter(), pass, entry.rect, page, ui.rect_contains_pointer(entry.rect), full);
            // The icon needs only its own inset and width; the label needs the card-label minimum.
            let icon_fits = entry.rect.width() * pass.zoom_scale >= 2.0 * LABEL_INSET + ICON_SIZE_SMALL;
            if let (true, Some((tab, _))) = (full && icon_fits, live_page(app, entry.id)) {
                paint_page_icon(app, ui, pass, entry.rect, tab, &page.url);
            }
        }
        CardBody { interact_rect: entry.rect, tooltip: page.map(|p| p.url) }
    }

    fn on_click(&self, app: &mut App, entry: &CardEntry) -> Option<GraphAction> {
        let (tab, page) = live_page(app, entry.id)?;
        app.trail_pick(tab, page);
        None
    }

    fn on_menu(&self, app: &mut App, entry: &CardEntry, at: egui::Pos2) {
        app.menus.trail_menu = Some((entry.id, at));
    }

    /// Every release (on a card, empty canvas, or the back button's `Eject`) opens the page as a card at `at`, from the
    /// tab whose trail it came from (spec decision 5), so the target is ignored.
    fn on_drop(
        &self,
        app: &mut App,
        entry: &CardEntry,
        _target: Option<(DropEffect, Option<CardId>)>,
        at: egui::Pos2,
    ) -> Option<GraphAction> {
        let (tab, page) = live_page(app, entry.id)?;
        app.open_branch_as_card(tab, page, Some(at.to_vec2()));
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card_id::testutil::node;
    use crate::nav_history::NavHistory;
    use crate::virtual_source::fake::Rig;

    /// /a -> /b -> /c and /a -> /d, cursor on /d, on the rig's tab, which is focused.
    fn forked() -> Rig {
        let mut rig = Rig::new();
        let tab = rig.tab();
        rig.app.nav_history.insert(
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
        let history = &rig.app.nav_history[&tab];
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
        rig.app.nav_history.insert(tab, NavHistory::visited(&["/a", "/b"]));
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
        rig.app.nav_history.insert(tab, NavHistory::with_tree(&[("/a", None, None)], 0));
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
        let page = TrailPage {
            serial: 0,
            parent: None,
            label: "Title".into(),
            url: String::new(),
            cursor: false,
            on_path: false,
        };
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
        assert!(rig.app.nav_history[&tab].move_pending(now), "the click started a move");
        // A second click on another page while that move is pending is ignored.
        let c = rig.screen(page_id(&rig, tab, 2)).center();
        rig.click(c);
        assert!(rig.app.nav_history[&tab].move_pending(now), "still the first move");
    }

    #[test]
    fn clicking_the_cursor_page_or_an_evicted_one_does_nothing() {
        let mut rig = forked();
        let tab = rig.tab();
        let d = rig.screen(page_id(&rig, tab, 3)).center();
        rig.click(d);
        assert!(!rig.app.nav_history[&tab].move_pending(std::time::Instant::now()), "the cursor page");
        rig.app.trail_pick(tab, 99);
        assert!(!rig.app.nav_history[&tab].move_pending(std::time::Instant::now()), "an index past the end");
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
        assert_eq!(rig.app.nav_history[&tab].export()["entries"], 4, "the trail is unchanged");
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
        rig.app.nav_history.insert(tab, h);
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
        let unknown = page_card(&rig.app.nav_history[&tab], 99);
        rig.app.menus.trail_menu = Some((unknown, egui::pos2(10.0, 10.0)));
        rig.frame(vec![], egui::Modifiers::NONE);
        assert!(rig.app.menus.trail_menu.is_none(), "a serial no page has names no page");
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
    fn a_page_evicted_at_the_cap_retires_only_its_own_id() {
        let mut rig = forked();
        let tab = rig.tab();
        let urls: Vec<String> = (0..50).map(|i| format!("/p{i}")).collect();
        let refs: Vec<&str> = urls.iter().map(String::as_str).collect();
        rig.app.nav_history.insert(tab, NavHistory::visited(&refs));
        rig.frame(vec![], egui::Modifiers::NONE);
        let before: Vec<CardId> = pages(&rig).into_iter().map(|c| c.id).collect();
        assert_eq!(before.len(), 50);
        let h = rig.app.nav_history.get_mut(&tab).unwrap();
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
        let label = rig.app.nav_history[&tab].tree()[0].label.clone();
        let (mut overlapped, mut label_heights) = (false, Vec::new());
        for zoom in [0.8_f32, 1.0, 2.0] {
            rig.app.graph.pending_zoom = Some(zoom);
            let mut shapes = Vec::new();
            for _ in 0..4 {
                shapes = frame_shapes(&mut rig);
                rig.app.camera.tween = None;
            }
            let strip: Vec<egui::Rect> =
                pages(&rig).iter().map(|p| rig.app.graph_world_rect_to_screen(p.rect)).collect();
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
                    rig.app.insert_placeholder_node(
                        id,
                        at,
                        crate::store::model::Kind::Image,
                        crate::Runtime::Image(image),
                    );
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
        let label = rig.app.nav_history[&rig.tab()].tree()[0].label.clone();
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
}
