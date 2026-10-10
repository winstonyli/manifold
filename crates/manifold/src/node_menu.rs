//! The node context menu: its actions, config and the App methods that build and apply it.

use super::*;
use crate::jar_defs::JarLabel;
use crate::sleep_state::SleepWhy;
use crate::store::model::{
    NodeId, TYPE_DOWNLOAD, TYPE_GROUP, TYPE_IMAGE, TYPE_JSON, TYPE_JSON_LEAF, TYPE_NOTE, TYPE_PDF, TYPE_TAB,
};

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
    /// The keyboard route for dragging a card onto a sibling group (`DropEffect::Join`). On a group it differs
    /// from the drag: "Move into" nests the group inside the other, where dropping it there merges the two
    /// (`drop_action`).
    MoveToGroup(String),
    Dissolve,
    ExportGroup,
    /// Delete for a file card, "Remove from List" for a download, Close
    /// for a tab, "Close Group and Contents" for a group.
    Delete,
    /// A tab's jar row: that jar (`None`: the default jar), picked by hand (`App::set_tab_jar`).
    SetJar(Option<NodeId>),
    /// "New Jar...": swaps the tab menu's items for a name field (`App::naming_jar`); the menu stays open.
    StartNewJar,
    /// Enter in the name field: creates the jar and picks it for the tab. The mapped `CommitRename` (see
    /// `build_node_menu`).
    NewJar(String),
    /// Escape in the name field, or the field losing focus without Enter (a click away; user decision, unlike a
    /// group rename): nothing is created.
    CancelNewJar,
    FollowGroupJar,
    SwitchJarNow,
    /// A group's jar row: its default for new tabs, cascaded to the unset tabs it governs.
    SetGroupDefaultJar(NodeId),
    ClearGroupDefaultJar,
    /// "Link to…", "Opened from…": starts pick-target mode for this edge (`App::connecting`); the next click on a
    /// valid target draws it (`App::finish_connect`).
    Connect(&'static str),
    /// "Disconnect from X": removes the `edge` edge between this node and `other`. `link`: the Link node the row
    /// lists, for an edge stored as one.
    Disconnect {
        edge: &'static str,
        other: String,
        link: Option<String>,
    },
}

impl NodeAction {
    /// The key that does this to a keyboard-focused card of `kind`, shown at the row's right edge. Del is
    /// whatever the card's Close does (`CardControl::Close`): Dissolve on a group, so "Close Group and
    /// Contents" (`Delete`), which closes the members too, is never hinted Del.
    pub(crate) fn key_hint(&self, kind: CardKind) -> Option<&'static str> {
        (*self == CardControl::Close.action(kind)).then_some("Del")
    }

    pub(crate) fn hover_text(&self) -> Option<&'static str> {
        match self {
            NodeAction::FitSubtree => Some("Zoom the graph to frame just this tab and its children"),
            NodeAction::SwitchJarNow => Some("Reload this tab in its new jar now"),
            _ => None,
        }
    }
}

/// One jar in a tab's or group's menu.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct JarRow {
    /// `None`: the default jar (tab menus only).
    pub(crate) id: Option<NodeId>,
    pub(crate) name: JarLabel,
    pub(crate) color: Option<egui::Color32>,
    /// The tab's jar, or the group's default.
    pub(crate) ticked: bool,
    /// Group menus: how many tabs picking it would change (`App::cascade_count`); `None` when none would, so no
    /// "(0)" shows.
    pub(crate) count: Option<usize>,
}

/// The jar part of a tab's or group's menu (`App::jar_menu`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct JarMenu {
    pub(crate) rows: Vec<JarRow>,
    /// A hand-picked tab whose group has a default: offers "Follow Group's Jar".
    pub(crate) can_follow: bool,
    /// A live tab waiting to switch with nothing else replacing its host (`App::can_switch_jar_now`): offers
    /// "Switch Jar Now".
    pub(crate) will_switch: bool,
    /// A group with a default: offers "Clear Default Jar".
    pub(crate) has_default: bool,
}

/// One row of a node menu's "Links" section: a verb that starts pick-target mode, or a "Disconnect from X".
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EdgeMenuRow {
    pub(crate) action: NodeAction,
    pub(crate) label: String,
    /// The row's hover: what happens next for a verb, the other card's whole name for a disconnect cut short.
    pub(crate) hover: Option<String>,
}

#[derive(Debug, PartialEq)]
pub(crate) enum NodeMenuItem {
    Button(&'static str, NodeAction),
    /// The "Color" label and tag-color swatch row, ending in a "no color"
    /// ring.
    Colors,
    /// One "Move into <name>" button per `(group id, name)`: the sibling groups a card can join.
    MoveTo(Vec<(String, String)>),
    /// A small heading, then one row per jar with its colour dot and a tick on the current one. `group`: the rows
    /// set the group's default rather than the tab's jar.
    Jars {
        heading: &'static str,
        rows: Vec<JarRow>,
        group: bool,
    },
    /// The "Links" heading, then its rows (`NodeMenuView::edges`).
    Edges(Vec<EdgeMenuRow>),
    Separator,
}

/// A node's right-click menu: its items, in order. `area_id` is the
/// popup's egui `Area` id, distinct per kind so egui's per-area memory
/// never bleeds between kinds. Every menu has a minimum width and no
/// maximum, so it grows to fit the swatch row.
#[derive(Debug, PartialEq)]
pub(crate) struct NodeMenuConfig {
    /// Whose menu: picks the row hinted Del (`NodeAction::key_hint`).
    kind: CardKind,
    area_id: &'static str,
    min_width: f32,
    items: Vec<NodeMenuItem>,
}

/// Draws a node's right-click menu and reports what was clicked this
/// frame, plus whether the click landed outside the menu. A free function,
/// like `build_list_popup_frame`, so the caller keeps `&mut self` free to
/// apply the action. `pos` stands in for a zero-size anchor rect, so the
/// menu opens right at the pointer. `rename` is the name draft (with
/// whether to focus it this frame, and the field's hint text) while a group
/// is being renamed or a jar named: the menu then shows just that field. `color` rings the node's current
/// swatch.
pub(crate) fn build_node_context_menu(
    ui: &mut egui::Ui,
    pos: egui::Pos2,
    fade: f32,
    interactable: bool,
    config: &NodeMenuConfig,
    color: Option<egui::Color32>,
    rename: Option<(&mut String, bool, Option<&str>)>,
) -> (Option<NodeAction>, bool) {
    let mut action = None;
    // The first item takes focus when the menu opens, so it can be driven from the keyboard.
    let open_key = egui::Id::new((config.area_id, "was_open"));
    let just_opened = interactable && !ui.data(|d| d.get_temp::<bool>(open_key).unwrap_or(false));
    ui.data_mut(|d| d.insert_temp(open_key, interactable));
    let mut first_item = just_opened;
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
            if let Some((draft, focus, hint)) = rename {
                let mut edit = egui::TextEdit::singleline(draft).desired_width(140.0);
                if let Some(hint) = hint {
                    edit = edit.hint_text(hint);
                }
                let response = ui.add(edit);
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
                        let mut response = menu_button(ui, *label, item_action.key_hint(config.kind), &mut first_item);
                        if let Some(text) = item_action.hover_text() {
                            response = response.on_hover_text(text);
                        }
                        if response.clicked() {
                            action = Some(item_action.clone());
                        }
                    }
                    NodeMenuItem::MoveTo(targets) => {
                        for (group, name) in targets {
                            // Cut to `ROW_UNITS`, so a long name cannot widen the menu; the hover has all of it.
                            let segs = crate::names::display_name(name, crate::names::UNTITLED);
                            let shown = crate::names::shorten(&segs, crate::names::ROW_UNITS);
                            let text = crate::names::name_job(ui, "Move into ", shown.as_deref().unwrap_or(&segs), "");
                            let mut response = menu_button(ui, text, None, &mut first_item);
                            let hover = crate::names::name_hover(name, crate::names::ROW_UNITS, "", str::to_string);
                            if !hover.is_empty() {
                                response = response.on_hover_text(hover);
                            }
                            if response.clicked() {
                                action = Some(NodeAction::MoveToGroup(group.clone()));
                            }
                        }
                    }
                    NodeMenuItem::Jars { heading, rows, group } => {
                        menu_section(ui, heading, |ui, grid| {
                            for row in rows {
                                // The label's marker in its own style (`JarLabel::append_to`); the rest in the button's.
                                let font = egui::FontSelection::Default.resolve(ui.style());
                                let plain = egui::TextFormat::simple(font.clone(), egui::Color32::PLACEHOLDER);
                                let mut text = egui::text::LayoutJob::default();
                                // The colour dot is painted in this leading gap, at the menu text's margin.
                                text.append("", MenuGrid::MARKER_SLOT, plain.clone());
                                row.name.append_to(&mut text, font, ui.visuals().weak_text_color());
                                if let Some(n) = row.count {
                                    text.append(&format!("({n})"), 8.0, plain);
                                }
                                // The current jar is the selected row: no tick to widen it.
                                let response = menu_row(ui, text, None, &mut first_item, row.ticked);
                                let dot = grid.marker_center(response.rect);
                                // The default jar has no colour: an outlined dot.
                                match row.color {
                                    Some(c) => {
                                        ui.painter().circle_filled(dot, 4.0, c);
                                    }
                                    None => {
                                        let stroke = egui::Stroke::new(1.0, ui.visuals().weak_text_color());
                                        ui.painter().circle_stroke(dot, 4.0, stroke);
                                    }
                                }
                                let response = match row.count {
                                    Some(n) if *group => response
                                        .on_hover_text(format!("Tabs following this group that would change: {n}")),
                                    _ => response,
                                };
                                if response.clicked() {
                                    action = Some(match (group, row.id) {
                                        (true, Some(id)) => NodeAction::SetGroupDefaultJar(id),
                                        (true, None) => NodeAction::ClearGroupDefaultJar,
                                        (false, id) => NodeAction::SetJar(id),
                                    });
                                }
                            }
                        });
                    }
                    NodeMenuItem::Edges(rows) => {
                        menu_section(ui, "Links", |ui, _| {
                            for row in rows {
                                let mut response = menu_button(ui, row.label.as_str(), None, &mut first_item);
                                if let Some(hover) = &row.hover {
                                    response = response.on_hover_text(hover);
                                }
                                if response.clicked() {
                                    action = Some(row.action.clone());
                                }
                            }
                        });
                    }
                    NodeMenuItem::Colors => {
                        if let Some(picked) = color_swatch_row(ui, color) {
                            action = Some(NodeAction::SetColor(picked));
                        }
                    }
                    NodeMenuItem::Separator => {
                        ui.add_space(3.0);
                        ui.separator();
                        ui.add_space(3.0);
                    }
                }
            }
        },
    );
    let clicked_outside = clicked_outside_popup(ui, area_rect, None);
    (action, clicked_outside)
}

/// Export Group's suggested file name: the group's clean name (name rules), or "Untitled" when nothing is left, then
/// ".json". Never the placeholder form, whose `<` `>` Windows forbids in file names.
fn export_file_name(name: &str) -> String {
    let clean = crate::names::clean_name(name);
    format!("{}.json", if clean.is_empty() { crate::names::UNTITLED } else { &clean })
}

/// The menu's layout grid. Every section (a heading and what follows) takes its spacing and left margin from here,
/// so a new section lines up with the others instead of hand-tuning numbers (the colour row and the jar rows each
/// had their own spacing bugs).
struct MenuGrid {
    /// Where the rows' text starts: the button padding. Headings and markers start here too.
    margin: f32,
}

impl MenuGrid {
    /// Space above a section's heading and below its content.
    const SECTION_GAP: f32 = 6.0;
    /// Space between a heading and its first row.
    const HEADING_GAP: f32 = 3.0;
    /// Width of a marker (a jar's colour dot, a swatch) and the gap after it, at the left margin.
    const MARKER_SLOT: f32 = 16.0;
    /// Space between neighbouring markers in a row of them.
    const MARKER_GAP: f32 = 4.0;

    fn of(ui: &egui::Ui) -> Self {
        Self { margin: ui.spacing().button_padding.x }
    }

    /// The centre of the marker slot in a menu row at `row`.
    fn marker_center(&self, row: egui::Rect) -> egui::Pos2 {
        row.left_center() + egui::vec2(self.margin + Self::MARKER_SLOT / 2.0, 0.0)
    }
}

/// A heading in the menu's small weak style, then `body`, with the grid's spacing around both.
fn menu_section<R>(ui: &mut egui::Ui, heading: &str, body: impl FnOnce(&mut egui::Ui, &MenuGrid) -> R) -> R {
    let grid = MenuGrid::of(ui);
    ui.add_space(MenuGrid::SECTION_GAP);
    ui.horizontal(|ui| {
        ui.add_space(grid.margin);
        ui.label(egui::RichText::new(heading).small().weak());
    });
    ui.add_space(MenuGrid::HEADING_GAP);
    let out = body(ui, &grid);
    ui.add_space(MenuGrid::SECTION_GAP);
    out
}

/// A menu row; takes focus when `first_item` is set (clearing it), and shows the focus ring.
fn menu_button(
    ui: &mut egui::Ui,
    label: impl Into<egui::WidgetText>,
    key: Option<&str>,
    first_item: &mut bool,
) -> egui::Response {
    menu_row(ui, label, key, first_item, false)
}

/// `menu_button`, drawn selected when `selected` (the current choice in a list of rows).
fn menu_row(
    ui: &mut egui::Ui,
    label: impl Into<egui::WidgetText>,
    key: Option<&str>,
    first_item: &mut bool,
    selected: bool,
) -> egui::Response {
    // As wide as the menu so far, so a key hint lands at the row's right edge.
    let mut button = egui::Button::new(label).min_size(egui::vec2(ui.min_rect().width(), 0.0)).selected(selected);
    if let Some(key) = key {
        button = button.shortcut_text(key);
    }
    let response = ui
        .scope(|ui| {
            if selected {
                // A quiet tint under normal text; the accent fill would swamp the row (the name keeps its colours).
                let visuals = ui.visuals_mut();
                visuals.selection.bg_fill = visuals.selection.bg_fill.gamma_multiply(0.3);
                visuals.selection.stroke.color = visuals.text_color();
            }
            ui.add(button)
        })
        .inner;
    if std::mem::take(first_item) {
        response.request_focus();
    }
    if response.has_focus() {
        focus_ring(ui, &response);
    }
    response
}

/// The "Color" label and one swatch per `TAG_COLOR_PALETTE` entry, then a
/// "no color" ring; `current` gets a ring of its own. Returns the clicked
/// choice (`Some(None)` for no color).
pub(crate) fn color_swatch_row(ui: &mut egui::Ui, current: Option<egui::Color32>) -> Option<Option<egui::Color32>> {
    let mut picked = None;
    menu_section(ui, "Color", |ui, grid| {
        ui.horizontal(|ui| {
            ui.add_space(grid.margin);
            ui.spacing_mut().item_spacing.x = MenuGrid::MARKER_GAP;
            for choice in TAG_COLOR_PALETTE.iter().copied().map(Some).chain([None]) {
                let (rect, response) =
                    ui.allocate_exact_size(egui::Vec2::splat(MenuGrid::MARKER_SLOT), egui::Sense::click());
                let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
                if response.has_focus() {
                    focus_ring(ui, &response);
                }
                match choice {
                    Some(swatch) => {
                        ui.painter().circle_filled(rect.center(), 7.0, swatch);
                        if current == Some(swatch) {
                            ui.painter().circle_stroke(
                                rect.center(),
                                7.0,
                                egui::Stroke::new(2.0, ui.visuals().text_color()),
                            );
                        }
                    }
                    None => {
                        ui.painter().circle_stroke(
                            rect.center(),
                            7.0,
                            egui::Stroke::new(1.5, ui.visuals().weak_text_color()),
                        );
                    }
                }
                if response.clicked() {
                    picked = Some(choice);
                }
            }
            ui.add_space(grid.margin - MenuGrid::MARKER_GAP);
        })
    });
    picked
}

/// Which items a node's right-click menu shows, from the state frozen in
/// `view`. Export Group is left out in headless mode, since its file
/// dialog would block on input that never comes (`EXPORTGROUP <path>`
/// saves directly instead).
pub(crate) fn node_menu_config(view: &NodeMenuView, headless: bool) -> NodeMenuConfig {
    use NodeAction as A;
    use NodeMenuItem::{Button, Colors, MoveTo, Separator};
    let file_items = || vec![Button("Open Containing Folder", A::Reveal), Button("Copy Path", A::CopyPath)];
    // Refresh (Image/Pdf/Json only), Open as Tab (any kind with a url) and
    // Copy URL (also a Download's), first in the menu.
    let url_items = |refresh: bool| {
        let mut items = Vec::new();
        if view.has_url && refresh {
            items.push(Button("Refresh", A::Refresh));
        }
        if view.has_url {
            items.push(Button("Open as Tab", A::OpenAsTab));
        }
        if view.has_copy_url {
            items.push(Button("Copy URL", A::CopyUrl));
        }
        items
    };
    // Every menu ends in the Colors row, the Links section (when it has rows), Remove from Group (when the node
    // is grouped), then the kind's delete items.
    let finish = |area_id, min_width, mut items: Vec<NodeMenuItem>, delete_items: Vec<NodeMenuItem>| {
        items.push(Colors);
        if !view.edges.is_empty() {
            items.push(NodeMenuItem::Edges(view.edges.clone()));
        }
        if !view.move_targets.is_empty() {
            items.extend([Separator, MoveTo(view.move_targets.clone())]);
        }
        if view.has_group {
            items.extend([Separator, Button("Remove from Group", A::RemoveFromGroup)]);
        }
        items.push(Separator);
        items.extend(delete_items);
        NodeMenuConfig { kind: view.kind, area_id, min_width, items }
    };
    let delete = |label| vec![Button(label, A::Delete)];
    // Tab, Unloaded and Group: the jar list and its actions, after the kind's own items. `live`: only a running tab
    // can need "Switch Jar Now" (a sleeping one spawns into its jar).
    let jar_items = |jars: &JarMenu, tab: bool, live: bool| -> Vec<NodeMenuItem> {
        let mut out = vec![NodeMenuItem::Jars {
            heading: if tab { "Jar" } else { "Default jar for new tabs" },
            rows: jars.rows.clone(),
            group: !tab,
        }];
        if tab {
            out.push(Button("New Jar...", A::StartNewJar));
            if jars.can_follow {
                out.push(Button("Follow Group's Jar", A::FollowGroupJar));
            }
            if live && jars.will_switch {
                out.push(Button("Switch Jar Now", A::SwitchJarNow));
            }
        } else if jars.has_default {
            out.push(Button("Clear Default Jar", A::ClearGroupDefaultJar));
        }
        out
    };
    match view.kind.ty {
        TYPE_DOWNLOAD => {
            let mut items = url_items(false);
            items.extend(file_items());
            finish("download_context_menu", 170.0, items, delete("Remove from List"))
        }
        TYPE_NOTE => {
            let mut items = url_items(false);
            items.push(Button("Copy Text", A::CopyText));
            finish("note_context_menu", 150.0, items, delete("Delete"))
        }
        TYPE_IMAGE => {
            let mut items = url_items(true);
            items.extend(file_items());
            finish("image_context_menu", 170.0, items, delete("Delete"))
        }
        TYPE_PDF => {
            let mut items = url_items(true);
            items.extend(file_items());
            finish("pdf_context_menu", 170.0, items, delete("Delete"))
        }
        TYPE_JSON | TYPE_JSON_LEAF => {
            let mut items = url_items(true);
            items.extend(file_items());
            finish("json_context_menu", 170.0, items, delete("Delete"))
        }
        TYPE_TAB if view.kind.loaded() => {
            let mut items = vec![
                Button(if view.pinned { "Unpin" } else { "Pin" }, A::TogglePin),
                Button("Duplicate", A::Duplicate),
                Button("Sleep", A::Sleep),
                Button("Copy URL", A::CopyUrl),
            ];
            items.push(Button("Fit Subtree", A::FitSubtree));
            items.extend(view.jars.iter().flat_map(|j| jar_items(j, true, true)));
            finish("tab_context_menu", 150.0, items, delete("Close"))
        }
        TYPE_TAB => {
            // Retrying a tab whose engine isn't installed asks to install it (`retry_missing_file`).
            let first = match (view.failed, view.engine_missing) {
                (true, true) => Button("Download Engine", A::RetryMissingFile),
                (true, false) => Button("Retry", A::RetryMissingFile),
                (false, _) => Button("Open", A::Open),
            };
            let mut items = vec![first];
            if view.failed && view.other_default_engine {
                items.push(Button("Open in Default Engine", A::OpenInDefaultEngine));
            }
            items.push(Button(if view.pinned { "Unpin" } else { "Pin" }, A::TogglePin));
            // The same as a live tab's, and Ctrl+D's (`duplicate_tab` takes either). Not for a failed tab: the copy
            // would start on the same engine and fail the same way.
            if !view.failed {
                items.push(Button("Duplicate", A::Duplicate));
            }
            items.push(Button("Copy URL", A::CopyUrl));
            items.extend(view.jars.iter().flat_map(|j| jar_items(j, true, false)));
            finish("unloaded_tab_context_menu", 170.0, items, delete("Close"))
        }
        // Presentation matches the type, not `is_container` (see `card_style`).
        TYPE_GROUP => {
            // Dissolve keeps the members (the card's Del, ✕ and middle-click do it); the second closes them all.
            let mut delete_items =
                vec![Button("Dissolve Group", A::Dissolve), Button("Close Group and Contents", A::Delete)];
            if !headless {
                delete_items.push(Button("Export Group...", A::ExportGroup));
            }
            let mut items = vec![Button("Rename", A::Rename), Button("Sleep Tabs", A::SleepGroup)];
            items.extend(view.jars.iter().flat_map(|j| jar_items(j, false, false)));
            finish("group_context_menu", 150.0, items, delete_items)
        }
        // A type this build doesn't know has no menu of its own yet (2b-6 Ruling 8): the shared tail only.
        _ => finish("node_context_menu", 150.0, Vec::new(), delete("Delete")),
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
    kind: CardKind,
    has_group: bool,
    color: Option<egui::Color32>,
    /// Image/Pdf/Json/Note: `node.source_url().is_some()`. Offers Refresh
    /// (Image/Pdf/Json) and Open as Tab (any of the four).
    has_url: bool,
    /// Image/Pdf/Json/Note/Download: `node.copyable_url().is_some()`, which offers Copy URL.
    has_copy_url: bool,
    /// Tab only.
    pinned: bool,
    /// Unloaded only: the default engine isn't the one that failed, so
    /// the menu offers it.
    other_default_engine: bool,
    /// Unloaded only: its engine failed, so the menu offers a retry.
    failed: bool,
    /// Unloaded only: it failed because its engine isn't installed, so the menu offers the download.
    engine_missing: bool,
    /// Sibling groups (same level, not this node) it can move into, as `(id, name)`.
    /// `pub(crate)` for headless `MOVETOGROUP`, which moves only where the menu would offer.
    pub(crate) move_targets: Vec<(String, String)>,
    /// Group: the rename draft, while this group is being renamed. Tab: the "New Jar..." name draft.
    renaming: Option<String>,
    /// Tab, Unloaded and Group: their jar list (`App::jar_menu`).
    jars: Option<JarMenu>,
    /// The Links section (`App::edge_menu`): empty in a JSON level and for a node no edge may join.
    pub(crate) edges: Vec<EdgeMenuRow>,
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
            // The link came from the page: no `file:`/`javascript:` new tab (`popup_gate.rs`).
            if !crate::popup_gate::page_tab_url_allowed(&menu.href) {
                // The scheme only: the rest is page content (a `data:` or `javascript:` body).
                self.queue_toast(match crate::popup_gate::scheme_for_log(&menu.href) {
                    Some(scheme) => format!("Can't open a {scheme}: link in a new tab"),
                    None => "Can't open this link in a new tab".to_string(),
                });
            } else if let Some(opener) = self.focused.clone() {
                self.spawn_tab_from(&opener, &menu.href);
            } else {
                self.spawn_new_tab(None, None, &menu.href);
            }
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
    /// rather than just closing the menu; while a jar is being named, only
    /// Enter creates it, and a click outside cancels it, as Escape does.
    pub(crate) fn build_node_menu(&mut self, ui: &mut egui::Ui) {
        if self.node_menu.as_ref().is_some_and(|(id, _)| !self.nodes.contains_key(id)) {
            self.node_menu = None;
        }
        self.drop_stray_jar_name();
        let current = self.node_menu.clone().and_then(|(id, pos)| self.node_menu_view(&id).map(|view| (id, pos, view)));
        let Some(((id, pos, view), fade)) =
            fade_popup_state(ui.ctx(), egui::Id::new("node_menu_fade"), &current, &mut self.node_menu_cache)
        else {
            return;
        };
        let live = current.is_some();
        let config = node_menu_config(&view, self.headless);
        let naming = self.naming_jar.as_ref().is_some_and(|(tid, _)| *tid == id);
        let mut frozen_draft = view.renaming.clone();
        let rename = match (&mut self.renaming_group, &mut self.naming_jar) {
            (Some((gid, draft)), _) if live && *gid == id => {
                Some((draft, take_focus(&mut self.pending_focus, FocusTarget::GroupRename), None))
            }
            (_, Some((tid, draft))) if live && *tid == id => {
                Some((draft, take_focus(&mut self.pending_focus, FocusTarget::JarName), Some("Jar name")))
            }
            _ => frozen_draft.as_mut().map(|draft| (draft, false, naming.then_some("Jar name"))),
        };
        let (action, clicked_outside) = build_node_context_menu(ui, pos, fade, live, &config, view.color, rename);
        // The name field reports the generic rename actions; while naming a jar they mean the jar's. Only Enter
        // creates it: losing focus any other way (a click away) cancels, unlike a group rename (user decision).
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
        let action = action.map(|a| match (a, naming) {
            (NodeAction::CommitRename(name), true) if enter => NodeAction::NewJar(name),
            (NodeAction::CommitRename(_) | NodeAction::CancelRename, true) => NodeAction::CancelNewJar,
            (a, _) => a,
        });
        let close = match action {
            Some(NodeAction::Rename | NodeAction::StartNewJar) => false,
            Some(_) => true,
            None => clicked_outside && view.renaming.is_none(),
        };
        if let Some(action) = action {
            self.apply_node_action(ui.ctx(), &id, action);
            self.save_session();
        }
        if close {
            self.node_menu = None;
            // Closed from the keyboard: focus goes back to the card the menu belongs to.
            if !ui.input(|i| i.pointer.any_click()) {
                ui.memory_mut(|m| m.request_focus(egui::Id::new(("graph_card", id.as_str()))));
            }
        }
    }

    /// "New Jar..."'s name field belongs to its tab's open menu: the draft is dropped once `node_menu` is no longer
    /// that tab's, so it can't reappear the next time the tab's menu opens. That covers the menu closing without the
    /// field reporting it (a right-click on empty graph, a change of group level, the tab gone) and another node's menu
    /// opening. Enter, Escape and a click away are the field's own (`NewJar`, `CancelNewJar`).
    fn drop_stray_jar_name(&mut self) {
        if self.naming_jar.as_ref().is_some_and(|(tid, _)| self.node_menu.as_ref().map(|(id, _)| id) != Some(tid)) {
            self.naming_jar = None;
        }
    }

    pub(crate) fn node_menu_view(&self, id: &str) -> Option<NodeMenuView> {
        let node = self.nodes.get(id)?;
        Some(NodeMenuView {
            kind: self.card_kind(id)?,
            has_group: self.mirror.group(id).is_some(),
            color: self.mirror.color(id),
            has_url: self.source_url(id).is_some(),
            has_copy_url: self.copyable_url(id).is_some(),
            pinned: self.mirror.pinned(id),
            other_default_engine: node.runtime.as_unloaded().is_some()
                && self.mirror.engine(id) != self.settings.default_starting_engine,
            failed: node.runtime.as_unloaded().is_some_and(|t| matches!(t.state, UnloadedState::Failed(_))),
            engine_missing: node.runtime.as_unloaded().is_some_and(|t| {
                let kind = self.mirror.engine(id);
                matches!(t.state, UnloadedState::Failed(_))
                    && !matches!(pick_outcome(kind, &self.engine_state(kind)), PickOutcome::Switch(_))
            }),
            move_targets: self
                .node_order
                .iter()
                .filter(|other| other.as_str() != id)
                .filter(|other| self.mirror.group(other) == self.mirror.group(id) && self.is_container(other))
                // name rule: drawn by display_name in the MoveTo rows
                .map(|other| (other.clone(), self.mirror.name(other).to_owned()))
                .collect(),
            renaming: [&self.renaming_group, &self.naming_jar]
                .into_iter()
                .flatten()
                .find(|(of, _)| of == id)
                .map(|(_, draft)| draft.clone()),
            jars: self.jar_menu(id),
            edges: self.edge_menu(id),
        })
    }

    /// The Links section (links spec "Menus and pick-target mode"): a verb row per user-creatable edge with at least
    /// one valid target at this level, then a "Disconnect from X" per live edge (`edges_of`). None in a JSON level,
    /// whose cards are not stored nodes.
    pub(crate) fn edge_menu(&self, id: &str) -> Vec<EdgeMenuRow> {
        if self.json_level.is_some() {
            return Vec::new();
        }
        let verbs = crate::caps::edges().user_creatable().filter_map(|row| {
            let label = row.verb()?;
            self.has_valid_target(row.id(), id).then(|| EdgeMenuRow {
                action: NodeAction::Connect(row.id()),
                label: label.to_string(),
                hover: Some("Then click a highlighted card (Esc cancels)".to_string()),
            })
        });
        let disconnects = self.edges_of(id).into_iter().map(|e| {
            // The other card as a screen reader names it, cut to a row's length.
            let name = self
                .nodes
                .get(&e.other)
                .map_or_else(String::new, |n| n.accessible_name(self.own_text(&e.other), self.source_url(&e.other)));
            let segs = crate::names::display_name(&name, crate::names::UNTITLED);
            let short = crate::names::shorten(&segs, crate::names::ROW_UNITS);
            // Which edge it is, since a pair can be joined by both (spec "Two lines on one pair").
            let what = match (e.edge, e.outgoing) {
                (crate::caps::OPENED_FROM, true) => "opened from it",
                (crate::caps::OPENED_FROM, false) => "opened from here",
                _ => "link",
            };
            EdgeMenuRow {
                label: format!(
                    "Disconnect from {} ({what})",
                    crate::names::to_plain(short.as_deref().unwrap_or(&segs))
                ),
                hover: short.is_some().then(|| crate::names::to_plain(&segs)),
                action: NodeAction::Disconnect { edge: e.edge, other: e.other, link: e.link },
            }
        });
        verbs.chain(disconnects).collect()
    }

    /// The tab and group menus' jar list (spec "UI"); `None` for every other kind.
    pub(crate) fn jar_menu(&self, id: &str) -> Option<JarMenu> {
        let named = |ticked: &dyn Fn(NodeId) -> bool, count: &dyn Fn(NodeId) -> Option<usize>| -> Vec<JarRow> {
            self.mirror
                .jar_defs()
                .iter()
                .map(|(jid, d)| JarRow {
                    id: Some(*jid),
                    name: self.jar_label(crate::jar::JarKey::Named(*jid)),
                    color: d.color,
                    ticked: ticked(*jid),
                    count: count(*jid),
                })
                .collect()
        };
        if let Some(choice) = self.tab_jar(id) {
            let group_default = self.inherited_jar(self.mirror.group(id));
            // Tick the jar the tab uses: Default for one naming a jar deleted elsewhere.
            let (key, _) = self.resolved_jar(&choice);
            let mut rows = vec![JarRow {
                id: None,
                name: JarLabel::built_in(),
                color: None,
                ticked: key == crate::jar::JarKey::Default,
                count: None,
            }];
            rows.extend(named(&|j| key == crate::jar::JarKey::Named(j), &|_| None));
            return Some(JarMenu {
                rows,
                can_follow: choice.set && group_default.is_some(),
                will_switch: self.can_switch_jar_now(id),
                has_default: false,
            });
        }
        if !self.is_container(id) {
            return None;
        }
        let default = self.mirror.default_jar(id);
        if self.mirror.jar_defs().is_empty() && default.is_none() {
            return None; // nothing to pick: jars are made from a tab's menu
        }
        Some(JarMenu {
            rows: named(&|j| default == Some(j), &|j| Some(self.cascade_count(id, j)).filter(|&n| n > 0)),
            can_follow: false,
            will_switch: false,
            has_default: default.is_some(),
        })
    }

    /// The rename field committed with `name` (`NodeAction::CommitRename`, and headless `RENAMEGROUP id name`): the
    /// field closes, and a group takes the clean name (`edit_name`). A name with nothing left once cleaned, or an id
    /// that isn't a group, changes nothing. The caller saves.
    pub(crate) fn commit_rename(&mut self, id: &str, name: &str) {
        self.renaming_group = None;
        let clean = crate::names::clean_name(name);
        if !clean.is_empty() && self.is_container(id) {
            self.edit_name(id, &clean);
        }
    }

    /// Applies whichever item was clicked in a node's right-click menu.
    /// Each action only ever appears on kinds where it makes sense
    /// (`node_menu_config`), so none of these re-check the kind.
    pub(crate) fn apply_node_action(&mut self, ctx: &egui::Context, id: &str, action: NodeAction) {
        // Pick-target mode ends on any other action (links spec).
        if !matches!(action, NodeAction::Connect(_)) {
            self.cancel_connect();
        }
        match action {
            NodeAction::Open => {
                if self.nodes.contains_key(id) {
                    self.selected_tabs.clear();
                    self.start_transition_to_item(id);
                }
            }
            NodeAction::CopyText => {
                if self.nodes.get(id).is_some_and(|n| n.as_note().is_some()) {
                    ctx.copy_text(self.note_text(id).to_owned());
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
            NodeAction::TogglePin => self.toggle_pin(id),
            NodeAction::Duplicate => self.duplicate_tab(id),
            NodeAction::Sleep => {
                self.request_sleep(id, SleepWhy::Manual);
            }
            NodeAction::SleepGroup => {
                self.sleep_group(id);
            }
            NodeAction::CopyUrl => {
                let node = self.nodes.get(id);
                if let Some(url) = self.copyable_url(id).map(str::to_owned) {
                    ctx.copy_text(url);
                } else if let Some(url) = node.and_then(|n| n.runtime.saved_tab()) {
                    ctx.copy_text(url.to_string());
                }
            }
            NodeAction::FitSubtree => self.camera.jump_to(self.fit_subtree_scene_rect(id)),
            NodeAction::Rename => {
                // name rule: the field edits the stored name
                let name = self.mirror.name(id).to_owned();
                self.renaming_group = Some((id.to_string(), name));
                self.pending_focus = Some(FocusTarget::GroupRename);
            }
            NodeAction::CommitRename(name) => self.commit_rename(id, &name),
            NodeAction::CancelRename => self.renaming_group = None,
            NodeAction::SetColor(color) => {
                if self.nodes.contains_key(id) {
                    self.edit_field(id, field::COLOR, session_sync::color_value(color));
                }
            }
            NodeAction::CycleColor => {
                if self.nodes.contains_key(id) {
                    let next = next_tag_color(self.mirror.color(id));
                    self.edit_field(id, field::COLOR, session_sync::color_value(next));
                }
            }
            NodeAction::ToggleMute => self.toggle_muted(id),
            NodeAction::RetryMissingFile => self.retry_missing_file(id),
            NodeAction::OpenInDefaultEngine => self.wake_tab(id, self.settings.default_starting_engine),
            NodeAction::RemoveFromGroup => self.eject(id),
            NodeAction::MoveToGroup(group) => self.apply_drop(id, Some((DropEffect::Join, Some(group)))),
            NodeAction::Dissolve => self.dissolve_group(id),
            NodeAction::ExportGroup => {
                let name = export_file_name(self.mirror.name(id)); // name rule: export_file_name cleans it
                if let Some(json) = self.export_group_json(id) {
                    if let Some((path, toast)) = save_dialog_with_toast(name, "Group") {
                        write_json_export(&json, &path, "group");
                        self.queue_toast(toast.0);
                    }
                }
            }
            NodeAction::Delete => self.close_node_and_save(id),
            NodeAction::SetJar(jar) => self.set_tab_jar(id, jar),
            NodeAction::StartNewJar => {
                self.naming_jar = Some((id.to_string(), String::new()));
                self.pending_focus = Some(FocusTarget::JarName);
            }
            NodeAction::NewJar(name) => {
                self.naming_jar = None;
                if !crate::names::clean_name(&name).is_empty() {
                    let jar = self.create_jar(&name);
                    self.set_tab_jar(id, Some(jar));
                }
            }
            NodeAction::CancelNewJar => self.naming_jar = None,
            NodeAction::FollowGroupJar => self.follow_group_jar(id),
            NodeAction::SwitchJarNow => self.switch_jar_now(id),
            NodeAction::SetGroupDefaultJar(jar) => {
                self.set_group_default_jar(id, jar);
            }
            NodeAction::ClearGroupDefaultJar => self.clear_group_default_jar(id),
            NodeAction::Connect(edge) => {
                if self.nodes.contains_key(id) {
                    self.connecting = Some((id.to_string(), edge));
                }
            }
            NodeAction::Disconnect { edge, other, link } => match link {
                // The very Link node the row listed: of two joining the same pair (drawn on two replicas at once),
                // the one clicked.
                Some(link) => {
                    if self.links_naming(id).contains(&link) {
                        self.delete_link(&link);
                    }
                }
                // Either way round: the row says which, and `disconnect` only removes an edge that is there.
                None => {
                    self.disconnect(edge, id, &other);
                    self.disconnect(edge, &other, id);
                }
            },
        }
    }
}

#[cfg(test)]
mod tests;
