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
/// Between the tab card's bottom edge and the root page.
const STRIP_GAP: f32 = 24.0;

const ID_PREFIX: &str = "virtual:trail:";

/// `virtual:trail:<tab>:<entry>` -> (tab, entry).
fn page_ids(id: &str) -> Option<(&str, usize)> {
    let (tab, entry) = id.strip_prefix(ID_PREFIX)?.rsplit_once(':')?;
    Some((tab, entry.parse().ok()?))
}

/// Every page's rect relative to the root page's top-left, from the shared tree layout.
pub(crate) fn page_rects(pages: &[TrailPage], cell: egui::Vec2, column_gap: f32, row_gap: f32) -> Vec<egui::Rect> {
    let nodes: Vec<TreeNode> = pages.iter().map(|p| TreeNode { parent: p.parent, size: cell }).collect();
    tree_layout(&nodes, column_gap, row_gap).into_iter().map(|min| egui::Rect::from_min_size(min, cell)).collect()
}

/// One page: the frame (accent outline for the cursor), the label clipped to the page, dimmed off the path.
pub(crate) fn paint_trail_page(painter: &egui::Painter, rect: egui::Rect, page: &TrailPage, hovered: bool) {
    painter.rect_filled(rect, 6.0, if hovered { SURFACE_HOVER } else { SURFACE_RAISED });
    if page.cursor {
        painter.rect_stroke(rect, 6.0, egui::Stroke::new(1.5, ACCENT), egui::StrokeKind::Inside);
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
    fn page_id(tab: &str, entry: usize) -> String {
        format!("{ID_PREFIX}{tab}:{entry}")
    }
}

impl VirtualSource for TrailSource {
    fn name(&self) -> &'static str {
        "trail"
    }

    fn entries(&self, app: &App) -> Vec<CardEntry> {
        let Some(tab) = app.trail_strip_tab() else { return Vec::new() };
        let (Some(history), Some(card)) = (app.nav_history.get(&tab), app.card_rect(&tab)) else { return Vec::new() };
        let pages = history.tree();
        let origin = card.left_bottom() + egui::vec2(0.0, STRIP_GAP);
        let kind = CardKind { ty: TYPE_TRAIL_PAGE, tags: crate::caps::TagSet::EMPTY };
        pages
            .iter()
            .zip(page_rects(&pages, PAGE, COLUMN_GAP, ROW_GAP))
            .map(|(page, rect)| CardEntry {
                id: Self::page_id(&tab, page.entry),
                kind,
                origin: Origin::Virtual("trail"),
                rect: rect.translate(origin.to_vec2()),
            })
            .collect()
    }

    fn edges(&self, app: &App, rect_of: &dyn Fn(&str) -> Option<egui::Rect>) -> Vec<LevelEdge> {
        let Some(tab) = app.trail_strip_tab() else { return Vec::new() };
        let Some(history) = app.nav_history.get(&tab) else { return Vec::new() };
        let pages = history.tree();
        pages
            .iter()
            .filter_map(|page| {
                let parent = &pages[page.parent?];
                let (from_id, to_id) = (Self::page_id(&tab, parent.entry), Self::page_id(&tab, page.entry));
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
        let page = page_ids(&entry.id).and_then(|(tab, i)| app.nav_history.get(tab)?.tree().into_iter().nth(i));
        if let Some(page) = page {
            paint_trail_page(ui.painter(), entry.rect, &page, false);
        }
        CardBody { interact_rect: entry.rect, tooltip: None }
    }

    fn on_click(&self, _app: &mut App, _entry: &CardEntry) -> Option<GraphAction> {
        None // Task 4
    }

    fn on_menu(&self, _app: &mut App, _entry: &CardEntry, _at: egui::Pos2) {} // never called: the kind has no `menu`

    fn on_drop(
        &self,
        _app: &mut App,
        _entry: &CardEntry,
        _target: Option<(DropEffect, Option<String>)>,
        _at: egui::Pos2,
    ) -> Option<GraphAction> {
        None // Task 4
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
        assert_eq!(cards[0].id, format!("virtual:trail:{tab}:0"));
        let below = rig.app.card_rect(&tab).unwrap().left_bottom() + egui::vec2(0.0, STRIP_GAP);
        assert_eq!(cards[0].rect.min, below, "the root page hangs under the tab card");
        assert_eq!(cards[0].rect.size(), PAGE);
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
        let id = |i: usize| format!("virtual:trail:{tab}:{i}");
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
            paint_trail_page(&painter, rect, &page, false);
        });
        output.textures_delta.clear();
    }
}
