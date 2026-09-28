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
    /// Leaves and enters horizontally. Unused until the JSON level (task
    /// 5) draws its own edges; `draw_graph_edges` already handles it.
    #[allow(dead_code)]
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
        GRAPH_DRAW_ORDER
            .iter()
            .flat_map(|&kind| self.nodes_at_current_level(kind))
            .filter_map(|id| {
                let node = self.nodes.get(&id)?;
                Some(LevelItem {
                    kind: LevelItemKind::Node(item_node_kind(&node.payload)),
                    rect: node_card_rect(node),
                    id,
                })
            })
            .collect()
    }

    /// The current level's edges: each stored node's `parent` link whose
    /// two ends are both on this level.
    pub(crate) fn level_edges(&self) -> Vec<LevelEdge> {
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
    /// force settle for stored nodes.
    pub(crate) fn layout_current_level(&mut self) {
        self.settle_current_level();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn download(path: &str) -> NodePayload {
        NodePayload::Download(DownloadPayload {
            path: path.to_string(),
            bytes_received: 0,
            total_bytes: None,
            completed: true,
        })
    }

    fn group(name: &str) -> NodePayload {
        NodePayload::Group(GroupPayload { name: name.to_string(), from_json: false })
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
