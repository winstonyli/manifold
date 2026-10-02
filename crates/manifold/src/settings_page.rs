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

        if ui.input(|i| i.key_pressed(egui::Key::Escape)) && !self.popup_open() && self.pending_engine_confirm.is_none()
        {
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
                control_font(ui);
                ui.add_space(12.0);

                self.settings_general_section(ui, &mut req);
                self.settings_current_tab_section(ui, &mut req);
                self.settings_engines_section(ui);
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
                menu_item_style(ui);
                for engine in SearchEngine::ALL {
                    let selected = self.settings.default_search_engine == engine;
                    if selectable_label(ui, selected, engine.label()).clicked() && !selected {
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
        let default_engine_response =
            self.engine_opener_button(ui, AnchoredPopup::DefaultEngine, self.settings.default_starting_engine);
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
        let mut minutes = self.settings.idle_sleep_minutes;
        setting_row(
            ui,
            "Sleep background tabs",
            Some(
                "Applies to open tabs. Sleeping closes the page; waking reloads it. Pinned tabs and tabs playing audio stay awake.",
            ),
            |ui| {
                if toggle(ui, minutes.is_some(), "Sleep background tabs").clicked() {
                    minutes = if minutes.is_some() { None } else { Some(10) };
                }
            },
        );
        if let Some(current) = minutes {
            setting_row(ui, "Idle time", None, |ui| {
                egui::ComboBox::from_id_salt("settings_idle_sleep").selected_text(format!("{current} min")).show_ui(
                    ui,
                    |ui| {
                        menu_item_style(ui);
                        for option in [10, 30, 60] {
                            if selectable_label(ui, current == option, format!("{option} min")).clicked() {
                                minutes = Some(option);
                            }
                        }
                    },
                );
            });
        }
        if minutes != self.settings.idle_sleep_minutes {
            self.settings.idle_sleep_minutes = minutes;
            self.save_settings();
        }
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
                    let current_engine_response = self.engine_opener_button(ui, AnchoredPopup::CurrentEngine, kind);
                    req.current_engine_button_rect = Some(current_engine_response.rect);
                });
                let node_muted = self.nodes.get(&id).and_then(|n| n.as_tab()).is_some_and(|n| n.muted);
                setting_row(ui, "Mute", None, |ui| {
                    if toggle(ui, node_muted, "Mute this tab").clicked() {
                        req.mute_toggle = true;
                    }
                });
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

    /// Settings, "Engines": each engine's state, and the actions that state allows -- Download/Update
    /// (both ask first, naming the size), Cancel while installing, Retry after a failure, and Remove
    /// (asks first) with the installed size for a downloaded runtime.
    pub(crate) fn settings_engines_section(&mut self, ui: &mut egui::Ui) {
        use crate::engine_install::EngineState;
        ui.add_space(10.0);
        ui.separator();
        ui.label(heading_text("Engines"));
        for kind in ENGINE_PICKER_ORDER {
            let state = self.engine_state(kind);
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                engine_dot(ui, kind, 10.0);
                ui.label(egui::RichText::new(engine_label(kind)).color(TEXT_PRIMARY));
            });
            let runtime = crate::process_manager::engine_runtime_dir(kind).filter(|dir| dir.exists());
            match &state {
                EngineState::Installed | EngineState::UpdateNeeded { .. } if runtime.is_some() => {
                    let size = runtime.as_deref().map(|dir| cached_dir_size(ui.ctx(), dir)).unwrap_or(0);
                    let (label, tone) = engine_state_label(&state);
                    ui.label(
                        egui::RichText::new(format!("{label} \u{b7} {} on disk", megabytes(size)))
                            .small()
                            .color(tone.color()),
                    );
                }
                _ => engine_state_line(ui, &state),
            }
            let removable = runtime.is_some() && !matches!(state, EngineState::Installing { .. });
            if matches!(state, EngineState::Installed | EngineState::Custom(_)) && !removable {
                continue;
            }
            ui.horizontal(|ui| {
                match &state {
                    EngineState::NotInstalled { .. } if ui.button("Download").clicked() => {
                        self.pending_engine_confirm = Some(EngineConfirm::Download(kind));
                    }
                    EngineState::UpdateNeeded { .. } if ui.button("Update").clicked() => {
                        self.pending_engine_confirm = Some(EngineConfirm::Download(kind));
                    }
                    EngineState::Failed(_) if ui.button("Retry").clicked() => {
                        self.pending_engine_confirm = Some(EngineConfirm::Download(kind));
                    }
                    EngineState::Installing { .. } if ui.button("Cancel").clicked() => self.cancel_engine_install(kind),
                    _ => {}
                }
                if removable && ui.button("Remove").on_hover_text("Delete the downloaded runtime").clicked() {
                    self.pending_engine_confirm = Some(EngineConfirm::Remove(kind));
                }
            });
        }
        self.settings_cef_directory_row(ui);
    }

    /// Settings, Engines: a CEF directory to use instead of a download. The text is committed (and
    /// validated, `set_cef_directory`) on Enter, focus loss, Browse or Clear, never per keystroke; a
    /// directory that fails validation shows why and is not used.
    fn settings_cef_directory_row(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.label(egui::RichText::new("Use your own Blink (CEF) folder").small().color(TEXT_PRIMARY));
        let mut commit = None;
        let configured = self.settings.cef_directory.clone().unwrap_or_default();
        ui.horizontal(|ui| {
            let draft = self.cef_dir_draft.get_or_insert_with(|| configured.clone());
            let response =
                ui.add(egui::TextEdit::singleline(draft).desired_width(220.0).hint_text("Folder with libcef.dll"));
            if response.lost_focus() {
                commit = Some(draft.clone());
            }
            if ui.button("Browse").clicked() {
                if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                    commit = Some(dir.to_string_lossy().into_owned());
                }
            }
            if !configured.is_empty() && ui.button("Clear").clicked() {
                commit = Some(String::new());
            }
        });
        if let Some(text) = commit {
            if text.trim() != configured {
                self.set_cef_directory(&text);
            }
            self.cef_dir_draft = None;
        }
        if let Some((_, Err(message))) = self.cef_dir_verdict() {
            ui.add(egui::Label::new(egui::RichText::new(message).small().color(DANGER_COLOR)).wrap());
        }
    }

    /// Settings, "Lists": the bookmarks/history/downloads buttons.
    pub(crate) fn settings_lists_section(&mut self, ui: &mut egui::Ui, req: &mut SettingsRequests) {
        ui.add_space(10.0);
        ui.separator();
        ui.label(heading_text("Lists"));
        ui.horizontal(|ui| {
            let bookmarks_response = ui.button(format!("Bookmarks ({})", self.bookmarks.len()));
            if self.popup_opener_clicked(ui, AnchoredPopup::Bookmarks, &bookmarks_response) {
                self.bookmarks_filter.clear();
            }
            req.bookmarks_button_rect = Some(bookmarks_response.rect);
            let history_response = ui.button(format!("History ({})", self.history.len()));
            if self.popup_opener_clicked(ui, AnchoredPopup::History, &history_response) {
                self.history_filter.clear();
            }
            req.history_button_rect = Some(history_response.rect);
            // Not `self.nodes.len()` (every graph node, not just
            // downloads) -- a real pre-existing bug found and fixed
            // when this button was still in the toolbar's own
            // "Lists" dropdown, carried forward here unchanged.
            let download_count = self.nodes.values().filter(|n| n.as_download().is_some()).count();
            let downloads_response = ui.button(format!("Downloads ({download_count})"));
            self.popup_opener_clicked(ui, AnchoredPopup::Downloads, &downloads_response);
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

/// `dir`'s size, recomputed at most every few seconds (egui memory) so the Settings page doesn't walk the
/// runtime's files on every frame.
fn cached_dir_size(ctx: &egui::Context, dir: &std::path::Path) -> u64 {
    let key = egui::Id::new(("engine_dir_size", dir));
    let now = std::time::Instant::now();
    if let Some((at, size)) = ctx.data(|d| d.get_temp::<(std::time::Instant, u64)>(key)) {
        if now.duration_since(at) < std::time::Duration::from_secs(3) {
            return size;
        }
    }
    let size = crate::engine_install::dir_size(dir);
    ctx.data_mut(|d| d.insert_temp(key, (now, size)));
    size
}
