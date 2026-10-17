//! Global keyboard shortcuts (and their headless equivalents) as App methods.

use super::*;

/// What a global shortcut does (`App::handle_global_shortcuts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shortcut {
    ReopenTab,
    NewTab,
    CloseAllLevel,
    CloseTab,
    FocusAddress,
    OpenFind,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    OpenDevtools,
    PrevTab,
    NextTab,
    QuickSwitch,
    NewGroup,
    ViewToggle,
    PinToggle,
    Reload,
    RenameGroup,
    DuplicateTab,
    ShortcutsHelp,
    Back,
    Forward,
}

/// One key chord: egui's modifiers and key (what `consume_key` matches),
/// the winit codes the same press arrives as on its way to a page
/// (`claims_chord`), and what it does.
pub(crate) struct Chord {
    pub(crate) mods: egui::Modifiers,
    pub(crate) key: egui::Key,
    pub(crate) codes: &'static [WinitKeyCode],
    pub(crate) action: Shortcut,
}

const CTRL_SHIFT: egui::Modifiers = egui::Modifiers { shift: true, ..egui::Modifiers::CTRL };

/// Every global shortcut, in the order they are tried. egui's matching
/// ignores an extra Shift, so each `Ctrl+Shift+X` comes before its plain
/// `Ctrl+X`, or the plain one would consume it. An action with two chords
/// fires on the first that matches.
#[rustfmt::skip] // One chord per line reads as the shortcut list it is.
pub(crate) const CHORDS: &[Chord] = {
    use egui::Key as K;
    use egui::Modifiers as M;
    use Shortcut as S;
    use WinitKeyCode as W;
    &[
        Chord { mods: CTRL_SHIFT, key: K::T, codes: &[W::KeyT], action: S::ReopenTab },
        Chord { mods: M::CTRL, key: K::T, codes: &[W::KeyT], action: S::NewTab },
        Chord { mods: CTRL_SHIFT, key: K::W, codes: &[W::KeyW], action: S::CloseAllLevel },
        Chord { mods: M::CTRL, key: K::W, codes: &[W::KeyW], action: S::CloseTab },
        Chord { mods: M::CTRL, key: K::L, codes: &[W::KeyL], action: S::FocusAddress },
        Chord { mods: M::CTRL, key: K::F, codes: &[W::KeyF], action: S::OpenFind },
        // `Ctrl+=` is what most keyboards send for "zoom in" (`+` needs Shift): accept both.
        Chord { mods: M::CTRL, key: K::Equals, codes: &[W::Equal], action: S::ZoomIn },
        Chord { mods: M::CTRL, key: K::Plus, codes: &[W::NumpadAdd], action: S::ZoomIn },
        Chord { mods: M::CTRL, key: K::Minus, codes: &[W::Minus, W::NumpadSubtract], action: S::ZoomOut },
        Chord { mods: M::CTRL, key: K::Num0, codes: &[W::Digit0, W::Numpad0], action: S::ZoomReset },
        Chord { mods: M::NONE, key: K::F12, codes: &[W::F12], action: S::OpenDevtools },
        Chord { mods: CTRL_SHIFT, key: K::Tab, codes: &[W::Tab], action: S::PrevTab },
        Chord { mods: M::CTRL, key: K::Tab, codes: &[W::Tab], action: S::NextTab },
        Chord { mods: M::CTRL, key: K::K, codes: &[W::KeyK], action: S::QuickSwitch },
        Chord { mods: CTRL_SHIFT, key: K::G, codes: &[W::KeyG], action: S::NewGroup },
        Chord { mods: M::CTRL, key: K::G, codes: &[W::KeyG], action: S::ViewToggle },
        Chord { mods: M::CTRL, key: K::P, codes: &[W::KeyP], action: S::PinToggle },
        Chord { mods: M::CTRL, key: K::R, codes: &[W::KeyR], action: S::Reload },
        Chord { mods: M::NONE, key: K::F5, codes: &[W::F5], action: S::Reload },
        Chord { mods: M::NONE, key: K::F2, codes: &[W::F2], action: S::RenameGroup },
        Chord { mods: M::CTRL, key: K::D, codes: &[W::KeyD], action: S::DuplicateTab },
        Chord { mods: M::NONE, key: K::F1, codes: &[W::F1], action: S::ShortcutsHelp },
        Chord { mods: M::ALT, key: K::ArrowLeft, codes: &[W::ArrowLeft], action: S::Back },
        Chord { mods: M::ALT, key: K::ArrowRight, codes: &[W::ArrowRight], action: S::Forward },
    ]
};

/// Whether `code` pressed with `modifiers` (the `manifold_view_protocol::modifiers` bits) is one of `CHORDS`, so
/// the page must not get it as well. The held Ctrl, Alt and Super must be exactly the chord's; an extra Shift is
/// ignored when the chord needs Ctrl or Alt (as egui's matching ignores it), and is a different chord otherwise.
pub(crate) fn claims_chord(code: WinitKeyCode, modifiers: u8) -> bool {
    use manifold_view_protocol::modifiers::{ALT, CONTROL, SHIFT, SUPER};
    CHORDS.iter().any(|c| {
        let wanted = if c.mods.ctrl { CONTROL } else { 0 } | if c.mods.alt { ALT } else { 0 };
        let mask = if wanted != 0 { CONTROL | ALT | SUPER } else { CONTROL | ALT | SUPER | SHIFT };
        c.codes.contains(&code) && modifiers & mask == wanted
    })
}

/// Whether a text field had keyboard focus as this pass's input arrived (`text_field_has_keys`).
#[derive(Default)]
struct TextFieldFocus {
    at_input: bool,
}

impl egui::Plugin for TextFieldFocus {
    fn debug_name(&self) -> &'static str {
        "text_field_focus"
    }

    /// Runs before `Memory::begin_pass`, which is where an Escape press drops focus.
    fn input_hook(&mut self, ctx: &egui::Context, _input: &mut egui::RawInput) {
        self.at_input = ctx.text_edit_focused();
    }
}

/// Whether this pass's keys belong to a text field, so a page-wide handler (graph navigation, Settings' Escape)
/// must leave them alone. True if a `TextEdit` had focus when the input arrived, or has it now. The first half is
/// needed for Escape: egui drops focus on Escape before any widget runs (`Memory::begin_pass`), so
/// `Context::text_edit_focused` already reads false on that frame. Reading it later in the frame doesn't work for
/// Enter either: a singleline `TextEdit` gives up focus while handling it. The first call on a context registers
/// the plugin that does the tracking, so it starts answering from the next pass.
pub(crate) fn text_field_has_keys(ctx: &egui::Context) -> bool {
    let at_input = ctx.plugin_or_default::<TextFieldFocus>().lock().at_input;
    at_input || ctx.text_edit_focused()
}

impl App {
    /// Browser-chrome keyboard shortcuts — checked via egui's own input
    /// polling (same mechanism `build_tab_view_ui`'s own `Escape` check uses),
    /// so they fire regardless of which widget has focus and in either
    /// view. `consume_key` removes the matching key event from this
    /// frame's input first, so a page never sees e.g. `Ctrl+T` leak
    /// through as a plain `T` keypress the way an unhandled key would
    /// (see `window_event`'s `KeyboardInput` arm). Order and chords: `CHORDS`.
    pub(crate) fn handle_global_shortcuts(&mut self, ui: &mut egui::Ui) {
        let fired: Vec<Shortcut> = ui.input_mut(|i| {
            let mut fired = Vec::new();
            for c in CHORDS {
                // An action's second chord is tried only if its first didn't fire, as `a || b` did.
                if !fired.contains(&c.action) && i.consume_key(c.mods, c.key) {
                    fired.push(c.action);
                }
            }
            fired
        });
        let on = |s: Shortcut| fired.contains(&s);
        if on(Shortcut::ReopenTab) {
            self.reopen_closed_shortcut();
        }
        if on(Shortcut::NewTab) {
            self.new_tab_shortcut();
        }
        if on(Shortcut::CloseAllLevel) {
            self.close_all_at_current_level();
        }
        if on(Shortcut::CloseTab) {
            self.close_focused_tab_shortcut();
        }
        if on(Shortcut::FocusAddress) {
            self.focus_address_bar_shortcut();
        }
        if on(Shortcut::OpenFind) {
            self.open_find_bar_shortcut();
        }
        if on(Shortcut::ZoomIn) {
            self.zoom_in_shortcut();
        }
        if on(Shortcut::ZoomOut) {
            self.zoom_out_shortcut();
        }
        if on(Shortcut::ZoomReset) {
            self.zoom_reset_shortcut();
        }
        if on(Shortcut::OpenDevtools) {
            self.open_devtools_shortcut();
        }
        if on(Shortcut::NextTab) || on(Shortcut::PrevTab) {
            self.cycle_focused_tab(on(Shortcut::NextTab));
        }
        if on(Shortcut::QuickSwitch) {
            self.open_quick_switcher();
        }
        if on(Shortcut::NewGroup) {
            self.new_group_shortcut();
        }
        // Neutralized while the quick-switcher is open, same reasoning as
        // its own Escape/arrow handling in `build_quick_switcher` -- a
        // `Ctrl+G` typed while it's open shouldn't also flip the view
        // underneath it.
        if on(Shortcut::ViewToggle) && self.menus.quick_switcher.is_none() {
            self.toggle_view_mode_shortcut();
        }
        if on(Shortcut::PinToggle) {
            self.pin_shortcut();
        }
        if on(Shortcut::Reload) {
            self.reload_focused_tab_shortcut();
        }
        if on(Shortcut::RenameGroup) {
            self.rename_current_group_shortcut();
        }
        if on(Shortcut::DuplicateTab) {
            self.duplicate_focused_tab_shortcut();
        }
        if on(Shortcut::ShortcutsHelp) {
            self.menus.shortcuts_help_open = !self.menus.shortcuts_help_open;
        }
        for (shortcut, dir) in
            [(Shortcut::Back, crate::nav_history::Dir::Back), (Shortcut::Forward, crate::nav_history::Dir::Forward)]
        {
            if let Some(id) = self.focused.clone().filter(|_| on(shortcut)) {
                self.navigate_history(&id, dir);
            }
        }
    }

    /// `Ctrl+P` / headless `TOGGLEPIN` — pins the selected cards, or unpins them all when every one is pinned; with
    /// nothing selected it toggles the focused card (the same effect as its pin dot or the menu's Pin/Unpin row).
    pub(crate) fn pin_shortcut(&mut self) {
        let ids: Vec<String> = if self.interaction.selected.is_empty() {
            self.focused.iter().cloned().collect()
        } else {
            self.interaction.selected.iter().cloned().collect()
        };
        let ids: Vec<String> = ids.into_iter().filter(|id| self.nodes.contains_key(id)).collect();
        let pin = ids.iter().any(|id| !self.mirror.pinned(id));
        for id in ids {
            self.edit_field(&id, field::PINNED, FieldValue::Bool(pin));
        }
    }

    /// The tab menu's Pin/Unpin row: toggles the focused tab alone, whatever is selected.
    pub(crate) fn pin_focused_tab_shortcut(&mut self) {
        let Some(id) = self.focused.clone() else { return };
        self.toggle_pin(&id);
    }

    /// Pins or unpins node `id` of any kind: a pinned card is left out of the graph layout.
    pub(crate) fn toggle_pin(&mut self, id: &str) {
        if self.nodes.contains_key(id) {
            let on = !self.mirror.pinned(id);
            self.edit_field(id, field::PINNED, FieldValue::Bool(on));
        }
    }

    /// `Ctrl+R`/`F5` / headless `RELOAD` — reloads the focused tab. Every
    /// engine exposes a plain no-argument reload (see `HostToView::
    /// Reload`'s own doc comment), so there's no can-reload guard the way
    /// `GoBack`/`GoForward` need one.
    pub(crate) fn reload_focused_tab_shortcut(&mut self) {
        let Some(id) = self.focused.clone() else { return };
        if let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
            node.tab.send(&HostToView::Reload);
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
        if !self.is_container(&group_id) {
            return;
        }
        // name rule: the field edits the stored name
        let name = self.mirror.name(&group_id).to_owned();
        self.renaming_group = Some((group_id, name));
        self.pending_focus = Some(FocusTarget::GroupRename);
    }

    /// `Ctrl+Shift+G` / toolbar "+ New Group" / headless `NEWGROUP` —
    /// creates an empty group at whichever level is currently being
    /// viewed, nested under `current_group` if that's not the top level
    /// (see `App::current_group`'s own doc comment on nesting). From a JSON
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
            self.set_focused_zoom((self.focused_zoom_factor() + 0.1).min(*crate::settings::ZOOM_RANGE.end()));
        }
    }

    /// `Ctrl+-` / headless `ZOOMOUT` — same Image-view/Tab-view split as
    /// `zoom_in_shortcut`; the Tab branch matches the settings popup's own
    /// "−" button.
    pub(crate) fn zoom_out_shortcut(&mut self) {
        if self.showing_image_view() {
            self.apply_image_zoom_action(ZoomChipAction::Out);
        } else {
            self.set_focused_zoom((self.focused_zoom_factor() - 0.1).max(*crate::settings::ZOOM_RANGE.start()));
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
    /// `HostToView::OpenDevTools`'s doc comment for per-engine support. An
    /// engine without DevTools (`ViewDesc::devtools`) gets a toast instead.
    pub(crate) fn open_devtools_shortcut(&mut self) {
        let Some(id) = self.focused.clone() else { return };
        let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) else { return };
        let kind = node.tab.kind;
        if kind.desc().devtools {
            node.tab.send(&HostToView::OpenDevTools);
        } else {
            self.queue_toast(format!("{} has no DevTools", kind.desc().label));
        }
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

    /// `Ctrl+Shift+T` / headless `REOPENTAB` — reopens the newest node this replica closed (spec decision 7: the
    /// replica id `closed` holds, which a restart keeps), where it was. Another replica's closes are left to the
    /// popup. A no-op with none.
    pub(crate) fn reopen_closed_shortcut(&mut self) {
        let Some(journal) = &self.journal else { return };
        let doc = journal.doc();
        let newest = doc.closed_roots().into_iter().find(|(id, _)| doc.nodes[id].closer() == self.replica);
        if let Some((id, _)) = newest {
            self.reopen_node(id);
        }
    }

    /// `Ctrl+W` / headless `CLOSETAB` — closes the focused tab, a no-op if
    /// none is focused (Graph view with nothing selected).
    pub(crate) fn close_focused_tab_shortcut(&mut self) {
        if let Some(id) = self.focused.clone() {
            self.close_node(&id);
        }
    }

    /// `Ctrl+D` / headless `DUPLICATETAB` — keyboard parity for the node
    /// context menu's own "Duplicate" entry: the selection when one is
    /// made, else the focused card. A no-op if neither exists.
    pub(crate) fn duplicate_focused_tab_shortcut(&mut self) {
        if let Some(id) = self.interaction.selected.iter().next().cloned().or_else(|| self.focused.clone()) {
            self.duplicate_selection(&id);
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
    /// the last term). An engine without find (`ViewDesc::find`) gets a toast
    /// instead.
    pub(crate) fn open_find_bar_shortcut(&mut self) {
        // On a JSON level, Ctrl+F searches the value instead.
        if self.view_mode == ViewMode::Graph && self.json_level.is_some() {
            self.focus_json_search();
            return;
        }
        if !self.showing_tab_view() {
            return;
        }
        // A sleeping or starting tab opens the bar as before; its engine is checked when it is live.
        let live = self.focused.as_deref().and_then(|id| self.nodes.get(id)).and_then(|n| n.as_tab());
        if let Some(kind) = live.map(|t| t.tab.kind).filter(|k| !k.desc().find) {
            self.queue_toast(format!("{} has no find in page", kind.desc().label));
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

#[cfg(test)]
mod chord_tests {
    use manifold_view_protocol::modifiers::{CONTROL, SHIFT};

    use super::*;

    #[test]
    fn shortcut_chords_are_claimed_and_page_chords_are_not() {
        assert!(claims_chord(WinitKeyCode::KeyW, CONTROL));
        assert!(claims_chord(WinitKeyCode::KeyT, CONTROL | SHIFT));
        assert!(claims_chord(WinitKeyCode::F5, 0));
        assert!(!claims_chord(WinitKeyCode::KeyC, CONTROL), "copy is the page's");
        assert!(!claims_chord(WinitKeyCode::KeyW, 0), "a plain W is typing");
        assert!(!claims_chord(WinitKeyCode::F5, SHIFT));
    }

    #[test]
    fn alt_left_and_right_are_back_and_forward_and_claimed_only_with_alt() {
        use manifold_view_protocol::modifiers::ALT;
        let action = |key| CHORDS.iter().find(|c| c.key == key && c.mods == egui::Modifiers::ALT).map(|c| c.action);
        assert_eq!(action(egui::Key::ArrowLeft), Some(Shortcut::Back));
        assert_eq!(action(egui::Key::ArrowRight), Some(Shortcut::Forward));
        for code in [WinitKeyCode::ArrowLeft, WinitKeyCode::ArrowRight] {
            assert!(claims_chord(code, ALT));
            assert!(claims_chord(code, ALT | SHIFT), "an extra Shift is ignored, as egui's matching ignores it");
            assert!(!claims_chord(code, 0), "a plain arrow is the page's (and the graph's)");
            assert!(!claims_chord(code, CONTROL), "Ctrl+arrow is a word jump");
            assert!(!claims_chord(code, CONTROL | ALT));
        }
    }

    /// `claims_chord` as it was before the table, kept verbatim as the reference.
    fn claims_chord_before(code: WinitKeyCode, modifiers: u8) -> bool {
        use WinitKeyCode::*;
        if modifiers & manifold_view_protocol::modifiers::CONTROL != 0 {
            matches!(
                code,
                KeyT | KeyW
                    | KeyL
                    | KeyF
                    | KeyD
                    | KeyP
                    | KeyK
                    | KeyG
                    | KeyR
                    | Tab
                    | Equal
                    | NumpadAdd
                    | Minus
                    | NumpadSubtract
                    | Digit0
                    | Numpad0
            )
        } else {
            modifiers == 0 && matches!(code, F1 | F2 | F5 | F12)
        }
    }

    #[test]
    fn the_table_claims_exactly_the_chords_it_did_before() {
        use WinitKeyCode::*;
        let codes = [
            KeyA,
            KeyD,
            KeyF,
            KeyG,
            KeyK,
            KeyL,
            KeyP,
            KeyR,
            KeyT,
            KeyW,
            KeyZ,
            Tab,
            Equal,
            NumpadAdd,
            Minus,
            NumpadSubtract,
            Digit0,
            Digit1,
            Numpad0,
            F1,
            F2,
            F3,
            F5,
            F12,
            Enter,
            Escape,
        ];
        let m = CONTROL | SHIFT;
        for code in codes {
            for mods in [0, CONTROL, SHIFT, m] {
                assert_eq!(claims_chord(code, mods), claims_chord_before(code, mods), "{code:?} {mods:#x}");
            }
        }
    }

    #[test]
    fn every_ctrl_shift_chord_is_tried_before_its_plain_ctrl_twin() {
        for (i, c) in CHORDS.iter().enumerate() {
            if c.mods.shift {
                assert!(
                    !CHORDS[..i].iter().any(|p| p.key == c.key && p.mods == egui::Modifiers::CTRL),
                    "{:?}: plain Ctrl would consume Ctrl+Shift first",
                    c.action
                );
            }
        }
    }
}
