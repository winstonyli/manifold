//! One tree layout for the JSON level and the trail strip (spec `2026-10-16-trail-strip-design.md`, decision 1): a
//! column per depth as wide as its widest node; a subtree's band is the larger of its node and its children's bands
//! plus gaps; a parent is centred on its children's span; node 0 is at the origin.

/// One node to lay out: its parent's index (`None` only for node 0) and its size.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TreeNode {
    pub(crate) parent: Option<usize>,
    pub(crate) size: egui::Vec2,
}

/// Each node's top-left. A parent's index is below its children's, so walking backwards sizes every child first.
pub(crate) fn tree_layout(nodes: &[TreeNode], column_gap: f32, sibling_gap: f32) -> Vec<egui::Pos2> {
    let n = nodes.len();
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut depth = vec![0usize; n];
    for (i, node) in nodes.iter().enumerate() {
        debug_assert_eq!(node.parent.is_none(), i == 0, "node 0 is the only root");
        if let Some(p) = node.parent {
            debug_assert!(p < i, "a parent comes before its children");
            children[p].push(i);
            depth[i] = depth[p] + 1;
        }
    }
    let span = |band: &[f32], kids: &[usize]| {
        kids.iter().map(|&k| band[k]).sum::<f32>() + sibling_gap * kids.len().saturating_sub(1) as f32
    };
    let mut band = vec![0.0; n];
    for i in (0..n).rev() {
        band[i] = nodes[i].size.y.max(span(&band, &children[i]));
    }
    let columns = depth.iter().copied().max().map_or(0, |d| d + 1);
    let mut width = vec![0.0f32; columns];
    for (node, &d) in nodes.iter().zip(&depth) {
        width[d] = width[d].max(node.size.x);
    }
    let mut left = vec![0.0f32; columns];
    for d in 1..columns {
        left[d] = left[d - 1] + width[d - 1] + column_gap;
    }
    let mut top = vec![0.0; n];
    let mut pos = vec![egui::Pos2::ZERO; n];
    for i in 0..n {
        let mut y = top[i] + (band[i] - span(&band, &children[i])) / 2.0;
        for &k in &children[i] {
            top[k] = y;
            y += band[k] + sibling_gap;
        }
        pos[i] = egui::pos2(left[depth[i]], top[i] + (band[i] - nodes[i].size.y) / 2.0);
    }
    let origin = pos.first().map_or(0.0, |p| p.y);
    for p in &mut pos {
        p.y -= origin;
    }
    pos
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(parent: Option<usize>, w: f32, h: f32) -> TreeNode {
        TreeNode { parent, size: egui::vec2(w, h) }
    }

    #[test]
    fn a_single_node_sits_at_the_origin_and_nothing_gives_nothing() {
        assert_eq!(tree_layout(&[node(None, 10.0, 4.0)], 5.0, 3.0), [egui::pos2(0.0, 0.0)]);
        assert!(tree_layout(&[], 5.0, 3.0).is_empty());
    }

    #[test]
    fn a_chain_is_one_row_with_growing_x() {
        let nodes = [node(None, 10.0, 4.0), node(Some(0), 10.0, 4.0), node(Some(1), 10.0, 4.0)];
        let pos = tree_layout(&nodes, 5.0, 3.0);
        assert_eq!(pos, [egui::pos2(0.0, 0.0), egui::pos2(15.0, 0.0), egui::pos2(30.0, 0.0)]);
    }

    #[test]
    fn a_fork_stacks_children_and_centres_the_parent() {
        let nodes = [node(None, 10.0, 4.0), node(Some(0), 10.0, 4.0), node(Some(0), 10.0, 4.0)];
        let pos = tree_layout(&nodes, 5.0, 3.0);
        // Children span 4 + 3 + 4 = 11, so the root (height 4) is centred 3.5 below their top; rebased, the root is at 0.
        assert_eq!(pos[1], egui::pos2(15.0, -3.5));
        assert_eq!(pos[2], egui::pos2(15.0, 3.5));
        assert_eq!(pos[0].y, 0.0, "the root's top is the origin");
        let span_mid = (pos[1].y + pos[2].y + 4.0) / 2.0;
        assert!((pos[0].y + 2.0 - span_mid).abs() < 1e-4, "parent centred on its children");
    }

    #[test]
    fn a_nested_fork_does_not_overlap_and_wide_nodes_widen_only_their_column() {
        // 0 -> 1 -> 3, 0 -> 2 (1 and 2 siblings; 3 under 1); node 3 is wide.
        let nodes =
            [node(None, 10.0, 4.0), node(Some(0), 10.0, 4.0), node(Some(0), 10.0, 4.0), node(Some(1), 30.0, 4.0)];
        let pos = tree_layout(&nodes, 5.0, 3.0);
        assert_eq!(pos[1].x, pos[2].x, "siblings share a column");
        assert_eq!(pos[3].x, pos[1].x + 10.0 + 5.0);
        let rect = |i: usize| egui::Rect::from_min_size(pos[i], nodes[i].size);
        for a in 0..4 {
            for b in a + 1..4 {
                assert!(!rect(a).intersects(rect(b)), "{a} and {b} overlap");
            }
        }
    }
}
