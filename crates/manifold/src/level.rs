//! What a graph level contains, whatever supplies it (spec
//! `2026-09-27-json-node-rework-design.md`, "Level sources"). Shared
//! graph chrome (fit and Reset View, the minimap, the LOD visible count,
//! the settle key, the right-click hit test, `EXPORTSTATE`'s `cards`, the
//! edges and the layout) reads the level through here instead of walking
//! `self.nodes` and `current_group` itself.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Origin {
    /// A stored node's card.
    Stored,
    /// A JSON level's card (`json_level.rs`): no node behind it.
    Json,
    /// A card a `VirtualSource` of this name supplies (`virtual_source.rs`): no node behind it.
    Virtual(&'static str),
}

/// One card on the current level, as the shared chrome sees it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CardEntry {
    pub(crate) id: CardId,
    /// What the card is: the key into the capability rows. A JSON level's card is `TYPE_JSON_LEVEL`, whose row
    /// grants no ability.
    pub(crate) kind: CardKind,
    pub(crate) origin: Origin,
    /// World space.
    pub(crate) rect: egui::Rect,
}

impl CardEntry {
    pub(crate) fn caps(&self) -> crate::caps::Caps {
        self.kind.caps()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EdgeCurve {
    /// `link_gestures::paint_bow`'s sideways bow between facing card sides, for every edge between stored nodes
    /// (`from` and `to` are the side midpoints). `EdgeKind` tells a link (dashed) from a `parent` link (solid).
    Bow,
    /// Leaves and enters horizontally: a JSON level's row-to-card links.
    Horizontal,
}

/// `EdgeCurve::Horizontal`'s curve, for the level's edges and for the trail popups' tree lines.
pub(crate) fn paint_horizontal_edge(painter: &egui::Painter, from: egui::Pos2, to: egui::Pos2, stroke: egui::Stroke) {
    let bend = ((to.x - from.x) / 2.0).max(20.0);
    painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
        [from, from + egui::vec2(bend, 0.0), to - egui::vec2(bend, 0.0), to],
        false,
        egui::Color32::TRANSPARENT,
        stroke,
    ));
}

/// A trail edge's stroke: the accent on the path to the cursor, else weak.
pub(crate) fn opened_edge_stroke(on_path: bool, inv_zoom: f32, weak: egui::Color32) -> egui::Stroke {
    egui::Stroke::new(2.0 * inv_zoom, if on_path { crate::theme::ACCENT } else { weak })
}

/// Which edge source a `LevelEdge` came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EdgeKind {
    /// A `parent` link (or a JSON level's row-to-card link): drawn solid.
    Opened,
    /// A Link node: drawn dashed, and never a layout spring.
    Link,
    /// A tab's or group's `jar` edge to a jar card on the same level: drawn dashed and faint, never clicked
    /// (`link` is `None`) and never a spring.
    Jar,
}

/// One edge on the current level, in world space.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LevelEdge {
    pub(crate) from: egui::Pos2,
    pub(crate) to: egui::Pos2,
    /// The cards at each end (a JSON level's card ids there).
    pub(crate) from_id: CardId,
    pub(crate) to_id: CardId,
    pub(crate) curve: EdgeCurve,
    pub(crate) highlight: bool,
    pub(crate) kind: EdgeKind,
    /// The Link node this line draws. `None` for an opened-from line and every JSON-level line, which no click takes.
    pub(crate) link: Option<String>,
}

/// How many of `items` count toward the level's settle key: every card but a virtual one. A source's entry count
/// can change with each navigation, and that must not re-settle (move) the stored cards. The one count both the
/// per-frame check and `App::current_level_settle_key` use.
pub(crate) fn settle_count(items: &[CardEntry]) -> usize {
    items.iter().filter(|item| !matches!(item.origin, Origin::Virtual(_))).count()
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
    pub(crate) fn level_items(&self) -> Vec<CardEntry> {
        self.level_items_from(&self.level_scan().layers)
    }

    /// `level_items` from an `at_level` scan already made this frame
    /// (`build_graph_ui`), so the level isn't walked twice. The stored (or JSON) cards come first, then each
    /// registered `VirtualSource`'s, so a virtual card draws on top. A JSON level lists its own cards only: its
    /// frame is `draw_json_level`, which draws no virtual card, so none may be hit-tested, selected or counted there.
    pub(crate) fn level_items_from(&self, at_level: &[Vec<String>]) -> Vec<CardEntry> {
        if let Some(level) = &self.json_level {
            return (0..level.cards.len())
                .map(|i| CardEntry {
                    id: level.card_id(i),
                    kind: CardKind { ty: crate::store::model::TYPE_JSON_LEVEL, tags: crate::caps::TagSet::EMPTY },
                    origin: Origin::Json,
                    rect: level.rects[i],
                })
                .collect();
        }
        let mut entries: Vec<CardEntry> = at_level
            .iter()
            .flatten()
            .filter_map(|id| {
                let node = self.nodes.get(id)?;
                Some(CardEntry {
                    kind: self.card_kind(id)?,
                    origin: Origin::Stored,
                    rect: node_card_rect(self.layout_pos(id), &node.runtime),
                    id: CardId::stored(id),
                })
            })
            .collect();
        entries.extend(self.virtual_sources.iter().flat_map(|source| source.entries(self)));
        entries
    }

    /// The current level's edges: each stored node's `parent` link, then each Link node, whose two ends are both on
    /// this level, then each `jar` edge whose two ends are. Between stored nodes an edge runs between the facing side midpoints of the two cards
    /// (`link_gestures::facing_sides`), not their centres. Link lines come in `link_ids` order, so the last of two on one pair is the greater id
    /// (`nearest_link`'s tie rule).
    pub(crate) fn level_edges(&self) -> Vec<LevelEdge> {
        self.level_edges_from(&self.level_items())
    }

    /// `level_edges` with the level's `items` already in hand (`level_items_from`), so the caller that has them does
    /// not scan the level again; a JSON level draws from its own rects and ignores them.
    pub(crate) fn level_edges_from(&self, items: &[CardEntry]) -> Vec<LevelEdge> {
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
        let rects: HashMap<CardId, egui::Rect> = items.iter().map(|e| (e.id, e.rect)).collect();
        let rect_of = |id: CardId| rects.get(&id).copied();
        let opened = self.nodes.iter().filter(|(id, _)| self.in_level(id)).filter_map(|(id, _)| {
            let parent_id = self.opened_from(id)?;
            self.nodes.get(parent_id).filter(|_| self.in_level(parent_id))?;
            let (from, to) =
                crate::link_gestures::facing_ends(rect_of(CardId::stored(parent_id))?, rect_of(CardId::stored(id))?);
            Some(LevelEdge {
                from,
                to,
                from_id: CardId::stored(parent_id),
                to_id: CardId::stored(id),
                curve: EdgeCurve::Bow,
                highlight: false,
                kind: EdgeKind::Opened,
                link: None,
            })
        });
        // A link shows only while both of its ends are live cards at this level.
        let links = self.doc.mirror.link_ids().filter_map(|link| {
            let (from, to) = self.doc.mirror.link_ends(&link.to_string());
            let (from, to) = (from?.to_string(), to?.to_string());
            let both_here = [&from, &to].iter().all(|end| self.nodes.contains_key(*end) && self.in_level(end));
            if !both_here {
                return None;
            }
            let (from_end, to_end) =
                crate::link_gestures::facing_ends(rect_of(CardId::stored(&from))?, rect_of(CardId::stored(&to))?);
            Some(LevelEdge {
                from: from_end,
                to: to_end,
                from_id: CardId::stored(&from),
                to_id: CardId::stored(&to),
                curve: EdgeCurve::Bow,
                highlight: false,
                kind: EdgeKind::Link,
                link: Some(link.to_string()),
            })
        });
        // A jar edge shows while its source and its jar are both live cards at this level. One reaching a jar in an
        // enclosing group, or a group's edge to a jar inside it, has no line here: its far end is on another level.
        let jars = self.nodes.keys().filter(|id| self.in_level(id)).filter_map(|id| {
            let jar = self.doc.mirror.jar(id)?.to_string();
            self.nodes.get(&jar).filter(|_| self.in_level(&jar))?;
            let (from, to) =
                crate::link_gestures::facing_ends(rect_of(CardId::stored(id))?, rect_of(CardId::stored(&jar))?);
            Some(LevelEdge {
                from,
                to,
                from_id: CardId::stored(id),
                to_id: CardId::stored(&jar),
                curve: EdgeCurve::Bow,
                highlight: false,
                kind: EdgeKind::Jar,
                link: None,
            })
        });
        let virtuals = self.virtual_sources.iter().flat_map(|source| source.edges(self, &rect_of));
        opened.chain(links).chain(jars).chain(virtuals).collect()
    }

    /// Lays out the current level with its source's own strategy: the
    /// force settle for stored nodes, the tree layout on a JSON level
    /// (where the settle never runs).
    pub(crate) fn layout_current_level(&mut self) {
        if let Some(level) = self.json_level.as_mut() {
            if self.nodes.get(&level.node_id).and_then(|n| n.as_json()).is_some() {
                level.relayout(self.doc.mirror.view(&level.node_id).unwrap_or(&crate::json_view::NO_VIEW));
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
    use crate::card_id::CardId;
    use crate::store::model::Kind;

    // Real node ids (actor 1, counters 1..): `CardId::stored` and the edges need text that parses as a `NodeId`.
    const G: &str = "0000000000000001-1";
    const D: &str = "0000000000000001-2";
    const INNER: &str = "0000000000000001-3";
    const AWAY: &str = "0000000000000001-4";
    const P: &str = "0000000000000001-5";
    const C: &str = "0000000000000001-6";
    const ELSEWHERE: &str = "0000000000000001-7";
    const D1: &str = "0000000000000001-8";
    const D2: &str = "0000000000000001-9";

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
        app.place_group(Some(G.into()), "G", crate::fetched::Placement { group: None, pos: egui::Vec2::ZERO });
        app.insert_placeholder_node(D.into(), egui::vec2(300.0, 0.0), Kind::Download, download("a.bin"));
        app.insert_placeholder_node(INNER.into(), egui::vec2(0.0, 0.0), Kind::Download, download("b.bin"));
        app.edit_group(INNER, Some(G.into()));

        let top: Vec<(CardId, CardKind)> = app.level_items().into_iter().map(|i| (i.id, i.kind)).collect();
        // Downloads draw before groups (`Caps::draw_layer`), whatever the insertion order.
        use crate::card_kind::fixtures::plain;
        use crate::store::model::{TYPE_DOWNLOAD, TYPE_GROUP};
        assert_eq!(top, [(CardId::stored(D), plain(TYPE_DOWNLOAD)), (CardId::stored(G), plain(TYPE_GROUP))]);
        assert_eq!(app.level_items()[0].rect, node_card_rect(app.layout_pos(D), &app.nodes[D].runtime));
        assert_eq!(app.level_scan().groups, [G]);

        app.current_group = Some(G.into());
        let inside: Vec<CardId> = app.level_items().into_iter().map(|i| i.id).collect();
        assert_eq!(inside, [CardId::stored(INNER)]);
        assert!(app.level_scan().groups.is_empty());
    }

    /// The one-pass scan buckets as the per-layer filters did: each layer holds exactly the level's nodes of that
    /// layer, in `node_order`, and `groups` exactly its containers.
    #[test]
    fn level_scan_buckets_every_node_on_the_level_by_layer_in_node_order() {
        let mut app = App::default();
        app.place_group(Some(G.into()), "G", crate::fetched::Placement { group: None, pos: egui::Vec2::ZERO });
        app.insert_placeholder_node(D1.into(), egui::Vec2::ZERO, Kind::Download, download("a.bin"));
        app.new_note_shortcut();
        app.insert_placeholder_node(D2.into(), egui::Vec2::ZERO, Kind::Download, download("b.bin"));
        app.insert_placeholder_node(AWAY.into(), egui::Vec2::ZERO, Kind::Download, download("c.bin"));
        app.edit_group(AWAY, Some(G.into()));
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
        assert_eq!(scan.layers[2], [D1, D2]);
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
    fn level_edges_from_reads_the_rects_it_is_given() {
        let mut app = App::default();
        app.insert_placeholder_node(P.into(), egui::vec2(0.0, 0.0), Kind::Download, download("p.bin"));
        app.insert_placeholder_node(C.into(), egui::vec2(100.0, 50.0), Kind::Download, download("c.bin"));
        app.edit_parent(C, Some(P.into()));
        assert_eq!(app.level_edges_from(&app.level_items()), app.level_edges());
        let mut items = app.level_items();
        let shift = egui::vec2(500.0, 0.0);
        items.iter_mut().filter(|e| e.id == CardId::stored(C)).for_each(|e| e.rect = e.rect.translate(shift));
        let moved = app.level_edges_from(&items);
        assert_eq!(moved[0].to, app.level_edges()[0].to + shift, "the edge follows the given rect");
    }

    #[test]
    fn level_edges_join_parent_and_child_facing_sides_on_this_level_only() {
        let mut app = App::default();
        app.insert_placeholder_node(P.into(), egui::vec2(0.0, 0.0), Kind::Download, download("p.bin"));
        app.insert_placeholder_node(C.into(), egui::vec2(100.0, 50.0), Kind::Download, download("c.bin"));
        app.insert_placeholder_node(ELSEWHERE.into(), egui::vec2(9.0, 9.0), Kind::Download, download("e.bin"));
        app.edit_parent(C, Some(P.into()));
        app.edit_parent(ELSEWHERE, Some(P.into()));
        app.edit_group(ELSEWHERE, Some("other".into()));

        let edges = app.level_edges();
        assert_eq!(edges.len(), 1);
        let rect = |id: &str| node_card_rect(app.layout_pos(id), &app.nodes[id].runtime);
        let (p, c) = (rect(P), rect(C));
        assert_eq!((edges[0].from, edges[0].to), crate::link_gestures::facing_ends(p, c));
        assert_eq!(edges[0].from, p.right_center(), "c lies mostly to the right of p: p's right side");
        assert_eq!(edges[0].to, c.left_center(), "and c's left side");
        assert_eq!(edges[0].curve, EdgeCurve::Bow);
        assert_eq!(
            (edges[0].from_id, edges[0].to_id, edges[0].link.as_deref()),
            (CardId::stored(P), CardId::stored(C), None),
            "an opened-from line names its ends and no Link node"
        );
    }

    #[test]
    fn a_virtual_source_adds_edges_between_its_entries_beside_the_stored_ones() {
        use crate::virtual_source::fake::{self, FakeSource};
        let mut app = App::default();
        app.insert_placeholder_node(P.into(), egui::vec2(0.0, 0.0), Kind::Download, download("p.bin"));
        app.insert_placeholder_node(C.into(), egui::vec2(100.0, 50.0), Kind::Download, download("c.bin"));
        app.edit_parent(C, Some(P.into()));
        app.register_virtual_source(std::rc::Rc::new(FakeSource::default()));
        let edges = app.level_edges();
        assert!(edges
            .iter()
            .any(|e| e.kind == EdgeKind::Opened && e.from_id == CardId::stored(P) && e.to_id == CardId::stored(C)));
        let virt: Vec<_> = edges.iter().filter(|e| e.from_id.is_virtual()).collect();
        assert_eq!(virt.len(), 1);
        assert_eq!((virt[0].from_id, virt[0].to_id), (fake::id('a'), fake::id('b')));
        assert_eq!((virt[0].from, virt[0].to), (egui::pos2(50.0, 30.0), egui::pos2(250.0, 30.0)));
    }

    #[test]
    fn a_jar_edge_draws_only_when_both_ends_are_on_this_level() {
        use crate::session_sync::tests::{add, add_jar, sleeping_tab};
        let mut app = App::default();
        let tab = add(&mut app, Kind::Tab, sleeping_tab("https://a/", "A"));
        let here = add_jar(&mut app, "Here");
        app.set_jar_edge(&tab, Some(here));
        let jar_lines = |app: &App| app.level_edges().into_iter().filter(|e| e.kind == EdgeKind::Jar).count();
        assert_eq!(jar_lines(&app), 1);
        app.edit_group(&here.to_string(), Some("elsewhere".into()));
        assert_eq!(jar_lines(&app), 0, "the jar is on another level");
    }
}
