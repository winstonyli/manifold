//! The Item view: the item toolbar every item shares, and the Tab view (its
//! toolbar, address bar and the spawned engine's content pane).

use super::*;
use crate::nav_history::Dir;
use crate::store::model::NodeId;

/// The "back to Graph" control every non-Graph item view shares (today,
/// only Tab view; see the design spec's "item-view shared chip" layer).
/// Deliberately not chip-wrapped, unlike the icon clusters elsewhere in
/// this file -- it sits immediately before those clusters in the Tab
/// toolbar's own row, and merging two different composition layers (this
/// shared control and Tab's own per-kind nav icons) into one visual chip
/// would require either layer to know about the other's rendering, which
/// the composition model deliberately keeps separate. A plain button
/// here reads as a clearly distinct action from the icon cluster next to
/// it, which is an acceptable, honest reading of the approved mockup
/// rather than a literal pixel reproduction of it.
pub(crate) fn render_return_to_graph(ui: &mut egui::Ui) -> bool {
    icon_button_eased(ui, egui::Id::new("icon_btn_graph"), egui_phosphor::light::GRAPH)
        .named_tip("Back to Graph (Esc, Ctrl+G)")
        .clicked()
}

/// Everything the Tab toolbar's own content (see `render_tab_toolbar_
/// content`) needs to report back to `build_tab_view_ui`, which applies these
/// exactly where the equivalent bare `let mut` locals were applied
/// before this struct existed -- this is a pure "package the same flags
/// as a return value" move, not a behavior change. `navigate` is
/// deliberately NOT a field here: today it's computed by `build_tab_view_ui`
/// itself, after this content returns, from `enter_pressed` plus the
/// suggestion list -- only `enter_pressed` needs to cross the function
/// boundary.
#[derive(Default)]
pub(crate) struct ItemToolbarActions {
    pub(crate) go_back: bool,
    pub(crate) go_forward: bool,
    pub(crate) reload_requested: bool,
    pub(crate) home_requested: bool,
    pub(crate) toggle_bookmark_requested: bool,
    /// The jar dot was clicked while the tab waits to switch: "Switch Jar Now".
    pub(crate) switch_jar: bool,
    pub(crate) close_split_requested: bool,
    /// The Graph button inside the Tab toolbar's nav chip was clicked.
    pub(crate) back_to_graph: bool,
    pub(crate) tab_actions_button_rect: Option<egui::Rect>,
    pub(crate) split_view_button_rect: Option<egui::Rect>,
    /// The Back chip's branch marker, when drawn: the branch list anchors to it.
    pub(crate) back_branches_rect: Option<egui::Rect>,
    pub(crate) forward_branches_rect: Option<egui::Rect>,
}

/// What `render_item_toolbar` (the Item toolbar's shared frame) hands back:
/// the frame's own two results plus whatever the per-kind content reported.
pub(crate) struct ItemToolbarFrame {
    /// Layer 2's "Graph" button was clicked.
    pub(crate) back_to_graph: bool,
    /// Layer 1's Help button -- the shortcuts-help popup anchors to it.
    pub(crate) shortcuts_help_button_rect: Option<egui::Rect>,
    pub(crate) actions: ItemToolbarActions,
}

impl App {
    /// The Item toolbar's shared frame -- the design spec's three layers,
    /// outer to inner. Layer 2 ("Graph") sits at the leading edge. Layer 1
    /// (the Universal chip) is drawn *first* into a right-to-left trailing
    /// child `Ui` so it's always rightmost, for every kind. Layer 3 (the
    /// focused kind's own content) then gets both the row `ui` for leading
    /// widgets and `trailing_ui` for trailing ones; a kind that needs the
    /// space in between (Tab's address bar) measures `trailing_ui` to size
    /// it. The frame finally advances the row past the trailing cluster.
    pub(crate) fn render_item_toolbar(&mut self, node_id: NodeId, ui: &mut egui::Ui) -> ItemToolbarFrame {
        let mut back_to_graph = false;
        let mut shortcuts_help_button_rect = None;
        let mut actions = ItemToolbarActions::default();
        toolbar_panel("toolbar", ui, |ui| {
            // Not `horizontal_centered` (`Align::Center`): measured live
            // that `egui::ComboBox` paints itself ~4.5pt lower than a
            // plain `Button`/`TextEdit` even when all three report the
            // exact same `response.rect.height()` (confirmed via
            // temporary debug prints: 27.0 for all three, but the combo
            // box's `rect.min.y` alone sat 4.5pt below the back button's)
            // — an internal quirk of `ComboBox::show_ui` wrapping its
            // content in its own nested `ui.horizontal(...)`, whose
            // layout-pass size estimate apparently doesn't quite match
            // what it actually paints. Since every item in this row is
            // now confirmed the same real height (the address-bar-margin
            // fix above made sure of that), top-aligning instead of
            // center-aligning sidesteps the mismatch entirely rather
            // than compensating for a specific pixel offset that could
            // silently drift if padding/fonts ever change.
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                // A Tab's Graph button lives in its nav chip (see
                // `render_tab_toolbar_content`); other kinds keep it standalone.
                let graph_in_chip =
                    self.card_kind(node_id).is_some_and(|k| k.ty == crate::store::model::TYPE_TAB && k.loaded());
                if !graph_in_chip && render_return_to_graph(ui) {
                    back_to_graph = true;
                }
                let mut trailing_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(ui.available_rect_before_wrap())
                        .layout(egui::Layout::right_to_left(egui::Align::Min)),
                );
                shortcuts_help_button_rect = Some(self.render_universal_chip(&mut trailing_ui));
                actions = self.render_item_toolbar_content(node_id, ui, &mut trailing_ui);
                ui.advance_cursor_after_rect(trailing_ui.min_rect());
                back_to_graph |= actions.back_to_graph;
            });
        });
        ItemToolbarFrame { back_to_graph, shortcuts_help_button_rect, actions }
    }

    pub(crate) fn render_item_toolbar_content(
        &mut self,
        node_id: NodeId,
        ui: &mut egui::Ui,
        trailing_ui: &mut egui::Ui,
    ) -> ItemToolbarActions {
        use crate::store::model::{TYPE_IMAGE, TYPE_TAB};
        let Some(kind) = self.card_kind(node_id) else { return ItemToolbarActions::default() };
        match kind.ty {
            TYPE_TAB if kind.loaded() => self.render_tab_toolbar_content(node_id, ui, trailing_ui),
            TYPE_IMAGE => self.render_image_toolbar_content(node_id, ui, trailing_ui),
            // An unloaded tab and every other kind have no toolbar content of their own.
            _ => ItemToolbarActions::default(),
        }
    }

    pub(crate) fn render_tab_toolbar_content(
        &mut self,
        focused_id: NodeId,
        ui: &mut egui::Ui,
        trailing_ui: &mut egui::Ui,
    ) -> ItemToolbarActions {
        let mut actions = ItemToolbarActions::default();
        let (can_go_back, can_go_forward) = self.trail_enabled(focused_id);
        let (back_fork, forward_fork) =
            self.nav_history.get(&focused_id).map_or((false, false), |h| (h.back_fork(), h.forward_fork()));
        let mut back_marker: Option<egui::Response> = None;
        let mut forward_marker: Option<egui::Response> = None;
        toolbar_chip(ui, |ui| {
            let mut bounds: Option<egui::Rect> = None;
            let mut grow = |r: egui::Rect| {
                bounds = Some(bounds.map_or(r, |b| b.union(r)));
            };
            // `◀`/`▶` were already a proven-safe Unicode gamble
            // (confirmed live elsewhere in this app), not a real
            // problem -- migrated to `egui_phosphor::CARET_LEFT`/
            // `CARET_RIGHT` anyway once the icon font was already
            // in place, for the same visual consistency the other
            // icon migrations this session already have.
            let graph = chip_icon_button(ui, egui::Id::new("icon_btn_graph"), egui_phosphor::light::GRAPH)
                .named_tip("Back to Graph (Esc, Ctrl+G)");
            grow(graph.rect);
            if graph.clicked() {
                actions.back_to_graph = true;
            }
            let back = chip_icon_button_enabled(
                ui,
                egui::Id::new("icon_btn_back"),
                egui_phosphor::light::CARET_LEFT,
                can_go_back,
            )
            .named_tip("Back");
            grow(back.rect);
            if back.clicked() {
                actions.go_back = true;
            }
            if back_fork {
                let marker =
                    chip_icon_button(ui, egui::Id::new("icon_btn_back_branches"), egui_phosphor::light::CARET_DOWN)
                        .named_tip("Other pages from the page before");
                grow(marker.rect);
                back_marker = Some(marker);
            }
            let forward = chip_icon_button_enabled(
                ui,
                egui::Id::new("icon_btn_forward"),
                egui_phosphor::light::CARET_RIGHT,
                can_go_forward,
            )
            .named_tip("Forward");
            grow(forward.rect);
            if forward.clicked() {
                actions.go_forward = true;
            }
            if forward_fork {
                let marker =
                    chip_icon_button(ui, egui::Id::new("icon_btn_forward_branches"), egui_phosphor::light::CARET_DOWN)
                        .named_tip("Pages opened from this page");
                grow(marker.rect);
                forward_marker = Some(marker);
            }
            // Was plain text ("Reload") to avoid a refresh-arrows
            // glyph gamble in this app's own font -- now a real
            // `egui_phosphor` icon instead, same reasoning as every
            // other icon migrated once the icon font was already
            // wired in.
            let reload = chip_icon_button(ui, egui::Id::new("icon_btn_reload"), egui_phosphor::light::ARROW_CLOCKWISE)
                .named_tip("Reload (Ctrl+R)");
            grow(reload.rect);
            if reload.clicked() {
                actions.reload_requested = true;
            }
            let home = chip_icon_button(ui, egui::Id::new("icon_btn_home"), egui_phosphor::light::HOUSE)
                .named_tip("Go to home page");
            grow(home.rect);
            if home.clicked() {
                actions.home_requested = true;
            }
            bounds.unwrap_or(ui.cursor())
        });
        if let Some(marker) = back_marker {
            self.popup_opener_clicked(ui, AnchoredPopup::BackBranches, &marker);
            actions.back_branches_rect = Some(marker.rect);
        }
        if let Some(marker) = forward_marker {
            self.popup_opener_clicked(ui, AnchoredPopup::ForwardBranches, &marker);
            actions.forward_branches_rect = Some(marker.rect);
        }

        // Address bar moved up here (right after nav/identity,
        // before any per-tab action button) and given most of
        // the row's width -- mirrors every mainstream browser's
        // own layout (nav controls, then a wide address bar,
        // then a handful of page-action icons to its right, then
        // an overflow/app menu at the far end), rather than the
        // stack of buttons that used to sit entirely to its
        // left, pushing it almost to the row's far end.
        //
        // The trailing cluster (Bookmark, View-as-JSON, Split +
        // Close Split, Tab actions) is drawn *first*, into the frame's
        // right-to-left `trailing_ui` (`render_item_toolbar`, which has
        // already put the Universal chip at its far right), so its real
        // consumed width can be measured -- not
        // guessed the way a former `trailing_reserve = 360.0`
        // constant was (sized "generously for the worst case,"
        // with nothing to catch a future addition silently
        // exceeding it). Added in reverse of their final visual
        // order since a right-to-left `Ui` places each new
        // widget immediately to the *left* of the previous one.
        let tab_actions_response =
            icon_button_eased(trailing_ui, egui::Id::new("icon_btn_tab_actions"), egui_phosphor::light::DOTS_THREE)
                .named_tip("Tab actions");
        self.popup_opener_clicked(trailing_ui, AnchoredPopup::TabActions, &tab_actions_response);
        actions.tab_actions_button_rect = Some(tab_actions_response.rect);
        if self.split_tab.is_some()
            && icon_button_eased(trailing_ui, egui::Id::new("icon_btn_close_split"), egui_phosphor::light::X)
                .named_tip("Close Split")
                .clicked()
        {
            actions.close_split_requested = true;
        }
        // Engine-vs-engine compare view -- see `App::split_tab`'s
        // own doc comment. Picking an engine here (re-)opens a
        // split against the focused tab's current url; the
        // secondary pane is view-only in this first cut, so its
        // own toolbar/address-bar controls only ever apply to
        // the primary (left) pane.
        let split_view_response =
            icon_button_eased(trailing_ui, egui::Id::new("icon_btn_split"), egui_phosphor::light::COLUMNS).named_tip(
                if self.split_tab.is_some() { "Split view: change engine" } else { "Split view: compare engines" },
            );
        self.popup_opener_clicked(trailing_ui, AnchoredPopup::SplitView, &split_view_response);
        actions.split_view_button_rect = Some(split_view_response.rect);
        let is_bookmarked = self
            .nodes
            .get(&focused_id)
            .and_then(|n| n.as_tab())
            .is_some_and(|n| self.bookmarks.iter().any(|b| b.url == n.url));
        // A standalone button, not a chip: pressed look for "already
        // bookmarked", a real `BOOKMARK_SIMPLE` icon rather than a star
        // glyph's tofu risk. Sits right after the address bar, mirroring
        // Chrome/Firefox's own star icon at the address bar's right edge.
        let bookmark = rest_borderless(trailing_ui, |ui| {
            ui.add(
                egui::Button::new(egui_phosphor::light::BOOKMARK_SIMPLE)
                    .selected(is_bookmarked)
                    .min_size(egui::vec2(32.0, 0.0)),
            )
        })
        .named_tip("Bookmark");
        if bookmark.clicked() {
            actions.toggle_bookmark_requested = true;
        }
        // The jar dot, beside the address bar (spec "UI"): the card chip's dots (`card::paint_jar_dots`, so a tab
        // waiting to switch shows its old jar's dot, an arrow, then the new one), its name and warnings as hover text.
        // A click switches a tab that is waiting to, unless another host change is already on its way.
        if let Some(mut chip) = self.jar_chip(focused_id) {
            // The dots sit on the toolbar, not a dark scrim: the theme's tag-color variants.
            let dark = trailing_ui.visuals().dark_mode;
            chip.color = tag_shown(chip.color, dark);
            chip.was_color = tag_shown(chip.was_color, dark);
            let text = trailing_ui.visuals().text_color();
            let arrow =
                crate::card::jar_chip_arrow(trailing_ui.painter(), &chip, egui::FontId::proportional(11.0), text);
            let width = crate::card::jar_dots_width(arrow.as_deref()) + 8.0;
            let (rect, response) = trailing_ui.allocate_exact_size(egui::vec2(width, 28.0), egui::Sense::click());
            let warn = trailing_ui.visuals().warn_fg_color;
            crate::card::paint_jar_dots(
                trailing_ui.painter(),
                rect.left() + 4.0,
                rect.center().y,
                &chip,
                arrow,
                text,
                warn,
            );
            let font = egui::FontSelection::Default.resolve(&trailing_ui.ctx().global_style());
            let response = response.on_hover_text(chip.hover_job(font, trailing_ui.visuals().weak_text_color()));
            let response =
                if chip.can_switch_now { response.on_hover_cursor(egui::CursorIcon::PointingHand) } else { response };
            if response.clicked() {
                actions.switch_jar = chip.can_switch_now;
            }
        }
        let trailing_width = trailing_ui.min_rect().width();

        // Now the address bar gets whatever's actually left,
        // not a guess -- drawn into the original `ui` (still
        // mid-way through its own left-to-right cursor, just
        // after the leading nav/identity cluster above),
        // sized to stop exactly where the trailing cluster's
        // real measured width begins.
        let text_width =
            (ui.available_rect_before_wrap().width() - trailing_width - ui.spacing().item_spacing.x).max(40.0);
        let response = self.omni_ui(ui, Host::Item, text_width);
        let node = self.nodes[&focused_id].as_tab().unwrap();
        let (url, loading, kind) = (node.url.clone(), node.loading, node.tab.kind);
        let identity = self.tab_identity(&url, loading, kind);
        // 8 px of field inside the border on the left, the same before the text on the right (the 32 px text margin
        // less this offset and the icon, plus the text's own side bearing).
        let icon_center = egui::pos2(response.rect.min.x + 9.0 + ICON_SIZE / 2.0, response.rect.center().y);
        if let Some(text) = identity.paint(ui, icon_center, ICON_SIZE, 1.0, ui.visuals().weak_text_color()) {
            text.draw(ui.painter(), text.center);
        }
        if !response.has_focus() {
            paint_keycaps(ui, response.rect.right_center() - egui::vec2(8.0, 0.0), &["Ctrl", "L"]);
        }

        actions
    }

    /// `ViewMode::Item`: whichever node is maximized -- `viewed_image` if
    /// set, else the focused Tab.
    pub(crate) fn build_item_ui(&mut self, ui: &mut egui::Ui) {
        match self.viewed_image {
            Some(image_id) => self.build_image_view_ui(ui, image_id),
            None => self.build_tab_view_ui(ui),
        }
    }

    pub(crate) fn build_tab_view_ui(&mut self, ui: &mut egui::Ui) {
        let Some(focused_id) = self.focused else {
            self.view_mode = ViewMode::Graph;
            return;
        };
        // Missing (mid-respawn), or a failed tab's card.
        if !self.nodes.get(&focused_id).is_some_and(|n| n.as_tab().is_some()) {
            if self.spawns.pending_respawn_focus == Some(focused_id) {
                self.build_respawn_placeholder(ui, focused_id);
            } else {
                self.view_mode = ViewMode::Graph;
            }
            return;
        }
        // Another view's edit mustn't come back over the address: `omni_sync_host` reseeds on a host change, and the
        // text resyncs to the tab's url unless the user is mid-edit here.
        if self.omni.host != Some(Host::Item) || !self.omni.editing() {
            if let Some(tab) = self.nodes.get(&focused_id).and_then(|n| n.as_tab()) {
                self.omni.text = tab.url.clone();
            }
        }
        // Exhaustive (no `..`): a new `ItemToolbarActions` field that isn't
        // wired up here fails to compile.
        let ItemToolbarFrame {
            back_to_graph,
            shortcuts_help_button_rect,
            actions:
                ItemToolbarActions {
                    go_back,
                    go_forward,
                    reload_requested,
                    home_requested,
                    toggle_bookmark_requested,
                    switch_jar,
                    close_split_requested,
                    // Folded into `back_to_graph` by `render_item_toolbar`.
                    back_to_graph: _,
                    tab_actions_button_rect,
                    split_view_button_rect,
                    back_branches_rect,
                    forward_branches_rect,
                },
        } = self.render_item_toolbar(focused_id, ui);

        self.build_shortcuts_help(ui, shortcuts_help_button_rect);

        if self.find.open {
            self.build_find_bar(ui);
        }

        self.build_context_menu(ui);

        // The omnibar's dropdown is drawn by `omni_ui` in the toolbar above; Escape closes it here, in the layered chain.
        let has_suggestions = self.omni.open;
        // Neutralized while the Ctrl+K popup is open: it does its own, separate Escape check in `build_omni_popup` to
        // close itself, and `key_pressed` is a non-consuming peek -- without this, the same Escape press would also
        // fire this chain (leaving to Graph, or dismissing the address-bar dropdown) underneath it.
        let escape_pressed = ui.input(|i| i.key_pressed(egui::Key::Escape))
            && self.menus.omni_popup.open.is_none()
            && !self.modal_open();
        if back_to_graph
            || (!has_suggestions && !self.find.open && self.menus.context_menu.open.is_none() && escape_pressed)
        {
            self.leave_to_graph();
            return;
        }
        if escape_pressed {
            // Context menu, then find bar, take priority over the
            // address-bar dropdown if more than one somehow ended up open
            // at once (independent widgets — nothing stops a user from
            // right-clicking while the find bar is also open) — closes one
            // thing per Escape press, same layered-dismiss shape the
            // dropdown-vs-Graph-view case above already has.
            if self.menus.context_menu.open.is_some() {
                self.menus.context_menu.open = None;
            } else if self.find.open {
                self.close_find_bar();
            } else if has_suggestions {
                self.omni.dismiss();
            }
        }
        for (requested, dir) in [(go_back, Dir::Back), (go_forward, Dir::Forward)] {
            if requested {
                self.navigate_history(focused_id, dir);
            }
        }
        if reload_requested {
            self.reload_focused_tab_shortcut();
        }
        if home_requested {
            let home_page = self.settings.home_page.clone();
            self.navigate_tab(focused_id, home_page);
        }
        if close_split_requested {
            self.close_split();
        }
        // The secondary pane's own small "which engine is this" label plus
        // a thin divider line -- drawn here (real egui, on top of both raw
        // content quads) rather than as part of either pane's own content,
        // since the secondary pane has no toolbar of its own to put it in
        // (view-only in this first cut, see `split_tab`'s own doc comment).
        if let Some(split_id) = self.split_tab {
            self.build_split_pane_label(ui, split_id);
        }
        // A freshly-spawned or just-respawned tab's content pane draws as
        // plain black until its view-host's first real frame arrives (see
        // `record_content_pass`'s own doc comment -- the pane still clears,
        // it just has nothing to draw) -- confirmed live, a genuinely jarring
        // blank screen with zero feedback, unlike the Graph-view card's own
        // spinner for the exact same "no thumbnail yet" state. Only covers
        // the primary pane; the split view's secondary pane (view-only,
        // still a first cut per `split_tab`'s own doc comment) is left as-is.
        let has_content = self.gpu.as_ref().is_some_and(|g| g.has_tab_texture(focused_id));
        if !has_content {
            self.build_content_spinner(ui);
        }
        if toggle_bookmark_requested {
            self.toggle_bookmark(focused_id);
        }
        if switch_jar {
            self.switch_jar_now(focused_id);
        }
        let ctx = ui.ctx().clone();
        if let Some((rect, fade)) = self.anchored_popup_fade(&ctx, AnchoredPopup::TabActions, tab_actions_button_rect) {
            self.build_tab_actions_popup(ui, rect, focused_id, fade);
        }
        for (dir, which, anchor) in [
            (Dir::Back, AnchoredPopup::BackBranches, back_branches_rect),
            (Dir::Forward, AnchoredPopup::ForwardBranches, forward_branches_rect),
        ] {
            // The marker went (the trail changed under an open list): nothing to anchor to.
            if anchor.is_none() {
                close_popup(&mut self.menus.open_popup, which);
            }
            if let Some((rect, fade)) = self.anchored_popup_fade(&ctx, which, anchor) {
                self.build_trail_branches_popup(ui, dir, rect, focused_id, fade);
            }
        }
        // Highlights the split's own current engine (if one is already
        // open) rather than nothing, so re-opening this shows what's
        // already showing.
        let split_kind =
            self.split_tab.as_ref().and_then(|id| self.nodes.get(id)).and_then(|n| n.as_tab()).map(|t| t.tab.kind);
        if let Some(kind) =
            self.build_anchored_view_picker(ui, AnchoredPopup::SplitView, split_view_button_rect, split_kind)
        {
            self.start_split(kind);
        }
    }

    /// Stand-in while the focused tab is being respawned or woken: just enough chrome to escape to Graph.
    fn build_respawn_placeholder(&mut self, ui: &mut egui::Ui, focused_id: NodeId) {
        // An engine swap or crash recovery is respawning the tab
        // the user is currently looking at — stay in Tab view
        // (don't bounce to Graph, see `pending_respawn_focus`'s doc
        // comment) and show a lightweight placeholder (with just
        // enough chrome to still escape to Graph) instead of
        // nothing until it reappears in `self.nodes`.
        // An Unloaded card still in `self.nodes` (state `Waking`)
        // is a tab being woken from sleep; anything else missing
        // here (a crash respawn, or an engine swap) is a real tab
        // reconnecting.
        let label = if self.nodes.get(&focused_id).and_then(|n| n.as_unloaded()).is_some() {
            "Loading…"
        } else {
            "Reconnecting…"
        };
        let mut back_to_graph = false;
        toolbar_panel("tab_strip_respawning", ui, |ui| {
            ui.horizontal_centered(|ui| {
                if render_return_to_graph(ui) {
                    back_to_graph = true;
                }
                ui.add(egui::Spinner::new().size(12.0));
                ui.label(label);
            });
        });
        if back_to_graph {
            self.view_mode = ViewMode::Graph;
            return;
        }
        ui.centered_and_justified(|ui| {
            ui.add(egui::Spinner::new().size(32.0));
        });
    }

    /// The split pane's engine label and the divider line between the panes.
    fn build_split_pane_label(&self, ui: &mut egui::Ui, split_id: NodeId) {
        let ppp = self.window.as_ref().map_or(1.0, |w| w.scale_factor()) as f32;
        let screen_size =
            self.window.as_ref().map(|w| w.inner_size()).unwrap_or(winit::dpi::PhysicalSize::new(1024, 768));
        let screen_max_y = screen_size.height as f32 / ppp;
        let divider_x = (screen_size.width as f32 / ppp) / 2.0;
        let kind = self.nodes.get(&split_id).and_then(|n| n.as_tab()).map(|t| t.tab.kind);
        ui.painter().line_segment(
            [egui::pos2(divider_x, CHROME_HEIGHT_POINTS), egui::pos2(divider_x, screen_max_y)],
            egui::Stroke::new(1.0, ui.visuals().weak_text_color()),
        );
        egui::Area::new(egui::Id::new("split_pane_label"))
            .fixed_pos(egui::pos2(divider_x + 8.0, CHROME_HEIGHT_POINTS + 6.0))
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::default()
                    .fill(ui.visuals().window_fill)
                    .corner_radius(4u8)
                    .inner_margin(egui::Margin::symmetric(8, 3))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            if let Some(kind) = kind {
                                view_dot(ui, kind, 8.0);
                            }
                            ui.label(kind.map(view_label).unwrap_or("?"));
                        });
                    });
            });
    }

    /// A spinner over the primary pane until its first frame arrives.
    fn build_content_spinner(&self, ui: &mut egui::Ui) {
        let ppp = self.window.as_ref().map_or(1.0, |w| w.scale_factor()) as f32;
        let screen_size =
            self.window.as_ref().map(|w| w.inner_size()).unwrap_or(winit::dpi::PhysicalSize::new(1024, 768));
        let screen_width = screen_size.width as f32 / ppp;
        let screen_height = screen_size.height as f32 / ppp;
        let primary_width = if self.split_tab.is_some() { screen_width / 2.0 } else { screen_width };
        let content_rect =
            egui::Rect::from_min_max(egui::pos2(0.0, CHROME_HEIGHT_POINTS), egui::pos2(primary_width, screen_height));
        ui.scope_builder(egui::UiBuilder::new().max_rect(content_rect), |ui| {
            ui.centered_and_justified(|ui| {
                ui.add(egui::Spinner::new().size(32.0));
            });
        });
    }
}
