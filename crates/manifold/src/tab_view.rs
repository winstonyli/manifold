//! The Item view: the item toolbar every item shares, and the Tab view (its
//! toolbar, address bar and the spawned engine's content pane).

use super::*;
use crate::nav_history::Dir;

/// Tab view's address bar and its autocomplete dropdown.
pub(crate) struct AddressBar {
    pub(crate) text: String,
    pub(crate) focused: bool,
    /// Open tabs matching `text`, recomputed synchronously (cheap, from
    /// data already in memory) whenever it changes.
    pub(crate) open_tab_suggestions: Vec<Suggestion>,
    /// Search-engine suggestions, which arrive later from `fetcher`'s
    /// debounced background fetch.
    pub(crate) search_suggestions: Vec<String>,
    pub(crate) fetcher: SuggestionFetcher,
    /// Index into `combined()` the user has moved to with Up/Down; `None`
    /// means no row is highlighted (Enter navigates to the typed text).
    /// Reset whenever the list changes or the dropdown closes, so a stale
    /// index can't point at an unrelated row.
    pub(crate) highlighted: Option<usize>,
    /// Frames in a row the field hasn't had focus. The dropdown survives one
    /// (egui's focus blip as its buttons appear, see `editing`) and closes
    /// after that: a click into the page must dismiss it.
    pub(crate) unfocused_frames: u8,
}

impl AddressBar {
    pub(crate) fn new(text: String) -> Self {
        Self {
            text,
            focused: false,
            open_tab_suggestions: Vec::new(),
            search_suggestions: Vec::new(),
            fetcher: SuggestionFetcher::new(),
            highlighted: None,
            unfocused_frames: 0,
        }
    }

    /// Records this frame's focus; dismisses the dropdown once focus has
    /// been away for more than one frame.
    pub(crate) fn note_focus(&mut self, has_focus: bool) {
        self.focused = has_focus;
        self.unfocused_frames = if has_focus { 0 } else { self.unfocused_frames.saturating_add(1) };
        if self.unfocused_frames > 1 && (!self.open_tab_suggestions.is_empty() || !self.search_suggestions.is_empty()) {
            self.dismiss();
        }
    }

    /// The dropdown's rows: open tabs first (jumping to one already loaded
    /// beats a fresh navigation), then searches.
    pub(crate) fn combined(&self) -> Vec<Suggestion> {
        let mut all = self.open_tab_suggestions.clone();
        all.extend(self.search_suggestions.iter().cloned().map(|query| Suggestion::Search { query }));
        all
    }

    /// Whether the user is mid-edit, so the text mustn't be resynced to the
    /// tab's URL. Not just `focused`: found live that the moment the
    /// suggestion dropdown's buttons first appear, egui's own keyboard-focus
    /// tracking transiently moves off the `TextEdit` for a frame (a real,
    /// reproducible bug, not a one-off) — `has_focus()` then reports false
    /// for that one frame, which (without this) blew away whatever the user
    /// had just typed by resyncing to the tab's real URL mid-keystroke.
    /// Suggestions being non-empty is a good proxy for "still actively
    /// editing" that survives that blip.
    pub(crate) fn editing(&self) -> bool {
        self.focused || !self.open_tab_suggestions.is_empty() || !self.search_suggestions.is_empty()
    }

    /// Closes the dropdown, and cancels the search fetch in flight: its
    /// response would otherwise reopen the dropdown after an Enter or
    /// Escape.
    pub(crate) fn dismiss(&mut self) {
        self.open_tab_suggestions.clear();
        self.search_suggestions.clear();
        self.highlighted = None;
        self.fetcher.cancel();
    }
}

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
    pub(crate) enter_pressed: bool,
    pub(crate) toggle_bookmark_requested: bool,
    /// The jar dot was clicked while the tab waits to switch: "Switch Jar Now".
    pub(crate) switch_jar: bool,
    pub(crate) close_split_requested: bool,
    /// The Graph button inside the Tab toolbar's nav chip was clicked.
    pub(crate) back_to_graph: bool,
    pub(crate) tab_actions_button_rect: Option<egui::Rect>,
    pub(crate) split_view_button_rect: Option<egui::Rect>,
    pub(crate) address_bar_rect: Option<egui::Rect>,
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
    pub(crate) fn render_item_toolbar(&mut self, node_id: &str, ui: &mut egui::Ui) -> ItemToolbarFrame {
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
        node_id: &str,
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
        focused_id: &str,
        ui: &mut egui::Ui,
        trailing_ui: &mut egui::Ui,
    ) -> ItemToolbarActions {
        let mut actions = ItemToolbarActions::default();
        let (can_go_back, can_go_forward) = self.trail_enabled(focused_id);
        let (back_fork, forward_fork) =
            self.nav_history.get(focused_id).map_or((false, false), |h| (h.back_fork(), h.forward_fork()));
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
            .get(focused_id)
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
        if let Some(chip) = self.jar_chip(focused_id) {
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
        let response = toolbar_field(ui, |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.address_bar.text)
                    .desired_width(text_width)
                    .min_size(egui::vec2(0.0, 28.0))
                    // The field is taller than its text: centre the text in it rather than leave the slack below.
                    .vertical_align(egui::Align::Center)
                    .hint_text("Search or enter address")
                    // Match `style.spacing.button_padding` (set in
                    // `apply_style`) so the address bar renders the
                    // same height as the back/forward buttons next
                    // to it — `TextEdit`'s own default margin
                    // (`Margin::symmetric(4, 2)`) doesn't share that
                    // style value, so left unset the row's tallest
                    // item (the text field) stood out with visibly
                    // more height than its neighbors instead of a
                    // level toolbar.
                    // Room for the favicon at the left and the Ctrl L keycaps at the right.
                    .margin(egui::Margin { left: 32, right: 56, top: 4, bottom: 4 }),
            )
        });
        if take_focus(&mut self.pending_focus, FocusTarget::AddressBar) {
            response.request_focus();
        }
        // Egui's own focus, not `response.has_focus()`: that is also false while
        // the OS window is unfocused, yet the field keeps its edit, so switching
        // windows mid-edit would close the dropdown and resync the typed text
        // away. A click into the page goes through egui and does move focus.
        self.address_bar.note_focus(ui.memory(|m| m.has_focus(response.id)));
        actions.address_bar_rect = Some(response.rect);
        let node = self.nodes[focused_id].as_tab().unwrap();
        let (url, loading, kind) = (node.url.clone(), node.loading, node.tab.kind);
        let identity = self.tab_identity(&url, loading, kind);
        let icon_center = egui::pos2(response.rect.min.x + 8.0 + FAVICON_SIZE.x / 2.0, response.rect.center().y);
        if let Some(text) = identity.paint(ui, icon_center, FAVICON_SIZE.x, 1.0, ui.visuals().weak_text_color()) {
            text.draw(ui.painter(), text.center);
        }
        if !response.has_focus() {
            paint_keycaps(ui, response.rect.right_center() - egui::vec2(8.0, 0.0), &["Ctrl", "L"]);
        }

        if response.changed() {
            self.address_bar.open_tab_suggestions =
                self.compute_open_tab_suggestions(&self.address_bar.text, focused_id);
            self.address_bar.fetcher.note_text_changed(&self.address_bar.text);
            // A changed query invalidates whatever row was
            // highlighted for the *previous* suggestion list —
            // keeping a stale index could silently highlight
            // (and Enter-accept) an unrelated row.
            self.address_bar.highlighted = None;
        }
        if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            actions.enter_pressed = true;
        }

        actions
    }

    /// `ViewMode::Item`: whichever node is maximized -- `viewed_image` if
    /// set, else the focused Tab.
    pub(crate) fn build_item_ui(&mut self, ui: &mut egui::Ui) {
        match self.viewed_image.clone() {
            Some(image_id) => self.build_image_view_ui(ui, &image_id),
            None => self.build_tab_view_ui(ui),
        }
    }

    pub(crate) fn build_tab_view_ui(&mut self, ui: &mut egui::Ui) {
        let Some(focused_id) = self.focused.clone() else {
            self.view_mode = ViewMode::Graph;
            return;
        };
        // Missing (mid-respawn), or a failed tab's card.
        if !self.nodes.get(&focused_id).is_some_and(|n| n.as_tab().is_some()) {
            if self.spawns.pending_respawn_focus.as_deref() == Some(focused_id.as_str()) {
                self.build_respawn_placeholder(ui, &focused_id);
            } else {
                self.view_mode = ViewMode::Graph;
            }
            return;
        }
        let editing_address = self.address_bar.editing();
        // `focused` is tab-only (see `App::focused`), so this always finds
        // a Tab; `if let` rather than `.unwrap()` so a future path that
        // breaks that invariant degrades to a stale address bar, not a
        // panic.
        if !editing_address {
            if let Some(tab) = self.nodes.get(&focused_id).and_then(|n| n.as_tab()) {
                self.address_bar.text = tab.url.clone();
            }
        }
        let mut navigate = None;
        // Whether Enter fired this frame (real keypress or headless `ENTER`)
        // — deferred here rather than acted on immediately inside the
        // toolbar closure below, since deciding what Enter *does* (accept
        // the highlighted dropdown row vs. plain-navigate) needs `combined`
        // (the suggestion list), which isn't computed until after this
        // closure returns.
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
                    enter_pressed,
                    toggle_bookmark_requested,
                    switch_jar,
                    close_split_requested,
                    // Folded into `back_to_graph` by `render_item_toolbar`.
                    back_to_graph: _,
                    tab_actions_button_rect,
                    split_view_button_rect,
                    address_bar_rect,
                    back_branches_rect,
                    forward_branches_rect,
                },
        } = self.render_item_toolbar(&focused_id, ui);

        self.build_shortcuts_help(ui, shortcuts_help_button_rect);

        if self.find.open {
            self.build_find_bar(ui);
        }

        self.build_context_menu(ui);

        if let Some(results) = self.address_bar.fetcher.tick() {
            self.address_bar.search_suggestions = results;
        }

        // Autocomplete dropdown: a separate `egui::Area` anchored right
        // below the address bar (screen space, not the toolbar `Panel`'s
        // own layout — the dropdown needs to float over the content area
        // below chrome, which a plain widget inside the toolbar couldn't
        // do without stretching the toolbar itself). Clicking a row accepts
        // it directly; Up/Down move `AddressBar::highlighted` (wrapping at
        // both ends) and Enter accepts whichever row is highlighted (see
        // `enter_pressed`'s resolution below) — reading arrow keys via
        // `ui.input(|i| i.key_pressed(...))` here, after the `TextEdit`
        // above already had its turn, is safe: `key_pressed` is a plain
        // per-frame boolean read, not a consumed event, and a singleline
        // `TextEdit` has no vertical text to move a cursor through, so
        // there's no competing behavior to fight over (Enter was the one
        // that actually needed care, via `enter_pressed`'s deferred
        // resolution — a singleline `TextEdit` losing focus on Enter is a
        // real, meaningful interaction to not double-handle).
        // Gated on `editing_address`, not raw `self.address_bar.focused`:
        // the same one-frame focus blip documented above (egui's focus
        // tracking transiently leaving the `TextEdit` when the dropdown's
        // buttons appear) was also hiding this whole Area, including the
        // buttons, on the exact frame a click landed on one — so the click
        // never reached anything and just silently dismissed the dropdown.
        let combined = self.address_bar.combined();
        if editing_address && !combined.is_empty() {
            self.build_address_suggestions(ui, &combined, address_bar_rect, &focused_id);
        }

        if enter_pressed {
            match self.address_bar.highlighted.filter(|&i| i < combined.len()) {
                Some(idx) => self.accept_suggestion(combined[idx].clone(), &focused_id),
                None => navigate = Some(self.address_bar.text.clone()),
            }
        }

        let has_suggestions =
            !self.address_bar.open_tab_suggestions.is_empty() || !self.address_bar.search_suggestions.is_empty();
        // Neutralized while the quick-switcher is open: it does its own,
        // separate Escape check in `build_quick_switcher` to close itself,
        // and `key_pressed` is a non-consuming peek -- without this, the
        // same Escape press would also fire this chain (leaving to Graph,
        // or dismissing the address-bar dropdown) underneath it.
        let escape_pressed =
            ui.input(|i| i.key_pressed(egui::Key::Escape)) && self.menus.quick_switcher.is_none() && !self.popup_open();
        if back_to_graph || (!has_suggestions && !self.find.open && self.menus.context_menu.is_none() && escape_pressed)
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
            if self.menus.context_menu.is_some() {
                self.menus.context_menu = None;
            } else if self.find.open {
                self.close_find_bar();
            } else if has_suggestions {
                self.address_bar.dismiss();
            }
        }
        for (requested, dir) in [(go_back, Dir::Back), (go_forward, Dir::Forward)] {
            if requested {
                self.navigate_history(&focused_id, dir);
            }
        }
        if reload_requested {
            self.reload_focused_tab_shortcut();
        }
        if home_requested {
            let home_page = self.settings.home_page.clone();
            self.navigate_tab(&focused_id, home_page);
        }
        if close_split_requested {
            self.close_split();
        }
        // The secondary pane's own small "which engine is this" label plus
        // a thin divider line -- drawn here (real egui, on top of both raw
        // content quads) rather than as part of either pane's own content,
        // since the secondary pane has no toolbar of its own to put it in
        // (view-only in this first cut, see `split_tab`'s own doc comment).
        if let Some(split_id) = self.split_tab.clone() {
            self.build_split_pane_label(ui, &split_id);
        }
        // A freshly-spawned or just-respawned tab's content pane draws as
        // plain black until its view-host's first real frame arrives (see
        // `record_content_pass`'s own doc comment -- the pane still clears,
        // it just has nothing to draw) -- confirmed live, a genuinely jarring
        // blank screen with zero feedback, unlike the Graph-view card's own
        // spinner for the exact same "no thumbnail yet" state. Only covers
        // the primary pane; the split view's secondary pane (view-only,
        // still a first cut per `split_tab`'s own doc comment) is left as-is.
        let has_content = self.gpu.as_ref().is_some_and(|g| g.has_tab_texture(&focused_id));
        if !has_content {
            self.build_content_spinner(ui);
        }
        if toggle_bookmark_requested {
            self.toggle_bookmark(&focused_id);
        }
        if switch_jar {
            self.switch_jar_now(&focused_id);
        }
        let ctx = ui.ctx().clone();
        if let Some((rect, fade)) = self.anchored_popup_fade(&ctx, AnchoredPopup::TabActions, tab_actions_button_rect) {
            self.build_tab_actions_popup(ui, rect, &focused_id, fade);
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
                self.build_trail_branches_popup(ui, dir, rect, &focused_id, fade);
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
        if let Some(text) = navigate {
            let url = resolve_navigation_url(&text, self.settings.default_search_engine);
            self.navigate_tab(&focused_id, url);
            self.address_bar.dismiss();
        }
    }

    /// Stand-in while the focused tab is being respawned or woken: just enough chrome to escape to Graph.
    fn build_respawn_placeholder(&mut self, ui: &mut egui::Ui, focused_id: &str) {
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
        let label = if self.nodes.get(focused_id).and_then(|n| n.as_unloaded()).is_some() {
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

    /// The address-bar suggestion dropdown: Up/Down move the highlight, a click accepts a row.
    fn build_address_suggestions(
        &mut self,
        ui: &mut egui::Ui,
        combined: &[Suggestion],
        address_bar_rect: Option<egui::Rect>,
        focused_id: &str,
    ) {
        // The arrows are the field's only while it has focus (or is in its one-frame blip).
        let live = self.address_bar.unfocused_frames <= 1;
        let arrow_down = live && ui.input(|i| i.key_pressed(egui::Key::ArrowDown));
        let arrow_up = live && ui.input(|i| i.key_pressed(egui::Key::ArrowUp));
        if arrow_down {
            self.address_bar.highlighted = Some(match self.address_bar.highlighted {
                Some(i) if i + 1 < combined.len() => i + 1,
                _ => 0,
            });
        }
        if arrow_up {
            self.address_bar.highlighted = Some(match self.address_bar.highlighted {
                Some(i) if i > 0 => i - 1,
                _ => combined.len() - 1,
            });
        }

        if let Some(rect) = address_bar_rect {
            let mut accepted = None;
            egui::Area::new(egui::Id::new("address_suggestions"))
                .fixed_pos(rect.left_bottom())
                .order(egui::Order::Foreground)
                .show(ui.ctx(), |ui| {
                    // Inner width minus the frame's padding and border, so the
                    // dropdown lines up with the address bar's edges.
                    let frame = popup_frame(ui);
                    let inner_width = rect.width() - frame.total_margin().sum().x;
                    frame.show(ui, |ui| {
                        menu_item_style(ui);
                        ui.set_width(inner_width);
                        // `ui.set_width` alone only sets the *container's*
                        // layout width (affects wrapping/available_width),
                        // not an individual `Button`'s own size — left
                        // unset, each row was only as wide as its own
                        // text, producing a ragged right edge instead of
                        // a uniform list. `min_size`'s width stretches
                        // each row to fill the container; height stays
                        // `0.0` (no minimum) so natural row height/padding
                        // is untouched.
                        let full_width = ui.available_width();
                        for (idx, suggestion) in combined.iter().enumerate() {
                            let highlighted = self.address_bar.highlighted == Some(idx);
                            let identity = match suggestion {
                                Suggestion::OpenTab { tab_id, url, .. } => self.row_identity(tab_id, url),
                                Suggestion::Search { .. } => {
                                    Identity::Glyph { glyph: egui_phosphor::light::MAGNIFYING_GLASS, color: None }
                                }
                            };
                            let mut button = egui::Button::new(row_label(ui, &suggestion.label(), 0.0))
                                .truncate()
                                .min_size(egui::vec2(full_width, 0.0));
                            if highlighted {
                                button = button.fill(ACCENT);
                            }
                            let row = ui.add(button);
                            paint_row_icon(ui, row.rect, ui.spacing().button_padding.x, identity);
                            if row.clicked() {
                                accepted = Some(suggestion.clone());
                            }
                        }
                    });
                });
            if let Some(suggestion) = accepted {
                self.accept_suggestion(suggestion, focused_id);
            }
        }
    }

    /// The split pane's engine label and the divider line between the panes.
    fn build_split_pane_label(&self, ui: &mut egui::Ui, split_id: &str) {
        let ppp = self.window.as_ref().map_or(1.0, |w| w.scale_factor()) as f32;
        let screen_size =
            self.window.as_ref().map(|w| w.inner_size()).unwrap_or(winit::dpi::PhysicalSize::new(1024, 768));
        let screen_max_y = screen_size.height as f32 / ppp;
        let divider_x = (screen_size.width as f32 / ppp) / 2.0;
        let kind = self.nodes.get(split_id).and_then(|n| n.as_tab()).map(|t| t.tab.kind);
        ui.painter().line_segment(
            [egui::pos2(divider_x, CHROME_HEIGHT_POINTS), egui::pos2(divider_x, screen_max_y)],
            egui::Stroke::new(1.0, ui.visuals().weak_text_color()),
        );
        egui::Area::new(egui::Id::new("split_pane_label"))
            .fixed_pos(egui::pos2(divider_x + 8.0, CHROME_HEIGHT_POINTS + 6.0))
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::default()
                    .fill(TOOLBAR_FILL)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Review C-B6: the dropdown survives one unfocused frame (the focus blip) but not two.
    #[test]
    fn the_dropdown_closes_after_focus_stays_away() {
        let mut bar = AddressBar::new(String::new());
        bar.search_suggestions = vec!["rust".into()];
        bar.note_focus(false);
        assert!(bar.editing(), "one frame is the known blip");
        bar.note_focus(true);
        bar.note_focus(false);
        assert!(bar.editing(), "regaining focus starts the count again");
        bar.note_focus(false);
        assert!(!bar.editing(), "dismissed");
    }
}
