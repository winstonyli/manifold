//! The Settings page: its sections, the requests they collect, and the
//! deferred effects and popups that run after the scroll area.

use super::*;

/// What the Settings page's sections (`App::settings_general_section`
/// and the rest) asked for or laid out this frame. `build_settings_page`
/// acts on it after its scroll area: `apply_settings_requests`, then
/// `build_settings_popups`.
#[derive(Default)]
pub(crate) struct SettingsRequests {
    pub(crate) save_as_requested: bool,
    pub(crate) open_session_name: Option<String>,
    pub(crate) delete_session_name: Option<String>,
    pub(crate) export_requested: bool,
    pub(crate) import_requested: bool,
    pub(crate) export_bookmarks_requested: bool,
    pub(crate) import_bookmarks_requested: bool,
    pub(crate) save_now_requested: bool,
    /// `settings_current_tab_section`: `self.focused` persists across a
    /// view switch (entering Settings doesn't clear it), so these act on
    /// whichever tab was focused right before Settings was opened, same
    /// as the toolbar controls they replaced.
    pub(crate) current_tab_id: Option<String>,
    pub(crate) mute_toggle: bool,
    pub(crate) screenshot_requested: bool,
    pub(crate) print_to_pdf_requested: bool,
    /// `settings_lists_section`: same popups the old toolbar "Lists"
    /// dropdown opened, just anchored to these buttons instead.
    pub(crate) bookmarks_button_rect: Option<egui::Rect>,
    pub(crate) history_button_rect: Option<egui::Rect>,
    pub(crate) downloads_button_rect: Option<egui::Rect>,
    pub(crate) default_engine_button_rect: Option<egui::Rect>,
    pub(crate) current_engine_button_rect: Option<egui::Rect>,
}

impl App {
    /// The Settings page (`ViewMode::Settings`): default search engine,
    /// default starting engine, zoom, and home page — see `settings.rs` for
    /// what each actually controls and `App::settings`'s own doc comment
    /// for why this is one shared page rather than per-view state. Every
    /// control saves to disk immediately on change (no separate "Save"
    /// step, consistent with every other instant-apply control in this
    /// chrome, e.g. the engine picker). **A real page, not a popup**: the
    /// original gear-icon dropdown was a fixed-position `Area` with no
    /// scroll area and a fixed 200pt width -- overflowed off the bottom of
    /// a small window once the saved-sessions list grew long enough,
    /// reported live by a user. A full page gets the whole window to
    /// scroll in instead of a hand-picked popup height.
    pub(crate) fn build_settings_page(&mut self, ui: &mut egui::Ui) {
        let mut req = SettingsRequests::default();

        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.close_settings_page();
        }

        toolbar_panel("settings_toolbar", ui, |ui| {
            ui.horizontal_centered(|ui| {
                if icon_button_eased(
                    ui,
                    egui::Id::new("icon_btn_settings_back"),
                    format!("{} Back", egui_phosphor::light::CARET_LEFT),
                )
                .clicked()
                {
                    self.close_settings_page();
                }
                ui.label(heading_text("Settings"));
            });
        });

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            egui::Frame::default().inner_margin(egui::Margin::symmetric(16, 0)).show(ui, |ui| {
                ui.set_max_width(480.0);
                ui.add_space(12.0);

                self.settings_general_section(ui, &mut req);
                self.settings_current_tab_section(ui, &mut req);
                self.settings_lists_section(ui, &mut req);
                self.settings_sessions_section(ui, &mut req);
                self.settings_bookmarks_section(ui, &mut req);

                ui.add_space(12.0);
            });
        });
        self.apply_settings_requests(&req);
        self.build_settings_popups(ui, &req);
    }

    /// Settings: search engine, starting engine, zoom and home page.
    pub(crate) fn settings_general_section(&mut self, ui: &mut egui::Ui, req: &mut SettingsRequests) {
        ui.label("Default search engine");
        egui::ComboBox::from_id_salt("settings_search_engine")
            .selected_text(self.settings.default_search_engine.label())
            .show_ui(ui, |ui| {
                for engine in SearchEngine::ALL {
                    let selected = self.settings.default_search_engine == engine;
                    if ui.selectable_label(selected, engine.label()).clicked() && !selected {
                        self.settings.default_search_engine = engine;
                        self.save_settings();
                    }
                }
            });
        ui.add(
            egui::Label::new(
                egui::RichText::new(
                    "Suggestions always come from DuckDuckGo; this only picks where a search actually goes.",
                )
                .small()
                .weak(),
            )
            .wrap(),
        );

        ui.add_space(8.0);
        ui.label("Default starting engine (new tabs)");
        let combo_text = egui::RichText::new(engine_label(self.settings.default_starting_engine))
            .color(engine_color(self.settings.default_starting_engine));
        let default_engine_response = ui.button(combo_text);
        if default_engine_response.clicked() {
            toggle_popup(&mut self.open_popup, AnchoredPopup::DefaultEngine);
        }
        req.default_engine_button_rect = Some(default_engine_response.rect);

        ui.add_space(8.0);
        ui.label("Zoom (new tabs; also applies to the current tab now)");
        ui.horizontal(|ui| {
            // Reads/writes the *focused tab's own* zoom, not the raw
            // `Settings::zoom_factor` -- see `set_focused_zoom`'s own
            // doc comment for the display-vs-reality bug this fixed.
            if ui.button(egui_phosphor::light::MINUS).clicked() {
                self.set_focused_zoom((self.focused_zoom_factor() - 0.1).max(0.5));
            }
            ui.label(format!("{:.0}%", self.focused_zoom_factor() * 100.0));
            if ui.button(egui_phosphor::light::PLUS).clicked() {
                self.set_focused_zoom((self.focused_zoom_factor() + 0.1).min(2.0));
            }
            if ui.button("Reset").clicked() {
                self.set_focused_zoom(1.0);
            }
        });

        ui.add_space(8.0);
        ui.label("Home page (new tabs)");
        if ui.add(egui::TextEdit::singleline(&mut self.settings.home_page).desired_width(f32::INFINITY)).lost_focus() {
            self.save_settings();
        }

        ui.add_space(8.0);
        ui.label("Put background tabs to sleep after");
        ui.horizontal(|ui| {
            let mut minutes = self.settings.idle_sleep_minutes;
            let label = match minutes {
                Some(m) => format!("{m} min"),
                None => "Never".to_string(),
            };
            for (text, value) in [("Never", None), ("10 min", Some(10)), ("30 min", Some(30)), ("60 min", Some(60))] {
                if ui.selectable_label(label == text, text).clicked() {
                    minutes = value;
                }
            }
            if minutes != self.settings.idle_sleep_minutes {
                self.settings.idle_sleep_minutes = minutes;
                self.save_settings();
            }
        });
        ui.add(
            egui::Label::new(
                egui::RichText::new(
                    "Applies to open tabs. Sleeping closes the page; waking reloads it. Pinned tabs and tabs playing audio stay awake.",
                )
                .small()
                .weak(),
            )
            .wrap(),
        );
    }

    /// Settings, "Current Tab": engine, mute, screenshot, print.
    pub(crate) fn settings_current_tab_section(&mut self, ui: &mut egui::Ui, req: &mut SettingsRequests) {
        ui.add_space(10.0);
        ui.separator();
        ui.label(heading_text("Current Tab"));
        // Engine swap, Mute, Screenshot, and Print to PDF used to
        // be live toolbar controls next to the address bar --
        // moved here to keep that toolbar down to nav + address
        // bar + a small handful of page actions, at the cost of a
        // click through Settings instead of an in-place toggle.
        // `focus_kind` is `None` either when no tab is focused at
        // all, or Settings was reached with a non-tab node
        // (group/note/etc.) focused -- either way, nothing here
        // applies to it.
        let focus_kind = self
            .focused
            .clone()
            .and_then(|id| self.nodes.get(&id).and_then(|n| n.as_tab()).map(|n| n.tab.kind).map(|kind| (id, kind)));
        match focus_kind {
            Some((id, kind)) => {
                req.current_tab_id = Some(id.clone());
                ui.horizontal(|ui| {
                    ui.label("Engine");
                    let combo_text = egui::RichText::new(engine_label(kind)).color(engine_color(kind));
                    let current_engine_response = ui.button(combo_text);
                    if current_engine_response.clicked() {
                        toggle_popup(&mut self.open_popup, AnchoredPopup::CurrentEngine);
                    }
                    req.current_engine_button_rect = Some(current_engine_response.rect);
                });
                let node_muted = self.nodes.get(&id).and_then(|n| n.as_tab()).is_some_and(|n| n.muted);
                let mute_label = if node_muted {
                    format!("{} Unmute", egui_phosphor::light::SPEAKER_X)
                } else {
                    format!("{} Mute", egui_phosphor::light::SPEAKER_HIGH)
                };
                if ui.selectable_label(node_muted, mute_label).clicked() {
                    req.mute_toggle = true;
                }
                ui.horizontal(|ui| {
                    if ui.button(format!("{} Screenshot", egui_phosphor::light::CAMERA)).clicked() {
                        req.screenshot_requested = true;
                    }
                    if !self.headless && ui.button(format!("{} Print to PDF", egui_phosphor::light::PRINTER)).clicked()
                    {
                        req.print_to_pdf_requested = true;
                    }
                });
            }
            None => {
                ui.add(egui::Label::new(egui::RichText::new("No tab focused").small().weak()).wrap());
            }
        }
    }

    /// Settings, "Lists": the bookmarks/history/downloads buttons.
    pub(crate) fn settings_lists_section(&mut self, ui: &mut egui::Ui, req: &mut SettingsRequests) {
        ui.add_space(10.0);
        ui.separator();
        ui.label(heading_text("Lists"));
        ui.horizontal(|ui| {
            let bookmarks_response = ui.button(format!("Bookmarks ({})", self.bookmarks.len()));
            if bookmarks_response.clicked() {
                toggle_popup(&mut self.open_popup, AnchoredPopup::Bookmarks);
                if self.open_popup == Some(AnchoredPopup::Bookmarks) {
                    self.bookmarks_filter.clear();
                }
            }
            req.bookmarks_button_rect = Some(bookmarks_response.rect);
            let history_response = ui.button(format!("History ({})", self.history.len()));
            if history_response.clicked() {
                toggle_popup(&mut self.open_popup, AnchoredPopup::History);
                if self.open_popup == Some(AnchoredPopup::History) {
                    self.history_filter.clear();
                }
            }
            req.history_button_rect = Some(history_response.rect);
            // Not `self.nodes.len()` (every graph node, not just
            // downloads) -- a real pre-existing bug found and fixed
            // when this button was still in the toolbar's own
            // "Lists" dropdown, carried forward here unchanged.
            let download_count = self.nodes.values().filter(|n| n.as_download().is_some()).count();
            let downloads_response = ui.button(format!("Downloads ({download_count})"));
            if downloads_response.clicked() {
                toggle_popup(&mut self.open_popup, AnchoredPopup::Downloads);
            }
            req.downloads_button_rect = Some(downloads_response.rect);
        });
    }

    /// Settings, "Sessions": save, save as, open/delete, export/import.
    pub(crate) fn settings_sessions_section(&mut self, ui: &mut egui::Ui, req: &mut SettingsRequests) {
        ui.add_space(10.0);
        ui.separator();
        ui.label(heading_text("Sessions"));
        if ui.button("Save Now").on_hover_text("Write the auto-restored session (session.mnf) now").clicked() {
            req.save_now_requested = true;
        }
        let last_saved = match self.last_session_save {
            Some(at) => format!("Last saved {}s ago", at.elapsed().as_secs()),
            None => "Not saved yet this run".to_string(),
        };
        ui.add(egui::Label::new(egui::RichText::new(last_saved).small().weak()).wrap());
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.session_name_draft).desired_width(120.0).hint_text("Name"));
            if ui.button("Save As").clicked() {
                req.save_as_requested = true;
            }
        });
        let names = session::list_named();
        if names.is_empty() {
            ui.add(egui::Label::new(egui::RichText::new("No saved sessions").small().weak()).wrap());
        }
        for name in names {
            ui.horizontal(|ui| {
                if ui.button(&name).on_hover_text("Open").clicked() {
                    req.open_session_name = Some(name.clone());
                }
                if ui.small_button(egui_phosphor::light::X).on_hover_text("Delete").clicked() {
                    req.delete_session_name = Some(name.clone());
                }
            });
        }
        // A real native file dialog blocks on OS/human
        // input -- fine for the real app (that's what a
        // modal dialog is for), but headless mode has
        // neither, so it would just hang forever with no
        // way to test it here. `EXPORTSESSION`/
        // `IMPORTSESSION` debug commands call
        // `export_session_to`/`json_file::load` directly with an
        // explicit path instead, bypassing the dialog
        // entirely (import still stages into
        // `pending_import` same as a real file pick would,
        // so `CONFIRMIMPORT`/`CANCELIMPORT` can headlessly
        // test the confirm popup too), the same "headless
        // calls the effect directly" pattern every real-
        // keyboard-shortcut debug command uses.
        if !self.headless {
            ui.horizontal(|ui| {
                if ui.button("Export...").clicked() {
                    req.export_requested = true;
                }
                if ui.button("Import...").clicked() {
                    req.import_requested = true;
                }
            });
        }
    }

    /// Settings, "Bookmarks": export/import.
    pub(crate) fn settings_bookmarks_section(&mut self, ui: &mut egui::Ui, req: &mut SettingsRequests) {
        ui.add_space(10.0);
        ui.separator();
        ui.label(heading_text("Bookmarks"));
        // Same "real file dialog blocks on OS/human input
        // neither exists in headless mode" reasoning as
        // Export/Import Session (`settings_sessions_section`) -- `EXPORTBOOKMARKS`/
        // `IMPORTBOOKMARKS` debug commands bypass the dialog
        // the same way.
        if !self.headless {
            ui.horizontal(|ui| {
                if ui.button("Export...").clicked() {
                    req.export_bookmarks_requested = true;
                }
                if ui.button("Import...").clicked() {
                    req.import_bookmarks_requested = true;
                }
            });
        }
    }

    /// What the Settings sections asked for, acted on once the page's
    /// scroll area is done (`build_settings_page`).
    pub(crate) fn apply_settings_requests(&mut self, req: &SettingsRequests) {
        if req.save_now_requested {
            self.save_session();
        }
        if req.save_as_requested {
            let name = sanitize_session_name(&self.session_name_draft);
            if !name.is_empty() {
                self.save_named_session(&name);
            }
        }
        if let Some(name) = &req.open_session_name {
            self.load_named_session(name);
            self.close_settings_page();
        }
        if let Some(name) = &req.delete_session_name {
            session::delete_named(name);
        }
        if req.export_requested {
            if let Some((path, toast)) = save_dialog_with_toast("session.json", "Session") {
                if self.export_session_to(&path) {
                    self.queue_toast(toast.0);
                }
            }
        }
        if req.import_requested {
            if let Some(path) = rfd::FileDialog::new().add_filter("Session", &["json"]).pick_file() {
                match crate::json_file::load(&path, "session") {
                    Ok(json) => {
                        self.pending_import = Some(json);
                        self.close_settings_page();
                    }
                    Err(err) => self.queue_toast(err),
                }
            }
        }
        if req.export_bookmarks_requested {
            if let Some((path, toast)) = save_dialog_with_toast("bookmarks.json", "Bookmarks") {
                bookmarks::save_to(&self.bookmarks, &path);
                self.queue_toast(toast.0);
            }
        }
        if req.import_bookmarks_requested {
            if let Some(path) = rfd::FileDialog::new().add_filter("Bookmarks", &["json"]).pick_file() {
                match bookmarks::load_from(&path) {
                    Ok(imported) => {
                        self.bookmarks = imported;
                        bookmarks::save(&self.bookmarks);
                    }
                    Err(err) => self.queue_toast(err),
                }
                self.close_settings_page();
            }
        }
        if req.mute_toggle {
            if let Some(id) = &req.current_tab_id {
                self.toggle_muted(id);
            }
        }
        if req.screenshot_requested {
            // No toast here -- the save hasn't happened yet. `save_toast`
            // is only set once `RenderOutcome::screenshot_result` actually
            // confirms success (see `handle_screenshot_result`).
            let path = screenshot_default_path();
            self.pending_screenshot = Some(path.to_string_lossy().to_string());
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
        if req.print_to_pdf_requested {
            if let Some(path) = rfd::FileDialog::new().set_file_name("page.pdf").add_filter("PDF", &["pdf"]).save_file()
            {
                self.start_print_to_pdf(path.to_string_lossy().into_owned());
            }
        }
    }

    /// The popups the Settings page anchors to its buttons: the three
    /// lists and the two engine pickers.
    pub(crate) fn build_settings_popups(&mut self, ui: &mut egui::Ui, req: &SettingsRequests) {
        let ctx = ui.ctx().clone();
        if let Some((rect, fade)) = self.anchored_popup_fade(&ctx, AnchoredPopup::Bookmarks, req.bookmarks_button_rect)
        {
            self.build_bookmarks_popup(ui, rect, fade);
        }
        if let Some((rect, fade)) = self.anchored_popup_fade(&ctx, AnchoredPopup::History, req.history_button_rect) {
            self.build_history_popup(ui, rect, fade);
        }
        if let Some((rect, fade)) = self.anchored_popup_fade(&ctx, AnchoredPopup::Downloads, req.downloads_button_rect)
        {
            self.build_downloads_popup(ui, rect, fade);
        }
        let default_engine = Some(self.settings.default_starting_engine);
        if let Some(kind) = self.build_anchored_engine_picker(
            ui,
            AnchoredPopup::DefaultEngine,
            req.default_engine_button_rect,
            default_engine,
        ) {
            self.settings.default_starting_engine = kind;
            self.save_settings();
        }
        let current_kind =
            req.current_tab_id.as_ref().and_then(|id| self.nodes.get(id)).and_then(|n| n.as_tab()).map(|t| t.tab.kind);
        if let Some(kind) = self.build_anchored_engine_picker(
            ui,
            AnchoredPopup::CurrentEngine,
            req.current_engine_button_rect,
            current_kind,
        ) {
            if let Some(id) = &req.current_tab_id {
                if Some(kind) != current_kind {
                    self.start_swap_to(id, kind);
                }
            }
        }
    }

    /// Gear-icon button (Tab or Graph toolbar) / headless
    /// `TOGGLESETTINGSPAGE` — enters the Settings page from whichever of
    /// Tab/Graph is currently showing, remembering it in
    /// `settings_return_view` so `close_settings_page` knows where to go
    /// back to. A no-op if already on the Settings page (doesn't clobber
    /// the return view with `Settings` itself).
    pub(crate) fn open_settings_page(&mut self) {
        if self.view_mode != ViewMode::Settings {
            self.settings_return_view = self.view_mode;
            self.view_mode = ViewMode::Settings;
        }
    }

    /// The Settings page's own Back button / `Escape` / `Ctrl+G` — returns
    /// to `settings_return_view`. A no-op if the Settings page isn't
    /// showing.
    pub(crate) fn close_settings_page(&mut self) {
        if self.view_mode == ViewMode::Settings {
            self.view_mode = self.settings_return_view;
        }
    }

    /// Gear-icon button's actual click handler -- opens the Settings page,
    /// or closes it back to `settings_return_view` if it's already open
    /// (the button doubles as Back while on the page, same toggle feel the
    /// old gear-icon popup had).
    pub(crate) fn toggle_settings_page(&mut self) {
        if self.view_mode == ViewMode::Settings {
            self.close_settings_page();
        } else {
            self.open_settings_page();
        }
    }
}
