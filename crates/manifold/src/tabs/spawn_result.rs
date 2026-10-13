//! Applying a finished tab spawn (`spawn_tab_async`) to App: the new node, a failed card, or a toast, per `SpawnPurpose`.

use super::*;

impl App {
    /// Applies a finished `spawn_tab_async` call — inserts the new tab node
    /// (or leaves a failed card, or toasts for a split), with per-`SpawnPurpose` follow-up
    /// matching what `spawn_root_tab`/`spawn_new_tab`/`respawn_tab` used to
    /// do synchronously right after `process_manager::spawn_tab` returned.
    /// Focus/visibility for `Respawn` is decided here, at completion time —
    /// same moment the old inline code decided it, just later in wall time.
    pub(crate) fn handle_tab_spawn_result(&mut self, outcome: TabSpawnOutcome) {
        let TabSpawnOutcome {
            spawn: TabSpawn { id, url, kind, parent, group, jar: spawn_jar, pos, purpose, carry, fallback_kind },
            generation,
            result,
            seeded,
            jar,
            seed_clears,
        } = outcome;
        let TabCarry { pinned, color, zoom_factor, title, muted } = carry;
        self.spawns.spawning.remove(&id);
        let remote = self.spawns.remote_in_flight.remove(&id);
        self.spawns.tab_spawns_in_flight = self.spawns.tab_spawns_in_flight.saturating_sub(1);
        if self.spawns.tab_spawns_in_flight == 0 {
            for waiter in self.spawns.tabs_spawned_waiters.drain(..) {
                let _ = waiter.send(());
            }
        }
        // Spawned for a session that's since been replaced: dropping the
        // `ViewTab` shuts its engine host down. See `session_generation`.
        if generation != self.spawns.session_generation {
            return;
        }
        // A replaced engine (crash, wake, swap) knows only `url`: from here
        // Back and Forward come from the host's list.
        if let Some(history) = self.nav_history.get_mut(&id) {
            history.view_reset(&url);
        }
        // A group dissolved while this spawn was still in flight (spawning
        // is async, see this fn's own doc comment) would otherwise leave
        // the new tab pointing at a group id nothing can navigate into —
        // same guard `reopen_closed`'s relinking applies for the same
        // reason, centralized here since every spawn path funnels through
        // this one function.
        let (parent, group, color, pinned, muted) = if self.spawns.wakes_in_flight.remove(&id) {
            match self.nodes.get(&id).and_then(|n| n.as_unloaded()) {
                Some(tab) => {
                    let Carried { color, pinned, parent, group, .. } = self.mirror.carried(&id);
                    (parent, group, color, pinned, tab.muted)
                }
                // Closed while waking (see `wakes_in_flight`): `remove_tab`
                // queued its delete, which this save commits -- without
                // this, nothing else ever saves it again.
                None => {
                    self.save_session();
                    return;
                }
            }
        } else {
            (parent, group, color, pinned, muted)
        };
        if matches!(purpose, SpawnPurpose::Respawn | SpawnPurpose::Restore | SpawnPurpose::Wake)
            && self.spawns.pending_respawn_focus.as_deref() == Some(id.as_str())
        {
            self.spawns.pending_respawn_focus = None;
        }
        match result {
            Ok(tab) => {
                self.note_spawned_jar(&id, jar, seeded, seed_clears);
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
                    tab.send(&HostToView::SetMuted { muted: true });
                }
                self.nodes.insert(
                    id.clone(),
                    GraphNode {
                        runtime: Runtime::Tab(TabPayload {
                            tab,
                            url: url.clone(),
                            title,
                            can_go_back: false,
                            can_go_forward: false,
                            loading: false,
                            audio_playing: false,
                            muted,
                            zoom_factor,
                            swap_target: None,
                        }),
                    },
                );
                let view_kind = session_sync::view_code(kind);
                let pos = Some(pos);
                self.seed_landing(
                    &id,
                    Carried {
                        pos,
                        color,
                        pinned,
                        parent,
                        group,
                        url: url.clone(),
                        view_kind,
                        jar: spawn_jar,
                        ..Default::default()
                    },
                );
                if !self.node_order.iter().any(|t| t == &id) {
                    self.node_order.push(id.clone());
                }
                self.apply_pending_local(&id);
                // A fresh attempt at a url resolves its earlier failed attempts; landing an existing tab (respawn,
                // restore, wake) is no new intent and leaves a failed card the user placed alone.
                if matches!(purpose, SpawnPurpose::Root | SpawnPurpose::New) {
                    self.clear_failed_cards(&url, &id);
                }
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
                        // A respawned tab is a fresh view-host process
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
                    // The switch failed: the old engine is written back (2b-3 Ruling 5).
                    self.edit_view_kind(&id, fallback);
                    self.queue_toast(format!(
                        "Couldn't switch to {}: {err}. Kept {}.",
                        view_label(kind),
                        view_label(fallback)
                    ));
                    let carry = TabCarry { pinned, color, zoom_factor, title, muted };
                    let spawn = TabSpawn {
                        id,
                        url,
                        kind: fallback,
                        parent,
                        group,
                        jar: spawn_jar,
                        pos,
                        purpose,
                        carry,
                        fallback_kind: None,
                    };
                    if let Some(remote) = remote {
                        self.spawns.remote_in_flight.insert(spawn.id.clone(), remote);
                    }
                    self.respawn_tab(spawn);
                    return;
                }
                // A tab keeps its place as a failed card rather than disappearing (see `UnloadedTabPayload`):
                // one that already existed (a crash restart, a restored session or group, a retry) and one just
                // asked for (Root, New). A split's secondary pane is not a card (`App::split_tab`).
                if !matches!(purpose, SpawnPurpose::Split) {
                    // "Couldn't start Blink (CEF): Blink (CEF) is not installed" says it twice.
                    let toast = match err {
                        process_manager::SpawnError::ViewMissing(_)
                        | process_manager::SpawnError::ViewOutdated(_)
                        | process_manager::SpawnError::ViewUnavailable(..) => {
                            format!("Couldn't open {}: {err}", Self::toast_url(&url))
                        }
                        _ => format!("Couldn't start {} for {}: {err}", view_label(kind), Self::toast_url(&url)),
                    };
                    self.queue_failure_toast(kind, &url, group.as_deref(), &err.to_string(), toast);
                    // The same intent failing again refreshes the card already standing for it.
                    if matches!(purpose, SpawnPurpose::Root | SpawnPurpose::New) {
                        if let Some(twin) = self.failed_twin(&id, &url, kind, group.as_deref()) {
                            if let Some(Runtime::Unloaded(tab)) =
                                self.nodes.get_mut(&twin).map(|node| &mut node.runtime)
                            {
                                tab.state = UnloadedState::Failed(err.to_string());
                            }
                            if matches!(purpose, SpawnPurpose::Root) {
                                self.view_mode = ViewMode::Graph;
                            }
                            self.queue_delete(&id);
                            self.save_session();
                            return;
                        }
                    }
                    // A retry (or restart) that fails beside a card already failed for its intent, which a Root/New
                    // failure made while the retry was waking, keeps the retried card (its id is the one in use) and
                    // drops the other: one card per intent, with this error.
                    if matches!(purpose, SpawnPurpose::Respawn | SpawnPurpose::Wake) {
                        if let Some(twin) = self.failed_twin(&id, &url, kind, group.as_deref()) {
                            self.record_closed(&twin);
                            self.remove_tab(&twin);
                        }
                    }
                    let state = UnloadedState::Failed(err.to_string());
                    let failed = UnloadedTabPayload { url: url.clone(), title, muted, zoom_factor, state };
                    let runtime = Runtime::Unloaded(failed);
                    self.nodes.insert(id.clone(), GraphNode { runtime });
                    // A tab the mirror holds already shows the engine that failed: a switch's target, written when
                    // it started (`edit_view_kind`); a new one lands on it here.
                    let view_kind = session_sync::view_code(kind);
                    let (pos, jar) = (Some(pos), spawn_jar);
                    self.seed_landing(
                        &id,
                        Carried { pos, color, pinned, parent, group, url, view_kind, jar, ..Default::default() },
                    );
                    if !self.node_order.iter().any(|t| t == &id) {
                        self.node_order.push(id.clone());
                    }
                    self.apply_pending_local(&id);
                    if matches!(purpose, SpawnPurpose::Root) {
                        // Success switches to Graph view; the failed card must be seen too.
                        self.view_mode = ViewMode::Graph;
                    }
                    self.land_remote_in_flight(&id, remote);
                    self.save_session();
                    return;
                }
                self.queue_toast(format!("Couldn't start split view: {err}"));
                // A save while it was in flight may have created it
                // (`spawning`); it's gone now (Ruling 4). One never
                // created is just forgotten.
                self.queue_delete(&id);
                self.save_session();
            }
        }
    }

    /// A tab for `url` landed (`landed`): failed cards for the same url in the landing tab's group (any engine),
    /// other than `landed`, are resolved. Each is recorded in Recently Closed first (it may be an arranged
    /// tab's card), then removed without a save: the landing's own save commits the delete, after the parked remote changes
    /// land. Another url's card, or the same url's card in another group, stays.
    fn clear_failed_cards(&mut self, url: &str, landed: &str) {
        let group = self.mirror.group(landed);
        let stale: Vec<String> = self
            .nodes
            .iter()
            .filter(|(id, node)| {
                id.as_str() != landed
                    && self.mirror.group(id) == group
                    && node
                        .as_unloaded()
                        .is_some_and(|tab| matches!(tab.state, UnloadedState::Failed(_)) && tab.url == url)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in stale {
            self.record_closed(&id);
            self.remove_tab(&id);
        }
    }

    /// Seeds a landing tab's mirror entry (`Mirror::seed`: only an id with none) and its layout entry, both at
    /// `carried.pos` (a new tab's spawn position, 2b-4 Ruling 3). A tab mid-respawn or waking keeps both: its layout
    /// entry is keyed by id, so the respawn's remove-and-reinsert leaves it where it was drawn (Ruling 1).
    ///
    /// A tab mid-respawn keeps its mirror entry
    /// and the link cascades reach it (`App::linked_ids`), but a new spawn has none until here: one whose group was
    /// removed while it was in flight would land in a group nothing can navigate into. It lands where the document
    /// shows it instead, in the mirror only (`follow_remote_group`): the removal was a cascade, not a move of this
    /// tab, and a queued move would override another replica's placement of it (final review I1).
    ///
    /// Likewise a jar deleted while a new spawn was in flight (`delete_jar` cannot reach a tab with no entry): the
    /// tab lands on the default jar, as the delete would have put it, and the document is told so whether its
    /// `Create` already named the jar or resolved it to none (2b-3 final review Minor 1).
    fn seed_landing(&mut self, id: &str, carried: Carried) {
        let ty = crate::store::model::Kind::Tab.type_str().unwrap_or_default().to_owned();
        let carried = Carried { ty, ..carried };
        self.layout.entry(id.to_owned()).or_insert(Layout::at(carried.pos.unwrap_or_default()));
        let new = !self.mirror.has(id);
        self.mirror.seed(id, &carried);
        if new && self.mirror.jar_choice(id).jar.is_some_and(|jar| !self.mirror.jar_defs().contains_key(&jar)) {
            self.edit_jar(id, crate::jar::JarChoice::default());
        }
        if self.mirror.group(id).is_some_and(|g| !self.nodes.contains_key(g)) {
            let group = self.shown_group(id);
            self.follow_remote_group(id, group);
        }
    }

    /// The nearest group above `id` in the document that is here: `Doc::effective_group`, except that a group this
    /// replica removed and hasn't saved yet is passed over too, as the save that deletes it will. `None` (the top
    /// level) for a node the document doesn't hold yet.
    fn shown_group(&self, id: &str) -> Option<String> {
        let doc = self.journal.as_ref()?.doc();
        let mut group = doc.nodes.get(&id.parse::<crate::store::model::NodeId>().ok()?)?.group;
        for _ in 0..=doc.nodes.len() {
            let at = group?;
            if self.nodes.contains_key(&at.to_string()) {
                return Some(at.to_string());
            }
            group = doc.nodes.get(&at)?.group;
        }
        None
    }
}
