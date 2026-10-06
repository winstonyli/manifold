//! Graph navigation: fit and bounds, settle, focus movement, group lookup, drops and merges.

use super::*;

impl App {
    /// `parent`'s own group, if it has one and still exists — the natural
    /// "a tab opened from another tab lands in the same group" rule used
    /// by every `spawn_new_tab` call site that has a real opener (Ctrl+T,
    /// the context menu's "Open in new tab", a page-triggered popup). The
    /// toolbar's own "+ New Tab" button uses `self.current_group` directly
    /// instead, since `self.focused` (its `parent`) isn't necessarily a
    /// tab inside whichever group's subgraph is currently being viewed.
    pub(crate) fn group_of(&self, id: &Option<String>) -> Option<String> {
        id.as_deref().filter(|id| self.nodes.contains_key(*id)).and_then(|id| self.mirror.group(id)).map(str::to_owned)
    }

    /// `node_order`, filtered to nodes of `kind` at whichever level
    /// `self.current_group` is currently viewing -- the id list
    /// `build_graph_ui`'s one card loop iterates for that kind, kept in stable
    /// insertion order (unlike filtering `self.nodes` directly, a
    /// `HashMap`'s iteration order isn't stable frame-to-frame). The kind
    /// filter matters even for one kind: a top-level group's own `group`
    /// is `None`, same as `current_group` at the top level, so a level
    /// check alone would leak group cards into every other kind's list.
    pub(crate) fn nodes_at_current_level(&self, kind: ItemNodeKind) -> Vec<String> {
        self.node_order
            .iter()
            .filter(|id| self.in_level(id) && self.nodes.get(*id).is_some_and(|n| item_node_kind(&n.payload) == kind))
            .cloned()
            .collect()
    }

    /// The live and unloaded tabs at the level being viewed, in draw order:
    /// what graph navigation, selection and Close All act on.
    pub(crate) fn tabs_at_current_level(&self) -> Vec<String> {
        self.node_order
            .iter()
            .filter(|id| self.in_level(id) && self.nodes.get(*id).is_some_and(|n| n.payload.saved_tab().is_some()))
            .cloned()
            .collect()
    }

    /// Where the dragged card `id` would drop right now, and what that
    /// would do (`drop_action`). The back button is tested first, by
    /// pointer, since it's a screen-space toolbar widget. Then the card at
    /// this level containing the dragged card's center, a group card before
    /// a tab card. Feeds both the live ring and the release, so the two
    /// can't disagree.
    pub(crate) fn drop_target(
        &self,
        id: &str,
        pointer: Option<egui::Pos2>,
        leave_button: Option<egui::Rect>,
    ) -> Option<(DropEffect, Option<String>)> {
        let node = self.nodes.get(id)?;
        let dragged = item_node_kind(&node.payload);
        let inside_group = self.current_group.is_some();
        if leave_button.zip(pointer).is_some_and(|(button, pointer)| button.contains(pointer)) {
            return drop_action(dragged, DropTarget::BackButton, inside_group).map(|effect| (effect, None));
        }
        let center = self.layout_pos(id).to_pos2();
        [ItemNodeKind::Group, ItemNodeKind::Tab, ItemNodeKind::Unloaded].into_iter().find_map(|kind| {
            let effect = drop_action(dragged, DropTarget::Card(kind), inside_group)?;
            let target = self
                .nodes_at_current_level(kind)
                .into_iter()
                .find(|other| other != id && self.card_rect(other).is_some_and(|rect| rect.contains(center)))?;
            Some((effect, Some(target)))
        })
    }

    /// "Remove from Group" from any path (eject button, menu, a drop on
    /// the way back). The card leaves this level, so it leaves the
    /// rectangle selection too, which only ever holds this level's tabs
    /// (`switch_group_level` clears it): a later multi-select drag would
    /// otherwise move, or Join would pull back in, a tab no longer shown.
    pub(crate) fn eject(&mut self, id: &str) {
        self.remove_from_group(id);
        self.selected_tabs.remove(id);
        self.refollow(id);
    }

    /// What a drop moves along with `id`: the whole rectangle selection
    /// when `id` is in it (it acts as one unit, as "Group Selected" does),
    /// which it empties, else just `id`.
    pub(crate) fn take_drag_unit(&mut self, id: &str) -> Vec<String> {
        if self.selected_tabs.contains(id) {
            self.selected_tabs.drain().collect()
        } else {
            vec![id.to_string()]
        }
    }

    /// A drag ends: each card it moved (`drag_moved`) is saved where it was dropped (`edit_pos`, 2b-4 Ruling 2).
    /// Written once, here, not every dragged frame: saves fire independently of the gesture (`poll_store`, a url
    /// change batched through `drain_tab_events`), so writing every frame would commit intermediate positions. A
    /// card removed mid-drag (closed, crashed, swapped) leaves its id in `drag_moved`; it is skipped here rather than
    /// cleared at every removal site. A drag that is no longer live (`live_drag`) saves nothing.
    pub(crate) fn release_drag(&mut self) {
        let live = self.live_drag().is_some();
        self.dragging = None;
        let moved = std::mem::take(&mut self.drag_moved);
        if !live {
            return;
        }
        for moved_id in moved {
            if self.nodes.contains_key(&moved_id) {
                self.edit_pos(&moved_id, Some(self.layout_pos(&moved_id)));
            }
        }
    }

    /// A card released after a drag (`GraphAction::Drop`), with where it
    /// landed, if anywhere. Saves the session on every release, since the
    /// drag itself moved the card.
    pub(crate) fn apply_drop(&mut self, id: &str, target: Option<(DropEffect, Option<String>)>) {
        match target {
            Some((DropEffect::Eject, _)) => self.eject(id),
            Some((DropEffect::Join, Some(group))) => {
                for member in self.take_drag_unit(id) {
                    if self.nodes.contains_key(&member) {
                        self.edit_group(&member, Some(group.clone()));
                    }
                    self.refollow(&member);
                }
            }
            Some((DropEffect::GroupWith, Some(other))) => {
                let group = self.alloc_node_id();
                let pos = self.layout_pos(&other);
                self.insert_group_at(group.clone(), Some(pos));
                let unit = self.take_drag_unit(id);
                for member in std::iter::once(other).chain(unit) {
                    if self.nodes.contains_key(&member) {
                        self.edit_group(&member, Some(group.clone()));
                    }
                    self.refollow(&member);
                }
            }
            Some((DropEffect::Reparent, Some(parent))) => {
                if !self.would_create_cycle(id, &parent) {
                    self.edit_parent(id, Some(parent));
                }
            }
            Some((DropEffect::Merge, Some(target))) => self.merge_groups(id, &target),
            _ => {}
        }
        self.save_session();
    }

    /// `source`'s members move into `target`, and `source` goes: a group
    /// card dropped onto another. Refused if `target` is nested inside
    /// `source` (directly or transitively, walking the same `.group` chain
    /// `group_breadcrumb` does): reassigning source's own members to target
    /// while target is one of them would leave it (or a group between them)
    /// pointing at itself once source is removed, a self-reference cycle.
    /// Refused too when the walk meets a group cycle in the mirror (two
    /// replicas' moves raced; the next save shows the document's groups).
    pub(crate) fn merge_groups(&mut self, source: &str, target: &str) {
        let mut ancestor = Some(target.to_string());
        let mut visited = HashSet::new();
        while let Some(cur) = ancestor {
            if cur == source || !visited.insert(cur.clone()) {
                return;
            }
            ancestor = self.nodes.get(&cur).and(self.mirror.group(&cur)).map(str::to_owned);
        }
        let moved: Vec<String> = self.members_of(source).map(str::to_owned).collect();
        for member in &moved {
            self.edit_group(member, Some(target.to_string()));
        }
        self.nodes.remove(source);
        self.queue_delete(source);
        self.node_order.retain(|g| g != source);
        self.forget_removed_group(source);
        // Only the members that moved: `target`'s own tabs keep their jar (deviation 12).
        for member in moved {
            self.refollow(&member);
        }
    }

    /// Would setting `node_id`'s parent to `new_parent_id` create a cycle?
    /// Walks `new_parent_id`'s own parent chain upward -- if it ever
    /// reaches `node_id`, `new_parent_id` is currently a descendant of
    /// `node_id`, so re-parenting `node_id` under it would loop. See the
    /// tab-onto-tab drag-to-reparent gesture (inside a group's own
    /// subgraph) this guards. A chain that is already a cycle (two replicas
    /// re-parented concurrently; the store doesn't cycle-check `parent`)
    /// counts as one too.
    pub(crate) fn would_create_cycle(&self, node_id: &str, new_parent_id: &str) -> bool {
        let mut current = Some(new_parent_id.to_string());
        let mut visited = HashSet::new();
        while let Some(cur) = current {
            if cur == node_id || !visited.insert(cur.clone()) {
                return true;
            }
            current = self.mirror.parent(&cur).map(str::to_owned);
        }
        false
    }

    /// Every ancestor above `self.current_group`, root-most first, as
    /// `(id, name)` pairs -- the toolbar's own breadcrumb trail now that
    /// groups nest. Empty at the true top level. Walks `Mirror::group`
    /// upward from the current group exactly the way `subtree_ids`/
    /// `dissolve_group` already do to find a single ancestor; this just
    /// collects the whole chain instead of stopping at the first one.
    pub(crate) fn group_breadcrumb(&self) -> Vec<(String, String)> {
        let mut chain = Vec::new();
        let mut visited = std::collections::HashSet::new();
        let mut current = self.current_group.clone();
        // Guards against an infinite loop on a cycle in the mirror's
        // `.group` chain: a local move and another replica's move can race
        // into one until the next save shows the document's groups (the
        // store refuses the move that closes it). Called every frame, a
        // hang here would freeze the whole app with no error.
        while let Some(id) = current {
            if !visited.insert(id.clone()) {
                break;
            }
            let Some(node) = self.nodes.get(&id) else { break };
            // A JSON level's crumb is its Json node, named by its file.
            let name = match &node.payload {
                // name rule: drawn by display_name in toolbar_breadcrumb
                NodePayload::Group(_) => self.mirror.name(&id).to_owned(),
                NodePayload::Json(json) => download_filename(&json.path).to_string(),
                _ => String::new(),
            };
            current = self.mirror.group(&id).map(str::to_owned);
            chain.push((id, name));
        }
        chain.reverse();
        chain
    }

    /// "Reset View"'s scene rect: bounds every node currently visible at
    /// this level (every kind -- same level rules `nodes_at_current_level`
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
    pub(crate) fn bounds_for_level<'a>(&self, ids: impl IntoIterator<Item = &'a String>) -> egui::Rect {
        self.bounds_for_rects(ids.into_iter().filter_map(|id| self.card_rect(id)))
    }

    /// `root_id` plus every descendant reachable through `parent` links,
    /// restricted to the same level `root_id` itself is on (its group
    /// must match) -- a descendant that ended up in a different group is
    /// invisible at this level, and would otherwise pull the fit rect
    /// toward an off-screen position. Not the most efficient traversal
    /// (checks every tab per frontier node), but this graph's tab counts
    /// stay small enough that it's not worth a real adjacency structure.
    pub(crate) fn subtree_ids(&self, root_id: &str) -> Vec<String> {
        if !self.nodes.contains_key(root_id) {
            return Vec::new();
        }
        let level_group = self.mirror.group(root_id);
        let mut result = vec![root_id.to_string()];
        let mut frontier = vec![root_id.to_string()];
        while let Some(current) = frontier.pop() {
            for id in self.nodes.keys() {
                if self.mirror.parent(id) == Some(current.as_str())
                    && self.mirror.group(id) == level_group
                    && !result.contains(id)
                {
                    result.push(id.clone());
                    frontier.push(id.clone());
                }
            }
        }
        result
    }

    /// The tab context menu's "Fit Subtree" entry -- same
    /// `bounds_for_level` margin/fallback shape as `fit_all_nodes_scene_rect`,
    /// just scoped to `subtree_ids` instead of the whole level. Sizing each
    /// member by its own `node_card_rect` (via `bounds_for_level`) rather
    /// than a flat `GRAPH_NODE_SIZE` matters here specifically: `subtree_ids`
    /// follows `parent` links, which include Download children, so a
    /// same-`GRAPH_NODE_SIZE` guess would mis-size those.
    pub(crate) fn fit_subtree_scene_rect(&self, root_id: &str) -> egui::Rect {
        self.bounds_for_level(&self.subtree_ids(root_id))
    }

    /// `(level, node_count)` for whichever level `self.current_group` is
    /// currently viewing -- the key `build_graph_ui`'s settle-check
    /// compares against `graph_settled_key`, factored out so every call
    /// site that changes `current_group` (the per-frame check, and
    /// entering/leaving a group) computes it the same way rather than
    /// duplicating the formula.
    pub(crate) fn current_level_settle_key(&self) -> (Option<String>, usize) {
        (self.current_group.clone(), self.level_items().len())
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
    pub(crate) fn nearest_tab_in_direction(&self, from: egui::Vec2, dir: egui::Vec2) -> Option<String> {
        let mut best: Option<(String, f32)> = None;
        for id in self.tabs_at_current_level() {
            let delta = self.layout_pos(&id) - from;
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
        let from_at_level = self
            .focused
            .as_deref()
            .filter(|id| self.in_level(id) && self.nodes.contains_key(*id))
            .map(|id| self.layout_pos(id));
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
        if let Some(id) = self.focused.clone() {
            let Some(node) = self.nodes.get(&id).filter(|_| self.in_level(&id)) else {
                return;
            };
            // A crashed focused tab stays focused as its failed card, which
            // has no Item view: Enter retries it, as its click does. A
            // sleeping/waking card instead opens like a tab (its own
            // engine wakes via `focus_tab`).
            if node.as_unloaded().is_some_and(|t| matches!(t.state, UnloadedState::Failed(_))) {
                self.retry_missing_file(&id);
            } else {
                self.start_transition_to_item(&id);
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
        // `dissolve_group` twice, `replace_running_session`). Harmless for
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
        let at_level = || self.nodes.keys().filter(|id| self.in_level(id));
        let layout = |id: &String| self.layout.get(id).copied().unwrap_or_default();
        let mut positions: HashMap<String, egui::Vec2> = at_level().map(|id| (id.clone(), layout(id).pos)).collect();
        let mut velocities: HashMap<String, egui::Vec2> = at_level().map(|id| (id.clone(), layout(id).vel)).collect();
        let edges: Vec<(String, String)> = self
            .nodes
            .iter()
            .filter(|(id, _)| self.in_level(id))
            .filter_map(|(id, _)| self.mirror.parent(id).map(|p| (p.to_owned(), id.clone())))
            .filter(|(p, c)| positions.contains_key(p) && positions.contains_key(c))
            .collect();
        let pinned: HashSet<String> = self
            .nodes
            .iter()
            .filter(|(id, _)| self.in_level(id) && self.mirror.pinned(id))
            .map(|(id, _)| id.clone())
            .collect();

        let sizes: HashMap<String, egui::Vec2> = self
            .nodes
            .iter()
            .filter(|(id, _)| self.in_level(id))
            .map(|(id, n)| (id.clone(), crate::card::node_card_size(&n.payload)))
            .collect();

        for _ in 0..SETTLE_ITERATIONS {
            graph_layout::step_sized(
                &mut positions,
                &mut velocities,
                &edges,
                &pinned,
                &sizes,
                GRAPH_WORLD_CENTER,
                SETTLE_DT,
            );
        }

        for (id, pos) in positions {
            let vel = velocities.get(&id).copied().unwrap_or_default();
            self.layout.insert(id, Layout { pos, vel });
        }
    }

    /// Where `id`'s card is drawn (`App::layout`); the origin for a node with no entry (a hand-built test node).
    pub(crate) fn layout_pos(&self, id: &str) -> egui::Vec2 {
        self.layout.get(id).map_or(egui::Vec2::ZERO, |layout| layout.pos)
    }

    /// Moves `id`'s card to `pos`, at rest: a placement, a remote pos, a restored personal one.
    pub(crate) fn snap_layout(&mut self, id: &str, pos: egui::Vec2) {
        self.layout.insert(id.to_owned(), Layout::at(pos));
    }

    /// `id`'s card in world space (`node_card_rect` at its layout position); `None` when there is no such node.
    pub(crate) fn card_rect(&self, id: &str) -> Option<egui::Rect> {
        Some(node_card_rect(self.layout_pos(id), &self.nodes.get(id)?.payload))
    }

    /// Whether the user is dragging `id` right now: the card under the pointer, or one moving with it as part of
    /// the selection (`drag_moved`), while that drag is live (`live_drag`).
    pub(crate) fn is_being_dragged(&self, id: &str) -> bool {
        self.live_drag().is_some_and(|dragged| dragged == id || self.drag_moved.iter().any(|m| m == id))
    }

    /// The grabbed card (`dragging`) while its drag is live: the card is still on the level shown. A grabbed card
    /// that left it (a level switch with the button held, another replica moving it into another group, a removal)
    /// is no longer drawn, so its card never sees the release; its drag is over, with no drop.
    fn live_drag(&self) -> Option<&str> {
        self.dragging.as_deref().filter(|dragged| self.nodes.contains_key(*dragged) && self.in_level(dragged))
    }

    /// false), with no drop: nothing is saved. The layout keeps the dragged positions (remote snaps were skipped
    /// while it ran); a later remote pos snaps again. Otherwise the
    /// false), with no drop: nothing is saved, and the cards it moved take remote positions again. Otherwise the
    /// card's next release, whenever it is drawn again, would save the stale layout pos over a remote one.
    pub(crate) fn end_stale_drag(&mut self, graph_shown: bool) {
        if self.dragging.is_some() && !(graph_shown && self.live_drag().is_some()) {
            self.dragging = None;
            self.drag_moved.clear();
        }
    }
}
