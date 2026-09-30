//! Tab lifecycle on App: spawn, respawn, focus, hide, split, leave to graph, destroy and remove.

use super::*;

impl App {
    /// Spawns the very first tab at startup: no parent, becomes focused,
    /// starts in Tab view (preserves "opens straight into a usable
    /// browser" from before M5 — Graph view is reached explicitly).
    pub(crate) fn spawn_root_tab(&mut self) {
        let id = self.alloc_node_id();
        let home_page = self.settings.home_page.clone();
        self.tab_spawn_error = None;
        let kind = self.settings.default_starting_engine;
        let zoom = self.settings.zoom_factor;
        self.spawn_tab_async(TabSpawn::new(id, home_page, kind, egui::Vec2::ZERO, SpawnPurpose::Root, zoom));
    }

    /// Runs `process_manager::spawn_tab` on a background thread and posts
    /// its result back via `event_proxy` as `UserEvent::TabSpawned` — see
    /// `TabSpawnOutcome`'s doc comment for why this can't run inline on the
    /// main/event-loop thread. `handle_tab_spawn_result` does what the old
    /// synchronous `Ok`/`Err` branches used to do, once the result actually
    /// arrives.
    pub(crate) fn spawn_tab_async(&mut self, spawn: TabSpawn) {
        self.tab_spawns_in_flight += 1;
        self.spawning.insert(spawn.id.clone(), session_sync::spawn_shared(&spawn));
        let generation = self.session_generation;
        #[cfg(test)]
        if self.event_proxy.is_none() {
            self.test_spawns.push((spawn, generation));
            return;
        }
        let proxy = self.event_proxy.clone().expect("event_proxy set in main before any tab spawn");
        std::thread::spawn(move || {
            let result = process_manager::spawn_tab(spawn.kind, &spawn.id, &spawn.url);
            let _ = proxy.send_event(UserEvent::TabSpawned(Box::new(TabSpawnOutcome { spawn, generation, result })));
        });
    }

    /// Applies a finished `spawn_tab_async` call — inserts the new tab node
    /// (or records `tab_spawn_error`), with per-`SpawnPurpose` follow-up
    /// matching what `spawn_root_tab`/`spawn_new_tab`/`respawn_tab` used to
    /// do synchronously right after `process_manager::spawn_tab` returned.
    /// Focus/visibility for `Respawn` is decided here, at completion time —
    /// same moment the old inline code decided it, just later in wall time.
    pub(crate) fn handle_tab_spawn_result(&mut self, outcome: TabSpawnOutcome) {
        let TabSpawnOutcome {
            spawn: TabSpawn { id, url, kind, parent, group, pos, vel, purpose, carry, fallback_kind },
            generation,
            result,
        } = outcome;
        let TabCarry { pinned, color, zoom_factor, title, muted } = carry;
        self.spawning.remove(&id);
        let remote = self.remote_in_flight.remove(&id);
        self.tab_spawns_in_flight = self.tab_spawns_in_flight.saturating_sub(1);
        if self.tab_spawns_in_flight == 0 {
            for waiter in self.tabs_spawned_waiters.drain(..) {
                let _ = waiter.send(());
            }
        }
        // Spawned for a session that's since been replaced: dropping the
        // `EngineTab` shuts its engine host down. See `session_generation`.
        if generation != self.session_generation {
            return;
        }
        // A group dissolved while this spawn was still in flight (spawning
        // is async, see this fn's own doc comment) would otherwise leave
        // the new tab pointing at a group id nothing can navigate into —
        // same guard `reopen_closed`'s relinking applies for the same
        // reason, centralized here since every spawn path funnels through
        // this one function.
        let (parent, group, pos, color, pinned, muted) = if self.wakes_in_flight.remove(&id) {
            match self.nodes.get(&id).and_then(|n| n.as_unloaded().map(|tab| (n, tab))) {
                Some((card, tab)) => {
                    (card.parent.clone(), card.group.clone(), card.pos, card.color, tab.pinned, tab.muted)
                }
                // Closed while waking (see `wakes_in_flight`): `spawning`
                // no longer holds this id (removed above), so the save
                // actually deletes it -- without this, a save taken while
                // the wake was still in flight had kept it (`spawning`),
                // and nothing else ever saves it again.
                None => {
                    self.save_session();
                    return;
                }
            }
        } else {
            (parent, group, pos, color, pinned, muted)
        };
        let group = group.filter(|g| self.nodes.contains_key(g));
        if matches!(purpose, SpawnPurpose::Respawn | SpawnPurpose::Restore | SpawnPurpose::Wake)
            && self.pending_respawn_focus.as_deref() == Some(id.as_str())
        {
            self.pending_respawn_focus = None;
        }
        match result {
            Ok(mut tab) => {
                let is_focused = match purpose {
                    SpawnPurpose::Root => true,
                    SpawnPurpose::New | SpawnPurpose::Split => false,
                    SpawnPurpose::Respawn | SpawnPurpose::Restore | SpawnPurpose::Wake => {
                        self.focused.as_deref() == Some(id.as_str())
                    }
                };
                // A split's secondary pane is always live even though it's
                // never `focused` -- true for a fresh `Split` spawn, and
                // also (the `self.split_tab` check) for a crash-respawn of
                // whatever tab currently happens to be playing that role,
                // which arrives as an ordinary `Respawn`, not `Split` --
                // see `SpawnPurpose::Split`'s own doc comment.
                let is_split = matches!(purpose, SpawnPurpose::Split) || self.split_tab.as_deref() == Some(id.as_str());
                // The focused tab stays hidden while Image view is showing
                // in front of it -- e.g. relaunching straight into Image
                // view, or a crash respawn of the tab Image view is
                // currently covering (see `spawned_tab_visible`'s doc).
                let visible = spawned_tab_visible(is_focused, is_split, self.showing_image_view());
                tab.set_visible(visible);
                if muted {
                    tab.send(&HostToEngine::SetMuted { muted: true });
                }
                self.nodes.insert(
                    id.clone(),
                    GraphNode {
                        parent,
                        group,
                        pos,
                        vel,
                        color,
                        payload: NodePayload::Tab(TabPayload {
                            tab,
                            url: url.clone(),
                            title,
                            can_go_back: false,
                            can_go_forward: false,
                            loading: false,
                            audio_playing: false,
                            muted,
                            pinned,
                            zoom_factor,
                            swap_target: None,
                        }),
                    },
                );
                if !self.node_order.iter().any(|t| t == &id) {
                    self.node_order.push(id.clone());
                }
                self.apply_pending_local(&id);
                match purpose {
                    SpawnPurpose::Root => {
                        self.focused = Some(id.clone());
                        self.view_mode = ViewMode::Graph;
                        self.address_bar.text = url;
                        self.send_resize_to(&id);
                        self.send_zoom_to(&id);
                    }
                    SpawnPurpose::New => {
                        self.send_zoom_to(&id);
                    }
                    SpawnPurpose::Split => {
                        self.split_tab = Some(id.clone());
                        self.send_zoom_to(&id);
                        self.send_resize_to(&id);
                        // The primary pane was rendering at full width until
                        // now -- it needs to shrink to half now that
                        // `content_size_px` sees `split_tab` as set.
                        if let Some(focused_id) = self.focused.clone() {
                            self.send_resize_to(&focused_id);
                        }
                    }
                    SpawnPurpose::Respawn | SpawnPurpose::Wake => {
                        // A wake is a host-started load, same as a restore
                        // (`note_host_load`); a plain respawn (crash
                        // recovery, engine swap) needs no such marking --
                        // it isn't a url change at all.
                        if matches!(purpose, SpawnPurpose::Wake) {
                            self.note_host_load(&id);
                        }
                        // A respawned tab is a fresh engine-host process
                        // that starts at that engine's own default zoom --
                        // unlike Root/New/Split/Restore above, this arm
                        // never told it about `zoom_factor` at all, so a
                        // crash-recovery or engine-swap respawn silently
                        // reset the page's zoom while Settings kept
                        // showing the configured value.
                        self.send_zoom_to(&id);
                        if is_focused {
                            if !self.address_bar.focused {
                                self.address_bar.text = url;
                            }
                            // Resizing only matters for what's actually
                            // shown -- skip it while Image view hides this
                            // tab, same as `visible` above.
                            if visible {
                                self.send_resize_to(&id);
                            }
                        } else if is_split {
                            self.send_resize_to(&id);
                        }
                    }
                    SpawnPurpose::Restore => {
                        self.note_host_load(&id);
                        self.send_zoom_to(&id);
                        if is_focused {
                            self.address_bar.text = url;
                            if visible {
                                self.send_resize_to(&id);
                            }
                        }
                    }
                }
                self.land_remote_in_flight(&id, remote);
                self.save_session();
            }
            Err(err) => {
                eprintln!("spawn tab failed for {id}: {err}");
                if let Some(fallback) = fallback_kind {
                    self.queue_toast(format!(
                        "Couldn't switch to {}: {err}. Kept {}.",
                        engine_label(kind),
                        engine_label(fallback)
                    ));
                    let carry = TabCarry { pinned, color, zoom_factor, title, muted };
                    let spawn = TabSpawn {
                        id,
                        url,
                        kind: fallback,
                        parent,
                        group,
                        pos,
                        vel,
                        purpose,
                        carry,
                        fallback_kind: None,
                    };
                    if let Some(remote) = remote {
                        self.remote_in_flight.insert(spawn.id.clone(), remote);
                    }
                    self.respawn_tab(spawn);
                    return;
                }
                // A tab that already existed (a crash restart, a restored
                // session or group, a retry) keeps its place as a failed
                // card rather than disappearing; see `UnloadedTabPayload`.
                if matches!(purpose, SpawnPurpose::Respawn | SpawnPurpose::Restore | SpawnPurpose::Wake) {
                    self.queue_toast(format!("Couldn't start {} for {url}: {err}", engine_label(kind)));
                    let state = UnloadedState::Failed(err.to_string());
                    let failed = UnloadedTabPayload { url, title, kind, pinned, muted, zoom_factor, state };
                    let payload = NodePayload::Unloaded(failed);
                    self.nodes.insert(id.clone(), GraphNode { parent, group, pos, vel, color, payload });
                    if !self.node_order.iter().any(|t| t == &id) {
                        self.node_order.push(id.clone());
                    }
                    self.apply_pending_local(&id);
                    self.land_remote_in_flight(&id, remote);
                    self.save_session();
                    return;
                }
                self.tab_spawn_error = Some(match purpose {
                    SpawnPurpose::Root => format!("Couldn't start a browser tab: {err}"),
                    SpawnPurpose::New => format!("Couldn't start a new tab: {err}"),
                    SpawnPurpose::Split => format!("Couldn't start split view: {err}"),
                    SpawnPurpose::Respawn => format!("Couldn't restart tab: {err}"),
                    SpawnPurpose::Restore => format!("Couldn't restore tab: {err}"),
                    // Unreachable in practice: a wake's failure always
                    // takes the failed-card branch above (the tab already
                    // exists), but the match must stay exhaustive.
                    SpawnPurpose::Wake => format!("Couldn't load tab: {err}"),
                });
                // A save while it was in flight may have created it
                // (`spawning`); it's gone now.
                self.save_session();
            }
        }
    }

    /// Graph view's "+ New Tab" action: a child of `parent` (or a new root
    /// if `parent` is `None`), spawned invisible — it stays in Graph view
    /// rather than auto-focusing (see plan's M5 scope decision 5), and
    /// background tabs render nothing until focused anyway. Also used for
    /// a page-triggered new tab (`EngineToHost::OpenTabRequested`), with
    /// that request's URL instead of the configured home page.
    /// Tab context menu's "Duplicate": a new tab at `id`'s current url and
    /// engine, sharing its parent and group -- a sibling, not a child of
    /// `id` itself, matching what "duplicate" means in a real browser's
    /// own tab strip.
    pub(crate) fn duplicate_tab(&mut self, id: &str) {
        let Some((parent, group, url, kind)) = self.nodes.get(id).and_then(|n| {
            n.payload.saved_tab().map(|(url, kind)| (n.parent.clone(), n.group.clone(), url.to_string(), kind))
        }) else {
            return;
        };
        self.spawn_new_tab_with_kind(parent, group, &url, kind, SpawnPurpose::New);
    }

    pub(crate) fn spawn_new_tab(&mut self, parent: Option<String>, group: Option<String>, url: &str) {
        self.spawn_new_tab_with_kind(parent, group, url, self.settings.default_starting_engine, SpawnPurpose::New);
    }

    /// Shared by `spawn_new_tab` (always `default_starting_engine`),
    /// `duplicate_tab` (the original tab's own engine), and `start_split`
    /// (`SpawnPurpose::Split` instead of `New`, everything else identical)
    /// -- `purpose` is threaded through to `spawn_tab_async` rather than
    /// hardcoded, since those cases genuinely differ in what should happen once the
    /// spawn completes (see `handle_tab_spawn_result`'s own per-purpose
    /// match). Returns the new tab's id, allocated synchronously even
    /// though the real engine-host process spawn (and thus the node's own
    /// insertion into `self.nodes`) completes asynchronously -- see
    /// `spawn_tab_async`'s own doc comment.
    pub(crate) fn spawn_new_tab_with_kind(
        &mut self,
        parent: Option<String>,
        group: Option<String>,
        url: &str,
        kind: EngineKind,
        purpose: SpawnPurpose,
    ) -> String {
        let id = self.alloc_node_id();
        // `parent`'s own position only if it actually shares `group` --
        // the toolbar's "+ New Tab" passes `self.current_group` as `group`
        // but `self.focused` (unconditionally) as `parent`, and those can
        // point at different levels (see `spawn_new_tab`'s own doc
        // comment). Falling back to any existing member of `group` keeps
        // the new tab near its actual new groupmates instead of near an
        // unrelated tab at a different level; `ZERO` (same as a fresh
        // group's own spawn point) covers a brand new, still-empty group.
        let parent_pos = parent
            .as_ref()
            .and_then(|p| self.nodes.get(p))
            .filter(|n| n.group == group)
            .map(|n| n.pos)
            .or_else(|| self.nodes.values().find(|n| n.group == group).map(|n| n.pos))
            .unwrap_or(egui::Vec2::ZERO);
        let pos = parent_pos + graph_layout::spawn_jitter(&id);
        self.tab_spawn_error = None;
        let spawn = TabSpawn::new(id.clone(), url.to_string(), kind, pos, purpose, self.settings.zoom_factor);
        self.spawn_tab_async(TabSpawn { parent, group, ..spawn });
        id
    }

    /// Stops the focused tab rendering (and drops its find bar, context
    /// menu, and split) -- what switching the maximized node away from it
    /// needs, whether to another tab (`focus_tab`) or an Image
    /// (`focus_item`). `focused` itself is left alone.
    pub(crate) fn hide_focused_tab(&mut self) {
        self.close_find_bar();
        self.context_menu = None;
        self.close_split();
        if let Some(old_id) = self.focused.clone() {
            if let Some(old) = self.nodes.get_mut(&old_id).and_then(|n| n.as_tab_mut()) {
                old.tab.set_visible(false);
            }
            if let Some(gpu) = &mut self.gpu {
                gpu.free_tab_texture(&old_id);
            }
        }
    }

    /// Peek-click commits focus: the tab losing focus (if any, and if
    /// different) is told to stop rendering, the newly focused one is told
    /// to start, and the view switches to Tab. At most one tab is ever the
    /// primary/focused one at a time -- a split's own secondary pane
    /// (`split_tab`) is the one exception, also visible but never focused;
    /// switching primary focus to a genuinely different tab always closes
    /// whatever split was open, since a split is only ever meaningful
    /// paired with the specific tab it was opened against. An unloaded tab
    /// (`UnloadedTabPayload`) is woken.
    pub(crate) fn focus_tab(&mut self, id: &str) {
        if self.focused.as_deref() != Some(id) {
            self.hide_focused_tab();
        }
        if let Some(node) = self.nodes.get_mut(id).and_then(|n| n.as_tab_mut()) {
            node.tab.set_visible(true);
            if !self.address_bar.focused {
                self.address_bar.text = node.url.clone();
            }
        }
        self.focused = Some(id.to_string());
        self.viewed_image = None;
        self.view_mode = ViewMode::Item;
        // An unloaded tab opens by starting its engine; Tab view shows the
        // respawning placeholder until it lands (`pending_respawn_focus`).
        if let Some(kind) = self.nodes.get(id).and_then(|n| n.as_unloaded()).map(|t| t.kind) {
            self.pending_respawn_focus = Some(id.to_string());
            self.wake_tab(id, kind);
        }
        self.send_resize_to(id);
        self.save_session();
    }

    /// Maximizes `id` into Item view, whatever its kind -- a Tab goes
    /// through `focus_tab`; an Image becomes `viewed_image` (the last tab
    /// stays `focused`, hidden, exactly as in Graph view). Kinds with no
    /// Item view are a no-op (and a debug-build assertion).
    pub(crate) fn focus_item(&mut self, id: &str) {
        match self.nodes.get(id).map(|n| item_node_kind(&n.payload)) {
            Some(ItemNodeKind::Tab | ItemNodeKind::Unloaded) => self.focus_tab(id),
            Some(ItemNodeKind::Image) => {
                self.hide_focused_tab();
                self.viewed_image = Some(id.to_string());
                self.image_view = ImageViewState::default();
                self.view_mode = ViewMode::Item;
                self.save_session();
            }
            Some(kind) => debug_assert!(false, "focus_item: {kind:?} nodes have no Item view"),
            None => {}
        }
    }

    /// Opens (or replaces) split view: the Tab-view toolbar's "Split"
    /// picker -- spawns a new tab at the focused tab's own current url on
    /// `kind`, parented to it and sharing its group, shown as the secondary
    /// pane once it finishes spawning (`SpawnPurpose::Split` -- see
    /// `handle_tab_spawn_result`). A no-op if there's no focused tab to
    /// compare against. Replacing an already-open split just spawns a
    /// fresh secondary tab and overwrites `split_tab` once *that* finishes
    /// spawning -- the old secondary tab is left exactly as `close_split`
    /// would leave it (unpaired, but not closed), since a real tab node no
    /// longer being shown in a split isn't otherwise different from any
    /// other background tab.
    pub(crate) fn start_split(&mut self, kind: EngineKind) {
        let Some(focused_id) = self.focused.clone() else { return };
        let Some(url) = self.nodes.get(&focused_id).and_then(|n| n.as_tab()).map(|t| t.url.clone()) else {
            return;
        };
        let group = self.group_of(&Some(focused_id.clone()));
        self.spawn_new_tab_with_kind(Some(focused_id), group, &url, kind, SpawnPurpose::Split);
    }

    /// Closes split view -- unpairs the secondary pane and stops its
    /// rendering/GPU texture, but doesn't close the tab itself: it remains
    /// a normal graph node, reachable and closable from Graph view like any
    /// other tab. A no-op if no split is open.
    pub(crate) fn close_split(&mut self) {
        let Some(id) = self.split_tab.take() else { return };
        if let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
            node.tab.set_visible(false);
        }
        if let Some(gpu) = &mut self.gpu {
            gpu.free_tab_texture(&id);
        }
        // The primary pane was rendering at half width for the split --
        // needs to grow back to the full content width now that
        // `content_size_px` no longer sees `split_tab` as set.
        if let Some(focused_id) = self.focused.clone() {
            self.send_resize_to(&focused_id);
        }
    }

    /// Leaves Item view for Graph view -- starts a shrink animation (see
    /// `Transition`) toward whichever node is shown, rather than switching
    /// instantly. For a Tab (`viewed_image` is `None`), the tab keeps
    /// rendering (`set_visible(true)`, unchanged) for the animation's whole
    /// duration, so the thumbnail it shrinks into shows real, current
    /// content instead of whatever frame happened to be on screen when the
    /// animation started, only actually suspended once the animation
    /// finishes (`finish_transition_if_done`). For an Image, the texture is
    /// already there either way, so there's nothing equivalent to suspend.
    pub(crate) fn leave_to_graph(&mut self) {
        self.context_menu = None;
        match self.viewed_image.clone().or_else(|| self.focused.clone()) {
            Some(id) => {
                self.transition =
                    Some(Transition { node_id: id, kind: TransitionKind::ToGraph, start: std::time::Instant::now() });
            }
            // No focused tab and no viewed image (shouldn't normally
            // happen — Item view always has one — but nothing to animate
            // either way).
            None => {
                self.view_mode = ViewMode::Graph;
                self.save_session();
            }
        }
    }

    /// Closes a tab: tears down its process (via `EngineTab`'s `Drop` —
    /// `Shutdown` + kill), frees its graph-view thumbnail, and reparents
    /// its children to its own parent so the forest stays connected rather
    /// than orphaning that whole subtree. Falls back to Graph view if the
    /// closed tab was focused. Records nothing: `close_node` is the close
    /// that feeds "Recently Closed".
    pub(crate) fn destroy_tab(&mut self, id: &str) {
        self.remove_tab(id);
        self.save_session();
    }

    /// `destroy_tab` without the save: a tab another replica deleted
    /// (`remove_remotely_deleted`), whose removal is not a local edit, or
    /// a tab converting into a node (`redirect_tab_to_content`), which
    /// saves once after inserting it.
    pub(crate) fn remove_tab(&mut self, id: &str) {
        let Some(parent) = self.nodes.get(id).filter(|n| n.payload.saved_tab().is_some()).map(|n| n.parent.clone())
        else {
            return;
        };
        for node in self.nodes.values_mut() {
            if node.parent.as_deref() == Some(id) {
                node.parent = parent.clone();
            }
        }
        self.nodes.remove(id);
        self.node_order.retain(|t| t != id);
        self.skip_convert.remove(id);
        // A rectangle-selected tab closed through any path other than
        // "Close Selected" itself (its own card close button, or its
        // containing group being closed) otherwise leaves a stale id in
        // `selected_tabs` -- harmless to the no-op `destroy_tab`/`nodes.get`
        // callers that already tolerate a missing id, but it skews
        // "Group Selected"'s centroid (divides by a count that includes
        // dead ids) and overcounts the toolbar's own "(N)" labels.
        self.selected_tabs.remove(id);
        if self.pending_respawn_focus.as_deref() == Some(id) {
            self.pending_respawn_focus = None;
        }
        if let Some(gpu) = &mut self.gpu {
            gpu.free_tab_thumbnail(id);
            gpu.free_tab_texture(id);
        }
        if self.dragging.as_deref() == Some(id) {
            self.dragging = None;
        }
        if self.hovered.as_deref() == Some(id) {
            self.hovered = None;
        }
        // A split only ever makes sense paired with a specific focused tab
        // (see `focus_tab`'s own doc comment) -- closing either half of it
        // (the tab node is already gone either way, so no `set_visible`/
        // resize cleanup to do the way `close_split` normally would) clears
        // the whole thing rather than leaving a dangling reference to a
        // node that no longer exists.
        if self.split_tab.as_deref() == Some(id) || self.focused.as_deref() == Some(id) {
            self.split_tab = None;
        }
        if self.focused.as_deref() == Some(id) {
            self.focused = None;
            // Only drop to Graph if a Tab was actually showing -- while an
            // Image is maximized, `focused` is just the hidden last tab
            // (see the Image Item View spec's "Plan-time corrections"), so
            // closing it should leave Image view showing exactly as
            // closing it from Graph view wouldn't change `view_mode`
            // either.
            if self.showing_tab_view() {
                self.view_mode = ViewMode::Graph;
            }
            // The tab (and its `EngineTab`) is already gone — nothing left
            // to send `FindStop` to, so only the UI side of the close.
            self.find.close();
        }
    }

    /// Shared by swap-finish and crash recovery: starts `spawn` (from
    /// `TabSpawn::respawn`, so the tab keeps its place in the graph and its
    /// `TabCarry`) and, if it's the focused tab, shows a spinner until it's
    /// back (only the focused tab is ever visible).
    pub(crate) fn respawn_tab(&mut self, spawn: TabSpawn) {
        if self.focused.as_deref() == Some(spawn.id.as_str()) {
            self.pending_respawn_focus = Some(spawn.id.clone());
        }
        self.spawn_tab_async(spawn);
    }

    /// A new tab at `url`, placed at `pos` in `group`. Returns its id.
    pub(crate) fn spawn_tab_at(&mut self, url: &str, pos: egui::Vec2, group: Option<String>) -> String {
        let id = self.alloc_node_id();
        let (kind, zoom) = (self.settings.default_starting_engine, self.settings.zoom_factor);
        // Cleared at the start of this spawn attempt too -- see
        // `tab_spawn_error`'s own doc comment.
        self.tab_spawn_error = None;
        let spawn = TabSpawn::new(id.clone(), url.to_string(), kind, pos, SpawnPurpose::New, zoom);
        self.spawn_tab_async(TabSpawn { group, ..spawn });
        id
    }

    /// `Ctrl+Tab`/`Ctrl+Shift+Tab`: focuses the next/previous tab in
    /// `tab_order` (insertion order — this graph-based UI has no tab strip
    /// to define a more meaningful order), wrapping at both ends. A no-op
    /// with zero or one tab open.
    pub(crate) fn cycle_focused_tab(&mut self, forward: bool) {
        // `node_order` holds every graph node (tabs, notes, images, PDFs,
        // JSON files), not just tabs -- filter to live and unloaded tabs
        // first (same filter `quick_switcher_matches` applies) so this
        // never lands on a non-tab node. Landing on an unloaded tab is
        // fine: `focus_tab` wakes it, and `render_tab_toolbar_content`
        // shows the respawning placeholder until it's live rather than
        // assuming `as_tab().unwrap()`.
        let tab_ids: Vec<&String> = self
            .node_order
            .iter()
            .filter(|id| self.nodes.get(id.as_str()).and_then(|n| n.payload.listed_tab()).is_some())
            .collect();
        if tab_ids.len() < 2 {
            return;
        }
        let current = self.focused.as_ref().and_then(|id| tab_ids.iter().position(|t| *t == id));
        let len = tab_ids.len() as isize;
        let next = match current {
            Some(i) => {
                let delta = if forward { 1 } else { -1 };
                (i as isize + delta).rem_euclid(len) as usize
            }
            None => 0,
        };
        let id = tab_ids[next].clone();
        self.focus_tab(&id);
    }
}
