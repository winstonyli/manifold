//! The Settings page: its sections, the requests they collect, and the
//! deferred effects and popups that run after the scroll area.

use super::*;
use crate::jar::JarKey;
use crate::jar_defs::{JarConfirm, JarLabel};
use crate::store::model::NodeId;

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
    /// `settings_jars_section`: a jar's name field committed a new name.
    pub(crate) rename_jar: Option<(NodeId, String)>,
    /// The Ephemeral checkbox changed; on, for a jar with a saved file, asks first (`App::request_jar_ephemeral`).
    pub(crate) jar_ephemeral: Option<(NodeId, bool)>,
    /// "Clear Cookies" or "Delete": asks first (`build_jar_confirm_popup`).
    pub(crate) jar_confirm: Option<JarConfirm>,
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
            && self.pending_engine_confirm.is_none()
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
                ui.add_space(12.0);

                self.settings_general_section(ui, &mut req);
                self.settings_current_tab_section(ui, &mut req);
                self.settings_engines_section(ui);
                self.settings_lists_section(ui, &mut req);
                self.settings_sessions_section(ui, &mut req);
                self.settings_jars_section(ui, &mut req);
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
        // A saved default this platform cannot run is kept, not replaced; say why new tabs will fail.
        let default_state = self.engine_state(self.settings.default_starting_engine);
        if matches!(default_state, crate::engine_install::EngineState::Unavailable(_)) {
            engine_state_line(ui, &default_state);
        }

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

        ui.add_space(8.0);
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
                let unavailable = matches!(state, EngineState::Unavailable(_));
                ui.label(egui::RichText::new(engine_label(kind)).color(if unavailable {
                    TEXT_WEAK
                } else {
                    TEXT_PRIMARY
                }));
            });
            let runtime = cached(ui.ctx(), ("engine_runtime", kind), DISK_CACHE_AGE, || {
                crate::process_manager::engine_runtime_dir(kind).filter(|dir| dir.exists())
            });
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
            // Leftovers (a kept `.part`, a failed install's `.tmp`) can exist with no runtime; they get a
            // button too, since nothing else would ever delete them.
            let leftovers = kind == EngineKind::Cef
                && cached(ui.ctx(), "engine_leftovers", DISK_CACHE_AGE, || {
                    crate::process_manager::engine_hosts_dir().is_some_and(|d| crate::engine_install::has_leftovers(&d))
                });
            let removable = (runtime.is_some() || leftovers) && !matches!(state, EngineState::Installing { .. });
            if matches!(state, EngineState::Installed | EngineState::Custom(_)) && !removable {
                continue;
            }
            ui.horizontal(|ui| {
                match &state {
                    EngineState::NotInstalled { .. } if ui.button("Download").clicked() => {
                        self.offer_engine_confirm(EngineConfirm::Download(kind));
                    }
                    EngineState::UpdateNeeded { .. } if ui.button("Update").clicked() => {
                        self.offer_engine_confirm(EngineConfirm::Download(kind));
                    }
                    EngineState::Failed(_) if ui.button("Retry").clicked() => {
                        self.offer_engine_confirm(EngineConfirm::Download(kind));
                    }
                    EngineState::Installing { .. } if ui.button("Cancel").clicked() => self.cancel_engine_install(kind),
                    _ => {}
                }
                let (label, hint) = if runtime.is_some() {
                    ("Remove", "Delete the downloaded runtime")
                } else {
                    ("Clear leftovers", "Delete the partial download and install leftovers")
                };
                if removable && ui.button(label).on_hover_text(hint).clicked() {
                    self.offer_engine_confirm(EngineConfirm::Remove(kind));
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
            self.cef_dir_draft = None;
        }
        match self.cef_dir_verdict() {
            Some((_, Err(message))) => {
                ui.add(egui::Label::new(egui::RichText::new(message).small().color(DANGER_COLOR)).wrap());
            }
            Some((dir, Ok(()))) => {
                let note = crate::cef_pin::pin_for_platform().and_then(|pin| {
                    cached(ui.ctx(), ("cef_newer_than_tested", &dir), DISK_CACHE_AGE, || {
                        crate::engine_install::newer_than_tested(&dir, pin)
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

    /// Settings, "Jars" (spec "UI"): the default jar first (it can only be cleared), then one row per named jar with
    /// its swatch, name, tab count, ephemeral toggle, Clear Cookies and Delete. Clearing and deleting ask first.
    pub(crate) fn settings_jars_section(&mut self, ui: &mut egui::Ui, req: &mut SettingsRequests) {
        ui.add_space(10.0);
        ui.separator();
        ui.label(heading_text("Jars"));
        // Each jar takes two lines, so a narrow window (480 px) clips nothing: its swatch, name and tab count (counts
        // aligned on the right, the name field taking the rest), then its actions under the name, wrapping if need be.
        // A jar whose name another jar shares shows its marker (`JarLabel::marker_text`, "#1a2b", in the marker style)
        // between the name field and the count: read-only text of its own, so it is never part of the name the field
        // edits and saves. The built-in jar shows its styled label (italic "Default", then "built-in").
        let count_text = |n: usize| egui::RichText::new(tab_count_label(n)).small().weak();
        let head =
            |ui: &mut egui::Ui, color, count: usize, marker: Option<String>, name: &mut dyn FnMut(&mut egui::Ui)| {
                ui.horizontal(|ui| {
                    jar_swatch(ui, color);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(count_text(count));
                        if let Some(marker) = marker {
                            let size = egui::FontSelection::Default.resolve(ui.style()).size;
                            let format = JarLabel::marker_format(size, ui.visuals().weak_text_color());
                            ui.label(egui::text::LayoutJob::single_section(marker, format))
                                .on_hover_text("Another jar has this name; this marks which one it is");
                        }
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| name(ui));
                    });
                });
            };
        let actions = |ui: &mut egui::Ui, add: &mut dyn FnMut(&mut egui::Ui)| {
            ui.horizontal_wrapped(|ui| {
                ui.add_space(JAR_SWATCH_WIDTH + ui.spacing().item_spacing.x);
                add(ui);
            });
            ui.add_space(4.0);
        };
        head(ui, egui::Color32::GRAY, self.jar_tab_count(JarKey::Default), None, &mut |ui| {
            // The built-in jar's label, styled as in the menus (`JarLabel::append_to`).
            let mut job = egui::text::LayoutJob::default();
            let font = egui::FontSelection::Default.resolve(ui.style());
            JarLabel::built_in().append_to(&mut job, font, ui.visuals().weak_text_color());
            ui.label(job);
        });
        actions(ui, &mut |ui| {
            if ui.button("Clear Cookies").clicked() {
                req.jar_confirm = Some(JarConfirm::Clear(JarKey::Default));
            }
        });
        for (&id, def) in self.mirror.jar_defs() {
            let marker = self.jar_label(JarKey::Named(id)).marker_text();
            head(ui, self.jar_color(JarKey::Named(id)), self.jar_tab_count(JarKey::Named(id)), marker, &mut |ui| {
                let mut draft = jar_name_draft(ui, id, &def.name); // name rule: the field edits the stored name
                let key = egui::Id::new(("jar_name", id));
                let edit =
                    egui::TextEdit::singleline(&mut draft).id(jar_name_edit_id(id)).desired_width(ui.available_width());
                let response = ui.add(edit);
                let hint = crate::names::hidden_hint(&crate::names::display_name(&def.name, crate::names::UNTITLED));
                let response = match hint {
                    Some(hint) => response.on_hover_text(hint),
                    None => response,
                };
                if response.lost_focus() {
                    ui.data_mut(|d| d.remove::<String>(key));
                    let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
                    if !escape && draft != def.name && *crate::names::clean_name(&draft) != *def.name {
                        req.rename_jar = Some((id, draft));
                    }
                } else if ui.memory(|m| m.has_focus(response.id)) {
                    // Not `response.has_focus()`: that is also false while the OS window is unfocused, yet the field
                    // still takes typed text (`TextEdit` and `jar_name_draft` ask egui memory alone), so the draft
                    // would be dropped each frame.
                    ui.data_mut(|d| d.insert_temp(key, draft));
                }
            });
            actions(ui, &mut |ui| {
                let mut ephemeral = def.ephemeral;
                if ui
                    .checkbox(&mut ephemeral, "Ephemeral")
                    .on_hover_text("Never saved to disk: its cookies are forgotten when Manifold quits")
                    .changed()
                {
                    req.jar_ephemeral = Some((id, ephemeral));
                }
                if ui.button("Clear Cookies").clicked() {
                    req.jar_confirm = Some(JarConfirm::Clear(JarKey::Named(id)));
                }
                if ui.button("Delete").clicked() {
                    req.jar_confirm = Some(JarConfirm::Delete(id));
                }
            });
        }
        if self.mirror.jar_defs().is_empty() {
            let hint = "Make a jar from a tab's right-click menu (New Jar...) to keep a second set of sign-ins.";
            ui.add(egui::Label::new(egui::RichText::new(hint).small().weak()).wrap());
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
        if let Some((id, name)) = &req.rename_jar {
            self.rename_jar(*id, name);
        }
        // A jar action clicked while a confirm dialog is up (they are non-modal) is ignored, so it neither replaces
        // the question being asked nor acts behind it (`offer_confirm`).
        let jar_ephemeral = req.jar_ephemeral.filter(|_| !self.confirm_open());
        if let Some((id, on)) = jar_ephemeral {
            self.request_jar_ephemeral(id, on); // may ask first instead (`JarConfirm::Ephemeral`)
        }
        if let Some(confirm) = req.jar_confirm {
            self.offer_jar_confirm(confirm);
        }
        if req.rename_jar.is_some() || jar_ephemeral.is_some() {
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

/// A Settings jar swatch's width; its actions line is indented by this much plus one item gap, under the name.
const JAR_SWATCH_WIDTH: f32 = 12.0;

/// A jar's colour dot in the Settings rows.
fn jar_swatch(ui: &mut egui::Ui, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(JAR_SWATCH_WIDTH), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 5.0, color);
}

/// A Settings jar row's tab count: "1 tab", "2 tabs".
fn tab_count_label(n: usize) -> String {
    format!("{n} tab{}", if n == 1 { "" } else { "s" })
}

/// A Settings jar-name field's widget id.
fn jar_name_edit_id(id: NodeId) -> egui::Id {
    egui::Id::new(("jar_name_edit", id))
}

/// What a Settings jar-name field shows. The draft lives in egui memory while the field has focus, so typing writes
/// nothing until it is committed on focus loss (Escape gives the edit up); it is read in the frame that reports the
/// loss too, which is when it is committed (egui may report it a frame late, after focus moved between frames, so
/// this asks the field's last response rather than `had_focus_last_frame`). Otherwise the jar's name: a draft left
/// by a field that went away while focused (the page closed mid-edit) is dropped, not shown when the page reopens.
fn jar_name_draft(ui: &egui::Ui, id: NodeId, name: &str) -> String {
    let key = egui::Id::new(("jar_name", id));
    let edit = jar_name_edit_id(id);
    if ui.memory(|m| m.has_focus(edit)) || ui.ctx().read_response(edit).is_some_and(|r| r.lost_focus()) {
        if let Some(draft) = ui.data(|d| d.get_temp::<String>(key)) {
            return draft;
        }
    } else {
        ui.data_mut(|d| d.remove::<String>(key));
    }
    name.to_string()
}

/// `dir`'s size, recomputed at most every few seconds (egui memory) so the Settings page doesn't walk the
/// runtime's files on every frame.
fn cached_dir_size(ctx: &egui::Context, dir: &std::path::Path) -> u64 {
    cached(ctx, ("engine_dir_size", dir), DISK_CACHE_AGE, || crate::engine_install::dir_size(dir))
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

    fn jars_frame(app: &mut App, ctx: &egui::Context) -> SettingsRequests {
        jars_frame_with(app, ctx, egui::RawInput::default())
    }

    /// The text the Jars section draws in one frame, as galleys (their `job` keeps each section's format).
    fn galleys(ctx: &egui::Context, app: &mut App) -> Vec<std::sync::Arc<egui::Galley>> {
        let mut req = SettingsRequests::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| app.settings_jars_section(ui, &mut req));
        output.textures_delta.clear();
        let text = output.shapes.into_iter().filter_map(|s| match s.shape {
            egui::Shape::Text(t) => Some(t.galley),
            _ => None,
        });
        text.collect()
    }

    /// Styled jar label markers: the built-in jar's row shows its label as menus do, an italic "Default" and a
    /// `built-in` marker, while a user jar named "Default" is a plain name in its field.
    #[test]
    fn the_built_in_jar_row_is_marked_apart_from_a_user_default() {
        let ctx = egui::Context::default();
        crate::theme::apply_fonts(&ctx);
        crate::theme::apply_style(&ctx);
        let mut app = App::default();
        app.create_jar("Default");
        galleys(&ctx, &mut app);
        let shown = galleys(&ctx, &mut app);
        let weak = ctx.global_style().visuals.weak_text_color();
        let mut want = egui::text::LayoutJob::default();
        let body = egui::FontSelection::Default.resolve(&ctx.global_style());
        crate::jar_defs::JarLabel::built_in().append_to(&mut want, body, weak);
        let built_in: Vec<_> = shown.iter().filter(|g| g.text() == "Default built-in").collect();
        assert_eq!(built_in.len(), 1, "{:?}", shown.iter().map(|g| g.text()).collect::<Vec<_>>());
        assert_eq!(built_in[0].job.sections, want.sections);
        let user: Vec<_> = shown.iter().filter(|g| g.text() == "Default").collect();
        assert_eq!(user.len(), 1, "the user jar's field");
        assert!(user[0]
            .job
            .sections
            .iter()
            .all(|s| !s.format.italics && s.format.font_id.family != egui::FontFamily::Monospace));
    }

    fn jars_frame_with(app: &mut App, ctx: &egui::Context, input: egui::RawInput) -> SettingsRequests {
        let mut req = SettingsRequests::default();
        let mut output = ctx.run_ui(input, |ui| app.settings_jars_section(ui, &mut req));
        output.textures_delta.clear(); // nothing uploads the font atlas here
        req
    }

    /// Fix batch A1: typing into a jar's name field keeps the draft and commits it on focus loss whether or not the OS
    /// window has focus. `Response::has_focus` is also false while the window is unfocused (`RawInput::focused`), but
    /// the `TextEdit` still takes the typed text, so a draft kept only under that check was dropped every frame.
    #[test]
    fn a_typed_jar_name_survives_with_the_window_focused_or_not() {
        for window_focused in [true, false] {
            let ctx = egui::Context::default();
            crate::theme::apply_fonts(&ctx);
            crate::theme::apply_style(&ctx);
            let mut app = App::default();
            let work = app.create_jar("Work");
            let input =
                |events: Vec<egui::Event>| egui::RawInput { focused: window_focused, events, ..Default::default() };
            ctx.memory_mut(|m| m.request_focus(jar_name_edit_id(work)));
            jars_frame_with(&mut app, &ctx, input(vec![]));
            jars_frame_with(&mut app, &ctx, input(vec![egui::Event::Text("Home".into())]));
            jars_frame_with(&mut app, &ctx, input(vec![]));
            let key = egui::Id::new(("jar_name", work));
            let draft = ctx.data(|d| d.get_temp::<String>(key));
            assert!(draft.as_deref().is_some_and(|d| d.contains("Home")), "window focused {window_focused}: {draft:?}");
            ctx.memory_mut(|m| m.surrender_focus(jar_name_edit_id(work)));
            let req = jars_frame_with(&mut app, &ctx, input(vec![]));
            assert_eq!(req.rename_jar, Some((work, draft.unwrap())), "window focused {window_focused}: committed");
        }
    }

    /// Fix batch B1: at a 480 px window (the page's 480 max, less its 16 px margins and a scroll bar) no jar row runs
    /// past the page's width; before, a named jar's one row put Delete under the scroll bar.
    #[test]
    fn the_jar_rows_fit_a_480_px_window() {
        const CONTENT: f32 = 480.0 - 2.0 * 16.0 - 8.0;
        let ctx = egui::Context::default();
        crate::theme::apply_fonts(&ctx);
        crate::theme::apply_style(&ctx);
        let mut app = App::default();
        // Two of a name (and a long one) draw their markers too (jar name labels).
        for name in ["Work", "Home", "A much longer jar name than most", "A much longer jar name than most", "Work"] {
            app.create_jar(name);
        }
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(480.0, 800.0))),
            ..Default::default()
        };
        let mut width = 0.0;
        for _ in 0..2 {
            let mut req = SettingsRequests::default();
            let mut output = ctx.run_ui(input(), |ui| {
                ui.set_max_width(CONTENT);
                control_font(ui);
                let left = ui.min_rect().left();
                app.settings_jars_section(ui, &mut req);
                width = ui.min_rect().right() - left;
            });
            output.textures_delta.clear();
        }
        assert!(width <= CONTENT + 0.5, "the Jars section is {width} wide in {CONTENT}");
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
        app.create_jar("Personal accounts");
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

    /// Fix batch B5: while a confirm dialog is up (they are non-modal), a second jar action from Settings neither
    /// replaces it nor acts, and a jar confirm and an engine confirm never show together.
    #[test]
    fn a_pending_confirm_ignores_further_jar_actions() {
        use crate::engine_info::EngineConfirm;
        let mut app = App::default();
        let work = app.create_jar("Work");
        let clear = JarConfirm::Clear(JarKey::Default);
        app.pending_jar_confirm = Some(clear);
        let req = SettingsRequests {
            jar_confirm: Some(JarConfirm::Delete(work)),
            jar_ephemeral: Some((work, true)),
            ..Default::default()
        };
        app.apply_settings_requests(&req);
        assert_eq!(app.pending_jar_confirm, Some(clear), "the first confirm stays");
        assert!(!app.mirror.jar_defs()[&work].ephemeral, "the click was ignored, not acted on");
        let download = EngineConfirm::Download(crate::process_manager::EngineKind::Cef);
        assert!(!app.offer_engine_confirm(download));
        assert_eq!(app.pending_engine_confirm, None, "no engine confirm over a jar confirm");

        app.pending_jar_confirm = None;
        assert!(app.offer_engine_confirm(download));
        app.apply_settings_requests(&req);
        assert_eq!(app.pending_jar_confirm, None, "no jar confirm over an engine confirm");
        assert!(!app.mirror.jar_defs()[&work].ephemeral);

        app.pending_engine_confirm = None;
        app.apply_settings_requests(&SettingsRequests {
            jar_confirm: Some(JarConfirm::Delete(work)),
            ..Default::default()
        });
        assert_eq!(app.pending_jar_confirm, Some(JarConfirm::Delete(work)), "with none up, it opens");
    }

    /// Jar name labels: two same-named jars each show their marker as text of its own beside the name field. The field
    /// holds and saves the stored name only, so the marker can never end up in a name.
    #[test]
    fn a_same_named_jars_marker_sits_beside_a_field_that_edits_only_the_name() {
        let ctx = egui::Context::default();
        crate::theme::apply_fonts(&ctx);
        crate::theme::apply_style(&ctx);
        let mut app = App::default();
        let (w1, w2) = (app.create_jar("Work"), app.create_jar("Work"));
        app.create_jar("Home");
        let texts = |app: &mut App, input: egui::RawInput| {
            let mut req = SettingsRequests::default();
            let mut output = ctx.run_ui(input, |ui| app.settings_jars_section(ui, &mut req));
            output.textures_delta.clear();
            let texts: Vec<String> = output
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                    _ => None,
                })
                .collect();
            (texts, req)
        };
        texts(&mut app, egui::RawInput::default());
        let (shown, _) = texts(&mut app, egui::RawInput::default());
        // Styled jar label markers: the marker is drawn in the marker style, as `JarLabel::append_to` draws it.
        let marker_galleys = galleys(&ctx, &mut app);
        let weak = ctx.global_style().visuals.weak_text_color();
        let body = ctx.global_style().text_styles[&egui::TextStyle::Body].size;
        for w in [w1, w2] {
            let frag = crate::jar_defs::jar_fragment(w, app.mirror.jar_defs()).unwrap();
            let marker = marker_galleys.iter().find(|g| g.text() == format!("#{frag}"));
            let marker = marker.unwrap_or_else(|| panic!("{w}'s marker in {shown:?}"));
            let want = crate::jar_defs::JarLabel::marker_format(body, weak);
            assert!(marker.job.sections.iter().all(|s| s.format == want), "{:?}", marker.job.sections);
        }
        assert_eq!(shown.iter().filter(|t| *t == "Work").count(), 2, "both fields hold the bare name: {shown:?}");
        assert!(shown.iter().any(|t| t == "Home") && !shown.iter().any(|t| t.starts_with("Home ·")), "{shown:?}");

        // Focused and left unchanged: no rename.
        ctx.memory_mut(|m| m.request_focus(jar_name_edit_id(w2)));
        jars_frame(&mut app, &ctx);
        ctx.memory_mut(|m| m.surrender_focus(jar_name_edit_id(w2)));
        assert_eq!(jars_frame(&mut app, &ctx).rename_jar, None);
        // Typed into: the request is the stored name as typed, with no marker in it.
        ctx.memory_mut(|m| m.request_focus(jar_name_edit_id(w2)));
        jars_frame(&mut app, &ctx);
        jars_frame_with(
            &mut app,
            &ctx,
            egui::RawInput { events: vec![egui::Event::Text("X".into())], ..Default::default() },
        );
        jars_frame(&mut app, &ctx);
        ctx.memory_mut(|m| m.surrender_focus(jar_name_edit_id(w2)));
        let (id, name) = jars_frame(&mut app, &ctx).rename_jar.expect("committed");
        assert_eq!(id, w2);
        assert!(name.contains('X') && name.contains("Work") && name.len() == 5, "{name:?}");
        app.apply_settings_requests(&SettingsRequests { rename_jar: Some((id, name.clone())), ..Default::default() });
        assert_eq!(app.mirror.jar_defs()[&w2].name, name, "stored as typed");
        assert_eq!(app.jar_label(JarKey::Named(w1)).to_string(), "Work", "no twin left");
    }

    #[test]
    fn a_jar_row_counts_tabs_in_the_singular_for_one() {
        assert_eq!([tab_count_label(0), tab_count_label(1), tab_count_label(2)], ["0 tabs", "1 tab", "2 tabs"]);
    }

    /// Task 6/7 review: a draft left by a field that lost its page while focused must not come back later, while a
    /// focused field keeps its draft and commits it when it loses focus.
    #[test]
    fn a_jar_name_draft_lives_only_while_its_field_has_focus() {
        let ctx = egui::Context::default();
        crate::theme::apply_fonts(&ctx);
        crate::theme::apply_style(&ctx);
        let mut app = App::default();
        let work = app.create_jar("Work");
        let key = egui::Id::new(("jar_name", work));
        ctx.data_mut(|d| d.insert_temp(key, "Stale".to_string()));
        jars_frame(&mut app, &ctx);
        assert_eq!(ctx.data(|d| d.get_temp::<String>(key)), None, "an orphaned draft is dropped");

        ctx.memory_mut(|m| m.request_focus(jar_name_edit_id(work)));
        jars_frame(&mut app, &ctx);
        ctx.data_mut(|d| d.insert_temp(key, "Home".to_string()));
        jars_frame(&mut app, &ctx);
        assert_eq!(ctx.data(|d| d.get_temp::<String>(key)).as_deref(), Some("Home"), "kept while focused");
        ctx.memory_mut(|m| m.surrender_focus(jar_name_edit_id(work)));
        let req = jars_frame(&mut app, &ctx);
        assert_eq!(req.rename_jar, Some((work, "Home".to_string())), "committed on focus loss");
        assert_eq!(ctx.data(|d| d.get_temp::<String>(key)), None);

        // The page closes while the field has focus: the draft is orphaned and must not show on reopening.
        ctx.memory_mut(|m| m.request_focus(jar_name_edit_id(work)));
        jars_frame(&mut app, &ctx);
        ctx.data_mut(|d| d.insert_temp(key, "Half-typed".to_string()));
        jars_frame(&mut app, &ctx);
        for _ in 0..3 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |_| {});
            output.textures_delta.clear();
        }
        let req = jars_frame(&mut app, &ctx);
        assert_eq!(req.rename_jar, None);
        assert_eq!(ctx.data(|d| d.get_temp::<String>(key)), None, "the closed page's draft is dropped");
    }

    /// Name rules: an edit that cleans back to the stored name requests no rename, and neither does focusing a
    /// synced name that holds a hidden character and leaving it unchanged (stored names are never rewritten).
    #[test]
    fn a_jar_field_requests_no_rename_for_the_same_clean_name() {
        let ctx = egui::Context::default();
        crate::theme::apply_fonts(&ctx);
        crate::theme::apply_style(&ctx);
        let mut app = App::default();
        let work = app.create_jar("Work");
        let synced = app.create_jar("x");
        app.mirror.set(
            &synced.to_string(),
            crate::store::model::field::NAME,
            crate::store::model::FieldValue::Str("Ho\u{200B}me".into()),
        );
        let text = |t: &str| egui::RawInput { events: vec![egui::Event::Text(t.into())], ..Default::default() };
        for (jar, typed) in [(work, Some("\u{200B}")), (synced, None)] {
            ctx.memory_mut(|m| m.request_focus(jar_name_edit_id(jar)));
            jars_frame(&mut app, &ctx);
            if let Some(typed) = typed {
                jars_frame_with(&mut app, &ctx, text(typed));
            }
            jars_frame(&mut app, &ctx);
            let draft = ctx.data(|d| d.get_temp::<String>(egui::Id::new(("jar_name", jar))));
            assert!(
                draft.as_deref().is_some_and(|d| d.contains('\u{200B}')),
                "the draft holds the hidden character: {draft:?}"
            );
            ctx.memory_mut(|m| m.surrender_focus(jar_name_edit_id(jar)));
            assert_eq!(jars_frame(&mut app, &ctx).rename_jar, None, "{typed:?}");
        }
    }

    /// Name rules: a jar's name field shows its stored name as typed, so its hover names any hidden character in it;
    /// a clean name's field has no hover.
    #[test]
    fn a_jar_field_hover_names_a_hidden_character() {
        for (name, want) in [("Wo\u{200B}rk", true), ("Work", false)] {
            let ctx = egui::Context::default();
            crate::theme::apply_fonts(&ctx);
            crate::theme::apply_style(&ctx);
            let mut app = App::default();
            let jar = app.create_jar("x");
            app.mirror.set(
                &jar.to_string(),
                crate::store::model::field::NAME,
                crate::store::model::FieldValue::Str(name.into()),
            );
            jars_frame(&mut app, &ctx);
            let field = ctx.read_response(jar_name_edit_id(jar)).expect("the jar's field").rect.center();
            let mut texts = Vec::new();
            for i in 0..4 {
                // The pointer moves onto the field once, then rests there past the tooltip delay.
                let events = if i == 0 { vec![egui::Event::PointerMoved(field)] } else { Vec::new() };
                let input = egui::RawInput { time: Some(f64::from(i)), events, ..Default::default() };
                let mut req = SettingsRequests::default();
                let mut output = ctx.run_ui(input, |ui| app.settings_jars_section(ui, &mut req));
                output.textures_delta.clear();
                texts.extend(output.shapes.into_iter().filter_map(|s| match s.shape {
                    egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                    _ => None,
                }));
            }
            let hinted = texts.iter().any(|t| t == "Hidden characters: U+200B zero width space");
            assert_eq!(hinted, want, "{name:?}: {texts:?}");
        }
    }
}
