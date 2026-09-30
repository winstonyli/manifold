//! Global keyboard shortcuts (and their headless equivalents) as App methods.

use super::*;

impl App {
    /// Browser-chrome keyboard shortcuts — checked via egui's own input
    /// polling (same mechanism `build_tab_view_ui`'s own `Escape` check uses),
    /// so they fire regardless of which widget has focus and in either
    /// view. `consume_key` removes the matching key event from this
    /// frame's input first, so a page never sees e.g. `Ctrl+T` leak
    /// through as a plain `T` keypress the way an unhandled key would
    /// (see `window_event`'s `KeyboardInput` arm). `Ctrl+Shift+Tab` and
    /// `Ctrl+Shift+T` are each checked before their plain (no-Shift)
    /// counterpart — egui's `matches_logically` ignores an extra Shift/Alt,
    /// so checking the less specific one first would consume a
    /// `Ctrl+Shift+X` press as if it were plain `Ctrl+X`.
    pub(crate) fn handle_global_shortcuts(&mut self, ui: &mut egui::Ui) {
        let (
            reopen_tab,
            new_tab,
            close_all_level,
            close_tab,
            focus_address,
            open_find,
            zoom_in,
            zoom_out,
            zoom_reset,
            open_devtools,
            prev_tab,
            next_tab,
            quick_switch,
            new_group,
            view_toggle,
            pin_toggle,
            reload,
            rename_group,
            duplicate_tab,
            shortcuts_help,
        ) = ui.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::T),
                i.consume_key(egui::Modifiers::CTRL, egui::Key::T),
                // Checked ahead of plain `Ctrl+W` below, same "extra Shift
                // matches the plain binding too" ordering reason `Ctrl+Shift+
                // T`/`Ctrl+Shift+G` already need.
                i.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::W),
                i.consume_key(egui::Modifiers::CTRL, egui::Key::W),
                i.consume_key(egui::Modifiers::CTRL, egui::Key::L),
                i.consume_key(egui::Modifiers::CTRL, egui::Key::F),
                // `Ctrl+=` is the physical key most keyboards actually send
                // for "zoom in" (`+` needs Shift on most layouts) — accept
                // both so either a real `Ctrl++` or the more common
                // `Ctrl+=` works.
                i.consume_key(egui::Modifiers::CTRL, egui::Key::Equals)
                    || i.consume_key(egui::Modifiers::CTRL, egui::Key::Plus),
                i.consume_key(egui::Modifiers::CTRL, egui::Key::Minus),
                i.consume_key(egui::Modifiers::CTRL, egui::Key::Num0),
                i.consume_key(egui::Modifiers::NONE, egui::Key::F12),
                i.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::Tab),
                i.consume_key(egui::Modifiers::CTRL, egui::Key::Tab),
                i.consume_key(egui::Modifiers::CTRL, egui::Key::K),
                // Checked ahead of plain `Ctrl+G` below -- `matches_logically`
                // ignores an extra Shift, so checking `Ctrl+G` first would
                // consume a `Ctrl+Shift+G` press as if it were plain
                // `Ctrl+G`, same ordering reason `Ctrl+Shift+T` already needs.
                i.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::G),
                i.consume_key(egui::Modifiers::CTRL, egui::Key::G),
                i.consume_key(egui::Modifiers::CTRL, egui::Key::P),
                i.consume_key(egui::Modifiers::CTRL, egui::Key::R)
                    || i.consume_key(egui::Modifiers::NONE, egui::Key::F5),
                i.consume_key(egui::Modifiers::NONE, egui::Key::F2),
                i.consume_key(egui::Modifiers::CTRL, egui::Key::D),
                i.consume_key(egui::Modifiers::NONE, egui::Key::F1),
            )
        });

        if reopen_tab {
            self.reopen_closed_shortcut();
        }
        if new_tab {
            self.new_tab_shortcut();
        }
        if close_all_level {
            self.close_all_at_current_level();
        }
        if close_tab {
            self.close_focused_tab_shortcut();
        }
        if focus_address {
            self.focus_address_bar_shortcut();
        }
        if open_find {
            self.open_find_bar_shortcut();
        }
        if zoom_in {
            self.zoom_in_shortcut();
        }
        if zoom_out {
            self.zoom_out_shortcut();
        }
        if zoom_reset {
            self.zoom_reset_shortcut();
        }
        if open_devtools {
            self.open_devtools_shortcut();
        }
        if next_tab || prev_tab {
            self.cycle_focused_tab(next_tab);
        }
        if quick_switch {
            self.quick_switcher = Some(QuickSwitcher { query: String::new(), selected: 0 });
            self.pending_focus = Some(FocusTarget::QuickSwitcher);
        }
        if new_group {
            self.new_group_shortcut();
        }
        // Neutralized while the quick-switcher is open, same reasoning as
        // its own Escape/arrow handling in `build_quick_switcher` -- a
        // `Ctrl+G` typed while it's open shouldn't also flip the view
        // underneath it.
        if view_toggle && self.quick_switcher.is_none() {
            self.toggle_view_mode_shortcut();
        }
        if pin_toggle {
            self.pin_focused_tab_shortcut();
        }
        if reload {
            self.reload_focused_tab_shortcut();
        }
        if rename_group {
            self.rename_current_group_shortcut();
        }
        if duplicate_tab {
            self.duplicate_focused_tab_shortcut();
        }
        if shortcuts_help {
            self.shortcuts_help_open = !self.shortcuts_help_open;
        }
    }

    /// `Ctrl+P` / headless `TOGGLEPIN` — pins/unpins the focused tab, same
    /// effect as its graph card's own pin dot or the tab context menu's
    /// Pin/Unpin entry.
    pub(crate) fn pin_focused_tab_shortcut(&mut self) {
        let Some(id) = self.focused.clone() else { return };
        if let Some(pinned) = self.nodes.get_mut(&id).and_then(|n| n.payload.pinned_mut()) {
            *pinned = !*pinned;
        }
    }

    /// `Ctrl+R`/`F5` / headless `RELOAD` — reloads the focused tab. Every
    /// engine exposes a plain no-argument reload (see `HostToEngine::
    /// Reload`'s own doc comment), so there's no can-reload guard the way
    /// `GoBack`/`GoForward` need one.
    pub(crate) fn reload_focused_tab_shortcut(&mut self) {
        let Some(id) = self.focused.clone() else { return };
        if let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
            node.tab.send(&HostToEngine::Reload);
        }
    }

    /// `F2` / headless `RENAMEGROUP` — starts renaming whichever group's
    /// own subgraph is currently being viewed, same effect as clicking its
    /// name button in the Graph toolbar (see that button's own click
    /// handler, which this mirrors exactly). A no-op at the top level
    /// (`current_group` is `None` there, nothing to rename) or while
    /// already renaming.
    pub(crate) fn rename_current_group_shortcut(&mut self) {
        let Some(group_id) = self.current_group.clone() else { return };
        if self.renaming_group.is_some() || self.current_json_node().is_some() {
            return;
        }
        let Some(name) = self.nodes.get(&group_id).and_then(|n| n.as_group()).map(|g| g.name.clone()) else { return };
        self.renaming_group = Some((group_id, name));
        self.pending_focus = Some(FocusTarget::GroupRename);
    }

    /// `Ctrl+Shift+G` / toolbar "+ New Group" / headless `NEWGROUP` —
    /// creates an empty group at whichever level is currently being
    /// viewed, nested under `current_group` if that's not the top level
    /// (see `GroupPayload`'s own doc comment on nesting). From a JSON
    /// level, leaves it first (`placement`).
    pub(crate) fn new_group_shortcut(&mut self) {
        let id = self.alloc_node_id();
        self.insert_group_at(id, None);
        self.save_session();
    }

    /// The Graph toolbar's "+ Note" button -- a note spawns at whichever
    /// level is currently being viewed, same as "+ New Tab" and (now)
    /// "+ New Group" itself, since a note is a
    /// plain member of a level's subgraph like a tab or download is.
    /// Leaves a JSON level first, like `new_group_shortcut`.
    pub(crate) fn new_note_shortcut(&mut self) {
        let id = self.alloc_node_id();
        self.insert_note_at(id, None);
        self.save_session();
    }

    /// `Ctrl+G` / headless `TOGGLEVIEW` — switches to whichever of Tab/Graph
    /// view isn't currently showing. Tab -> Graph reuses `leave_to_graph`'s
    /// own shrink animation. Graph -> Tab jumps to `self.focused` directly
    /// (`focus_tab`, not the graph node's expand animation) and updates
    /// `current_group` to match it first -- same reasoning the quick-
    /// switcher's own jump has: `self.focused` isn't guaranteed to be a tab
    /// at the level currently being viewed (e.g. after `Ctrl+Tab` cycling
    /// or a quick-switcher jump into a different group), so there's no
    /// reliable on-screen node to animate from.
    pub(crate) fn toggle_view_mode_shortcut(&mut self) {
        match self.view_mode {
            ViewMode::Item => self.leave_to_graph(),
            ViewMode::Settings => self.close_settings_page(),
            ViewMode::Graph => {
                let Some(id) = self.focused.clone() else { return };
                if self.nodes.contains_key(&id) {
                    self.show_level_of(&id);
                    self.focus_tab(&id);
                }
            }
        }
    }

    /// `Ctrl+=`/`Ctrl++` / headless `ZOOMIN` — zooms the maximized Image
    /// while one is showing (see the Image item view spec's "2. Image
    /// view" section); otherwise the same step/cap the settings popup's
    /// own "+" button uses for the focused Tab's page zoom.
    pub(crate) fn zoom_in_shortcut(&mut self) {
        if self.showing_image_view() {
            self.apply_image_zoom_action(ZoomChipAction::In);
        } else {
            self.set_focused_zoom((self.focused_zoom_factor() + 0.1).min(2.0));
        }
    }

    /// `Ctrl+-` / headless `ZOOMOUT` — same Image-view/Tab-view split as
    /// `zoom_in_shortcut`; the Tab branch matches the settings popup's own
    /// "−" button.
    pub(crate) fn zoom_out_shortcut(&mut self) {
        if self.showing_image_view() {
            self.apply_image_zoom_action(ZoomChipAction::Out);
        } else {
            self.set_focused_zoom((self.focused_zoom_factor() - 0.1).max(0.5));
        }
    }

    /// `Ctrl+0` / headless `ZOOMRESET` — same Image-view/Tab-view split as
    /// `zoom_in_shortcut`; the Tab branch matches the settings popup's own
    /// "Reset" button.
    pub(crate) fn zoom_reset_shortcut(&mut self) {
        if self.showing_image_view() {
            self.apply_image_zoom_action(ZoomChipAction::Reset100);
        } else {
            self.set_focused_zoom(1.0);
        }
    }

    /// `F12` / headless `OPENDEVTOOLS` — opens the focused tab's engine's
    /// own native DevTools UI (a separate real window the engine manages
    /// itself; nothing comes back to `manifold` for this, so there's no
    /// state to track here beyond sending the message). See
    /// `HostToEngine::OpenDevTools`'s doc comment for per-engine support.
    pub(crate) fn open_devtools_shortcut(&mut self) {
        let Some(id) = self.focused.clone() else { return };
        let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) else { return };
        node.tab.send(&HostToEngine::OpenDevTools);
    }

    /// `Ctrl+T` / headless `NEWTAB` — same call the "+ New Tab" button
    /// makes — a child of whichever tab is currently focused, staying put
    /// rather than auto-focusing (see `spawn_new_tab`'s own doc comment).
    pub(crate) fn new_tab_shortcut(&mut self) {
        let home_page = self.settings.home_page.clone();
        let parent = self.focused.clone();
        // From a JSON level: leave it, and land in the level then shown (the
        // focused id may be a level card, not a stored node).
        let group = if self.current_json_node().is_some() {
            self.leave_json_level();
            self.current_group.clone()
        } else {
            self.group_of(&parent)
        };
        self.spawn_new_tab(parent, group, &home_page);
    }

    /// `Ctrl+Shift+T` / headless `REOPENTAB` — reopens the newest "Recently
    /// Closed" entry of any kind, where it was (`reopen_closed`); a no-op
    /// if there's none.
    pub(crate) fn reopen_closed_shortcut(&mut self) {
        if let Some(entry) = self.closed.pop() {
            self.closed_changed_at = Some(std::time::Instant::now());
            self.reopen_closed(entry);
        }
    }

    /// `Ctrl+W` / headless `CLOSETAB` — closes the focused tab, a no-op if
    /// none is focused (Graph view with nothing selected).
    pub(crate) fn close_focused_tab_shortcut(&mut self) {
        if let Some(id) = self.focused.clone() {
            self.close_node(&id);
        }
    }

    /// `Ctrl+D` / headless `DUPLICATETAB` — keyboard parity for the tab
    /// context menu's own "Duplicate" entry, applied to the focused tab. A
    /// no-op if none is focused (Graph view with nothing selected).
    pub(crate) fn duplicate_focused_tab_shortcut(&mut self) {
        if let Some(id) = self.focused.clone() {
            self.duplicate_tab(&id);
        }
    }

    /// `Ctrl+L` / headless `FOCUSADDRESS` — only meaningful in Tab view,
    /// since Graph view has no address bar to focus. Queues a focus request
    /// rather than setting `address_bar.focused` directly: real egui focus
    /// can only be requested on the widget, when `build_tab_view_ui` builds
    /// it (see `FocusTarget`).
    pub(crate) fn focus_address_bar_shortcut(&mut self) {
        if self.showing_tab_view() {
            self.pending_focus = Some(FocusTarget::AddressBar);
            // `request_focus()` only takes hold starting the *next* frame
            // after it's called (immediate-mode: the widget this frame
            // already painted before `build_tab_view_ui` gets to call it) — a
            // real keypress always gets a following frame for free, but
            // this command can otherwise land with nothing further
            // scheduled to redraw (Tab view, unlike Graph view's physics,
            // has no continuous redraw loop of its own), leaving the
            // pending flag applied-but-invisible until some unrelated
            // event happens to trigger another frame.
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
    }

    /// `Ctrl+F` / headless `OPENFINDBAR` — the find bar in Tab view, the
    /// outline's search box on a JSON level (`focus_json_search`), nothing
    /// elsewhere in Graph view. Opening the find bar with existing text
    /// re-searches for it (matches a real browser's find bar remembering
    /// the last term).
    pub(crate) fn open_find_bar_shortcut(&mut self) {
        // On a JSON level, Ctrl+F searches the value instead.
        if self.view_mode == ViewMode::Graph && self.json_level.is_some() {
            self.focus_json_search();
            return;
        }
        if !self.showing_tab_view() {
            return;
        }
        self.find.open = true;
        self.pending_focus = Some(FocusTarget::FindBar);
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        if !self.find.text.is_empty() {
            self.send_find_start();
        }
    }
}
