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
    Node(CardKind),
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
    /// `link_gestures::paint_bow`'s sideways bow between facing card sides, for every edge between stored nodes
    /// (`from` and `to` are the side midpoints). `EdgeKind` tells a link (dashed) from a `parent` link (solid).
    Bow,
    /// Leaves and enters horizontally: a JSON level's row-to-card links.
    Horizontal,
}

/// Which edge source a `LevelEdge` came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EdgeKind {
    /// A `parent` link (or a JSON level's row-to-card link): drawn solid.
    Opened,
    /// A Link node: drawn dashed, and never a layout spring.
    Link,
}

/// One edge on the current level, in world space.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LevelEdge {
    pub(crate) from: egui::Pos2,
    pub(crate) to: egui::Pos2,
    /// The cards at each end (a JSON level's card ids there).
    pub(crate) from_id: String,
    pub(crate) to_id: String,
    pub(crate) curve: EdgeCurve,
    pub(crate) highlight: bool,
    pub(crate) kind: EdgeKind,
    /// The Link node this line draws. `None` for an opened-from line and every JSON-level line, which no click takes.
    pub(crate) link: Option<String>,
}

/// One walk of `node_order` at the level being viewed (`App::level_scan`).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct LevelScan {
    /// The stored nodes by draw layer (`Caps::draw_layer`), each layer in `node_order`: the order
    /// `build_graph_ui`'s card loop draws them, a later layer on top.
    pub(crate) layers: [Vec<String>; crate::caps::DRAW_LAYERS],
    /// The container cards (`is_container`) among them, in `node_order`: what the toolbar's group chip, Ungroup
    /// All and a drop's first search act on.
    pub(crate) groups: Vec<String>,
}

impl App {
    /// The current level's stored nodes, bucketed in one pass over `node_order` (stable insertion order, unlike
    /// `self.nodes`, a `HashMap`). The kind check matters even at the top level: a top-level group's own `group` is
    /// `None`, same as `current_group` there, so a level check alone could not tell the layers apart.
    pub(crate) fn level_scan(&self) -> LevelScan {
        let mut scan = LevelScan::default();
        for id in &self.node_order {
            if !self.in_level(id) {
                continue;
            }
            let Some(kind) = self.card_kind(id) else { continue };
            // Every builtin row's layer fits (`caps::rows` tests); a row past the top draws on top.
            let layer = usize::from(kind.caps().draw_layer).min(crate::caps::DRAW_LAYERS - 1);
            scan.layers[layer].push(id.clone());
            if is_container(kind.ty) {
                scan.groups.push(id.clone());
            }
        }
        scan
    }

    /// The current level's items, bottom-first in draw order: stored
    /// nodes layer by layer (`level_scan`), and each layer in
    /// `node_order`.
    pub(crate) fn level_items(&self) -> Vec<LevelItem> {
        self.level_items_from(&self.level_scan().layers)
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
                    kind: LevelItemKind::Node(self.card_kind(id)?),
                    rect: node_card_rect(self.layout_pos(id), &node.runtime),
                    id: id.clone(),
                })
            })
            .collect()
    }

    /// The current level's edges: each stored node's `parent` link, then each Link node, whose two ends are both on
    /// this level. Between stored nodes an edge runs between the facing side midpoints of the two cards
    /// (`link_gestures::facing_sides`), not their centres. Link lines come in `link_ids` order, so the last of two on one pair is the greater id
    /// (`nearest_link`'s tie rule).
    pub(crate) fn level_edges(&self) -> Vec<LevelEdge> {
        // A JSON level: from the parent's row port (its right edge, at the
        // row's middle) to the child's header.
        if let Some(level) = &self.json_level {
            return level
                .cards
                .iter()
                .zip(&level.rects)
                .enumerate()
                .filter_map(|(i, (card, rect))| {
                    let (parent, row) = card.parent?;
                    let from_rect = level.rects[parent];
                    let row_y = from_rect.top() + json_view::HEADER_HEIGHT + json_view::ROW_HEIGHT * (row as f32 + 0.5);
                    Some(LevelEdge {
                        from: egui::pos2(from_rect.right(), row_y),
                        to: egui::pos2(rect.left(), rect.top() + json_view::HEADER_HEIGHT / 2.0),
                        from_id: level.card_id(parent),
                        to_id: level.card_id(i),
                        curve: EdgeCurve::Horizontal,
                        highlight: level.selected.as_deref() == Some(card.pointer.as_str()),
                        kind: EdgeKind::Opened,
                        link: None,
                    })
                })
                .collect();
        }
        let rect_of = |id: &str| node_card_rect(self.layout_pos(id), &self.nodes[id].runtime);
        let opened = self.nodes.iter().filter(|(id, _)| self.in_level(id)).filter_map(|(id, _)| {
            let parent_id = self.mirror.parent(id)?;
            self.nodes.get(parent_id).filter(|_| self.in_level(parent_id))?;
            let (from, to) = crate::link_gestures::facing_ends(rect_of(parent_id), rect_of(id));
            Some(LevelEdge {
                from,
                to,
                from_id: parent_id.to_string(),
                to_id: id.clone(),
                curve: EdgeCurve::Bow,
                highlight: false,
                kind: EdgeKind::Opened,
                link: None,
            })
        });
        // A link shows only while both of its ends are live cards at this level.
        let links = self.mirror.link_ids().filter_map(|link| {
            let (from, to) = self.mirror.link_ends(&link.to_string());
            let (from, to) = (from?.to_string(), to?.to_string());
            let both_here = [&from, &to].iter().all(|end| self.nodes.contains_key(*end) && self.in_level(end));
            if !both_here {
                return None;
            }
            let (from_end, to_end) = crate::link_gestures::facing_ends(rect_of(&from), rect_of(&to));
            Some(LevelEdge {
                from: from_end,
                to: to_end,
                from_id: from,
                to_id: to,
                curve: EdgeCurve::Bow,
                highlight: false,
                kind: EdgeKind::Link,
                link: Some(link.to_string()),
            })
        });
        opened.chain(links).collect()
    }

    /// Lays out the current level with its source's own strategy: the
    /// force settle for stored nodes, the tree layout on a JSON level
    /// (where the settle never runs).
    pub(crate) fn layout_current_level(&mut self) {
        if let Some(level) = self.json_level.as_mut() {
            if self.nodes.get(&level.node_id).and_then(|n| n.as_json()).is_some() {
                level.relayout(self.mirror.view(&level.node_id).unwrap_or(&crate::json_view::NO_VIEW));
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
            return self.group_of(&self.current_group);
        }
        self.current_group.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::model::Kind;

    fn download(path: &str) -> Runtime {
        Runtime::Download(DownloadPayload {
            path: path.to_string(),
            bytes_received: 0,
            total_bytes: None,
            completed: true,
            failed: false,
        })
    }

    #[test]
    fn level_items_list_the_current_level_bottom_first_in_draw_order() {
        let mut app = App::default();
        app.place_group(Some("g".into()), "G", crate::fetched::Placement { group: None, pos: egui::Vec2::ZERO });
        app.insert_placeholder_node("d".into(), egui::vec2(300.0, 0.0), Kind::Download, download("a.bin"));
        app.insert_placeholder_node("inner".into(), egui::vec2(0.0, 0.0), Kind::Download, download("b.bin"));
        app.edit_group("inner", Some("g".into()));

        let top: Vec<(String, LevelItemKind)> = app.level_items().into_iter().map(|i| (i.id, i.kind)).collect();
        // Downloads draw before groups (`Caps::draw_layer`), whatever the insertion order.
        use crate::card_kind::fixtures::plain;
        use crate::store::model::{TYPE_DOWNLOAD, TYPE_GROUP};
        assert_eq!(
            top,
            [
                ("d".to_string(), LevelItemKind::Node(plain(TYPE_DOWNLOAD))),
                ("g".to_string(), LevelItemKind::Node(plain(TYPE_GROUP))),
            ]
        );
        assert_eq!(app.level_items()[0].rect, node_card_rect(app.layout_pos("d"), &app.nodes["d"].runtime));
        assert_eq!(app.level_scan().groups, ["g"]);

        app.current_group = Some("g".into());
        let inside: Vec<String> = app.level_items().into_iter().map(|i| i.id).collect();
        assert_eq!(inside, ["inner"]);
        assert!(app.level_scan().groups.is_empty());
    }

    /// The one-pass scan buckets as the per-layer filters did: each layer holds exactly the level's nodes of that
    /// layer, in `node_order`, and `groups` exactly its containers.
    #[test]
    fn level_scan_buckets_every_node_on_the_level_by_layer_in_node_order() {
        let mut app = App::default();
        app.place_group(Some("g".into()), "G", crate::fetched::Placement { group: None, pos: egui::Vec2::ZERO });
        app.insert_placeholder_node("d1".into(), egui::Vec2::ZERO, Kind::Download, download("a.bin"));
        app.new_note_shortcut();
        app.insert_placeholder_node("d2".into(), egui::Vec2::ZERO, Kind::Download, download("b.bin"));
        app.insert_placeholder_node("away".into(), egui::Vec2::ZERO, Kind::Download, download("c.bin"));
        app.edit_group("away", Some("g".into()));
        let scan = app.level_scan();
        let by_filter = |pick: &dyn Fn(CardKind) -> bool| -> Vec<String> {
            app.node_order
                .iter()
                .filter(|id| app.in_level(id) && app.card_kind(id).is_some_and(pick))
                .cloned()
                .collect()
        };
        for (layer, ids) in scan.layers.iter().enumerate() {
            assert_eq!(*ids, by_filter(&|kind| usize::from(kind.caps().draw_layer) == layer), "layer {layer}");
        }
        assert_eq!(scan.groups, by_filter(&|kind| is_container(kind.ty)));
        assert_eq!(scan.layers.concat().len(), 4, "the top level's nodes, once each");
        assert_eq!(scan.layers[2], ["d1", "d2"]);
    }

    #[test]
    fn level_items_from_a_precomputed_scan_match_a_fresh_one() {
        let mut app = App::default();
        for _ in 0..3 {
            app.new_note_shortcut();
        }
        assert_eq!(app.level_items_from(&app.level_scan().layers), app.level_items());
    }

    #[test]
    fn level_edges_join_parent_and_child_facing_sides_on_this_level_only() {
        let mut app = App::default();
        app.insert_placeholder_node("p".into(), egui::vec2(0.0, 0.0), Kind::Download, download("p.bin"));
        app.insert_placeholder_node("c".into(), egui::vec2(100.0, 50.0), Kind::Download, download("c.bin"));
        app.insert_placeholder_node("elsewhere".into(), egui::vec2(9.0, 9.0), Kind::Download, download("e.bin"));
        app.edit_parent("c", Some("p".into()));
        app.edit_parent("elsewhere", Some("p".into()));
        app.edit_group("elsewhere", Some("other".into()));

        let edges = app.level_edges();
        assert_eq!(edges.len(), 1);
        let rect = |id: &str| node_card_rect(app.layout_pos(id), &app.nodes[id].runtime);
        let (p, c) = (rect("p"), rect("c"));
        assert_eq!((edges[0].from, edges[0].to), crate::link_gestures::facing_ends(p, c));
        assert_eq!(edges[0].from, p.right_center(), "c lies mostly to the right of p: p's right side");
        assert_eq!(edges[0].to, c.left_center(), "and c's left side");
        assert_eq!(edges[0].curve, EdgeCurve::Bow);
        assert_eq!(
            (edges[0].from_id.as_str(), edges[0].to_id.as_str(), edges[0].link.as_deref()),
            ("p", "c", None),
            "an opened-from line names its ends and no Link node"
        );
    }
}
