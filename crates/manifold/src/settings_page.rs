//! The Settings page: its sections, the requests they collect, and the
//! deferred effects and popups that run after the scroll area.

use super::*;

/// Space above the first and below the last section of the settings page.
const PAGE_PAD: f32 = 12.0;
/// Space between a section's stacked fields.
const FIELD_GAP: f32 = 8.0;

/// The start of a settings section: a gap, a rule, then the section's heading.
fn section_start(ui: &mut egui::Ui, title: &str) {
    ui.add_space(10.0);
    ui.separator();
    ui.label(heading_text(title));
}

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
    pub(crate) welcome_tour_requested: bool,
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
    pub(crate) default_view_button_rect: Option<egui::Rect>,
    pub(crate) current_view_button_rect: Option<egui::Rect>,
}

/// The Home page field's id, so tests (and focus requests) can name it.
pub(crate) const HOME_PAGE_FIELD: &str = "settings_home_page";

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
        let typing = crate::shortcuts::text_field_has_keys(ui.ctx());

        // Escape in a field is that field's: it reverts the edit (CEF dir, jar name, Home page).
        if ui.input(|i| i.key_pressed(egui::Key::Escape))
            && !typing
            && !self.popup_open()
            && self.view_ui.pending_view_confirm.is_none()
            && self.pending_jar_confirm.is_none()
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
                ui.set_max_width(ui.available_width().min(480.0)); // never wider than the window
                control_font(ui);
                ui.add_space(PAGE_PAD);

                self.settings_general_section(ui, &mut req);
                self.settings_current_tab_section(ui, &mut req);
                self.settings_view_kinds_section(ui);
                self.settings_lists_section(ui, &mut req);
                self.settings_sessions_section(ui, &mut req);
                self.settings_bookmarks_section(ui, &mut req);
                self.settings_help_section(ui, &mut req);

                ui.add_space(PAGE_PAD);
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
                for view_kind in SearchEngine::ALL {
                    let selected = self.settings.default_search_engine == view_kind;
                    if selectable_label(ui, selected, view_kind.label()).clicked() && !selected {
                        self.settings.default_search_engine = view_kind;
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

        ui.add_space(FIELD_GAP);
        ui.label("Default starting engine (new tabs)");
        let default_view_response =
            self.view_opener_button(ui, AnchoredPopup::DefaultViewKind, self.settings.default_starting_view_kind);
        req.default_view_button_rect = Some(default_view_response.rect);
        // A saved default this platform cannot run is kept, not replaced; say why new tabs will fail.
        let default_state = self.view_status(self.settings.default_starting_view_kind);
        if matches!(default_state, crate::view_install::ViewStatus::Unavailable(_)) {
            view_status_line(ui, &default_state);
        }

        ui.add_space(FIELD_GAP);
        ui.label("Zoom (new tabs; also applies to the current tab now)");
        ui.horizontal(|ui| {
            // Reads/writes the *focused tab's own* zoom, not the raw
            // `Settings::zoom_factor` -- see `set_focused_zoom`'s own
            // doc comment for the display-vs-reality bug this fixed.
            if ui.button(egui_phosphor::light::MINUS).named_tip("Zoom out").clicked() {
                self.set_focused_zoom((self.focused_zoom_factor() - 0.1).max(*crate::settings::ZOOM_RANGE.start()));
            }
            ui.label(format!("{:.0}%", self.focused_zoom_factor() * 100.0));
            if ui.button(egui_phosphor::light::PLUS).named_tip("Zoom in").clicked() {
                self.set_focused_zoom((self.focused_zoom_factor() + 0.1).min(*crate::settings::ZOOM_RANGE.end()));
            }
            if ui.button("Reset").clicked() {
                self.set_focused_zoom(1.0);
            }
        });

        ui.add_space(FIELD_GAP);
        ui.label("Home page (new tabs)");
        let saved = self.settings.home_page.clone();
        let draft = self.home_page_draft.get_or_insert_with(|| saved.clone());
        let response =
            ui.add(egui::TextEdit::singleline(draft).id(egui::Id::new(HOME_PAGE_FIELD)).desired_width(f32::INFINITY));
        if response.lost_focus() {
            // Escape gives up the edit, as the CEF folder row does. Unchanged text isn't saved: `lost_focus` stays
            // true for a second frame.
            let draft = self.home_page_draft.take();
            if let Some(text) = draft.filter(|text| *text != saved && !ui.input(|i| i.key_pressed(egui::Key::Escape))) {
                self.settings.home_page = text;
                self.save_settings();
            }
        } else if !ui.memory(|m| m.has_focus(response.id)) {
            // Track outside edits (a settings reload). Not `response.has_focus()`: that is also false while the OS
            // window is unfocused, yet the field still takes typed text, so the draft would be dropped each frame.
            self.home_page_draft = None;
        }

        ui.add_space(FIELD_GAP);
        let mut minutes = self.settings.idle_sleep_minutes;
        setting_row(
            ui,
            "Sleep background tabs",
            Some(
                "Applies to open tabs. Sleeping closes the page; waking reloads it. \
                 Pinned tabs and tabs playing audio stay awake.",
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
        section_start(ui, "Current Tab");
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
                    let current_view_response = self.view_opener_button(ui, AnchoredPopup::CurrentViewKind, kind);
                    req.current_view_button_rect = Some(current_view_response.rect);
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
    /// (asks first) with the installed size for downloaded files.
    pub(crate) fn settings_view_kinds_section(&mut self, ui: &mut egui::Ui) {
        use crate::view_install::ViewStatus;
        section_start(ui, "Engines");
        for kind in VIEW_PICKER_ORDER {
            let state = self.view_status(kind);
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                view_dot(ui, kind, 10.0);
                let unavailable = matches!(state, ViewStatus::Unavailable(_));
                ui.label(egui::RichText::new(view_label(kind)).color(if unavailable {
                    TEXT_WEAK
                } else {
                    TEXT_PRIMARY
                }));
            });
            // What this engine has installed (host files, CEF's runtime): present means Remove and a size line.
            let runtime = cached(ui.ctx(), ("view_runtime", kind), DISK_CACHE_AGE, || {
                Some(crate::view_install::installed_paths(kind)).filter(|paths| !paths.is_empty())
            });
            match &state {
                ViewStatus::Installed | ViewStatus::UpdateNeeded { .. } if runtime.is_some() => {
                    let size = runtime
                        .as_deref()
                        .map(|paths| paths.iter().map(|path| cached_dir_size(ui.ctx(), path)).sum::<u64>())
                        .unwrap_or(0);
                    let (label, tone) = view_status_label(&state);
                    ui.label(
                        egui::RichText::new(format!("{label} \u{b7} {} on disk", megabytes(size)))
                            .small()
                            .color(tone.color()),
                    );
                }
                _ => view_status_line(ui, &state),
            }
            // Leftovers (a kept `.part`, a failed install's `.tmp`) can exist with no runtime; they get a
            // button too, since nothing else would ever delete them.
            let leftovers = cached(ui.ctx(), ("view_leftovers", kind), DISK_CACHE_AGE, || {
                crate::view_install::kind_has_leftovers(kind)
            });
            let removable = (runtime.is_some() || leftovers) && !matches!(state, ViewStatus::Installing { .. });
            if matches!(state, ViewStatus::Installed | ViewStatus::Custom(_)) && !removable {
                continue;
            }
            ui.horizontal(|ui| {
                match &state {
                    ViewStatus::NotInstalled { .. } if ui.button("Download").clicked() => {
                        self.offer_view_confirm(ViewConfirm::Download(kind));
                    }
                    ViewStatus::UpdateNeeded { .. } if ui.button("Update").clicked() => {
                        self.offer_view_confirm(ViewConfirm::Download(kind));
                    }
                    ViewStatus::Failed(_) if ui.button("Retry").clicked() => {
                        self.offer_view_confirm(ViewConfirm::Download(kind));
                    }
                    ViewStatus::Installing { .. } if ui.button("Cancel").clicked() => self.cancel_view_install(kind),
                    _ => {}
                }
                let (label, hint) = if runtime.is_some() {
                    ("Remove", "Delete the downloaded files")
                } else {
                    ("Clear leftovers", "Delete the partial download and install leftovers")
                };
                if removable && ui.button(label).on_hover_text(hint).clicked() {
                    self.offer_view_confirm(ViewConfirm::Remove(kind));
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
            let draft = self.view_ui.cef_dir_draft.get_or_insert_with(|| configured.clone());
            let response = ui.add(
                egui::TextEdit::singleline(draft)
                    .desired_width(220.0)
                    .hint_text(format!("Folder with {}", crate::cef_platform::PLATFORM.runtime_lib)),
            );
            if response.lost_focus() {
                // Escape gives up the edit instead of committing it.
                commit = Some(if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    configured.clone()
                } else {
                    draft.clone()
                });
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
            } else {
                // Same folder (Browse again, or Enter): its contents may have been fixed on disk.
                self.invalidate_cef_dir_check();
            }
            self.view_ui.cef_dir_draft = None;
        }
        match self.cef_dir_verdict() {
            Some((_, Err(message))) => {
                ui.add(egui::Label::new(egui::RichText::new(message).small().color(DANGER_COLOR)).wrap());
            }
            Some((dir, Ok(()))) => {
                let note = crate::cef_pin::pin_for_platform().and_then(|pin| {
                    cached(ui.ctx(), ("cef_newer_than_tested", &dir), DISK_CACHE_AGE, || {
                        crate::view_install::newer_than_tested(&dir, pin)
                    })
                });
                if let Some(note) = note {
                    ui.add(egui::Label::new(egui::RichText::new(note).small().weak()).wrap());
                }
            }
            None => {}
        }
    }

    /// Settings, "Lists": the bookmarks/history/downloads buttons.
    pub(crate) fn settings_lists_section(&mut self, ui: &mut egui::Ui, req: &mut SettingsRequests) {
        section_start(ui, "Lists");
        ui.horizontal(|ui| {
            let bookmarks_response = ui.button(format!("Bookmarks ({})", self.bookmarks.len()));
            if self.popup_opener_clicked(ui, AnchoredPopup::Bookmarks, &bookmarks_response) {
                self.history_ui.bookmarks_filter.clear();
            }
            req.bookmarks_button_rect = Some(bookmarks_response.rect);
            let history_response = ui.button(format!("History ({})", self.history.len()));
            if self.popup_opener_clicked(ui, AnchoredPopup::History, &history_response) {
                self.history_ui.history_filter.clear();
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
        section_start(ui, "Sessions");
        if ui.button("Save Now").on_hover_text("Write the auto-restored session (session.mnf) now").clicked() {
            req.save_now_requested = true;
        }
        let last_saved = match self.writes.last_session_save {
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
        let names = cached(ui.ctx(), "named_sessions", DISK_CACHE_AGE, session::list_named);
        if names.is_empty() {
            ui.add(egui::Label::new(egui::RichText::new("No saved sessions").small().weak()).wrap());
        }
        for name in names {
            // Cut to `ROW_UNITS`, so a long name cannot widen the page; the hover has all of it.
            let segs = crate::names::display_name(&name, crate::names::UNTITLED);
            let shown = crate::names::shorten(&segs, crate::names::ROW_UNITS);
            let hover = crate::names::name_hover(&name, crate::names::ROW_UNITS, "Open", |f| format!("Open ({f})"));
            ui.horizontal(|ui| {
                let text = crate::names::name_job(ui, "", shown.as_deref().unwrap_or(&segs), "");
                if ui.button(text).on_hover_text(hover).clicked() {
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
        section_start(ui, "Bookmarks");
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

    /// Settings, "Help": the welcome tour again, for anyone who missed it on first launch.
    pub(crate) fn settings_help_section(&mut self, ui: &mut egui::Ui, req: &mut SettingsRequests) {
        section_start(ui, "Help");
        if ui.button("Welcome tour").on_hover_text("Open the first-launch tour in a new tab").clicked() {
            req.welcome_tour_requested = true;
        }
    }

    /// What the Settings sections asked for, acted on once the page's
    /// scroll area is done (`build_settings_page`).
    pub(crate) fn apply_settings_requests(&mut self, req: &SettingsRequests) {
        if req.welcome_tour_requested {
            self.close_settings_page();
            let group = self.current_group.clone();
            self.spawn_new_tab(None, group, "manifold://welcome");
        }
        if req.save_now_requested {
            self.save_session();
        }
        if req.save_as_requested {
            let draft = self.session_name_draft.clone();
            self.save_named_session(&draft);
        }
        if let Some(name) = &req.open_session_name {
            self.load_named_session(name);
            self.close_settings_page();
        }
        if let Some(name) = &req.delete_session_name {
            self.offer_session_delete(name.clone());
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
            self.debug.pending_screenshot = Some(path.to_string_lossy().to_string());
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
        let default_view_kind = Some(self.settings.default_starting_view_kind);
        if let Some(kind) = self.build_anchored_view_picker(
            ui,
            AnchoredPopup::DefaultViewKind,
            req.default_view_button_rect,
            default_view_kind,
        ) {
            self.settings.default_starting_view_kind = kind;
            self.save_settings();
        }
        let current_kind =
            req.current_tab_id.as_ref().and_then(|id| self.nodes.get(id)).and_then(|n| n.as_tab()).map(|t| t.tab.kind);
        if let Some(kind) = self.build_anchored_view_picker(
            ui,
            AnchoredPopup::CurrentViewKind,
            req.current_view_button_rect,
            current_kind,
        ) {
            if let Some(id) = &req.current_tab_id {
                if self.swap_in_flight(id).is_some() {
                    // A swap, or a cookie hold ahead of one (up to 2 s), is on its way: this pick would be dropped.
                    self.queue_toast("An engine change is already in progress for this tab".to_owned());
                } else if Some(kind) != current_kind {
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
            self.end_note_edits();
            self.invalidate_cef_dir_check();
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
    cached(ctx, ("view_dir_size", dir), DISK_CACHE_AGE, || crate::view_install::dir_size(dir))
}

/// How long the page's disk reads (`cached`) are reused before they are read again.
const DISK_CACHE_AGE: std::time::Duration = std::time::Duration::from_secs(3);

/// The key all disk-read caches on this page share a namespace under, so
/// `forget_disk_cache` clears them together.
const DISK_CACHE: &str = "settings_disk_cache";

/// `compute()`, recomputed at most every `max_age` (egui temp memory): the
/// page redraws every frame, and these are disk reads.
fn cached<T: Clone + Send + Sync + 'static>(
    ctx: &egui::Context,
    key: impl std::hash::Hash + std::fmt::Debug, // egui 0.36's `Id::new` wants `Debug` too
    max_age: std::time::Duration,
    compute: impl FnOnce() -> T,
) -> T {
    let id = egui::Id::new((DISK_CACHE, key));
    let generation = ctx.data(|d| d.get_temp::<u64>(egui::Id::new(DISK_CACHE)).unwrap_or(0));
    let now = std::time::Instant::now();
    if let Some((at, seen, value)) = ctx.data(|d| d.get_temp::<(std::time::Instant, u64, T)>(id)) {
        if seen == generation && now.duration_since(at) < max_age {
            return value;
        }
    }
    let value = compute();
    ctx.data_mut(|d| d.insert_temp(id, (now, generation, value.clone())));
    value
}

/// Makes every `cached` value recompute on its next read: after a change
/// the page asked for (Save As, a session deleted, an engine removed or its
/// leftovers cleared, an install finished). A generation counter, not
/// removal: egui's temp map can't remove by key prefix.
pub(crate) fn forget_disk_cache(ctx: &egui::Context) {
    ctx.data_mut(|d| {
        let id = egui::Id::new(DISK_CACHE);
        let next = d.get_temp::<u64>(id).unwrap_or(0) + 1;
        d.insert_temp(id, next);
    });
}

/// `profile::pictures_dir()`\manifold-<unix-seconds>.png, or the temp folder without one — same
/// "pick the obvious default, no save-as dialog" posture `downloads_dir()`
/// (in each view-host's own download-handling code) already takes, just
/// on the manifold side for the toolbar's "Screenshot" button. The
/// timestamp is the simplest collision-free name that needs no extra
/// state; a real "Save As" dialog would be the alternative, but this
/// project doesn't build custom save UI anywhere else either.
fn screenshot_default_path() -> std::path::PathBuf {
    let dir = profile::pictures_dir().unwrap_or_else(std::env::temp_dir);
    let _ = std::fs::create_dir_all(&dir);
    let timestamp =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    dir.join(format!("manifold-{timestamp}.png"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cached_disk_read_runs_once_until_forgotten() {
        let ctx = egui::Context::default();
        let calls = std::cell::Cell::new(0);
        let read = || {
            calls.set(calls.get() + 1);
            vec!["a".to_string()]
        };
        let hour = std::time::Duration::from_secs(3600);
        assert_eq!(cached(&ctx, "names", hour, read), vec!["a".to_string()]);
        assert_eq!(cached(&ctx, "names", hour, read), vec!["a".to_string()]);
        assert_eq!(calls.get(), 1);
        forget_disk_cache(&ctx);
        cached(&ctx, "names", hour, read);
        assert_eq!(calls.get(), 2);
    }

    /// A CEF folder change (`invalidate_cef_dir_check`) and Save As, from Settings or headless, forget the
    /// page's disk reads.
    #[test]
    fn a_cef_folder_change_or_save_as_forgets_the_disk_reads() {
        let mut app = App::default();
        let calls = std::cell::Cell::new(0);
        let read = || calls.set(calls.get() + 1);
        let hour = std::time::Duration::from_secs(3600);
        cached(&app.egui_ctx, "x", hour, read);
        app.set_cef_directory("");
        cached(&app.egui_ctx, "x", hour, read);
        assert_eq!(calls.get(), 2, "the folder change forgot it");
        app.save_named_session("Trip");
        cached(&app.egui_ctx, "x", hour, read);
        assert_eq!(calls.get(), 3, "Save As forgot it");
    }

    /// Review C-B5: Escape in a Settings field reverts that field and keeps the page open.
    #[test]
    fn escape_in_a_settings_field_reverts_it_and_stays_on_the_page() {
        let ctx = egui::Context::default();
        crate::theme::apply_fonts(&ctx);
        crate::theme::apply_style(&ctx);
        let mut app = App::default();
        app.settings.home_page = "https://old".into();
        app.open_settings_page();
        let input = |events| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 3000.0))),
            events,
            ..Default::default()
        };
        let key = |key| egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let frame = |app: &mut App, events| {
            ctx.run_ui(input(events), |ui| app.build_settings_page(ui)).textures_delta.clear();
        };
        ctx.memory_mut(|m| m.request_focus(HOME_PAGE_FIELD.into()));
        for _ in 0..2 {
            frame(&mut app, vec![]);
        }
        frame(&mut app, vec![egui::Event::Text("x".into())]);
        assert_eq!(app.settings.home_page, "https://old", "typing edits the draft only");
        frame(&mut app, vec![key(egui::Key::Escape)]);
        assert_eq!(app.view_mode, ViewMode::Settings, "still on Settings");
        assert_eq!(app.settings.home_page, "https://old", "Escape reverted the field");
        frame(&mut app, vec![]);
        assert_eq!(app.settings.home_page, "https://old", "and stays reverted");

        // Enter still commits, and with no field focused Escape leaves the page.
        ctx.memory_mut(|m| m.request_focus(HOME_PAGE_FIELD.into()));
        for _ in 0..2 {
            frame(&mut app, vec![]);
        }
        frame(&mut app, vec![egui::Event::Text("y".into())]);
        frame(&mut app, vec![key(egui::Key::Enter)]);
        assert!(app.settings.home_page.contains('y'), "Enter committed: {}", app.settings.home_page);
        frame(&mut app, vec![]);
        frame(&mut app, vec![key(egui::Key::Escape)]);
        assert_ne!(app.view_mode, ViewMode::Settings, "Escape outside a field closes Settings");
    }

    /// Fix batch B1, live check: the whole page in a 480 px window keeps every text inside it. The page's
    /// `set_max_width(480)` widened its column past the window when the window was under 480 plus margins, so the
    /// jar rows' right-aligned tab counts ran off the edge.
    #[test]
    fn the_settings_page_keeps_its_text_inside_a_480_px_window() {
        let ctx = egui::Context::default();
        crate::theme::apply_fonts(&ctx);
        crate::theme::apply_style(&ctx);
        let mut app = App::default();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(480.0, 3000.0))),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input(), |ui| app.build_settings_page(ui));
        output.textures_delta.clear();
        let mut output = ctx.run_ui(input(), |ui| app.build_settings_page(ui));
        output.textures_delta.clear();
        let mut texts = 0;
        for shape in &output.shapes {
            if let egui::Shape::Text(text) = &shape.shape {
                if text.galley.text().trim().is_empty() {
                    continue;
                }
                texts += 1;
                let rect = text.visual_bounding_rect();
                assert!(rect.right() <= 480.0 - 16.0, "{:?} ends at {}", text.galley.text(), rect.right());
            }
        }
        assert!(texts > 20, "the page drew its text ({texts})");
    }
}
