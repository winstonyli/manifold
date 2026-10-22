//! A jar's lifecycle on `App`: the Default jar, create, ephemeral, edge scope, load, save and clear.

use super::*;

impl App {
    /// The Default jar: the lowest-id live jar with `IS_DEFAULT` (`ensure_default_jar` leaves exactly one). Cheap
    /// enough per card per frame: the mirror's index holds one id, two for a moment after two replicas' first
    /// launches; shown jars only, a closed or hidden one has left the mirror. Each costs a hash lookup.
    pub(crate) fn default_jar(&self) -> Option<NodeId> {
        self.doc.mirror.default_jar_ids().find(|&id| self.jar_exists(id))
    }

    /// Whether node `id` is the Default jar.
    pub(crate) fn is_default_jar(&self, id: NodeId) -> bool {
        // A frame asks per card: only a card that could be the Default gets as far as `default_jar`.
        self.doc.mirror.default_jar_ids().any(|d| d == id) && self.default_jar() == Some(id)
    }

    /// The startup and sync invariant (spec "The Default jar"): with no Default, one is made at the top level; with
    /// several (two replicas' concurrent first launches), the lowest id keeps the flag and each other is cleared and
    /// renamed, a deterministic repair every replica makes alike. Returns whether it wrote anything; the caller
    /// saves. A Default inside a group moves to the top level.
    pub(crate) fn ensure_default_jar(&mut self) -> bool {
        // A Default never sits in a group, hidden ones included (spec decision 10): one that arrived inside a group
        // moves out. One this replica shows moves now; one hidden under a closed group is queued, and the save's
        // visibility pass shows it, so no second Default is made meanwhile.
        let grouped: Vec<(NodeId, bool)> = self
            .doc
            .journal
            .as_ref()
            .map(|j| {
                let doc = j.doc();
                doc.live()
                    .filter(|&(id, n)| n.kind() == Kind::Jar && doc.effective_group(id).is_some())
                    .filter(|(_, n)| n.get(field::IS_DEFAULT) == FieldValue::Bool(true))
                    .map(|(id, _)| (id, self.nodes.contains_key(&id)))
                    .collect()
            })
            .unwrap_or_default();
        let (mut queued_hidden, mut moved_here) = (false, false);
        for (id, here) in grouped {
            if here {
                self.edit_group(id, None);
                moved_here = true;
            } else {
                self.doc.writes.queue_move(id, None);
                queued_hidden = true;
            }
        }
        if queued_hidden {
            return true;
        }
        let defaults: Vec<NodeId> = self.doc.mirror.default_jar_ids().filter(|&id| self.jar_exists(id)).collect();
        let Some((_, losers)) = defaults.split_first() else {
            let pos = crate::graph_layout::GRAPH_WORLD_CENTER + crate::graph_layout::jitter_from_text(DEFAULT_NAME);
            let id = self.create_jar(DEFAULT_NAME, crate::fetched::Placement { group: None, pos });
            self.edit_field(id, field::IS_DEFAULT, FieldValue::Bool(true));
            return true;
        };
        for &loser in losers {
            self.edit_field(loser, field::IS_DEFAULT, FieldValue::Bool(false));
            self.edit_field(loser, field::NAME, FieldValue::Str(OLD_DEFAULT_NAME.to_owned()));
        }
        moved_here || !losers.is_empty()
    }

    /// A new named jar under `name`'s clean form (name rules), with the first palette colour no jar shows (one without
    /// a colour shows the first, `jar_color`), so deleting a jar doesn't make the next one repeat a colour; with every
    /// colour taken, they cycle by count. A card placed at `at`; its fields go through `edit_field`, and the caller's
    /// `save_session` creates it.
    pub(crate) fn create_jar(&mut self, name: &str, at: crate::fetched::Placement) -> NodeId {
        let defs = self.jar_defs();
        let shown = |c: &egui::Color32| defs.values().any(|d| d.color.unwrap_or(TAG_COLOR_PALETTE[0]) == *c);
        let free = TAG_COLOR_PALETTE.iter().find(|c| !shown(c));
        let color = *free.unwrap_or(&TAG_COLOR_PALETTE[defs.len() % TAG_COLOR_PALETTE.len()]);
        let key = self.place_node(None, Kind::Jar, Runtime::Jar, at);
        self.edit_field(key, field::NAME, FieldValue::Str(crate::names::clean_name(name).into_owned()));
        self.edit_field(key, field::COLOR, color_value(Some(color)));
        key
    }

    /// Where a jar made for `tab` goes: its level (group), just above it.
    pub(crate) fn jar_placement_beside(&self, tab: NodeId) -> crate::fetched::Placement {
        crate::fetched::Placement {
            group: self.doc.mirror.group(tab),
            pos: self.layout_pos(tab) + egui::vec2(0.0, -80.0),
        }
    }

    /// The jar whose label (`named_jar_label`) is `text`, else one whose stored name is: of several with that name, the
    /// lowest node id (`jar_defs` iterates in id order), so the pick is the same every time and on every device. Both
    /// compared exactly, after trimming `text`.
    pub(crate) fn jar_named(&self, text: &str) -> Option<NodeId> {
        let text = text.trim();
        let defs = self.jar_defs();
        let label = defs.keys().find(|&&id| named_jar_label(id, &defs).is_some_and(|l| l.to_string() == text));
        // name rule: SETJAR matches the stored name
        label.or_else(|| defs.iter().find(|(_, d)| d.name == text).map(|(id, _)| id)).copied()
    }

    /// `SETJAR`: the focused tab picks the jar `name` names (`jar_named`: a label or a stored name, tried as typed and
    /// then cleaned; "default", any case, for the default jar), made under the clean name first if none has it.
    /// Nothing happens, and no jar is made, when the focused node is not a tab or nothing is left of the name once
    /// cleaned (name rules). Returns whether it acted.
    pub(crate) fn set_focused_tab_jar_named(&mut self, name: &str) -> bool {
        let Some(tab) = self.focused.filter(|&t| self.is_tab(t)) else { return false };
        let clean = crate::names::clean_name(name).into_owned();
        if clean.is_empty() {
            return false;
        }
        let jar = if clean.eq_ignore_ascii_case("default") {
            self.default_jar()
        } else {
            let existing = self.jar_named(name).or_else(|| self.jar_named(&clean));
            Some(existing.unwrap_or_else(|| self.create_jar(&clean, self.jar_placement_beside(tab))))
        };
        self.set_jar_edge(tab, jar);
        true
    }

    /// Turns jar `id`'s ephemeral flag on or off (deviation 10; final review I1), a local toggle (after
    /// `request_jar_ephemeral`'s confirm): the flag goes through `edit_field`. See `switch_jar_ephemeral`.
    pub(crate) fn set_jar_ephemeral(&mut self, id: NodeId, on: bool) {
        self.switch_jar_ephemeral(id, on);
    }

    /// Jar `id`'s ephemeral flag has just changed to `on`, locally or from another replica (the mirror already
    /// holds it). Turning it on: the jar's cookies are kept in memory for the running tabs (loaded from its file
    /// first if they were not, whatever the flag now says), then the file is deleted unless the jar is locked (an
    /// unreadable file is never touched, and a locked jar saves nothing anyway). Turning it off: the jar's cookies
    /// are written to a new file (a locked one writes nothing).
    pub(crate) fn after_jar_ephemeral_changed(&mut self, id: NodeId, on: bool) {
        if on {
            if let (false, Some(dir)) = (self.jars.contains_key(&id), self.jars_dir()) {
                let jar = Jar::load(&dir, id);
                self.publish_jar(id, jar);
            }
            if let (Some(dir), Some(false)) = (self.jars_dir(), self.jars.get(&id).map(Jar::is_locked)) {
                if let Err(e) = Jar::delete_files(&dir, id) {
                    eprintln!("jar: could not delete {id}: {e}");
                }
            }
        } else {
            self.ensure_jar_loaded(id);
            self.save_jar(id);
        }
    }

    /// A local Make/Stop Ephemeral (`set_jar_ephemeral`, after any confirm): writes the flag, then its effects.
    fn switch_jar_ephemeral(&mut self, id: NodeId, on: bool) {
        if !self.jar_exists(id) || self.doc.mirror.jar_def(id).ephemeral == on {
            return;
        }
        self.edit_field(id, field::EPHEMERAL, FieldValue::Bool(on));
        self.after_jar_ephemeral_changed(id, on);
    }

    /// A jar card's Make Ephemeral. Turning it on for a jar with a saved file asks first (`JarConfirm::Ephemeral`):
    /// that file, its saved logins, is deleted. Anything else acts at once.
    pub(crate) fn request_jar_ephemeral(&mut self, id: NodeId, on: bool) {
        // A jar whose cookies are not on disk yet has logins to lose too: save them, so it asks first.
        if on && self.cookies.jar_dirty.contains_key(&id) {
            self.save_jar(id);
        }
        if on && self.has_saved_file(id) {
            self.offer_confirm(Confirm::Jar(JarConfirm::Ephemeral(id)));
        } else {
            self.set_jar_ephemeral(id, on);
        }
    }

    fn has_saved_file(&self, id: NodeId) -> bool {
        self.jars_dir().is_some_and(|dir| Jar::path(&dir, id).exists())
    }

    /// A removed jar's in-memory state: its cookies saved if dirty (a Recently Closed entry may bring it back, and
    /// its file is what comes back with it), then dropped. Files are not touched here: closing never wipes
    /// credentials (spec "Backing files"); the startup prune deletes a file nothing holds.
    pub(crate) fn forget_jar_memory(&mut self, id: NodeId) {
        if self.cookies.jar_dirty.contains_key(&id) {
            self.save_jar(id);
        }
        self.jars.remove(&id);
        if let Ok(mut snapshots) = self.cookies.jar_snapshots.lock() {
            snapshots.remove(&id);
        }
    }

    /// `profile::jars_dir()`, or a test's temp dir. Never the real profile under test (`profile::dir()` is None).
    pub(crate) fn jars_dir(&self) -> Option<std::path::PathBuf> {
        #[cfg(test)]
        if let Some(dir) = &self.jars_dir_override {
            return Some(dir.clone());
        }
        crate::profile::jars_dir()
    }

    /// The jar a source resolves to (spec "Resolution") and whether it dangles. `own` is the source's own edge and
    /// `origin` the level that edge reaches out from (`reach_origin`). The walk stops at the first edge it meets,
    /// from `own` outward through each enclosing group's edge (a group's edge reaching out from its own contents).
    /// A live jar in reach is the jar. A jar gone, or out of reach (an edge that arrived so from another device), is
    /// the Default with `true`: an explicit edge to nothing warns rather than climbing on. No edge anywhere: the
    /// Default. `None` only when there is no Default.
    ///
    /// Per card per frame (`jar_chip`, `will_switch`, `sweep_jar_switches`): one mirror lookup per enclosing group,
    /// and at the edge it meets a reach walk of the same depth; `default_jar` is an index read. No allocation.
    pub(crate) fn resolve_jar(&self, own: Option<NodeId>, origin: Option<NodeId>) -> Option<(NodeId, bool)> {
        let (mut edge, mut reach, mut next) = (own, origin, origin);
        for _ in 0..=self.nodes.len() {
            if let Some(jar) = edge {
                let at = self.doc.mirror.group(jar);
                let in_reach = self.levels_from(reach).any(|level| level == at);
                return if self.jar_exists(jar) && in_reach {
                    Some((jar, false))
                } else {
                    self.default_jar().map(|d| (d, true))
                };
            }
            let Some(g) = next.filter(|g| self.nodes.contains_key(g)) else { break };
            edge = self.doc.mirror.jar(g);
            reach = Some(g);
            next = self.doc.mirror.group(g);
        }
        self.default_jar().map(|d| (d, false))
    }

    /// `resolve_jar` for a tab or group the mirror holds (live, asleep or mid-respawn).
    pub(crate) fn effective_jar(&self, id: NodeId) -> Option<(NodeId, bool)> {
        self.resolve_jar(self.doc.mirror.jar(id), self.reach_origin(id))
    }

    /// Whether `id` is a tab (live, unloaded or mid-respawn: `linked_ids`); false for any other node and for a new
    /// spawn, which has no mirror entry until it lands.
    pub(crate) fn is_tab(&self, id: NodeId) -> bool {
        match self.nodes.get(&id) {
            Some(node) => node.runtime.saved_tab().is_some(),
            None => self.is_linked(id), // only a tab respawns
        }
    }

    /// The menu's and SETJAR's pick for tab or group `id`: draw its `jar` edge to `jar`, or clear it (`None`,
    /// "Inherit"). A refused edge (out of reach, not a jar) changes nothing. Live tabs whose jar changed switch.
    pub(crate) fn set_jar_edge(&mut self, id: NodeId, jar: Option<NodeId>) {
        match jar {
            Some(jar) => {
                let _ = self.connect(JAR_EDGE, id, jar);
            }
            None => {
                if let Some(old) = self.doc.mirror.jar(id) {
                    self.disconnect(JAR_EDGE, id, old);
                }
            }
        }
        self.sweep_jar_switches();
    }

    /// After a local move of `moved` (a card, or a group with its contents): every edge whose row reaches beyond
    /// one level (`Scope::SameLevelOrAncestor`) with an end in `moved`'s subtree is checked again, and one whose live
    /// target is now out of reach is cleared, with a toast naming it (spec "Scope"). A dangling edge (target gone)
    /// is left: it warns, and is never rewritten. Then live tabs whose jar changed switch.
    pub(crate) fn recheck_edge_scope(&mut self, moved: NodeId) {
        let rows: Vec<(&'static str, u32)> = crate::caps::edges()
            .user_creatable()
            .filter(|r| r.scope() == Scope::SameLevelOrAncestor)
            .filter_map(|r| match r.storage() {
                Storage::Field(no) => Some((r.id(), no)),
                _ => None,
            })
            .collect();
        let mut cleared = Vec::new();
        for (edge, no) in rows {
            let scope = crate::caps::edges().row(edge).expect("a listed row").scope();
            for source in self.linked_ids().collect::<Vec<_>>() {
                let FieldValue::Id(Some(target)) = self.doc.mirror.get(source, no) else { continue };
                let touched = self.within(source, moved) || self.within(target, moved);
                if touched && self.nodes.contains_key(&target) && !self.in_scope(scope, source, target) {
                    self.edit_field(source, no, FieldValue::Id(None));
                    cleared.push(target);
                }
            }
        }
        for jar in cleared {
            let name = self.jar_label(jar);
            self.queue_toast(format!("{name} is out of reach here; the edge to it was removed"));
        }
        self.sweep_jar_switches();
    }

    /// Loads `key`'s jar the first time a tab needs it (spec "Manifold state": named jars load lazily). An ephemeral
    /// jar starts empty and is never read from disk.
    pub(crate) fn ensure_jar_loaded(&mut self, id: NodeId) {
        if self.jars.contains_key(&id) {
            return;
        }
        let jar = match self.jars_dir() {
            Some(dir) if !self.is_ephemeral(id) => Jar::load(&dir, id),
            _ => Jar::default(),
        };
        self.publish_jar(id, jar);
    }

    /// Stores `jar` under `id` and copies it to the spawn threads' snapshot map.
    fn publish_jar(&mut self, id: NodeId, jar: Jar) {
        if let Ok(mut snapshots) = self.cookies.jar_snapshots.lock() {
            snapshots.insert(id, jar.clone());
        }
        self.jars.insert(id, jar);
    }

    fn is_ephemeral(&self, id: NodeId) -> bool {
        // The mirror, not `jar_defs`: a jar being removed (`forget_jar_memory`) has left `nodes` but not yet the mirror.
        self.doc.mirror.jar_def(id).ephemeral
    }

    /// Writes jar `id`'s file unless it is ephemeral. A failure is logged (never cookie values) and leaves other jars
    /// alone. Settles the jar's dirty mark (`jar_dirty`): every write goes through here.
    pub(crate) fn save_jar(&mut self, id: NodeId) {
        self.cookies.jar_dirty.remove(&id); // written now, or never (ephemeral, no dir)
        let (Some(dir), Some(jar)) = (self.jars_dir(), self.jars.get(&id)) else { return };
        if self.is_ephemeral(id) {
            return;
        }
        #[cfg(test)]
        self.jar_saves.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if let Err(e) = jar.save(&dir, id) {
            eprintln!("jar: could not save {id}: {e}");
        }
    }

    /// Empties jar `id` and saves it, then switches the live tabs running on it (deviation 14): their engines still
    /// hold the old cookies. A jar that no longer exists is left alone, so no file is written for it.
    pub(crate) fn clear_jar_cookies(&mut self, id: NodeId) {
        if !self.jar_exists(id) {
            return;
        }
        self.ensure_jar_loaded(id);
        let Some(jar) = self.jars.get(&id) else { return };
        // Cleared in place, so a locked jar stays locked and its unreadable file is still never overwritten.
        let mut jar = jar.clone();
        jar.cookies.clear();
        // Counted with the emptied copy, under one lock: a host still spawning whose seed was read before this is
        // caught when it lands (`note_spawned_jar`), as it has no `jar_tabs` entry to mark below.
        if let Ok(mut snapshots) = self.cookies.jar_snapshots.lock() {
            snapshots.insert_cleared(id, jar.clone());
        }
        self.jars.insert(id, jar);
        self.save_jar(id);
        // Their engines still hold the old cookies: they must be replaced although their jar is unchanged.
        let running: Vec<NodeId> =
            self.cookies.jar_tabs.iter().filter(|(_, (j, _))| *j == id).map(|(tab, _)| *tab).collect();
        self.cookies.jar_reseed.extend(running);
        self.sweep_jar_switches();
    }

    /// The live jar cards' definitions, in id order (the order labels and pickers use).
    pub(crate) fn jar_defs(&self) -> BTreeMap<NodeId, JarDef> {
        self.nodes
            .iter()
            .filter(|(_, n)| matches!(n.runtime, Runtime::Jar))
            .map(|(id, _)| (*id, self.doc.mirror.jar_def(*id)))
            .collect()
    }

    /// Whether `id` is a live jar card.
    pub(crate) fn jar_exists(&self, id: NodeId) -> bool {
        self.nodes.get(&id).is_some_and(|n| matches!(n.runtime, Runtime::Jar))
    }
}
