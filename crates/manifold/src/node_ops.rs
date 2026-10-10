//! Node operations: closing, destroying, muting, pinning, bookmarking and printing nodes.

use super::*;

impl App {
    /// The shared fold behind both of the above (and `level_items`'
    /// world bounds more generally): every rect's union, expanded by a
    /// flat margin, or `default_graph_scene_rect()` when there's nothing
    /// to fit.
    pub(crate) fn bounds_for_rects(&self, rects: impl IntoIterator<Item = egui::Rect>) -> egui::Rect {
        let bounds = rects.into_iter().fold(egui::Rect::NOTHING, egui::Rect::union);
        if !bounds.is_finite() {
            return default_graph_scene_rect();
        }
        bounds.expand(GRAPH_FIT_MARGIN)
    }

    /// Graph view's own `Escape` handling / headless `DISMISSPEEK` —
    /// dismisses the currently-hovered node's peek preview early, without
    /// needing the pointer to move away first. See `peek_suppressed`'s own
    /// doc comment for why this can't just clear `hovered` directly.
    pub(crate) fn dismiss_hover_peek(&mut self) {
        self.interaction.peek_suppressed = self.interaction.hovered.clone();
    }

    /// Like `insert_group_at`, for `new_note_shortcut` and the card's note.
    /// Doesn't save; the caller does.
    pub(crate) fn insert_note_at(&mut self, id: String, pos: Option<egui::Vec2>) {
        let at = self.placement(&id, pos);
        let note = NotePayload { text: String::new(), editing: true };
        self.place_node(Some(id), store::model::Kind::Note, Runtime::Note(note), at);
    }

    /// Starts an unloaded tab's engine (`UnloadedTabPayload`) on `kind`, which becomes its engine (`edit_engine`).
    /// The card stays, showing a spinner, until the outcome replaces it:
    /// the live tab under the same id, or a failed card with the new
    /// error. Ignored while a wake is already in flight.
    pub(crate) fn wake_tab(&mut self, id: &str, kind: EngineKind) {
        let Carried { pos, color, pinned, parent, group, jar, .. } = self.mirror.carried(id);
        let pos = pos.unwrap_or_default(); // unused: the card's layout entry stays (`TabSpawn::pos`)
        let Some(node) = self.nodes.get_mut(id) else { return };
        let Runtime::Unloaded(tab) = &mut node.runtime else { return };
        if tab.state == UnloadedState::Waking {
            return;
        }
        tab.state = UnloadedState::Waking;
        let (zoom_factor, title, muted) = (tab.zoom_factor, tab.title.clone(), tab.muted);
        let carry = TabCarry { pinned, color, zoom_factor, title, muted };
        let url = tab.url.clone();
        self.spawns.wakes_in_flight.insert(id.to_string());
        // Another engine is saved as the wake starts, not when it lands (2b-3 Ruling 5).
        self.edit_engine(id, kind);
        let (id, purpose, fallback_kind) = (id.to_string(), SpawnPurpose::Wake, None);
        self.respawn_tab(TabSpawn { id, url, kind, parent, group, jar, pos, purpose, carry, fallback_kind });
    }

    /// `Ctrl+Shift+W` / headless `CLOSEALLLEVEL` — closes every tab
    /// `tabs_at_current_level` returns (top level: every ungrouped tab,
    /// leaving groups and their members untouched; inside a group: every
    /// tab member: the tabs that group's own "Close Group and Contents"
    /// context-menu entry would close, though that entry also closes the
    /// group and its other members). Each closes through the
    /// normal `close_node` path, same "just that action run once per tab"
    /// shape `close_all_except` already has.
    pub(crate) fn close_all_at_current_level(&mut self) {
        let ids = self.tabs_at_current_level();
        self.begin_tabs_shutdown(&ids);
        for id in ids {
            self.close_node(&id);
        }
    }

    /// Toggles `id`'s mute state and sends the real `SetMuted` — shared by
    /// Tab view's "Muted" toolbar button and Graph view's own audio-dot
    /// click (see `TabPayload::muted`'s doc comment for the WebView2/WebKit-
    /// only caveat).
    pub(crate) fn toggle_muted(&mut self, id: &str) {
        if let Some(node) = self.nodes.get_mut(id).and_then(|n| n.as_tab_mut()) {
            node.muted = !node.muted;
            let muted = node.muted;
            node.tab.send(&HostToEngine::SetMuted { muted });
        }
    }

    /// "Print to PDF": routes through the exact same GPU screenshot
    /// capture `pending_screenshot` already drives (a temp PNG, cleaned up
    /// once converted), rather than teaching `gpu::GpuState` anything
    /// about PDF -- `handle_screenshot_result` checks `pending_print_to_pdf` once that capture
    /// actually completes and does the PNG-to-PDF conversion there. Called
    /// by both the real toolbar button (after a real file-picker chooses
    /// `pdf_path`) and the headless `PRINTTOPDF` debug command (which
    /// supplies `pdf_path` directly, bypassing the picker).
    pub(crate) fn start_print_to_pdf(&mut self, pdf_path: String) {
        let timestamp =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let temp_png = std::env::temp_dir().join(format!("manifold-print-{timestamp}.png"));
        let png = temp_png.to_string_lossy().into_owned();
        self.debug.pending_screenshot = Some(png.clone());
        self.debug.pending_print_to_pdf = Some((png, pdf_path));
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Tab view's "Bookmark" toggle: adds `id`'s current title/url if not
    /// already bookmarked (matched by url), removes it if it is. Persists
    /// immediately, same "no separate Save step" posture every other
    /// instant-apply control in this chrome already has.
    pub(crate) fn toggle_bookmark(&mut self, id: &str) {
        let Some(node) = self.nodes.get(id).and_then(|n| n.as_tab()) else { return };
        let url = node.url.clone();
        if let Some(index) = self.bookmarks.iter().position(|b| b.url == url) {
            self.bookmarks.remove(index);
        } else {
            let title = if node.title.is_empty() { url.clone() } else { node.title.clone() };
            self.bookmarks.push(bookmarks::Bookmark { title, url, folder: None });
        }
        bookmarks::save(&self.bookmarks);
    }

    /// Tab view's "Mute Others" button: mutes every open tab except
    /// `keep_id`, regardless of any already-muted state, without touching
    /// `keep_id`'s own mute state either way. A one-shot action, not a
    /// toggle -- a second click would just mute everything again, a no-op
    /// for anything already muted. Includes sleeping and waking tabs (no
    /// engine to send `SetMuted` to, but a wake's landing already sends it
    /// from the card's own `muted`, and personal state saves it either
    /// way -- see `UnloadedTabPayload::muted`).
    pub(crate) fn mute_all_except(&mut self, keep_id: &str) {
        let other_ids: Vec<String> = self
            .nodes
            .iter()
            .filter(|(id, n)| id.as_str() != keep_id && n.runtime.saved_tab().is_some())
            .map(|(id, _)| id.clone())
            .collect();
        for id in other_ids {
            let Some(node) = self.nodes.get_mut(&id) else { continue };
            match &mut node.runtime {
                Runtime::Tab(tab) if !tab.muted => {
                    tab.muted = true;
                    tab.tab.send(&HostToEngine::SetMuted { muted: true });
                }
                Runtime::Unloaded(tab) => tab.muted = true,
                _ => {}
            }
        }
    }

    /// Tab view's "Close Others" button: closes every open tab except
    /// `keep_id` through the normal `close_node` path (teardown, reparenting,
    /// Recently Closed all still apply -- this is just that same
    /// action run once per other tab, not a bulk shortcut around it).
    pub(crate) fn close_all_except(&mut self, keep_id: &str) {
        let other_ids: Vec<String> = self
            .node_order
            .iter()
            .filter(|id| {
                id.as_str() != keep_id && self.nodes.get(id.as_str()).is_some_and(|n| n.runtime.saved_tab().is_some())
            })
            .cloned()
            .collect();
        self.begin_tabs_shutdown(&other_ids);
        for id in other_ids {
            self.close_node(&id);
        }
    }

    /// Tab view's "Pin All"/"Unpin All" buttons: sets every open tab's
    /// `pinned` flag to `pin`, regardless of its current state. One-shot
    /// actions, not toggles -- same "just run the single-tab effect once
    /// per tab" shape as `mute_all_except`, minus the "except" since pinning
    /// has no natural tab to exclude the way muting excludes the one
    /// they're presumably still listening to.
    pub(crate) fn set_all_pinned(&mut self, pin: bool) {
        let tabs: Vec<String> =
            self.nodes.iter().filter(|(_, n)| n.runtime.saved_tab().is_some()).map(|(id, _)| id.clone()).collect();
        for id in tabs {
            self.edit_field(&id, field::PINNED, FieldValue::Bool(pin));
        }
    }

    /// Closes one node: records it (and, for a group, everything under it)
    /// as one "Recently Closed" entry, then `destroy_node`s it. The single
    /// user-facing close: a card's hover ✕, its context-menu Delete/Close/
    /// "Remove from List", the Downloads popup's Remove, Ctrl+W, Close
    /// Selected and the rest. It doesn't save (callers decide:
    /// `close_node_and_save`); a tab's or group's destroy saves itself.
    pub(crate) fn close_node(&mut self, id: &str) {
        self.record_closed(id);
        self.destroy_node(id);
    }

    /// Removes one node with its kind's full cleanup, recording nothing: a
    /// Group recurses (`destroy_group`), a Tab goes through `destroy_tab`,
    /// and a Download/Note/Image/Pdf/Json frees its GPU texture if it's an
    /// Image/Pdf, then `remove_node`s it.
    /// (Before this was shared, `destroy_tab`'s own no-op-on-non-tab guard
    /// left every non-tab, non-group member of a closed group merely
    /// reparented by `dissolve_group`, never actually destroyed -- a gap
    /// confirmed while building "Expand as Graph", whose whole output is
    /// `JsonPayload` leaves nested inside Groups.)
    pub(crate) fn destroy_node(&mut self, id: &str) {
        let Some(kind) = self.card_kind(id) else {
            return;
        };
        if is_container(kind.ty) {
            self.destroy_group(id);
        } else if kind.ty == store::model::TYPE_TAB {
            self.destroy_tab(id);
        } else {
            self.remove_node(id);
        }
    }

    /// `close_node` for a single user action (hover ✕, context-menu
    /// Delete/Close, Downloads popup Remove), then persists the session --
    /// except for a tab or group, whose `destroy_tab`/`destroy_group`
    /// already save.
    pub(crate) fn close_node_and_save(&mut self, id: &str) {
        let persisted =
            self.card_kind(id).is_some_and(|kind| kind.ty != store::model::TYPE_TAB && !is_container(kind.ty));
        self.close_node(id);
        if persisted {
            self.save_session();
        }
    }
}
