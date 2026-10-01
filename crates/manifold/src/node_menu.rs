//! The node context menu: its actions, config and the App methods that build and apply it.

use super::*;
use crate::sleep_state::SleepWhy;

/// Something done to one node, from its right-click menu or its card's
/// own buttons, applied by `App::apply_node_action`.
/// `build_node_context_menu` and the card loops only report which one was
/// clicked; the caller owns every effect.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum NodeAction {
    /// Opens the node into Item view (a tab or image card's click).
    Open,
    CopyText,
    Reveal,
    CopyPath,
    /// Fetches a node's source URL again, in place.
    Refresh,
    /// A new tab at the node's source URL, beside it.
    OpenAsTab,
    TogglePin,
    Duplicate,
    /// A live tab goes to sleep (`App::request_sleep`).
    Sleep,
    /// A group's live tabs all do (`App::sleep_group`).
    SleepGroup,
    CopyUrl,
    FitSubtree,
    /// Swaps the group menu's items for a name field (see
    /// `build_node_context_menu`); the menu stays open.
    Rename,
    /// The rename field lost focus by any means but Escape. Carries the
    /// typed name rather than reading `renaming_group` when applied, so it
    /// still lands if leaving the group (which clears the draft) is applied
    /// first: clicking the toolbar's back button while renaming does both
    /// in one frame.
    CommitRename(String),
    /// Escape in the rename field.
    CancelRename,
    SetColor(Option<egui::Color32>),
    /// A card's color swatch: the next color in `TAG_COLOR_PALETTE`.
    CycleColor,
    ToggleMute,
    RetryMissingFile,
    /// A failed tab's card: tries again on the default engine.
    OpenInDefaultEngine,
    RemoveFromGroup,
    Dissolve,
    ExportGroup,
    /// Delete for a file card, "Remove from List" for a download, Close
    /// for a tab or group.
    Delete,
}

impl NodeAction {
    pub(crate) fn hover_text(&self) -> Option<&'static str> {
        match self {
            NodeAction::FitSubtree => Some("Zoom the graph to frame just this tab and its children"),
            _ => None,
        }
    }
}

#[derive(Debug, PartialEq)]
pub(crate) enum NodeMenuItem {
    Button(&'static str, NodeAction),
    /// The "Color" label and tag-color swatch row, ending in a "no color"
    /// ring.
    Colors,
    Separator,
}

/// A node's right-click menu: its items, in order. `area_id` is the
/// popup's egui `Area` id, distinct per kind so egui's per-area memory
/// never bleeds between kinds. Every menu has a minimum width and no
/// maximum, so it grows to fit the swatch row.
#[derive(Debug, PartialEq)]
pub(crate) struct NodeMenuConfig {
    area_id: &'static str,
    min_width: f32,
    items: Vec<NodeMenuItem>,
}

/// Draws a node's right-click menu and reports what was clicked this
/// frame, plus whether the click landed outside the menu. A free function,
/// like `build_list_popup_frame`, so the caller keeps `&mut self` free to
/// apply the action. `pos` stands in for a zero-size anchor rect, so the
/// menu opens right at the pointer. `rename` is the group-name draft (and
/// whether to focus it this frame) while a group is being renamed: the
/// menu then shows just that field. `color` rings the node's current
/// swatch.
pub(crate) fn build_node_context_menu(
    ui: &mut egui::Ui,
    pos: egui::Pos2,
    fade: f32,
    interactable: bool,
    config: &NodeMenuConfig,
    color: Option<egui::Color32>,
    rename: Option<(&mut String, bool)>,
) -> (Option<NodeAction>, bool) {
    let mut action = None;
    let anchor = egui::Rect::from_min_size(pos, egui::Vec2::ZERO);
    let area_rect = build_list_popup_frame(
        ui.ctx(),
        config.area_id,
        anchor,
        config.min_width,
        f32::INFINITY,
        fade,
        interactable,
        |ui| {
            if let Some((draft, focus)) = rename {
                let response = ui.add(egui::TextEdit::singleline(draft).desired_width(140.0));
                if focus {
                    response.request_focus();
                }
                if response.lost_focus() {
                    action = Some(if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        NodeAction::CancelRename
                    } else {
                        NodeAction::CommitRename(draft.clone())
                    });
                }
                return;
            }
            for item in &config.items {
                match item {
                    NodeMenuItem::Button(label, item_action) => {
                        let mut response = ui.button(*label);
                        if let Some(text) = item_action.hover_text() {
                            response = response.on_hover_text(text);
                        }
                        if response.clicked() {
                            action = Some(item_action.clone());
                        }
                    }
                    NodeMenuItem::Colors => {
                        if let Some(picked) = color_swatch_row(ui, color) {
                            action = Some(NodeAction::SetColor(picked));
                        }
                    }
                    NodeMenuItem::Separator => {
                        ui.separator();
                    }
                }
            }
        },
    );
    let clicked_outside = clicked_outside_popup(ui, area_rect, None);
    (action, clicked_outside)
}

/// The "Color" label and one swatch per `TAG_COLOR_PALETTE` entry, then a
/// "no color" ring; `current` gets a ring of its own. Returns the clicked
/// choice (`Some(None)` for no color).
pub(crate) fn color_swatch_row(ui: &mut egui::Ui, current: Option<egui::Color32>) -> Option<Option<egui::Color32>> {
    let mut picked = None;
    ui.add_space(4.0);
    ui.label("Color");
    ui.horizontal(|ui| {
        for choice in TAG_COLOR_PALETTE.iter().copied().map(Some).chain([None]) {
            let (rect, response) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
            let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
            match choice {
                Some(swatch) => {
                    ui.painter().circle_filled(rect.center(), 8.0, swatch);
                    if current == Some(swatch) {
                        ui.painter().circle_stroke(
                            rect.center(),
                            8.0,
                            egui::Stroke::new(2.0, ui.visuals().text_color()),
                        );
                    }
                }
                None => {
                    ui.painter().circle_stroke(
                        rect.center(),
                        8.0,
                        egui::Stroke::new(1.5, ui.visuals().weak_text_color()),
                    );
                }
            }
            if response.clicked() {
                picked = Some(choice);
            }
        }
    });
    picked
}

/// Which items a node's right-click menu shows, from the state frozen in
/// `view`. Export Group is left out in headless mode, since its file
/// dialog would block on input that never comes (`EXPORTGROUP <path>`
/// saves directly instead).
pub(crate) fn node_menu_config(view: &NodeMenuView, headless: bool) -> NodeMenuConfig {
    use NodeAction as A;
    use NodeMenuItem::{Button, Colors, Separator};
    let file_items = || vec![Button("Open Containing Folder", A::Reveal), Button("Copy Path", A::CopyPath)];
    // Refresh (Image/Pdf/Json only) and Open as Tab (any kind with a url),
    // first in the menu.
    let url_items = |refresh: bool| {
        let mut items = Vec::new();
        if view.has_url && refresh {
            items.push(Button("Refresh", A::Refresh));
        }
        if view.has_url {
            items.push(Button("Open as Tab", A::OpenAsTab));
        }
        items
    };
    // Every menu ends in the Colors row, Remove from Group (when the node
    // is grouped), then the kind's delete items.
    let finish = |area_id, min_width, mut items: Vec<NodeMenuItem>, delete_items: Vec<NodeMenuItem>| {
        items.push(Colors);
        if view.has_group {
            items.extend([Separator, Button("Remove from Group", A::RemoveFromGroup)]);
        }
        items.push(Separator);
        items.extend(delete_items);
        NodeMenuConfig { area_id, min_width, items }
    };
    let delete = |label| vec![Button(label, A::Delete)];
    match view.kind {
        ItemNodeKind::Download => finish("download_context_menu", 170.0, file_items(), delete("Remove from List")),
        ItemNodeKind::Note => {
            let mut items = url_items(false);
            items.push(Button("Copy Text", A::CopyText));
            finish("note_context_menu", 150.0, items, delete("Delete"))
        }
        ItemNodeKind::Image => {
            let mut items = url_items(true);
            items.extend(file_items());
            finish("image_context_menu", 170.0, items, delete("Delete"))
        }
        ItemNodeKind::Pdf => {
            let mut items = url_items(true);
            items.extend(file_items());
            finish("pdf_context_menu", 170.0, items, delete("Delete"))
        }
        ItemNodeKind::Json => {
            let mut items = url_items(true);
            items.extend(file_items());
            finish("json_context_menu", 170.0, items, delete("Delete"))
        }
        ItemNodeKind::Tab => {
            let mut items = vec![
                Button(if view.pinned { "Unpin" } else { "Pin" }, A::TogglePin),
                Button("Duplicate", A::Duplicate),
                Button("Sleep", A::Sleep),
                Button("Copy URL", A::CopyUrl),
            ];
            items.push(Button("Fit Subtree", A::FitSubtree));
            finish("tab_context_menu", 150.0, items, delete("Close"))
        }
        ItemNodeKind::Unloaded => {
            let mut items =
                if view.failed { vec![Button("Retry", A::RetryMissingFile)] } else { vec![Button("Open", A::Open)] };
            if view.failed && view.other_default_engine {
                items.push(Button("Open in Default Engine", A::OpenInDefaultEngine));
            }
            items.push(Button(if view.pinned { "Unpin" } else { "Pin" }, A::TogglePin));
            items.push(Button("Copy URL", A::CopyUrl));
            finish("unloaded_tab_context_menu", 170.0, items, delete("Close"))
        }
        ItemNodeKind::Group => {
            let mut delete_items = vec![Button("Dissolve", A::Dissolve), Button("Close Group", A::Delete)];
            if !headless {
                delete_items.push(Button("Export Group...", A::ExportGroup));
            }
            finish(
                "group_context_menu",
                150.0,
                vec![Button("Rename", A::Rename), Button("Sleep Tabs", A::SleepGroup)],
                delete_items,
            )
        }
    }
}

/// Everything a node's right-click menu shows that its own actions can
/// change, read each frame while the menu is open and frozen in
/// `node_menu_cache` for the fade-out. The fading menu keeps the look it
/// had when an item was clicked, instead of redrawing the new state (a
/// "Remove from Group" row vanishing, "Pin" turning into "Unpin", ...), and
/// still fades when the action removed the node itself.
#[derive(Clone)]
pub(crate) struct NodeMenuView {
    kind: ItemNodeKind,
    has_group: bool,
    color: Option<egui::Color32>,
    /// Image/Pdf/Json/Note: `node.source_url().is_some()`. Offers Refresh
    /// (Image/Pdf/Json) and Open as Tab (any of the four).
    has_url: bool,
    /// Tab only.
    pinned: bool,
    /// Unloaded only: the default engine isn't the one that failed, so
    /// the menu offers it.
    other_default_engine: bool,
    /// Unloaded only: its engine failed, so the menu offers a retry.
    failed: bool,
    /// Group only: the rename draft, while this group is being renamed.
    renaming: Option<String>,
}

impl App {
    /// The minimal right-click context menu: one "Open in new tab" item at
    /// `self.context_menu`'s position, if a menu is currently showing.
    /// Dismissed by clicking the item (and acting on it), a click anywhere
    /// outside the menu's own rect, or Escape (see the Escape-priority
    /// chain in `build_tab_view_ui`).
    pub(crate) fn build_context_menu(&mut self, ui: &mut egui::Ui) {
        let current = self.context_menu.clone();
        let Some((menu, fade)) =
            fade_popup_state(ui.ctx(), egui::Id::new("context_menu_fade"), &current, &mut self.context_menu_cache)
        else {
            return;
        };
        let mut open_tab = false;
        let mut close = false;

        let area = egui::Area::new(egui::Id::new("context_menu"))
            .fixed_pos(egui::pos2(menu.x, menu.y))
            .order(egui::Order::Foreground)
            .interactable(current.is_some())
            .show(ui.ctx(), |ui| {
                ui.set_opacity(fade);
                popup_frame(ui).show(ui, |ui| {
                    menu_item_style(ui);
                    if ui.button("Open in new tab").clicked() {
                        open_tab = true;
                        close = true;
                    }
                });
            });

        let clicked_outside = clicked_outside_popup(ui, area.response.rect, None);
        if clicked_outside {
            close = true;
        }

        if open_tab {
            let parent = self.focused.clone();
            let group = self.group_of(&parent);
            self.spawn_new_tab(parent, group, &menu.href);
        }
        if close {
            self.context_menu = None;
        }
    }

    /// The one right-click context menu for every node card. Fades via the
    /// shared `node_menu`/`node_menu_cache` (one fade Id for all kinds --
    /// per-kind Ids would go unpolled while another kind's menu is open,
    /// breaking `fade_popup_state`'s continuous-polling rule), draws the
    /// items `node_menu_config` lists for the node's kind, and applies the
    /// clicked one. A node that's gone (closed from its own menu, or
    /// elsewhere) closes the menu. While fading out, the menu draws its
    /// frozen `NodeMenuView` and is click-through, so a second click (say,
    /// the rest of a double-click) can't act again. While a group is being
    /// renamed, a click outside commits the name (the field loses focus)
    /// rather than just closing the menu.
    pub(crate) fn build_node_menu(&mut self, ui: &mut egui::Ui) {
        if self.node_menu.as_ref().is_some_and(|(id, _)| !self.nodes.contains_key(id)) {
            self.node_menu = None;
        }
        let current = self.node_menu.clone().and_then(|(id, pos)| self.node_menu_view(&id).map(|view| (id, pos, view)));
        let Some(((id, pos, view), fade)) =
            fade_popup_state(ui.ctx(), egui::Id::new("node_menu_fade"), &current, &mut self.node_menu_cache)
        else {
            return;
        };
        let live = current.is_some();
        let config = node_menu_config(&view, self.headless);
        let mut frozen_draft = view.renaming.clone();
        let rename = match &mut self.renaming_group {
            Some((gid, draft)) if live && *gid == id => {
                Some((draft, take_focus(&mut self.pending_focus, FocusTarget::GroupRename)))
            }
            _ => frozen_draft.as_mut().map(|draft| (draft, false)),
        };
        let (action, clicked_outside) = build_node_context_menu(ui, pos, fade, live, &config, view.color, rename);
        let close = match action {
            Some(NodeAction::Rename) => false,
            Some(_) => true,
            None => clicked_outside && view.renaming.is_none(),
        };
        if let Some(action) = action {
            self.apply_node_action(ui.ctx(), &id, action);
            self.save_session();
        }
        if close {
            self.node_menu = None;
        }
    }

    pub(crate) fn node_menu_view(&self, id: &str) -> Option<NodeMenuView> {
        let node = self.nodes.get(id)?;
        Some(NodeMenuView {
            kind: item_node_kind(&node.payload),
            has_group: node.group.is_some(),
            color: node.color,
            has_url: node.source_url().is_some(),
            pinned: node.payload.pinned(),
            other_default_engine: node
                .payload
                .as_unloaded()
                .is_some_and(|t| t.kind != self.settings.default_starting_engine),
            failed: node.payload.as_unloaded().is_some_and(|t| matches!(t.state, UnloadedState::Failed(_))),
            renaming: self.renaming_group.as_ref().filter(|(gid, _)| gid == id).map(|(_, draft)| draft.clone()),
        })
    }

    /// Applies whichever item was clicked in a node's right-click menu.
    /// Each action only ever appears on kinds where it makes sense
    /// (`node_menu_config`), so none of these re-check the kind.
    pub(crate) fn apply_node_action(&mut self, ctx: &egui::Context, id: &str, action: NodeAction) {
        match action {
            NodeAction::Open => {
                if self.nodes.contains_key(id) {
                    self.selected_tabs.clear();
                    self.start_transition_to_item(id);
                }
            }
            NodeAction::CopyText => {
                if let Some(text) = self.nodes.get(id).and_then(|n| n.as_note()).map(|n| n.text.clone()) {
                    ctx.copy_text(text);
                }
            }
            NodeAction::Reveal => self.reveal_node(id),
            NodeAction::CopyPath => {
                if let Some(path) = self.node_file_path(id) {
                    ctx.copy_text(path);
                }
            }
            NodeAction::Refresh => self.refresh_node(id),
            NodeAction::OpenAsTab => self.open_as_tab(id),
            NodeAction::TogglePin => {
                if let Some(pinned) = self.nodes.get_mut(id).and_then(|n| n.payload.pinned_mut()) {
                    *pinned = !*pinned;
                }
            }
            NodeAction::Duplicate => self.duplicate_tab(id),
            NodeAction::Sleep => {
                self.request_sleep(id, SleepWhy::Manual);
            }
            NodeAction::SleepGroup => {
                self.sleep_group(id);
            }
            NodeAction::CopyUrl => {
                if let Some((url, _)) = self.nodes.get(id).and_then(|n| n.payload.saved_tab()) {
                    ctx.copy_text(url.to_string());
                }
            }
            NodeAction::FitSubtree => self.camera.jump_to(self.fit_subtree_scene_rect(id)),
            NodeAction::Rename => {
                let name = self.nodes.get(id).and_then(|n| n.as_group()).map(|g| g.name.clone()).unwrap_or_default();
                self.renaming_group = Some((id.to_string(), name));
                self.pending_focus = Some(FocusTarget::GroupRename);
            }
            NodeAction::CommitRename(name) => {
                self.renaming_group = None;
                let trimmed = name.trim();
                if !trimmed.is_empty() {
                    if let Some(group) = self.nodes.get_mut(id).and_then(|n| n.as_group_mut()) {
                        group.name = trimmed.to_string();
                    }
                }
            }
            NodeAction::CancelRename => self.renaming_group = None,
            NodeAction::SetColor(color) => {
                if let Some(node) = self.nodes.get_mut(id) {
                    node.color = color;
                }
            }
            NodeAction::CycleColor => {
                if let Some(node) = self.nodes.get_mut(id) {
                    node.color = next_tag_color(node.color);
                }
            }
            NodeAction::ToggleMute => self.toggle_muted(id),
            NodeAction::RetryMissingFile => self.retry_missing_file(id),
            NodeAction::OpenInDefaultEngine => self.wake_tab(id, self.settings.default_starting_engine),
            NodeAction::RemoveFromGroup => self.eject(id),
            NodeAction::Dissolve => self.dissolve_group(id),
            NodeAction::ExportGroup => {
                let name = self.nodes.get(id).and_then(|n| n.as_group()).map(|g| g.name.clone()).unwrap_or_default();
                if let Some(json) = self.export_group_json(id) {
                    if let Some((path, toast)) = save_dialog_with_toast(format!("{name}.json"), "Group") {
                        write_json_export(&json, &path, "group");
                        self.queue_toast(toast.0);
                    }
                }
            }
            NodeAction::Delete => self.close_node_and_save(id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // One row per kind, pinned to the exact menus each kind showed before
    // the per-kind builders were merged (see the node-menu spec), and
    // before the tab and group menus joined them.
    #[test]
    fn node_menu_config_matches_each_kinds_menu() {
        fn view_of(kind: ItemNodeKind) -> NodeMenuView {
            NodeMenuView {
                kind,
                has_group: false,
                color: None,
                has_url: false,
                pinned: false,
                other_default_engine: false,
                failed: false,
                renaming: None,
            }
        }
        fn menu_of(view: &NodeMenuView, headless: bool) -> (&'static str, f32, String) {
            let config = node_menu_config(view, headless);
            let items: Vec<&str> = config
                .items
                .iter()
                .map(|item| match item {
                    NodeMenuItem::Button(label, _) => *label,
                    NodeMenuItem::Colors => "<colors>",
                    NodeMenuItem::Separator => "---",
                })
                .collect();
            (config.area_id, config.min_width, items.join(" | "))
        }
        fn menu(kind: ItemNodeKind, has_group: bool, pinned: bool, headless: bool) -> (&'static str, f32, String) {
            let view = NodeMenuView { has_group, pinned, ..view_of(kind) };
            menu_of(&view, headless)
        }
        for has_group in [false, true] {
            // Every kind can join a group (drag onto it), so every menu
            // offers "Remove from Group" when grouped.
            let g = if has_group { " | --- | Remove from Group" } else { "" };
            let menu = |kind, pinned| menu(kind, has_group, pinned, false);
            assert_eq!(
                menu(ItemNodeKind::Download, false),
                (
                    "download_context_menu",
                    170.0,
                    format!("Open Containing Folder | Copy Path | <colors>{g} | --- | Remove from List")
                )
            );
            assert_eq!(
                menu(ItemNodeKind::Note, false),
                ("note_context_menu", 150.0, format!("Copy Text | <colors>{g} | --- | Delete"))
            );
            assert_eq!(
                menu(ItemNodeKind::Image, false),
                (
                    "image_context_menu",
                    170.0,
                    format!("Open Containing Folder | Copy Path | <colors>{g} | --- | Delete")
                )
            );
            assert_eq!(
                menu(ItemNodeKind::Pdf, false),
                ("pdf_context_menu", 170.0, format!("Open Containing Folder | Copy Path | <colors>{g} | --- | Delete"))
            );
            assert_eq!(
                menu(ItemNodeKind::Json, false),
                (
                    "json_context_menu",
                    170.0,
                    format!("Open Containing Folder | Copy Path | <colors>{g} | --- | Delete")
                )
            );
            assert_eq!(
                menu(ItemNodeKind::Tab, false),
                (
                    "tab_context_menu",
                    150.0,
                    format!("Pin | Duplicate | Sleep | Copy URL | Fit Subtree | <colors>{g} | --- | Close")
                )
            );
            assert_eq!(
                menu(ItemNodeKind::Tab, true).2,
                format!("Unpin | Duplicate | Sleep | Copy URL | Fit Subtree | <colors>{g} | --- | Close")
            );
            assert_eq!(
                menu(ItemNodeKind::Group, false),
                (
                    "group_context_menu",
                    150.0,
                    format!("Rename | Sleep Tabs | <colors>{g} | --- | Dissolve | Close Group | Export Group...")
                )
            );
            assert_eq!(
                menu(ItemNodeKind::Unloaded, false),
                ("unloaded_tab_context_menu", 170.0, format!("Open | Pin | Copy URL | <colors>{g} | --- | Close"))
            );
        }
        // A failed tab offers Retry (and the default engine, when it isn't
        // the engine that failed) instead of Open; a sleeping/waking one
        // just offers Open.
        let failed = NodeMenuView { other_default_engine: true, failed: true, ..view_of(ItemNodeKind::Unloaded) };
        assert_eq!(
            menu_of(&failed, false).2,
            "Retry | Open in Default Engine | Pin | Copy URL | <colors> | --- | Close"
        );
        // Export Group's file dialog can't run headless.
        assert_eq!(
            menu(ItemNodeKind::Group, false, false, true).2,
            "Rename | Sleep Tabs | <colors> | --- | Dissolve | Close Group"
        );
        // A url on Image/Pdf/Json/Note offers Refresh (not Note) and Open as
        // Tab, ahead of the kind's own items.
        let with_url = |kind| NodeMenuView { kind, has_url: true, ..view_of(kind) };
        assert_eq!(
            menu_of(&with_url(ItemNodeKind::Image), false).2,
            "Refresh | Open as Tab | Open Containing Folder | Copy Path | <colors> | --- | Delete"
        );
        assert_eq!(
            menu_of(&with_url(ItemNodeKind::Json), false).2,
            "Refresh | Open as Tab | Open Containing Folder | Copy Path | <colors> | --- | Delete"
        );
        assert_eq!(
            menu_of(&with_url(ItemNodeKind::Note), false).2,
            "Open as Tab | Copy Text | <colors> | --- | Delete"
        );
    }
}
