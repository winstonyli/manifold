//! Graph navigation: fit and bounds, settle, focus movement, group lookup, drops and merges.

use super::*;
use crate::store::model::NodeId;

/// The Graph view's pointer-driven state (`App::interaction`): drag, hover, selection and the pending new-card choice.
#[derive(Default)]
pub(crate) struct GraphInteraction {
    /// Graph view: the node currently being dragged (pinned in the physics
    /// sim — see `graph_layout::step`'s `pinned` parameter) and the node
    /// currently hovered (drives the peek-preview popup).
    pub(crate) dragging: Option<CardId>,
    /// Every node the current drag is moving (a lone card, or the whole
    /// selection when the dragged card is selected — see `take_drag_unit`
    /// and the `dragged()` branch that fills this in). Read and cleared at
    /// release (`release_drag`), where each one's `pos` is written exactly
    /// once (`edit_pos`, 2b-4 Ruling 2): writing every frame of the drag
    /// instead would let an intermediate position reach the store, since a
    /// save can fire mid-drag from an unrelated event (`poll_store`, or a
    /// url change batched through `drain_tab_events`), independent of the
    /// gesture. Until then a remote pos for any of them doesn't snap its
    /// layout (`is_being_dragged`). Cleared without a write when the drag
    /// stops being live (`end_stale_drag`).
    pub(crate) drag_moved: Vec<CardId>,
    /// Where each card of the drag released last stood in the document before `release_drag` wrote its new pos, for
    /// the drop's undo (`apply_drop` takes it; a drop with no effect discards it).
    pub(crate) drag_origin: Vec<(CardId, Option<egui::Vec2>)>,
    pub(crate) hovered: Option<CardId>,
    /// Set to `hovered` by an Escape press while a hover-peek preview
    /// is showing, so that preview can be dismissed early without needing
    /// the pointer to actually move away -- the peek drawing code skips
    /// rendering while this matches the current `hovered` id. Cleared
    /// whenever `hovered` changes to anything else (including `None`), so
    /// hovering the same or a different node again always shows a fresh
    /// peek.
    pub(crate) peek_suppressed: Option<CardId>,
    /// Graph view rectangle-select: `Some` (holding the screen-space anchor
    /// point) for exactly the duration of a right-button drag over the
    /// graph panel -- the right button specifically, since the primary
    /// button is already Scene's own pan gesture (`drag_pan_buttons` is
    /// restricted to `PRIMARY` in `build_graph_ui` so the two never fight
    /// over the same drag). `selected` is the committed result, kept
    /// until the next selection (a fresh right-drag) or an explicit clear
    /// (a plain click on empty background, or Escape).
    pub(crate) rect_select_start: Option<egui::Pos2>,
    pub(crate) selected: HashSet<CardId>,
    /// World position of the small "New Tab/Note/Group/File" card shown
    /// after a plain right-click (no real drag) on empty Graph-view
    /// background -- see the marquee-release logic below for where this
    /// gets set (same gesture, disambiguated from a real rectangle-select
    /// drag by movement distance) and `build_graph_ui`'s own rendering of
    /// it. Deliberately *not* a `GraphNode`/`self.nodes` entry -- it's a
    /// pending choice, not real graph content, so it never needs excluding
    /// from `level_scan`/session persistence/drag-into-group/
    /// filters the way a real new node kind would. Cleared on any camera
    /// movement (pan/zoom/Reset View/entering or leaving a group/minimap
    /// click), Escape, clicking elsewhere, or picking one of its own
    /// actions -- deliberately short-lived rather than a droppable/
    /// draggable placeholder (a real node's card lingers regardless of
    /// camera movement; this one is a transient choice, not content, so
    /// letting it silently drift out of sync with a moved camera would be
    /// the wrong feel for it).
    pub(crate) pending_new_node: Option<egui::Vec2>,
    /// A note just made (`insert_note_at`) whose editor takes the keyboard when it first draws, so typing starts at
    /// once, and the graph frames it has left to be drawn in: a note spawned out of view must not grab the keyboard
    /// when the camera reaches it later. Taken by the note's editor.
    pub(crate) focus_new_note: Option<(CardId, u8)>,
}

impl GraphInteraction {
    /// Drops `id`, a node being removed, from the state that names a single node. `drag_moved` and `pending_new_node`
    /// are not here: a gone id in `drag_moved` is skipped at release, and the pending card names no node.
    pub(crate) fn forget(&mut self, id: CardId) {
        if self.dragging == Some(id) {
            self.dragging = None;
        }
        if self.hovered == Some(id) {
            self.hovered = None;
        }
        if self.peek_suppressed == Some(id) {
            self.peek_suppressed = None;
        }
        if self.focus_new_note.as_ref().is_some_and(|(n, _)| *n == id) {
            self.focus_new_note = None;
        }
        self.selected.remove(&id);
    }
}

impl App {
    /// `parent`'s own group, if it has one and still exists — the natural
    /// "a tab opened from another tab lands in the same group" rule used
    /// by every `spawn_new_tab` call site that has a real opener (Ctrl+T,
    /// the context menu's "Open in new tab", a page-triggered popup). The
    /// toolbar's own "+ New Tab" button uses `self.current_group` directly
    /// instead, since `self.focused` (its `parent`) isn't necessarily a
    /// tab inside whichever group's subgraph is currently being viewed.
    pub(crate) fn group_of(&self, id: Option<NodeId>) -> Option<NodeId> {
        id.filter(|id| self.nodes.contains_key(id)).and_then(|id| self.doc.mirror.group(id))
    }

    /// The live and unloaded tabs at the level being viewed, in `node_order` (the order within a draw layer,
    /// `level_scan`): what graph navigation, selection and Close All act on.
    pub(crate) fn tabs_at_current_level(&self) -> Vec<NodeId> {
        self.node_order
            .iter()
            .filter(|id| self.in_level(**id) && self.nodes.get(*id).is_some_and(|n| n.runtime.saved_tab().is_some()))
            .copied()
            .collect()
    }

    /// Where the dragged card `id` would drop right now, and what that
    /// would do (`drop_action`). The back button is tested first, by
    /// pointer, since it's a screen-space toolbar widget. Then the card at
    /// this level containing the probe point, searched virtual card first,
    /// then group, live tab, sleeping tab, among kinds granting
    /// `drop_target`. The probe is a stored card's own position, or the
    /// pointer for a virtual card (it never moves, so it has no position to
    /// carry). A stored dragged card never targets a virtual one: its
    /// effects are all stored. Feeds both the live ring and the release, so
    /// the two can't disagree.
    pub(crate) fn drop_target(
        &self,
        id: CardId,
        pointer: Option<egui::Pos2>,
        leave_button: Option<egui::Rect>,
    ) -> Option<(DropEffect, Option<CardId>)> {
        use crate::store::model::{TYPE_TAB, TYPE_VIRTUAL_PREFIX};
        let items = self.level_items();
        let dragged_entry = items.iter().find(|item| item.id == id)?;
        let dragged = dragged_entry.kind;
        let inside_group = self.current_group.is_some();
        if leave_button.zip(pointer).is_some_and(|(button, pointer)| button.contains(pointer)) {
            return drop_action(dragged, DropTarget::BackButton, inside_group).map(|effect| (effect, None));
        }
        // A stored card is carried by its own position; a virtual one (it stays where its source put
        // it) is probed at the pointer, which the drag moves instead.
        let center = if dragged_entry.origin == Origin::Stored {
            self.layout_pos(id.node()?).to_pos2()
        } else {
            self.graph_screen_point_to_world(pointer?).to_pos2()
        };
        // A virtual card (it draws on top) before a group card before a tab card, and a live tab before a sleeping
        // one: the per-kind search order the drop has always had, each search's hit the topmost card of its kind in
        // draw order under the probe. One pass over the level finds all four; a target the drop table refuses
        // (`drop_action` None) moves the search on. Only a kind granting `drop_target` is a candidate.
        let searches: [&dyn Fn(CardKind) -> bool; 4] = [
            &|k| k.ty.starts_with(TYPE_VIRTUAL_PREFIX),
            &|k| is_container(k.ty),
            &|k| k.ty == TYPE_TAB && k.loaded(),
            &|k| k.ty == TYPE_TAB && !k.loaded(),
        ];
        let mut hits: [Option<(CardId, CardKind)>; 4] = [None; 4];
        // A stored card has only stored effects (`apply_drop`), so a virtual card is no target for it.
        let stored_drag = dragged_entry.origin == Origin::Stored;
        // Topmost first: the last in draw order is drawn over the rest.
        for other in items.iter().rev().filter(|other| {
            other.id != id && !(stored_drag && matches!(other.origin, Origin::Virtual(_))) && other.caps().drop_target
        }) {
            let Some(slot) = searches.iter().position(|pick| pick(other.kind)) else { continue };
            if hits[slot].is_none() && other.rect.contains(center) {
                hits[slot] = Some((other.id, other.kind));
            }
        }
        hits.iter().flatten().find_map(|&(target, kind)| {
            let effect = drop_action(dragged, DropTarget::Card(kind), inside_group)?;
            Some((effect, Some(target)))
        })
    }

    /// "Remove from Group" from any path (eject button, menu, a drop on
    /// the way back). The card leaves this level, so it leaves the
    /// rectangle selection too, which only ever holds this level's tabs
    /// (`switch_group_level` clears it): a later multi-select drag would
    /// otherwise move, or Join would pull back in, a tab no longer shown.
    pub(crate) fn eject(&mut self, id: NodeId) {
        self.remove_from_group(id);
        self.interaction.selected.remove(&CardId::from(id));
        self.recheck_edge_scope(id);
    }

    /// What a drop moves along with `id`: the whole rectangle selection
    /// when `id` is in it (it acts as one unit, as "Group Selected" does),
    /// which it empties, else just `id`. `target` (the group or card dropped on) is never part of the unit: it would
    /// be moved into itself.
    pub(crate) fn take_drag_unit(&mut self, id: CardId, target: NodeId) -> Vec<NodeId> {
        if self.interaction.selected.contains(&id) {
            self.take_selected_stored().into_iter().filter(|member| *member != target).collect()
        } else {
            id.node().into_iter().collect()
        }
    }

    /// A drag ends: each card it moved (`drag_moved`) is saved where it was dropped (`edit_pos`, 2b-4 Ruling 2).
    /// Written once, here, not every dragged frame: saves fire independently of the gesture (`poll_store`, a url
    /// change batched through `drain_tab_events`), so writing every frame would commit intermediate positions. A
    /// card removed mid-drag (closed, crashed, swapped) leaves its id in `drag_moved`; it is skipped here rather than
    /// cleared at every removal site. A drag that is no longer live (`live_drag`) saves nothing.
    pub(crate) fn release_drag(&mut self) {
        let live = self.live_drag().is_some();
        // A release no drop followed (the window lost focus) must not leave its origins for a later drop.
        self.interaction.drag_origin.clear();
        self.interaction.dragging = None;
        let moved = std::mem::take(&mut self.interaction.drag_moved);
        if !live {
            return;
        }
        for moved_id in moved {
            let Some(key) = moved_id.node() else { continue };
            if self.nodes.contains_key(&key) {
                self.interaction.drag_origin.push((moved_id, self.doc.mirror.pos(key)));
                self.edit_pos(key, Some(self.layout_pos(key)));
            }
        }
    }

    /// A card released after a drag (`GraphAction::Drop`), with where it
    /// landed, if anywhere. Saves the session on every release, since the
    /// drag itself moved the card.
    pub(crate) fn apply_drop(&mut self, id: CardId, target: Option<(DropEffect, Option<CardId>)>) {
        let origin = std::mem::take(&mut self.interaction.drag_origin);
        // Every effect below is a stored one: a virtual card at either end is its source's business (`on_drop`),
        // never a group to create, a link to make or a move to write.
        let Some(id) = id.node() else { return };
        let target = match target {
            Some((effect, Some(other))) => match other.node() {
                Some(other) => Some((effect, Some(other))),
                None => return,
            },
            Some((effect, None)) => Some((effect, None)),
            None => None,
        };
        // Every card's group and parent before the effect, so the step names exactly the cards it changed (a menu
        // Join moves the whole selection with no drag to say which).
        let before = self.membership_snapshot();
        let effect = target;
        match target {
            Some((DropEffect::Eject, _)) => self.eject(id),
            // A group closed or deleted since the drop began (a remote close after the menu opened) is not shown: a
            // node joined to it would vanish with it.
            Some((DropEffect::Join, Some(group))) if self.nodes.contains_key(&group) => {
                for member in self.take_drag_unit(CardId::from(id), group) {
                    if self.nodes.contains_key(&member) {
                        self.edit_group(member, Some(group));
                    }
                    self.recheck_edge_scope(member);
                }
            }
            Some((DropEffect::GroupWith, Some(other))) => {
                let unit = self.take_drag_unit(CardId::from(id), other);
                let at = self.layout_pos(other);
                self.group_cards(std::iter::once(other).chain(unit), at);
            }
            Some((DropEffect::Reparent, Some(parent))) => {
                let _ = self.connect(crate::caps::OPENED_FROM, id, parent);
            }
            Some((DropEffect::Merge, Some(target))) => self.merge_groups(id, target),
            _ => {}
        }
        // A Merge records its own fold; a drop with no effect or no change records nothing.
        if let Some((effect, other)) = effect.filter(|(effect, _)| *effect != DropEffect::Merge) {
            let moved = self.moved_since(before, &origin);
            if !moved.is_empty() {
                let created =
                    (effect == DropEffect::GroupWith).then(|| other.and_then(|o| self.doc.mirror.group(o))).flatten();
                let message = self.drop_message(effect, &moved, other);
                self.record_undo(message, UndoKind::Revert { moved, created });
            }
        }
        self.save_session();
    }

    /// A new group at `at` holding `members` (those shown; the others only get their edge scope rechecked); returns
    /// the group. A GroupWith drop and an accepted suggestion both end here.
    pub(crate) fn group_cards(&mut self, members: impl IntoIterator<Item = NodeId>, at: egui::Vec2) -> NodeId {
        let group = self.alloc_node_id();
        self.insert_group_at(group, Some(at));
        for member in members {
            if self.nodes.contains_key(&member) {
                self.edit_group(member, Some(group));
            }
            self.recheck_edge_scope(member);
        }
        group
    }

    /// Every shown card's group and parent, for `moved_since`.
    pub(crate) fn membership_snapshot(&self) -> Vec<(NodeId, Option<NodeId>, Option<NodeId>)> {
        self.nodes.keys().map(|&w| (w, self.doc.mirror.group(w), self.doc.mirror.parent(w))).collect()
    }

    /// The cards whose group or parent differs from `before`, each with its before and after values. `origin` says
    /// where dragged cards started (a card not in it did not move).
    pub(crate) fn moved_since(
        &self,
        before: Vec<(NodeId, Option<NodeId>, Option<NodeId>)>,
        origin: &[(CardId, Option<egui::Vec2>)],
    ) -> Vec<Moved> {
        before
            .into_iter()
            .filter_map(|(w, group, parent)| {
                let now_group = self.doc.mirror.group(w);
                let now_parent = self.doc.mirror.parent(w);
                (group != now_group || parent != now_parent).then(|| {
                    let pos_now = self.doc.mirror.pos(w);
                    let pos_before = origin.iter().find(|(o, _)| *o == CardId::from(w)).map_or(pos_now, |(_, p)| *p);
                    Moved { id: w, group: (group, now_group), parent: (parent, now_parent), pos: (pos_before, pos_now) }
                })
            })
            .collect()
    }

    /// What the toast says of a drop: "Moved 2 into Research".
    fn drop_message(&self, effect: DropEffect, moved: &[Moved], other: Option<NodeId>) -> String {
        let name = |id: Option<NodeId>| id.map(|id| self.drop_target_name(id)).unwrap_or_default();
        match effect {
            DropEffect::Join => format!("Moved {} into {}", moved.len(), name(other)),
            DropEffect::GroupWith => format!("Grouped {} cards", moved.len()),
            DropEffect::Eject => "Moved out of group".to_owned(),
            DropEffect::Reparent => {
                format!("Moved {} under {}", name(moved.first().map(|m| m.id)), name(other))
            }
            DropEffect::Merge => String::new(),
        }
    }

    /// `source`'s members move into `target`, and `source` closes holding them (`fold_group`; the toast's Undo
    /// reopens it around them): a group card dropped onto another. Refused if `target` is nested inside
    /// `source` (directly or transitively, walking the same `.group` chain
    /// `group_breadcrumb` does): reassigning source's own members to target
    /// while target is one of them would leave it (or a group between them)
    /// pointing at itself once source is removed, a self-reference cycle.
    /// Refused too when the walk meets a group cycle in the mirror (two
    /// replicas' moves raced; the next save shows the document's groups).
    pub(crate) fn merge_groups(&mut self, source: NodeId, target: NodeId) {
        // Refused too when `target` is not shown (closed or deleted remotely): its new members would vanish with it.
        if !self.nodes.contains_key(&target) {
            return;
        }
        let mut above = crate::walk::ancestors(target, |k| self.nodes.get(k).and(self.doc.mirror.group(*k)));
        if target == source || above.any(|a| a == source) || above.looped() {
            return;
        }
        let message = format!("Merged {} into {}", self.drop_target_name(source), self.drop_target_name(target));
        // Its links move to `target` and stay there on an undo: the source keeps none.
        self.retarget_links(source, target);
        for link in self.links_naming(source) {
            self.delete_link(link);
        }
        // The source closes holding its members, so the toast's Undo reopens it around them.
        self.fold_groups(vec![(source, Some(target))], message);
    }

    /// Would setting `node_id`'s parent to `new_parent_id` create a cycle?
    /// Walks `new_parent_id`'s own parent chain upward -- if it ever
    /// reaches `node_id`, `new_parent_id` is currently a descendant of
    /// `node_id`, so re-parenting `node_id` under it would loop. See the
    /// tab-onto-tab drag-to-reparent gesture (inside a group's own
    /// subgraph) this guards. A chain that is already a cycle (two replicas
    /// re-parented concurrently; the store doesn't cycle-check `parent`)
    /// counts as one too.
    pub(crate) fn would_create_cycle(&self, node_id: NodeId, new_parent_id: NodeId) -> bool {
        // The raw edge on purpose: a cycle through a closed card would come back with a reopen.
        let mut above = crate::walk::ancestors(new_parent_id, |k| self.doc.mirror.parent(*k));
        new_parent_id == node_id || above.any(|a| a == node_id) || above.looped()
    }

    /// Every ancestor above `self.current_group`, root-most first, as
    /// `(id, name)` pairs -- the toolbar's own breadcrumb trail now that
    /// groups nest. Empty at the true top level. Walks `Mirror::group`
    /// upward from the current group exactly the way `subtree_ids`/
    /// `take_out` already does to find a single ancestor; this just
    /// collects the whole chain instead of stopping at the first one.
    pub(crate) fn group_breadcrumb(&self) -> Vec<(NodeId, String)> {
        let Some(current) = self.current_group else { return Vec::new() };
        // Called every frame: `ancestors` ends on a cycle in the mirror's `.group` chain (a local move and another
        // replica's move can race into one until the next save), so it can't freeze the app.
        let up = crate::walk::ancestors(current, |k| self.doc.mirror.group(*k));
        let mut chain = Vec::new();
        for id in std::iter::once(current).chain(up) {
            let Some(node) = self.nodes.get(&id) else { break };
            // A JSON level's crumb is its Json node, named by its file.
            let name = match &node.runtime {
                // A group has no runtime of its own (2b-6 Ruling 8).
                // name rule: drawn by display_name in toolbar_breadcrumb.
                Runtime::None => self.doc.mirror.name(id).to_owned(),
                Runtime::Json(json) => download_filename(&json.path).to_string(),
                _ => String::new(),
            };
            chain.push((id, name));
        }
        chain.reverse();
        chain
    }

    /// "Reset View"'s scene rect: bounds every node currently visible at
    /// this level (every kind -- same level rules `level_scan`
    /// already uses) instead of always framing
    /// the same fixed world-space rect regardless of how many nodes exist
    /// or where they've drifted to. Each node contributes its own
    /// half-size (a
    /// node's *card*, not just its center point, must fit on screen) plus
    /// a flat margin so a node isn't left flush against the viewport edge.
    /// Falls back to `default_graph_scene_rect()` when there's nothing to
    /// fit (an empty group, or before any tab has spawned) — an empty
    /// bounding box has no sensible center/size to frame.
    pub(crate) fn fit_all_nodes_scene_rect(&self) -> egui::Rect {
        self.bounds_for_rects(self.level_items().into_iter().map(|item| item.rect))
    }

    /// Same computation `fit_all_nodes_scene_rect` needs, but taking
    /// already-known ids for a level instead of recomputing them -- lets
    /// the minimap (drawn every frame, well after `build_graph_ui`'s own
    /// per-frame `*_at_level` locals are already in hand) reuse this
    /// without redoing the level scans a second time per frame.
    pub(crate) fn bounds_for_level<'a>(&self, ids: impl IntoIterator<Item = &'a NodeId>) -> egui::Rect {
        self.bounds_for_rects(ids.into_iter().filter_map(|id| self.card_rect(*id)))
    }

    /// `root_id` plus every descendant reachable through `parent` links,
    /// restricted to the same level `root_id` itself is on (its group
    /// must match) -- a descendant that ended up in a different group is
    /// invisible at this level, and would otherwise pull the fit rect
    /// toward an off-screen position. Not the most efficient traversal
    /// (checks every tab per frontier node), but this graph's tab counts
    /// stay small enough that it's not worth a real adjacency structure.
    pub(crate) fn subtree_ids(&self, root_id: NodeId) -> Vec<NodeId> {
        if !self.nodes.contains_key(&root_id) {
            return Vec::new();
        }
        let level_group = self.doc.mirror.group(root_id);
        let children = |current: &NodeId| {
            let opened_here = |id: &&NodeId| self.opened_from(**id) == Some(*current);
            self.nodes
                .keys()
                .filter(opened_here)
                .filter(|id| self.doc.mirror.group(**id) == level_group)
                .copied()
                .collect()
        };
        let mut result = vec![root_id];
        result.extend(crate::walk::descendants(root_id, children, |_| true));
        result
    }

    /// The tab context menu's "Fit Subtree" entry -- same
    /// `bounds_for_level` margin/fallback shape as `fit_all_nodes_scene_rect`,
    /// just scoped to `subtree_ids` instead of the whole level. Sizing each
    /// member by its own `node_card_rect` (via `bounds_for_level`) rather
    /// than a flat `GRAPH_NODE_SIZE` matters here specifically: `subtree_ids`
    /// follows `parent` links, which include Download children, so a
    /// same-`GRAPH_NODE_SIZE` guess would mis-size those.
    pub(crate) fn fit_subtree_scene_rect(&self, root_id: NodeId) -> egui::Rect {
        self.bounds_for_level(&self.subtree_ids(root_id))
    }

    /// `(level, node_count)` for whichever level `self.current_group` is
    /// currently viewing -- the key `build_graph_ui`'s settle-check
    /// compares against `graph_settled_key`, factored out so every call
    /// site that changes `current_group` (the per-frame check, and
    /// entering/leaving a group) computes it the same way rather than
    /// duplicating the formula.
    pub(crate) fn current_level_settle_key(&self) -> (Option<NodeId>, usize) {
        (self.current_group, crate::level::settle_count(&self.level_items()))
    }

    /// Directional spatial navigation for arrow-key graph focus: among tab
    /// nodes at the current level lying roughly in `dir` from `from`,
    /// returns whichever minimizes `along + perp * 2.0` (distance in the
    /// travel direction, penalized for straying sideways) -- the standard
    /// heuristic behind CSS's spatial navigation, picking the node that
    /// "feels like" the next one over rather than just the closest by raw
    /// distance (which would favor a diagonal neighbor over a same-row one).
    /// Only tabs are candidates, not downloads/groups: `self.focused` is a
    /// tab id everywhere else it's read (address bar sync, Ctrl+W, "+ New
    /// Tab" parent), so keeping it tab-only here avoids handing those call
    /// sites an id type they don't expect.
    pub(crate) fn nearest_tab_in_direction(&self, from: egui::Vec2, dir: egui::Vec2) -> Option<NodeId> {
        let mut best: Option<(NodeId, f32)> = None;
        for id in self.tabs_at_current_level() {
            let delta = self.layout_pos(id) - from;
            let along = delta.dot(dir);
            if along <= 0.5 {
                continue;
            }
            let perp = (delta - dir * along).length();
            let score = along + perp * 2.0;
            if best.as_ref().is_none_or(|(_, s)| score < *s) {
                best = Some((id, score));
            }
        }
        best.map(|(id, _)| id)
    }

    /// Moves `self.focused` to the nearest tab node in `dir` (see
    /// `nearest_tab_in_direction`), or -- if nothing at this level is
    /// currently focused -- just picks a starting node so the first arrow
    /// press has something to move from next time. Shared by
    /// `handle_graph_keys`' real-keyboard arrow handling and the headless
    /// `GRAPHLEFT`/`GRAPHRIGHT`/`GRAPHUP`/`GRAPHDOWN` debug commands (a real
    /// `WindowEvent::KeyboardInput` can't be synthesized from application
    /// code -- see `debug_control`'s own doc comment -- so headless mode
    /// calls this directly instead).
    pub(crate) fn move_graph_focus(&mut self, dir: egui::Vec2) {
        let from_at_level =
            self.focused.filter(|&id| self.in_level(id) && self.nodes.contains_key(&id)).map(|id| self.layout_pos(id));
        match from_at_level {
            Some(from) => {
                if let Some(next) = self.nearest_tab_in_direction(from, dir) {
                    self.focused = Some(next);
                }
            }
            None => {
                if let Some(first) = self.tabs_at_current_level().into_iter().next() {
                    self.focused = Some(first);
                }
            }
        }
    }

    /// Opens whichever node `move_graph_focus` last landed on, the same way
    /// clicking its card does -- the Enter-key equivalent of graph-view
    /// keyboard navigation, and the headless `GRAPHOPEN` command.
    pub(crate) fn open_focused_graph_node(&mut self) {
        if let Some(id) = self.focused {
            let Some(node) = self.nodes.get(&id).filter(|_| self.in_level(id)) else {
                return;
            };
            // A crashed focused tab stays focused as its failed card, which
            // has no Item view: Enter retries it, as its click does. A
            // sleeping/waking card instead opens like a tab (its own
            // engine wakes via `focus_tab`).
            if node.as_unloaded().is_some_and(|t| matches!(t.state, UnloadedState::Failed(_))) {
                self.retry_missing_file(id);
            } else {
                self.start_transition_to_item(id);
            }
        }
    }

    /// Runs the force-directed layout to convergence for whichever level
    /// `self.current_group` is currently viewing, then stops -- unlike the
    /// old always-on-every-frame simulation, positions are static between
    /// calls to this. Continuous physics had a real, user-visible cost:
    /// every unpinned node (the drop target included) kept reacting to
    /// forces *while* being dragged onto, so a slow drag toward a group or
    /// another tab could chase a target that kept fleeing the incoming
    /// drag's own repulsion -- confirmed live while building the
    /// drag-to-group feature, where only a single fast cursor jump (no
    /// intermediate positions for the target to react to) reliably landed
    /// a drop. Calling this only when the visible node set actually
    /// changes (see `graph_settled_key`) keeps the one real benefit of the
    /// old approach -- new/remaining nodes still auto-spread apart instead
    /// of stacking -- without anything ever moving on its own while the
    /// user is trying to point at it.
    pub(crate) fn settle_current_level(&mut self) {
        // 90 iterations at a fixed 1/60s step is ~1.5 simulated seconds --
        // comfortably past the "well under a second" convergence this
        // layout already documented under continuous 60fps physics, with
        // margin for a slightly larger graph. Trivial cost for the tens
        // of nodes this app's graph view actually holds.
        const SETTLE_ITERATIONS: u32 = 90;
        const SETTLE_DT: f32 = 1.0 / 60.0;

        // Not doubled via a stray `.chain()` of the identical filter over
        // itself any more -- **found during a performance audit**: all
        // three of these (plus the write-back loop below) collected the
        // exact same filtered `self.nodes.iter()` twice, a duplicate-loop
        // artifact from the tabs/downloads/groups -> one `self.nodes` map
        // migration (the same recurring bug this project has already found
        // and fixed four separate times elsewhere -- `App::merge_groups`,
        // `remove_node` on a group twice, `replace_running_session`). Harmless for
        // `positions`/`velocities` (a `HashMap` insert of the same key
        // twice just overwrites itself), but a real correctness bug for
        // `edges`: a plain `Vec`, so every spring edge was fed into
        // `graph_layout::step` *twice* -- doubling the effective spring
        // strength for every one of this function's 90 warmup iterations.
        // `settle_current_level` is `graph_layout::step_sized`'s only call site
        // in this whole app (the layout doesn't run continuously every
        // frame -- see this function's own doc comment on why: it's a
        // one-time burst triggered only when the visible node set actually
        // changes), so this bug affected every settle animation, not a
        // background steady-state cost.
        // The layout only (`App::layout`): physics never writes the document pos (2b-4 Ruling 2).
        let at_level = || self.nodes.keys().filter(|id| self.in_level(**id));
        let layout = |id: NodeId| self.layout.get(&id).copied().unwrap_or_default();
        let mut positions: HashMap<NodeId, egui::Vec2> = at_level().map(|id| (*id, layout(*id).pos)).collect();
        let mut velocities: HashMap<NodeId, egui::Vec2> = at_level().map(|id| (*id, layout(*id).vel)).collect();
        let edges = self.spring_edges();
        let pinned: HashSet<NodeId> = self
            .nodes
            .iter()
            .filter(|(id, _)| self.in_level(**id) && self.doc.mirror.pinned(**id))
            .map(|(id, _)| *id)
            .collect();

        let sizes: HashMap<NodeId, egui::Vec2> = self
            .nodes
            .iter()
            .filter(|(id, _)| self.in_level(**id))
            .map(|(id, n)| (*id, crate::card::node_card_size(&n.runtime)))
            .collect();

        for _ in 0..SETTLE_ITERATIONS {
            graph_layout::step_sized(
                &mut positions,
                &mut velocities,
                &edges,
                &pinned,
                &sizes,
                graph_layout::GRAPH_WORLD_CENTER,
                SETTLE_DT,
            );
        }

        for (id, pos) in positions {
            let vel = velocities.get(&id).copied().unwrap_or_default();
            self.layout.insert(id, Layout { pos, vel });
        }
    }

    /// The `(opener, opened)` pairs that pull on each other in the layout: the level's edges whose table row has
    /// `affects_layout` (today `opened-from`, stored as `parent`). A link is not one, so it never moves a card.
    pub(crate) fn spring_edges(&self) -> Vec<(NodeId, NodeId)> {
        if !crate::caps::edges().row(crate::caps::OPENED_FROM).is_some_and(|row| row.affects_layout()) {
            return Vec::new();
        }
        let live = |id: NodeId| self.nodes.contains_key(&id) && self.in_level(id);
        self.nodes
            .keys()
            .filter(|id| live(**id))
            .filter_map(|id| self.opened_from(*id).map(|p| (p, *id)))
            .filter(|(p, _)| live(*p))
            .collect()
    }

    /// Where `id`'s card is drawn (`App::layout`); the origin for a node with no entry (a hand-built test node).
    pub(crate) fn layout_pos(&self, id: NodeId) -> egui::Vec2 {
        self.layout.get(&id).map_or(egui::Vec2::ZERO, |layout| layout.pos)
    }

    /// Moves `id`'s card to `pos`, at rest: a placement, a remote pos, a restored personal one.
    pub(crate) fn snap_layout(&mut self, id: NodeId, pos: egui::Vec2) {
        self.layout.insert(id, Layout::at(pos));
    }

    /// `id`'s card in world space (`node_card_rect` at its layout position); `None` when there is no such node.
    pub(crate) fn card_rect(&self, id: NodeId) -> Option<egui::Rect> {
        Some(node_card_rect(self.layout_pos(id), &self.nodes.get(&id)?.runtime))
    }

    /// Whether the user is dragging `id` right now: the card under the pointer, or one moving with it as part of
    /// the selection (`drag_moved`), while that drag is live (`live_drag`).
    pub(crate) fn is_being_dragged(&self, id: CardId) -> bool {
        self.live_drag().is_some_and(|dragged| dragged == id || self.interaction.drag_moved.contains(&id))
    }

    /// The grabbed card (`dragging`) while its drag is live: the card is still on the level shown. A grabbed card
    /// that left it (a level switch with the button held, another replica moving it into another group, a removal)
    /// is no longer drawn, so its card never sees the release; its drag is over, with no drop.
    /// A virtual card (no node) is live while a source still shows it and its kind is a `drag_source`.
    fn live_drag(&self) -> Option<CardId> {
        self.interaction.dragging.filter(|dragged| match dragged.node() {
            Some(key) if self.nodes.contains_key(&key) => self.in_level(key),
            _ => self.level_items().iter().any(|item| item.id == *dragged && item.caps().drag_source),
        })
    }

    /// Ends a drag that is no longer live (`live_drag`), or any drag when Graph view isn't shown (`graph_shown`
    /// false), with no drop: nothing is saved. The layout keeps the dragged positions (remote snaps were skipped
    /// while it ran); a later remote pos snaps again. Otherwise the card's next release, whenever it is drawn again,
    /// would save the stale layout pos over a remote one.
    pub(crate) fn end_stale_drag(&mut self, graph_shown: bool) {
        if self.interaction.dragging.is_some() && !(graph_shown && self.live_drag().is_some()) {
            self.interaction.dragging = None;
            self.interaction.drag_moved.clear();
        }
    }

    /// Drops from the selection every card no longer drawn on this level: a tab closed, mid-respawn, or moved by
    /// another replica. What stays is a card of this level whose kind grants `selectable` (a stored card's row does;
    /// a virtual kind's grants it only if its source's row says so). A JSON level's cards are never selected. It
    /// only ever holds this level's cards (`eject`, `switch_group_level`); otherwise Close Selected or Group
    /// Selected would act on a card the user can't see.
    pub(crate) fn end_stale_selection(&mut self) {
        // Every frame: with nothing selected there is nothing to prune, and the level (every source's entries) is
        // not built for it.
        if self.interaction.selected.is_empty() {
            return;
        }
        let selectable: HashSet<CardId> = self.selectable_ids();
        self.interaction.selected.retain(|id| selectable.contains(id));
    }

    /// The ids of this level's cards that can be selected (`Caps::selectable`), whatever supplies them.
    pub(crate) fn selectable_ids(&self) -> HashSet<CardId> {
        self.level_items().into_iter().filter(|item| item.caps().selectable).map(|item| item.id).collect()
    }

    /// Takes the stored cards out of the selection for a selection action and leaves the virtual ones in it: a
    /// virtual card has no node for the action to change. What it returns is the nodes' ids.
    pub(crate) fn take_selected_stored(&mut self) -> HashSet<NodeId> {
        let (taken, kept) =
            std::mem::take(&mut self.interaction.selected).into_iter().partition(|id| id.node().is_some());
        self.interaction.selected = kept;
        taken.into_iter().filter_map(|id: CardId| id.node()).collect()
    }

    /// How many selected cards a selection action would act on: those that are not virtual (`take_selected_stored`).
    pub(crate) fn selected_stored_count(&self) -> usize {
        self.interaction.selected.iter().filter(|id| id.node().is_some()).count()
    }
}

impl App {
    /// A drop target's name for the hint, through the display rule and cut to a row's width.
    pub(crate) fn drop_target_name(&self, id: NodeId) -> String {
        let name = if self.is_container(id) {
            // name rule: drawn by display_name just below
            self.doc.mirror.name(id).to_owned()
        } else {
            self.listed_tab_label(id).unwrap_or_default()
        };
        let segs = crate::names::display_name(&name, crate::names::UNTITLED);
        crate::names::to_plain(&crate::names::shorten(&segs, crate::names::ROW_UNITS).unwrap_or(segs))
    }
}

#[cfg(test)]
mod tests {
    /// `GraphInteraction::forget` clears what `forget_id` cleared by hand: the dragged, hovered and peek-suppressed
    /// node if it is `id`, and `id` in the selection, leaving other nodes' state and the rest alone.
    #[test]
    fn forget_clears_only_the_removed_ids_state() {
        use crate::card_id::testutil::card;
        let (t, u) = (card(1), card(2));
        let mut gi = GraphInteraction {
            dragging: Some(t),
            drag_moved: vec![t],
            hovered: Some(t),
            peek_suppressed: Some(t),
            selected: HashSet::from([t, u]),
            pending_new_node: Some(egui::Vec2::ZERO),
            ..Default::default()
        };
        gi.forget(t);
        assert!(gi.dragging.is_none() && gi.hovered.is_none() && gi.peek_suppressed.is_none());
        assert_eq!(gi.selected, HashSet::from([u]));
        assert_eq!(gi.drag_moved, [t], "not forgotten: skipped at release");
        assert!(gi.pending_new_node.is_some());
        gi.dragging = Some(u);
        gi.forget(t);
        assert_eq!(gi.dragging, Some(u), "another node's state stays");
    }

    use super::*;
    use crate::store::model::Kind;

    /// Frame cost of the level scan (2b-6 Ruling 10), timing, not correctness, so ignored:
    /// `cargo test -p manifold --bins level_pass_cost -- --ignored --nocapture`. Three variants over the same 2000
    /// nodes: the enum pass Graph view had before 2b-6 (eight passes, each matching the payload against one kind),
    /// the eight `card_kind`-filtered passes plus the group pass Task 2 left, and the single bucketed pass
    /// (`level_scan`). Release-build numbers are recorded in the 2b-6 plan's Outcome
    /// (`docs/superpowers/plans/2026-10-06-node-model-2b-6-enums-to-type.md`).
    #[test]
    #[ignore]
    fn level_pass_cost() {
        use crate::bench::{report, timed};
        const N: usize = 200;
        let f = crate::bench::level_fixture(true);
        assert_eq!((f.groups.len(), f.nodes.len()), (8, 2000));
        let levels = [None, Some(f.groups[0])];
        let mut app = f.app; // `f`'s other fields (the live tabs' peers) live to the end of the test
        for level in levels {
            app.current_group = level;
            let in_level = app.node_order.iter().filter(|id| app.in_level(**id)).count();
            println!("level {level:?}: {in_level} of {} nodes", app.node_order.len());

            // (a) The old enum pass: eight per-kind calls, each a walk of `node_order` keeping the nodes whose
            // payload variant was that kind (the card enum 2b-6 deleted, matched per node per pass).
            let old_layer = |payload: &Runtime| match payload {
                Runtime::Tab(_) => 0,
                Runtime::Unloaded(_) => 1,
                Runtime::Download(_) => 2,
                Runtime::Note(_) | Runtime::Jar | Runtime::Unknown => 3,
                Runtime::Image(_) => 4,
                Runtime::Pdf(_) => 5,
                Runtime::Json(_) => 6,
                Runtime::None => 7, // the old Group variant
            };
            let old_pass = || {
                std::array::from_fn::<Vec<NodeId>, 8, _>(|layer| {
                    app.node_order
                        .iter()
                        .filter(|id| {
                            app.in_level(**id) && app.nodes.get(*id).is_some_and(|n| old_layer(&n.runtime) == layer)
                        })
                        .copied()
                        .collect()
                })
            };
            report("old: 8 enum passes", timed(N, || drop(old_pass())));

            // (b) Task 2's: eight `card_kind` passes by draw layer plus the group pass.
            let at_level = |pick: &dyn Fn(CardKind) -> bool| -> Vec<NodeId> {
                app.node_order
                    .iter()
                    .filter(|id| app.in_level(**id) && app.card_kind(**id).is_some_and(pick))
                    .copied()
                    .collect()
            };
            let eight_pass = || {
                let layers: [Vec<NodeId>; crate::caps::DRAW_LAYERS] =
                    std::array::from_fn(|layer| at_level(&|kind| usize::from(kind.caps().draw_layer) == layer));
                let groups = at_level(&|kind| is_container(kind.ty));
                (layers, groups)
            };
            report("task 2: 8 card_kind passes + groups", timed(N, || drop(eight_pass())));

            // (c) Task 3's single pass, and where its per-node cost goes: the level filter alone, plus `card_kind`,
            // plus `caps()` (the draw layer), plus `is_container` (the whole of `level_scan`).
            report("task 3: level_scan, one pass", timed(N, || drop(app.level_scan())));
            let mut sink = 0usize;
            let level_only = || app.node_order.iter().filter(|id| app.in_level(**id)).count();
            report("  in_level only", timed(N, || sink += level_only()));
            let kinds_only =
                || app.node_order.iter().filter(|id| app.in_level(**id)).filter_map(|&id| app.card_kind(id)).count();
            report("  + card_kind", timed(N, || sink += kinds_only()));
            let layers_only = || {
                app.node_order
                    .iter()
                    .filter(|id| app.in_level(**id))
                    .filter_map(|&id| app.card_kind(id))
                    .map(|kind| usize::from(kind.caps().draw_layer))
                    .sum::<usize>()
            };
            report("  + caps().draw_layer", timed(N, || sink += layers_only()));
            assert!(sink > 0);

            let (layers, groups) = eight_pass();
            let scan = app.level_scan();
            assert_eq!((scan.layers, scan.groups), (layers, groups));
            assert_eq!(old_pass().concat().len(), scan_len(&app));
        }
    }

    /// Wave 2 (H3): one `settle_current_level` (90 steps of the O(n²) pair loop) at the top level (about 675 cards)
    /// and in the first group (about 167), from a grid. Timing, not correctness, so ignored:
    /// `cargo test --release -j 8 -p manifold --bins settle_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn settle_cost() {
        use crate::bench::{level_fixture, report, spread, timed};
        let mut f = level_fixture(false);
        spread(&mut f);
        let levels = [None, Some(f.groups[0])];
        let mut app = f.app;
        let start = app.layout.clone();
        for level in levels {
            app.current_group = level;
            let cards = app.nodes.keys().filter(|id| app.in_level(**id)).count();
            let what = format!("settle, {} ({cards} cards)", if level.is_some() { "g0" } else { "top" });
            report(
                &what,
                timed(10, || {
                    app.layout = start.clone();
                    app.settle_current_level();
                }),
            );
        }
    }

    fn scan_len(app: &App) -> usize {
        app.level_scan().layers.concat().len()
    }

    /// H1: another replica moves a selected tab into a group; the next frame drops it from the selection, so Close
    /// Selected closes only what is still on screen.
    #[test]
    fn a_tab_another_replica_moved_away_leaves_the_selection() {
        use crate::session_sync::tests::{add, add_group, sleeping_tab};
        let ctx = crate::theme::testutil::ctx(true);
        let mut app = App { view_mode: crate::view_modes::ViewMode::Settings, ..App::default() };
        let a = add(&mut app, Kind::Tab, sleeping_tab("https://a.test/", "a"));
        let b = add(&mut app, Kind::Tab, sleeping_tab("https://b.test/", "b"));
        let h = add_group(&mut app, "H");
        app.interaction.selected.extend([CardId::from(a), CardId::from(b)]);
        app.follow_remote_group(b, Some(h)); // another replica's move
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 1000.0))),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| app.build_ui_inner(ui));
        output.textures_delta.clear();
        assert_eq!(app.interaction.selected, std::collections::HashSet::from([CardId::from(a)]));
        app.apply_graph_action(&ctx, crate::graph_ui::GraphAction::CloseSelected);
        assert!(!app.nodes.contains_key(&a), "the tab still shown is closed");
        assert!(app.nodes.contains_key(&b), "the tab moved away is not");
    }

    /// A selection of groups and notes dragged onto one of its own groups: the target stays out of the unit, so no
    /// group becomes its own parent, and the others join it.
    #[test]
    fn dropping_a_mixed_selection_on_a_selected_group_never_nests_it_in_itself() {
        use crate::session_sync::tests::add_group;
        let mut app = App::default();
        let (g1, g2) = (add_group(&mut app, "G1"), add_group(&mut app, "G2"));
        app.new_note_shortcut();
        let note = app
            .node_order
            .iter()
            .find(|id| app.card_kind(**id).is_some_and(|k| k.ty == crate::store::model::TYPE_NOTE))
            .copied()
            .unwrap();
        app.interaction.selected.extend([CardId::from(g1), CardId::from(g2), CardId::from(note)]);
        app.apply_drop(CardId::from(g1), Some((DropEffect::Join, Some(CardId::from(g2)))));
        assert_eq!(app.doc.mirror.group(g2), None, "the target is not moved into itself");
        assert_eq!(app.doc.mirror.group(g1), Some(g2));
        assert_eq!(app.doc.mirror.group(note), Some(g2));
        assert!(app.interaction.selected.is_empty());
    }
}
