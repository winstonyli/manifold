//! The node context menu: its actions, config and the App methods that build and apply it.

use super::*;
use crate::caps::JAR_EDGE;
use crate::jar_defs::{JarConfirm, JarLabel};
use crate::sleep_state::SleepWhy;
use crate::store::model::{NodeId, TYPE_DOWNLOAD, TYPE_GROUP, TYPE_JAR, TYPE_NOTE, TYPE_TAB};

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
    /// A live tab that stopped responding gets a new engine host (`App::restart_view`).
    RestartView,
    /// A live tab that stopped responding has its engine host ended, leaving a failed card (`App::end_view`).
    EndView,
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
    OpenInDefaultViewKind,
    RemoveFromGroup,
    /// The keyboard route for dragging a card onto a sibling group (`DropEffect::Join`). On a group it differs
    /// from the drag: "Move into" nests the group inside the other, where dropping it there merges the two
    /// (`drop_action`).
    MoveToGroup(NodeId),
    Dissolve,
    ExportGroup,
    /// Delete for a file card, "Remove from List" for a download, Close
    /// for a tab, "Close Group and Contents" for a group.
    Delete,
    /// A tab's or group's jar row: draw its `jar` edge to that jar, or (`None`, "Inherit") clear it (`App::set_jar_edge`).
    SetJar(Option<NodeId>),
    /// "New Jar...": swaps the tab menu's items for a name field (`App::naming_jar`); the menu stays open.
    StartNewJar,
    /// Enter in the name field: creates the jar and picks it for the tab. The mapped `CommitRename` (see
    /// `build_node_menu`).
    NewJar(String),
    /// Escape in the name field, or the field losing focus without Enter (a click away; user decision, unlike a
    /// group rename): nothing is created.
    CancelNewJar,
    SwitchJarNow,
    /// A jar card's "Clear Cookies...": asks first (`JarConfirm::Clear`).
    ClearJarCookies,
    /// A jar card's "Make Ephemeral" (`true`) / "Stop Ephemeral": asks first when a saved file exists.
    SetJarEphemeral(bool),
    /// "Link to…", "Opened from…": starts pick-target mode for this edge (`App::connecting`); the next click on a
    /// valid target draws it (`App::finish_connect`).
    Connect(&'static str),
    /// "Disconnect from X": removes the `edge` edge between this node and `other`. `link`: the Link node the row
    /// lists, for an edge stored as one.
    Disconnect {
        edge: &'static str,
        other: NodeId,
        link: Option<NodeId>,
    },
    /// "Color…", "Jar…": swaps the menu's items for that page; the menu stays open.
    OpenPage(MenuPage),
    /// A page's "Back" row: the menu's own items again.
    MenuBack,
}

/// A page of a node's menu that replaces its items while open (`NodeMenu::page`): the long or wide
/// sections, so the menu itself stays short.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MenuPage {
    Color,
    /// A tab's or group's jar list and its actions.
    Jar,
}

impl NodeAction {
    /// The key that does this to a keyboard-focused card, shown at the row's right edge. Del is whatever the card's
    /// Close does (`CardControl::Close`): `Delete` on every kind, so on a group "Close Group and Contents" is hinted
    /// Del and "Dissolve Group" is not.
    pub(crate) fn key_hint(&self) -> Option<&'static str> {
        (*self == CardControl::Close.action()).then_some("Del")
    }

    pub(crate) fn hover_text(&self) -> Option<&'static str> {
        match self {
            NodeAction::Dissolve => Some("Keeps the members and removes the group card"),
            NodeAction::FitSubtree => Some("Zoom the graph to frame just this tab and its children"),
            NodeAction::SwitchJarNow => Some("Reload this tab in its new jar now"),
            NodeAction::RestartView => Some("Start a new engine for this tab at its current page"),
            NodeAction::EndView => Some("Close this tab's engine; click the card to start it again"),
            _ => None,
        }
    }
}

/// One jar in a tab's or group's menu.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct JarRow {
    /// `None`: the "Inherit" row, which clears the card's own edge.
    pub(crate) id: Option<NodeId>,
    pub(crate) name: JarLabel,
    pub(crate) color: Option<egui::Color32>,
    /// The card's own edge names this jar (or, for the Inherit row, it has none).
    pub(crate) ticked: bool,
}

/// The jar part of a tab's or group's menu (`App::jar_menu`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct JarMenu {
    pub(crate) rows: Vec<JarRow>,
    /// A live tab waiting to switch with nothing else replacing its host (`App::can_switch_jar_now`): offers
    /// "Switch Jar Now".
    pub(crate) will_switch: bool,
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
    /// The "Color" label and tag-color swatch row, ending in a "no color" ring: the Color page's body.
    Colors,
    /// A row that opens a page of the menu.
    Page(&'static str, MenuPage),
    /// One "Move into <name>" button per `(group id, name)`: the sibling groups a card can join.
    MoveTo(Vec<(NodeId, String)>),
    /// A small heading, then one row per jar with its colour dot and a tick on the card's own edge.
    Jars {
        heading: &'static str,
        rows: Vec<JarRow>,
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
    area_id: &'static str,
    min_width: f32,
    items: Vec<NodeMenuItem>,
    /// What the Jar page holds (tab and group menus).
    jar_page: Vec<NodeMenuItem>,
    /// The page showing instead of `items`, if any (`NodeMenu::page`); the caller sets it.
    page: Option<MenuPage>,
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
                let mut edit = egui::TextEdit::singleline(draft).desired_width(140.0).margin(FIELD_MARGIN);
                if let Some(hint) = hint {
                    edit = edit.hint_text(hint);
                }
                let response = toolbar_field(ui, |ui| ui.add(edit));
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
            nav_page(ui, egui::Id::new(config.page.map(|p| p as u8)));
            if let Some(page) = config.page {
                // A page: a Back row, then the one section it holds.
                if menu_button(ui, "\u{2039} Back", None).clicked() {
                    action = Some(NodeAction::MenuBack);
                }
                menu_separator(ui);
                match page {
                    MenuPage::Color => draw_menu_item(ui, &NodeMenuItem::Colors, color, &mut action),
                    MenuPage::Jar => {
                        for item in &config.jar_page {
                            draw_menu_item(ui, item, color, &mut action);
                        }
                    }
                }
                return;
            }
            for item in &config.items {
                draw_menu_item(ui, item, color, &mut action);
            }
        },
    );
    let clicked_outside = clicked_outside_popup(ui, area_rect, None);
    (action, clicked_outside)
}

/// One item of a node menu (or of one of its pages), reporting a click in `action`.
fn draw_menu_item(
    ui: &mut egui::Ui,
    item: &NodeMenuItem,
    color: Option<egui::Color32>,
    action: &mut Option<NodeAction>,
) {
    match item {
        NodeMenuItem::Button(label, item_action) => {
            let mut response = menu_button(ui, *label, item_action.key_hint());
            if let Some(text) = item_action.hover_text() {
                response = response.on_hover_text(text);
            }
            if response.clicked() {
                *action = Some(item_action.clone());
            }
        }
        NodeMenuItem::MoveTo(targets) => {
            for (group, name) in targets {
                // Cut to `ROW_UNITS`, so a long name cannot widen the menu; the hover has all of it.
                let segs = crate::names::display_name(name, crate::names::UNTITLED);
                let shown = crate::names::shorten(&segs, crate::names::ROW_UNITS);
                let text = crate::names::name_job(ui, "Move into ", shown.as_deref().unwrap_or(&segs), "");
                let mut response = menu_button(ui, text, None);
                let hover = crate::names::name_hover(name, crate::names::ROW_UNITS, "", str::to_string);
                if !hover.is_empty() {
                    response = response.on_hover_text(hover);
                }
                if response.clicked() {
                    *action = Some(NodeAction::MoveToGroup(*group));
                }
            }
        }
        NodeMenuItem::Jars { heading, rows } => {
            menu_section(ui, heading, |ui, grid| {
                for row in rows {
                    // The label's marker in its own style (`JarLabel::append_to`); the rest in the button's.
                    let font = egui::FontSelection::Default.resolve(ui.style());
                    let plain = egui::TextFormat::simple(font.clone(), egui::Color32::PLACEHOLDER);
                    let mut text = egui::text::LayoutJob::default();
                    // The colour dot is painted in this leading gap, at the menu text's margin.
                    text.append("", MenuGrid::MARKER_SLOT, plain.clone());
                    row.name.append_to(&mut text, font, ui.visuals().weak_text_color());
                    // The current jar is the selected row: no tick to widen it.
                    let response = menu_row(ui, text, None, row.ticked);
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
                    if response.clicked() {
                        *action = Some(NodeAction::SetJar(row.id));
                    }
                }
            });
        }
        NodeMenuItem::Edges(rows) => {
            menu_section(ui, "Links", |ui, _| {
                for row in rows {
                    let mut response = menu_button(ui, row.label.as_str(), None);
                    if let Some(hover) = &row.hover {
                        response = response.on_hover_text(hover);
                    }
                    if response.clicked() {
                        *action = Some(row.action.clone());
                    }
                }
            });
        }
        NodeMenuItem::Page(label, page) => {
            if menu_button(ui, *label, None).clicked() {
                *action = Some(NodeAction::OpenPage(*page));
            }
        }
        NodeMenuItem::Colors => {
            if let Some(picked) = color_swatch_row(ui, color) {
                *action = Some(NodeAction::SetColor(picked));
            }
        }
        NodeMenuItem::Separator => {
            menu_separator(ui);
        }
    }
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

/// A menu row, a stop of the menu's keyboard navigation (`nav_row`).
fn menu_button(ui: &mut egui::Ui, label: impl Into<egui::WidgetText>, key: Option<&str>) -> egui::Response {
    menu_row(ui, label, key, false)
}

/// `menu_button`, drawn selected when `selected` (the current choice in a list of rows).
fn menu_row(
    ui: &mut egui::Ui,
    label: impl Into<egui::WidgetText>,
    key: Option<&str>,
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
    nav_row(ui, &response);
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
            let mut swatches = Vec::new();
            for choice in TAG_COLOR_PALETTE.iter().copied().map(Some).chain([None]) {
                let (rect, response) =
                    ui.allocate_exact_size(egui::Vec2::splat(MenuGrid::MARKER_SLOT), egui::Sense::click());
                let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
                if response.has_focus() {
                    focus_ring(ui, &response);
                }
                swatches.push(response.id);
                match choice {
                    Some(swatch) => {
                        ui.painter().circle_filled(rect.center(), 7.0, tag_shown(swatch, ui.visuals().dark_mode));
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
            nav_stop(ui, &swatches);
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
    use NodeMenuItem::{Button, MoveTo, Page, Separator};
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
    // Every menu has the same order: the kind's own actions (open, copy, sleep, files), a rule, the shared
    // organising rows (Rename, Duplicate, Pin, Color, Links), then Move into and Remove from Group, then the
    // kind's delete items last.
    let finish = |area_id, min_width, mut items: Vec<NodeMenuItem>, delete_items: Vec<NodeMenuItem>, jar_page| {
        if !items.is_empty() {
            items.push(Separator);
        }
        items.push(Button("Rename", A::Rename));
        // Not on a failed tab: the copy would start on the same engine and fail the same way.
        if !view.failed {
            items.push(Button("Duplicate", A::Duplicate));
        }
        items.push(Button(if view.pinned { "Unpin" } else { "Pin" }, A::TogglePin));
        items.push(Page("Color...", MenuPage::Color));
        if !view.edges.is_empty() {
            items.push(NodeMenuItem::Edges(view.edges.clone()));
        }
        if !view.move_targets.is_empty() {
            items.extend([Separator, MoveTo(view.move_targets.clone())]);
        }
        if view.has_group {
            items.extend([Separator, Button("Remove from Group", A::RemoveFromGroup)]);
        }
        if !delete_items.is_empty() {
            items.push(Separator);
            items.extend(delete_items);
        }
        NodeMenuConfig { area_id, min_width, items, jar_page, page: None }
    };
    let delete = |label| vec![Button(label, A::Delete)];
    // Tab, Unloaded and Group: the jar list and its actions, after the kind's own items. `live`: only a running tab
    // can need "Switch Jar Now" (a sleeping one spawns into its jar).
    let jar_items = |jars: &JarMenu, live: bool| -> Vec<NodeMenuItem> {
        let mut out =
            vec![NodeMenuItem::Jars { heading: "Jar", rows: jars.rows.clone() }, Button("New Jar...", A::StartNewJar)];
        if live && jars.will_switch {
            out.push(Button("Switch Jar Now", A::SwitchJarNow));
        }
        out
    };
    // The Jar row, in the place the jar section was; the rows themselves are the Jar page's.
    let jar_row = |items: &mut Vec<NodeMenuItem>, live: bool| -> Vec<NodeMenuItem> {
        let Some(jars) = &view.jars else { return Vec::new() };
        items.push(Page("Jar...", MenuPage::Jar));
        jar_items(jars, live)
    };
    match view.kind.ty {
        TYPE_DOWNLOAD => {
            let mut items = url_items(false);
            items.extend(file_items());
            finish("download_context_menu", 170.0, items, delete("Remove from List"), Vec::new())
        }
        TYPE_NOTE => {
            let mut items = url_items(false);
            items.push(Button("Copy Text", A::CopyText));
            finish("note_context_menu", 150.0, items, delete("Delete"), Vec::new())
        }
        // Image, Pdf and Json: the kinds with `Caps::file`, each with its own area id.
        _ if view.kind.caps().file => {
            let mut items = url_items(true);
            items.extend(file_items());
            finish(view.kind.caps().menu_area, 170.0, items, delete("Delete"), Vec::new())
        }
        TYPE_TAB if view.kind.loaded() => {
            let mut items = Vec::new();
            if view.unresponsive {
                items.extend([Button("Restart View", A::RestartView), Button("End View", A::EndView), Separator]);
            }
            items.extend([
                Button("Fit Subtree", A::FitSubtree),
                Button("Sleep", A::Sleep),
                Button("Copy URL", A::CopyUrl),
            ]);
            let jar_page = jar_row(&mut items, true);
            finish("tab_context_menu", 150.0, items, delete("Close"), jar_page)
        }
        TYPE_TAB => {
            // Retrying a tab whose engine isn't installed asks to install it (`retry_missing_file`).
            let first = match (view.failed, view.view_missing) {
                (true, true) => Button("Download Engine", A::RetryMissingFile),
                (true, false) => Button("Retry", A::RetryMissingFile),
                (false, _) => Button("Open", A::Open),
            };
            let mut items = vec![first];
            if view.failed && view.other_default_view_kind {
                items.push(Button("Open in Default Engine", A::OpenInDefaultViewKind));
            }
            items.push(Button("Copy URL", A::CopyUrl));
            let jar_page = jar_row(&mut items, false);
            finish("unloaded_tab_context_menu", 170.0, items, delete("Close"), jar_page)
        }
        // Presentation matches the type, not `is_container` (see `card_style`).
        TYPE_GROUP => {
            // Dissolve keeps the members; the second closes them all, as the card's Del, ✕ and middle-click do.
            let delete_items =
                vec![Button("Dissolve Group", A::Dissolve), Button("Close Group and Contents", A::Delete)];
            let mut items = vec![Button("Sleep Tabs", A::SleepGroup)];
            if !headless {
                items.push(Button("Export Group...", A::ExportGroup));
            }
            let jar_page = jar_row(&mut items, false);
            finish("group_context_menu", 150.0, items, delete_items, jar_page)
        }
        TYPE_JAR => {
            let ephemeral = if view.ephemeral {
                Button("Stop Ephemeral", A::SetJarEphemeral(false))
            } else {
                Button("Make Ephemeral", A::SetJarEphemeral(true))
            };
            let items = vec![Button("Clear Cookies...", A::ClearJarCookies), ephemeral];
            finish(
                "jar_context_menu",
                150.0,
                items,
                if view.closable { delete("Close") } else { Vec::new() },
                Vec::new(),
            )
        }
        // A type this build doesn't know (`TYPE_UNKNOWN`): the shared rows only (Rename, Duplicate, Pin, Color, links,
        // Move to) and Delete.
        _ => finish("node_context_menu", 150.0, Vec::new(), delete("Delete"), Vec::new()),
    }
}

/// Everything a node's right-click menu shows that its own actions can
/// change, read each frame while the menu is open and frozen in
/// `node_menu_fade` for the fade-out. The fading menu keeps the look it
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
    /// Live tab only: its engine stopped reading (`ViewTab::is_unresponsive`), so the menu leads with Restart
    /// Engine and End View.
    unresponsive: bool,
    /// Unloaded only: the default engine isn't the one that failed, so
    /// the menu offers it.
    other_default_view_kind: bool,
    /// Unloaded only: its engine failed, so the menu offers a retry.
    failed: bool,
    /// Unloaded only: it failed because its engine isn't installed, so the menu offers the download.
    view_missing: bool,
    /// Sibling groups (same level, not this node) it can move into, as `(id, name)`.
    /// `pub(crate)` for headless `MOVETOGROUP`, which moves only where the menu would offer.
    pub(crate) move_targets: Vec<(NodeId, String)>,
    /// Group: the rename draft, while this group is being renamed. Tab: the "New Jar..." name draft.
    renaming: Option<String>,
    /// Jar only: the jar is ephemeral, so the menu offers to stop.
    ephemeral: bool,
    /// False for the Default jar, whose menu has no Close.
    closable: bool,
    /// Tab, Unloaded and Group: their jar list (`App::jar_menu`).
    jars: Option<JarMenu>,
    /// The Links section (`App::edge_menu`): empty in a JSON level and for a node no edge may join.
    pub(crate) edges: Vec<EdgeMenuRow>,
}

impl App {
    /// The minimal right-click context menu: one "Open in new tab" item at
    /// `self.menus.context_menu.open`'s position, if a menu is currently showing.
    /// Dismissed by clicking the item (and acting on it), a click anywhere
    /// outside the menu's own rect, or Escape (see the Escape-priority
    /// chain in `build_tab_view_ui`).
    pub(crate) fn build_context_menu(&mut self, ui: &mut egui::Ui) {
        let current = self.menus.context_menu.open.clone();
        let Some((menu, fade)) = self.menus.context_menu.state(ui.ctx(), egui::Id::new("context_menu_fade")) else {
            return;
        };
        let mut open_tab = false;
        let mut close = false;

        let area = egui::Area::new(egui::Id::new("context_menu"))
            .fixed_pos(egui::pos2(menu.x, menu.y))
            .interactable(current.is_some());
        let area = menu_popup(ui.ctx(), area, fade, |ui| {
            if ui.button("Open in new tab").clicked() {
                open_tab = true;
                close = true;
            }
        });

        let clicked_outside = clicked_outside_popup(ui, area.rect, None);
        if clicked_outside {
            close = true;
        }

        if open_tab {
            // The link came from the page: no `file:`/`javascript:` new tab (`page_tab_gate.rs`).
            if !crate::page_tab_gate::page_tab_url_allowed(&menu.href) {
                // The scheme only: the rest is page content (a `data:` or `javascript:` body).
                self.queue_toast(match crate::page_tab_gate::scheme_for_log(&menu.href) {
                    Some(scheme) => format!("Can't open a {scheme}: link in a new tab"),
                    None => "Can't open this link in a new tab".to_string(),
                });
            } else if let Some(opener) = self.focused {
                self.spawn_tab_from(opener, &menu.href, None);
            } else {
                self.spawn_new_tab(None, None, &menu.href);
            }
        }
        if close {
            self.menus.context_menu.open = None;
        }
    }

    /// The one right-click context menu for every node card. Fades via the
    /// shared `node_menu` (one fade Id for all kinds --
    /// per-kind Ids would go unpolled while another kind's menu is open,
    /// breaking `Fading::state`'s continuous-polling rule), draws the
    /// items `node_menu_config` lists for the node's kind, and applies the
    /// clicked one. A node that's gone (closed from its own menu, or
    /// elsewhere) closes the menu. While fading out, the menu draws its
    /// frozen `NodeMenuView` and is click-through, so a second click (say,
    /// the rest of a double-click) can't act again. While a group is being
    /// renamed, a click outside commits the name (the field loses focus)
    /// rather than just closing the menu; while a jar is being named, only
    /// Enter creates it, and a click outside cancels it, as Escape does.
    pub(crate) fn build_node_menu(&mut self, ui: &mut egui::Ui) {
        if self.menus.node_menu.open.as_ref().is_some_and(|(id, _)| !self.nodes.contains_key(id)) {
            self.menus.node_menu.close();
        }
        self.drop_stray_jar_name();
        let current =
            self.menus.node_menu.open.and_then(|(id, pos)| self.node_menu_view(id).map(|view| (id, pos, view)));
        let live = current.is_some();
        self.menus.node_menu.fade.open = current;
        let Some(((id, pos, view), fade)) = self.menus.node_menu.fade.state(ui.ctx(), egui::Id::new("node_menu_fade"))
        else {
            self.menus.node_menu.page = None;
            return;
        };
        let mut config = node_menu_config(&view, self.headless);
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
        config.page = self.menus.node_menu.page;
        let (action, clicked_outside) = build_node_context_menu(ui, pos, fade, live, &config, view.color, rename);
        match action {
            Some(NodeAction::OpenPage(page)) => self.menus.node_menu.page = Some(page),
            Some(NodeAction::MenuBack) => self.menus.node_menu.page = None,
            _ => {}
        }
        // The name field reports the generic rename actions; while naming a jar they mean the jar's. Only Enter
        // creates it: losing focus any other way (a click away) cancels, unlike a group rename (user decision).
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
        let action = action.map(|a| match (a, naming) {
            (NodeAction::CommitRename(name), true) if enter => NodeAction::NewJar(name),
            (NodeAction::CommitRename(_) | NodeAction::CancelRename, true) => NodeAction::CancelNewJar,
            (a, _) => a,
        });
        let close = match action {
            Some(NodeAction::Rename | NodeAction::StartNewJar | NodeAction::OpenPage(_) | NodeAction::MenuBack) => {
                false
            }
            Some(_) => true,
            None => clicked_outside && view.renaming.is_none(),
        };
        if let Some(action) = action.filter(|a| !matches!(a, NodeAction::OpenPage(_) | NodeAction::MenuBack)) {
            self.apply_node_action(ui.ctx(), id, action);
            self.save_session();
        }
        if close {
            self.menus.node_menu.close();
            // Closed from the keyboard: focus goes back to the card the menu belongs to.
            if !ui.input(|i| i.pointer.any_click()) {
                ui.memory_mut(|m| m.request_focus(CardId::from(id).widget()));
            }
        }
    }

    /// "New Jar..."'s name field belongs to its tab's open menu: the draft is dropped once `node_menu` is no longer
    /// that tab's, so it can't reappear the next time the tab's menu opens. That covers the menu closing without the
    /// field reporting it (a right-click on empty graph, a change of group level, the tab gone) and another node's menu
    /// opening. Enter, Escape and a click away are the field's own (`NewJar`, `CancelNewJar`).
    fn drop_stray_jar_name(&mut self) {
        if self.naming_jar.as_ref().is_some_and(|(tid, _)| self.menus.node_menu.open.map(|(id, _)| id) != Some(*tid)) {
            self.naming_jar = None;
        }
    }

    pub(crate) fn node_menu_view(&self, id: NodeId) -> Option<NodeMenuView> {
        let node = self.nodes.get(&id)?;
        Some(NodeMenuView {
            kind: self.card_kind(id)?,
            has_group: self.doc.mirror.group(id).is_some(),
            color: self.doc.mirror.color(id),
            has_url: self.source_url(id).is_some(),
            has_copy_url: self.copyable_url(id).is_some(),
            pinned: self.doc.mirror.pinned(id),
            unresponsive: node.as_tab().is_some_and(|t| t.tab.is_unresponsive(std::time::Instant::now())),
            other_default_view_kind: node.runtime.as_unloaded().is_some()
                && self.doc.mirror.view_kind(id) != self.settings.default_starting_view_kind,
            failed: node.runtime.as_unloaded().is_some_and(|t| matches!(t.state, UnloadedState::Failed(_))),
            view_missing: node.runtime.as_unloaded().is_some_and(|t| {
                let kind = self.doc.mirror.view_kind(id);
                matches!(t.state, UnloadedState::Failed(_))
                    && !matches!(pick_outcome(kind, &self.view_status(kind)), PickOutcome::Switch(_))
            }),
            move_targets: self
                .node_order
                .iter()
                .filter(|&&other| other != id)
                .filter(|&&other| self.doc.mirror.group(other) == self.doc.mirror.group(id) && self.is_container(other))
                // name rule: drawn by display_name in the MoveTo rows
                .map(|&other| (other, self.doc.mirror.name(other).to_owned()))
                .collect(),
            renaming: [&self.renaming_group, &self.naming_jar]
                .into_iter()
                .flatten()
                .find(|(of, _)| *of == id)
                .map(|(_, draft)| draft.clone()),
            jars: self.jar_menu(id),
            edges: self.edge_menu(id),
            ephemeral: matches!(node.runtime, Runtime::Jar) && self.doc.mirror.jar_def(id).ephemeral,
            closable: !self.is_default_jar(id),
        })
    }

    /// The Links section (links spec "Menus and pick-target mode"): a verb row per user-creatable edge with at least
    /// one valid target at this level, then a "Disconnect from X" per live edge (`edges_of`). None in a JSON level,
    /// whose cards are not stored nodes.
    pub(crate) fn edge_menu(&self, id: NodeId) -> Vec<EdgeMenuRow> {
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
            let name = self.accessible_name(e.other).unwrap_or_default();
            let segs = crate::names::display_name(&name, crate::names::UNTITLED);
            let short = crate::names::shorten(&segs, crate::names::ROW_UNITS);
            // Which edge it is, since a pair can be joined by both (spec "Two lines on one pair").
            let what = match (e.edge, e.outgoing) {
                (crate::caps::OPENED_FROM, true) => "opened from it",
                (crate::caps::OPENED_FROM, false) => "opened from here",
                (crate::caps::JAR_EDGE, true) => "uses it",
                (crate::caps::JAR_EDGE, false) => "used by it",
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

    /// The tab and group menus' jar list (spec "UI"): "Inherit", then every jar the card's edge could name
    /// (`valid_targets`) and the one it does. `None` for every other kind.
    pub(crate) fn jar_menu(&self, id: NodeId) -> Option<JarMenu> {
        if !self.is_tab(id) && !self.is_container(id) {
            return None;
        }
        let own = self.doc.mirror.jar(id);
        let inherit = JarRow {
            id: None,
            name: JarLabel { name: "Inherit".into(), marker: None },
            color: None,
            ticked: own.is_none(),
        };
        let targets = self.valid_targets(JAR_EDGE, id).into_iter();
        let defs = self.jar_defs();
        let rows = std::iter::once(inherit)
            .chain(
                targets
                    .chain(own.filter(|j| defs.contains_key(j)))
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .map(|jar| JarRow {
                        id: Some(jar),
                        name: self.jar_label(jar),
                        color: defs.get(&jar).and_then(|d| d.color),
                        ticked: own == Some(jar),
                    }),
            )
            .collect();
        Some(JarMenu { rows, will_switch: self.can_switch_jar_now(id) })
    }

    /// The rename field committed with `name` (`NodeAction::CommitRename`, and headless `RENAMEGROUP id name`): the
    /// field closes, and a group takes the clean name (`edit_name`). A name with nothing left once cleaned, or an id
    /// that isn't a group, changes nothing. The caller saves.
    pub(crate) fn commit_rename(&mut self, id: NodeId, name: &str) {
        self.renaming_group = None;
        let clean = crate::names::clean_name(name);
        // A group or jar always has a name (an empty draft changes nothing); any other node's name is an override of
        // its derived label, which an empty draft clears.
        if self.named_by_name(id) {
            if !clean.is_empty() {
                self.edit_name(id, &clean);
            }
        } else if self.nodes.contains_key(&id) {
            self.edit_name(id, &clean);
        }
    }

    /// Applies whichever item was clicked in a node's right-click menu.
    /// Each action only ever appears on kinds where it makes sense
    /// (`node_menu_config`), so none of these re-check the kind.
    pub(crate) fn apply_node_action(&mut self, ctx: &egui::Context, id: NodeId, action: NodeAction) {
        // Pick-target mode ends on any other action (links spec).
        if !matches!(action, NodeAction::Connect(_)) {
            self.cancel_connect();
        }
        match action {
            NodeAction::Open => {
                if self.nodes.contains_key(&id) {
                    self.interaction.selected.clear();
                    self.start_transition_to_item(id);
                }
            }
            NodeAction::CopyText => {
                if self.nodes.get(&id).is_some_and(|n| n.as_note().is_some()) {
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
            NodeAction::Duplicate => self.duplicate_selection(id),
            NodeAction::Sleep => {
                self.request_sleep(id, SleepWhy::Manual);
            }
            NodeAction::SleepGroup => {
                self.sleep_group(id);
            }
            NodeAction::RestartView => {
                self.restart_view(id);
            }
            NodeAction::EndView => {
                self.end_view(id);
            }
            NodeAction::CopyUrl => {
                let node = self.nodes.get(&id);
                if let Some(url) = self.copyable_url(id).map(str::to_owned) {
                    ctx.copy_text(url);
                } else if let Some(url) = node.and_then(|n| n.runtime.saved_tab()) {
                    ctx.copy_text(url.to_string());
                }
            }
            NodeAction::FitSubtree => self.camera.jump_to(self.fit_subtree_scene_rect(id)),
            NodeAction::Rename => {
                // name rule: the field edits the stored name
                let name = self.doc.mirror.name(id).to_owned();
                self.renaming_group = Some((id, name));
                self.pending_focus = Some(FocusTarget::GroupRename);
            }
            NodeAction::CommitRename(name) => self.commit_rename(id, &name),
            NodeAction::CancelRename => self.renaming_group = None,
            NodeAction::SetColor(color) => {
                if self.nodes.contains_key(&id) {
                    self.edit_field(id, field::COLOR, session_sync::color_value(color));
                }
            }
            NodeAction::CycleColor => {
                if self.nodes.contains_key(&id) {
                    let next = next_tag_color(self.doc.mirror.color(id));
                    self.edit_field(id, field::COLOR, session_sync::color_value(next));
                }
            }
            NodeAction::ToggleMute => self.toggle_muted(id),
            NodeAction::RetryMissingFile => self.retry_missing_file(id),
            NodeAction::OpenInDefaultViewKind => self.wake_tab(id, self.settings.default_starting_view_kind),
            NodeAction::RemoveFromGroup => self.apply_drop(CardId::from(id), Some((DropEffect::Eject, None))),
            NodeAction::MoveToGroup(group) => {
                self.apply_drop(CardId::from(id), Some((DropEffect::Join, Some(CardId::from(group)))))
            }
            NodeAction::Dissolve => self.ungroup(&[id]),
            NodeAction::ExportGroup => {
                let name = export_file_name(self.doc.mirror.name(id)); // name rule: export_file_name cleans it
                if let Some(json) = self.export_group_json(id) {
                    if let Some((path, toast)) = save_dialog_with_toast(name, "Group") {
                        write_json_export(&json, &path, "group");
                        self.queue_toast(toast.0);
                    }
                }
            }
            NodeAction::Delete => self.close_node(id),
            // The menu's own state (`build_node_menu`): nothing to apply.
            NodeAction::OpenPage(_) | NodeAction::MenuBack => {}
            NodeAction::SetJar(jar) => self.set_jar_edge(id, jar),
            NodeAction::StartNewJar => {
                self.naming_jar = Some((id, String::new()));
                self.pending_focus = Some(FocusTarget::JarName);
            }
            NodeAction::NewJar(name) => {
                self.naming_jar = None;
                if !crate::names::clean_name(&name).is_empty() {
                    let jar = self.create_jar(&name, self.jar_placement_beside(id));
                    self.set_jar_edge(id, Some(jar));
                }
            }
            NodeAction::CancelNewJar => self.naming_jar = None,
            NodeAction::ClearJarCookies => {
                self.offer_confirm(Confirm::Jar(JarConfirm::Clear(id)));
            }
            NodeAction::SetJarEphemeral(on) => {
                self.request_jar_ephemeral(id, on);
            }
            NodeAction::SwitchJarNow => self.switch_jar_now(id),
            NodeAction::Connect(edge) => {
                if self.nodes.contains_key(&id) {
                    self.link.connecting = Some((id, edge));
                }
            }
            NodeAction::Disconnect { edge, other, link } => match link {
                // The very Link node the row listed: of two joining the same pair (drawn on two replicas at once),
                // the one clicked.
                Some(link) => {
                    if self.links_naming(id).contains(&link) {
                        self.delete_link(link);
                    }
                }
                // Either way round: the row says which, and `disconnect` only removes an edge that is there.
                None => {
                    self.disconnect(edge, id, other);
                    self.disconnect(edge, other, id);
                }
            },
        }
    }
}

#[cfg(test)]
mod tests;
