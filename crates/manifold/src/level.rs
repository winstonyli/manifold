//! What a graph level contains, whatever supplies it (spec
//! `2026-09-27-json-node-rework-design.md`, "Level sources"). Shared
//! graph chrome (fit and Reset View, the minimap, the LOD visible count,
//! the settle key, the right-click hit test, `EXPORTSTATE`'s `cards`, the
//! edges and the layout) reads the level through here instead of walking
//! `self.nodes` and `current_group` itself.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LevelItemKind {
    /// A stored node's card.
    Node(ItemNodeKind),
    /// A JSON level's card (`json_level.rs`).
    JsonCard,
}

/// One card on the current level, as the shared chrome sees it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LevelItem {
    pub(crate) id: String,
    pub(crate) kind: LevelItemKind,
    /// World space.
    pub(crate) rect: egui::Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EdgeCurve {
    /// `draw_graph_edge`'s sideways bow, for stored nodes' `parent` links.
    Bow,
    /// Leaves and enters horizontally: a JSON level's row-to-card links.
    Horizontal,
}

/// One edge on the current level, in world space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LevelEdge {
    pub(crate) from: egui::Pos2,
    pub(crate) to: egui::Pos2,
    pub(crate) curve: EdgeCurve,
    pub(crate) highlight: bool,
}

impl App {
    /// The current level's items, bottom-first in draw order: stored
    /// nodes kind by kind in `GRAPH_DRAW_ORDER`, and each kind in
    /// `node_order`.
    pub(crate) fn level_items(&self) -> Vec<LevelItem> {
        self.level_items_from(&GRAPH_DRAW_ORDER.map(|kind| self.nodes_at_current_level(kind)))
    }

    /// `level_items` from an `at_level` scan already made this frame
    /// (`build_graph_ui`), so the level isn't walked twice.
    pub(crate) fn level_items_from(&self, at_level: &[Vec<String>]) -> Vec<LevelItem> {
        if let Some(level) = &self.json_level {
            return (0..level.cards.len())
                .map(|i| LevelItem { id: level.card_id(i), kind: LevelItemKind::JsonCard, rect: level.rects[i] })
                .collect();
        }
        at_level
            .iter()
            .flatten()
            .filter_map(|id| {
                let node = self.nodes.get(id)?;
                Some(LevelItem {
                    kind: LevelItemKind::Node(item_node_kind(&node.payload)),
                    rect: node_card_rect(node),
                    id: id.clone(),
                })
            })
            .collect()
    }

    /// The current level's edges: each stored node's `parent` link whose
    /// two ends are both on this level.
    pub(crate) fn level_edges(&self) -> Vec<LevelEdge> {
        // A JSON level: from the parent's row port (its right edge, at the
        // row's middle) to the child's header.
        if let Some(level) = &self.json_level {
            return level
                .cards
                .iter()
                .zip(&level.rects)
                .filter_map(|(card, rect)| {
                    let (parent, row) = card.parent?;
                    let from_rect = level.rects[parent];
                    let row_y = from_rect.top() + json_view::HEADER_HEIGHT + json_view::ROW_HEIGHT * (row as f32 + 0.5);
                    Some(LevelEdge {
                        from: egui::pos2(from_rect.right(), row_y),
                        to: egui::pos2(rect.left(), rect.top() + json_view::HEADER_HEIGHT / 2.0),
                        curve: EdgeCurve::Horizontal,
                        highlight: level.selected.as_deref() == Some(card.pointer.as_str()),
                    })
                })
                .collect();
        }
        self.nodes
            .values()
            .filter(|node| node.group == self.current_group)
            .filter_map(|node| {
                let parent = self.nodes.get(node.parent.as_ref()?).filter(|p| p.group == self.current_group)?;
                Some(LevelEdge {
                    from: parent.pos.to_pos2(),
                    to: node.pos.to_pos2(),
                    curve: EdgeCurve::Bow,
                    highlight: false,
                })
            })
            .collect()
    }

    /// Lays out the current level with its source's own strategy: the
    /// force settle for stored nodes, the tree layout on a JSON level
    /// (where the settle never runs).
    pub(crate) fn layout_current_level(&mut self) {
        if let Some(level) = self.json_level.as_mut() {
            if let Some(json) = self.nodes.get(&level.node_id).and_then(|n| n.as_json()) {
                level.relayout(&json.view);
            }
            return;
        }
        self.settle_current_level();
    }

    /// Where a new node goes (spec "Level sources"): the level being
    /// viewed, except on a JSON level, where it's the Json node's own
    /// group. Based on `current_group`, not `json_level`, so it is right
    /// before the first frame builds the level (plan ruling 3).
    pub(crate) fn insertion_group(&self) -> Option<String> {
        if self.current_json_node().is_some() {
            return self.current_group.as_ref().and_then(|id| self.nodes.get(id)).and_then(|n| n.group.clone());
        }
        self.current_group.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn download(path: &str) -> NodePayload {
        NodePayload::Download(DownloadPayload {
            path: path.to_string(),
            url: None,
            bytes_received: 0,
            total_bytes: None,
            completed: true,
            failed: false,
        })
    }

    fn group(name: &str) -> NodePayload {
        NodePayload::Group(GroupPayload::named(name.to_string()))
    }

    #[test]
    fn level_items_list_the_current_level_bottom_first_in_draw_order() {
        let mut app = App::default();
        app.insert_placeholder_node("g".into(), egui::vec2(0.0, 0.0), group("G"));
        app.insert_placeholder_node("d".into(), egui::vec2(300.0, 0.0), download("a.bin"));
        app.insert_placeholder_node("inner".into(), egui::vec2(0.0, 0.0), download("b.bin"));
        app.nodes.get_mut("inner").unwrap().group = Some("g".into());

        let top: Vec<(String, LevelItemKind)> = app.level_items().into_iter().map(|i| (i.id, i.kind)).collect();
        // Downloads draw before groups (`GRAPH_DRAW_ORDER`), whatever the insertion order.
        assert_eq!(
            top,
            [
                ("d".to_string(), LevelItemKind::Node(ItemNodeKind::Download)),
                ("g".to_string(), LevelItemKind::Node(ItemNodeKind::Group)),
            ]
        );
        assert_eq!(app.level_items()[0].rect, node_card_rect(&app.nodes["d"]));

        app.current_group = Some("g".into());
        let inside: Vec<String> = app.level_items().into_iter().map(|i| i.id).collect();
        assert_eq!(inside, ["inner"]);
    }

    #[test]
    fn level_items_from_a_precomputed_scan_match_a_fresh_one() {
        let mut app = App::default();
        for _ in 0..3 {
            app.new_note_shortcut();
        }
        let at_level = GRAPH_DRAW_ORDER.map(|kind| app.nodes_at_current_level(kind));
        assert_eq!(app.level_items_from(&at_level), app.level_items());
    }

    #[test]
    fn level_edges_join_parent_and_child_centers_on_this_level_only() {
        let mut app = App::default();
        app.insert_placeholder_node("p".into(), egui::vec2(0.0, 0.0), download("p.bin"));
        app.insert_placeholder_node("c".into(), egui::vec2(100.0, 50.0), download("c.bin"));
        app.insert_placeholder_node("elsewhere".into(), egui::vec2(9.0, 9.0), download("e.bin"));
        app.nodes.get_mut("c").unwrap().parent = Some("p".into());
        app.nodes.get_mut("elsewhere").unwrap().parent = Some("p".into());
        app.nodes.get_mut("elsewhere").unwrap().group = Some("other".into());

        let edges = app.level_edges();
        assert_eq!(edges.len(), 1);
        assert_eq!((edges[0].from, edges[0].to), (egui::pos2(0.0, 0.0), egui::pos2(100.0, 50.0)));
        assert_eq!(edges[0].curve, EdgeCurve::Bow);
    }
}
