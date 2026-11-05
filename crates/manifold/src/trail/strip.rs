//! The trail strip (spec `2026-10-16-trail-strip-design.md`): the focused tab's page tree as virtual cards under the
//! tab's card, and the painter and geometry the Back/Forward popups share.

use crate::card::{label_fits, CardBody, CardPass, DropEffect};
use crate::card_id::{fnv64, CardId};
use crate::identity::{Identity, ICON_SIZE_SMALL};
use crate::level::{CardEntry, EdgeCurve, EdgeKind, LevelEdge, Origin};
use crate::popups::menu_popup;
use crate::store::model::{NodeId, TYPE_TRAIL_PAGE};
use crate::theme::{accent, palette};
use crate::trail::history::{NavHistory, TrailPage};
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
mod tests;
