//! The anchored toolbar popups (recently closed, bookmarks, history,
//! downloads, tab actions) and the floating windows: find bar, quick
//! switcher, shortcuts help and import confirm.

use super::*;

/// A Tab actions row: label, the key shown at its edge, and what it does.
type TabActionRow = (&'static str, &'static str, fn(&mut App));

/// Every dropdown/popup (bookmarks, history, downloads, tab actions, tab
/// context menu, group context menu, recently-closed) closes itself on a
/// click landing outside both its own area and, where one exists, the
/// button that opened it -- this was the same `ui.input(|i| ...)` check
/// reimplemented at each of the ~9 call sites. `anchor_rect` is `None` at
/// the sites that never had one (the anchor button already consumes its
/// own click before this runs, so there's nothing left to exclude).
pub(crate) fn clicked_outside_popup(ui: &egui::Ui, area_rect: egui::Rect, anchor_rect: Option<egui::Rect>) -> bool {
    ui.input(|i| {
        i.pointer.any_click()
            && i.pointer
                .interact_pos()
                .is_some_and(|pos| !area_rect.contains(pos) && anchor_rect.is_none_or(|r| !r.contains(pos)))
    })
}

/// The `Area`/`Frame` shell every toolbar list-dropdown (Recently Closed,
/// Bookmarks, History, Downloads) wraps its own contents in -- pinned below
/// `anchor_rect`, same fill/stroke/corner-radius/margin, same min/max width
/// (`f32::INFINITY` for no maximum).
/// A free function (not a method) so callers can still freely capture
/// `&mut self` in `add_contents` -- an `&mut self` method here would
/// conflict with that same borrow. Returns the drawn area's own rect, for
/// the caller's own `clicked_outside_popup` check. `fade` (0.0-1.0, from
/// `App::anchored_popup_fade`) sets the whole popup's opacity, so it fades
/// in/out instead of popping instantly. `interactable` is whether the popup
/// is open: while fading out it is click-through, so the rest of a
/// double-click on an item can't run it twice.
#[allow(clippy::too_many_arguments)] // egui builder: each is a plain knob
pub(crate) fn build_list_popup_frame(
    ctx: &egui::Context,
    id: &str,
    anchor_rect: egui::Rect,
    min_width: f32,
    max_width: f32,
    fade: f32,
    interactable: bool,
    add_contents: impl FnOnce(&mut egui::Ui),
) -> egui::Rect {
    egui::Area::new(egui::Id::new(id))
        .fixed_pos(anchor_rect.left_bottom())
        .order(egui::Order::Foreground)
        .interactable(interactable)
        .show(ctx, |ui| {
            ui.set_opacity(fade);
            popup_frame(ui).show(ui, |ui| {
                menu_item_style(ui);
                ui.set_min_width(min_width);
                // An infinite cap makes egui lay out NaN rects; no cap
                // lets the area size to its contents.
                if max_width.is_finite() {
                    ui.set_max_width(max_width);
                }
                add_contents(ui);
                list_row_nav(ui, interactable);
            });
        })
        .response
        .rect
}

fn list_rows_key(ui: &egui::Ui) -> egui::Id {
    ui.layer_id().id.with("list_rows")
}

/// A clickable text row in a list popup (`build_list_popup_frame`). Keyboard-focusable: a focused
/// row shows `focus_ring`, Enter clicks it, and the popup's Up/Down move between rows.
pub(crate) fn list_row(ui: &mut egui::Ui, text: impl Into<egui::WidgetText>) -> egui::Response {
    // A focused widget gets the "active" text color, which is black in this theme.
    let active_fg = ui.visuals().widgets.active.fg_stroke;
    ui.visuals_mut().widgets.active.fg_stroke = egui::Stroke::new(active_fg.width, TEXT_PRIMARY);
    let response = ui
        .add(egui::Label::new(text).truncate().sense(egui::Sense::click()))
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    ui.visuals_mut().widgets.active.fg_stroke = active_fg;
    if response.has_focus() {
        focus_ring(ui, &response);
        response.scroll_to_me(None);
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                response.id,
                egui::EventFilter { tab: true, vertical_arrows: true, ..Default::default() },
            )
        });
    }
    let key = list_rows_key(ui);
    ui.data_mut(|d| d.get_temp_mut_or_default::<Vec<egui::Id>>(key).push(response.id));
    response
}

/// After a list popup's rows are laid out: focuses the first row when the popup opens, and moves
/// focus between rows on Up/Down (wrapping).
fn list_row_nav(ui: &mut egui::Ui, interactable: bool) {
    let key = list_rows_key(ui);
    let rows: Vec<egui::Id> = ui.data_mut(|d| d.remove_temp(key)).unwrap_or_default();
    let open_key = key.with("was_open");
    let just_opened = interactable && !ui.data(|d| d.get_temp::<bool>(open_key).unwrap_or(false));
    ui.data_mut(|d| d.insert_temp(open_key, interactable));
    if !interactable || rows.is_empty() {
        return;
    }
    if just_opened {
        ui.memory_mut(|m| m.request_focus(rows[0]));
        return;
    }
    let Some(at) = rows.iter().position(|id| ui.memory(|m| m.has_focus(*id))) else { return };
    let (down, up) = ui.input_mut(|i| {
        (
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
        )
    });
    // Down is +1, Up is -1 (as len - 1, mod len).
    let step = usize::from(down) + (rows.len() - 1) * usize::from(up);
    if step != 0 {
        ui.memory_mut(|m| m.request_focus(rows[(at + step) % rows.len()]));
    }
}

/// Lets a popup driven by `Option<T>` app state (the right-click context
/// menus) fade out instead of vanishing the instant that state goes back
/// to `None` -- the position/content those need to keep drawing during a
/// fade-out would otherwise disappear the same frame the menu "closes."
/// `cache` is a sibling field remembering the last real value; `id` keys
/// the fade animation (must be unique per popup). Polled every frame
/// regardless of `current`'s state, same "continuous polling" reasoning
/// the toolbar dropdowns' own fade needs (see their own call sites) --
/// calling `ctx.animate_bool` only while `current` is `Some` would leave
/// real elapsed time untracked while it's `None`, making the next open
/// jump straight to visible instead of easing in. Returns `None` once the
/// fade has actually finished (nothing left to draw), or `Some((value,
/// fade))` otherwise -- `value` is `current`'s own content while it's
/// `Some`, else the frozen `cache`.
pub(crate) fn fade_popup_state<T: Clone>(
    ctx: &egui::Context,
    id: egui::Id,
    current: &Option<T>,
    cache: &mut Option<T>,
) -> Option<(T, f32)> {
    let fade = ctx.animate_bool(id, current.is_some());
    if let Some(value) = current {
        *cache = Some(value.clone());
    }
    if fade <= 0.0 {
        return None;
    }
    cache.clone().map(|value| (value, fade))
}

/// The popups anchored below a toolbar or Settings button. At most one is
/// open (`App::open_popup`), so opening one closes whichever was open,
/// whether a button, a headless click or a shortcut opened it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum AnchoredPopup {
    RecentlyClosed,
    Bookmarks,
    History,
    Downloads,
    TabActions,
    DefaultEngine,
    CurrentEngine,
    SplitEngine,
}

/// Opens `which`, or closes it if it's the one open.
pub(crate) fn toggle_popup(open: &mut Option<AnchoredPopup>, which: AnchoredPopup) {
    *open = if *open == Some(which) { None } else { Some(which) };
}

/// Closes `which` if it's the one open. A popup still fading out runs its
/// own close checks (a click outside it, say), and those mustn't close the
/// popup that replaced it.
pub(crate) fn close_popup(open: &mut Option<AnchoredPopup>, which: AnchoredPopup) {
    if *open == Some(which) {
        *open = None;
    }
}

/// Find-in-page state: tied to the focused tab only, not persisted per-tab,
/// and closed (with `FindStop`) on tab switch or close, the same "not a full
/// session" scope as the address bar's suggestions. See
/// `EngineToHost::FindResult`'s doc comment for why `active_index` is always
/// 0 on a WebKit tab and Servo tabs never populate `result`.
#[derive(Default)]
pub(crate) struct FindState {
    pub(crate) open: bool,
    /// Mirrors the last `FindStart`. Kept across a close, so reopening
    /// searches for it again, as a real browser's find bar does.
    pub(crate) text: String,
    pub(crate) match_case: bool,
    /// `(count, active_index)`; `None` until the first `FindResult` for the
    /// current search arrives, so the bar shows nothing rather than a stale
    /// count while a fresh search is in flight.
    pub(crate) result: Option<(u32, u32)>,
}

impl FindState {
    /// The UI side of closing the bar: `App::close_find_bar` also sends
    /// `FindStop`, which a destroyed tab can't receive. The next session
    /// starts case-insensitive, as a real browser's find bar does.
    pub(crate) fn close(&mut self) {
        self.open = false;
        self.result = None;
        self.match_case = false;
    }
}

impl App {
    /// `which`'s anchor rect and fade (0.0-1.0), or `None` once it has
    /// faded out or while its button isn't drawn. Call it every frame,
    /// open or not: egui's animation manager needs continuous polling to
    /// track real elapsed time, and polling only while open made a
    /// re-open jump straight to visible instead of easing in.
    pub(crate) fn anchored_popup_fade(
        &self,
        ctx: &egui::Context,
        which: AnchoredPopup,
        anchor: Option<egui::Rect>,
    ) -> Option<(egui::Rect, f32)> {
        let open = self.open_popup == Some(which);
        let fade = ctx.animate_bool(egui::Id::new(("anchored_popup_fade", which)), open);
        if fade > 0.0 {
            anchor.map(|rect| (rect, fade))
        } else {
            None
        }
    }

    /// One of the three engine pickers (Settings' default and current-tab
    /// engines, the Tab toolbar's Split): the tier/pros/cons cards of
    /// `build_engine_picker_popup` below `anchor`, with `highlighted`
    /// marked. Closes on a pick or a click outside, and returns the engine
    /// picked; what picking does is the caller's.
    pub(crate) fn build_anchored_engine_picker(
        &mut self,
        ui: &egui::Ui,
        which: AnchoredPopup,
        anchor: Option<egui::Rect>,
        highlighted: Option<EngineKind>,
    ) -> Option<EngineKind> {
        let (rect, fade) = self.anchored_popup_fade(ui.ctx(), which, anchor)?;
        let open = self.open_popup == Some(which);
        let (area_rect, picked) =
            build_engine_picker_popup(ui.ctx(), &format!("{which:?}_popup"), rect, highlighted, fade, open);
        let escape = open && ui.input(|i| i.key_pressed(egui::Key::Escape));
        if picked.is_some() || escape || clicked_outside_popup(ui, area_rect, Some(rect)) {
            close_popup(&mut self.open_popup, which);
        }
        picked
    }

    pub(crate) fn popup_opener_key(which: AnchoredPopup) -> egui::Id {
        egui::Id::new(("popup_opener", which))
    }

    /// The settings page's button showing `kind`, opening engine picker `which`. Its label is
    /// the engine's color, which is unreadable on the gold a focused button gets, so a
    /// focused one uses the dark label instead.
    pub(crate) fn engine_opener_button(
        &mut self,
        ui: &mut egui::Ui,
        which: AnchoredPopup,
        kind: EngineKind,
    ) -> egui::Response {
        let focused = ui
            .data(|d| d.get_temp::<egui::Id>(Self::popup_opener_key(which)))
            .is_some_and(|id| ui.memory(|m| m.has_focus(id)));
        let color = if focused { BG } else { engine_color(kind) };
        let response = ui.button(egui::RichText::new(engine_label(kind)).color(color));
        self.popup_opener_clicked(ui, which, &response);
        response
    }

    /// The button that opens popup `which`: toggles it when clicked and remembers itself, so
    /// `build_ui` can hand focus back to it when the popup closes from the keyboard. Returns
    /// whether the popup was just opened.
    pub(crate) fn popup_opener_clicked(
        &mut self,
        ui: &egui::Ui,
        which: AnchoredPopup,
        button: &egui::Response,
    ) -> bool {
        ui.data_mut(|d| d.insert_temp(Self::popup_opener_key(which), button.id));
        if button.clicked() {
            toggle_popup(&mut self.open_popup, which);
        }
        button.clicked() && self.open_popup == Some(which)
    }

    /// Whether an anchored popup is open: Escape closes it (`build_ui`), so the settings page and
    /// Tab view must not also act on the same press.
    pub(crate) fn popup_open(&self) -> bool {
        self.open_popup.is_some()
    }

    /// The "Recently Closed" dropdown's row for one entry: an icon (drawn
    /// into `ui` as a side effect, same as the row's other icons) and the
    /// text -- `name` for a group (with its member count), `title` (its
    /// recorded title) or else `url` for a tab, `text` for a note, `path`
    /// for every file-backed kind.
    fn closed_entry_label(
        ui: &mut egui::Ui,
        node: &store::json::JsonNode,
        title: Option<&str>,
        member_count: usize,
    ) -> String {
        let str_field = |name: &str| node.fields.get(name).and_then(|v| v.as_str()).unwrap_or_default().to_string();
        match node.kind.as_str() {
            "tab" => {
                engine_dot(ui, EngineKind::from_name(&str_field("engine")).unwrap_or(EngineKind::Servo), 8.0);
                title.map_or_else(|| str_field("url"), str::to_string)
            }
            "group" => {
                ui.label(egui_phosphor::light::FOLDER_SIMPLE);
                format!("{} ({member_count})", str_field("name"))
            }
            "note" => {
                ui.label(egui_phosphor::light::NOTE_PENCIL);
                str_field("text").lines().next().unwrap_or_default().to_string()
            }
            "image" => {
                ui.label(egui_phosphor::light::IMAGE);
                str_field("path")
            }
            "pdf" => {
                ui.label(egui_phosphor::light::FILE_PDF);
                str_field("path")
            }
            _ => {
                // "json" / "json_leaf", and anything a newer build added.
                ui.label(egui_phosphor::light::BRACKETS_CURLY);
                str_field("path")
            }
        }
    }

    /// The "Recently Closed" toolbar dropdown (Graph view, at any level):
    /// every entry, newest first; click one to reopen it where it was.
    pub(crate) fn build_recently_closed_popup(&mut self, ui: &mut egui::Ui, anchor_rect: egui::Rect, fade: f32) {
        let mut close = false;
        let mut reopen_index: Option<usize> = None;
        let open = self.open_popup == Some(AnchoredPopup::RecentlyClosed);
        let area_rect =
            build_list_popup_frame(ui.ctx(), "recently_closed_popup", anchor_rect, 220.0, 320.0, fade, open, |ui| {
                for (index, entry) in self.closed.iter().enumerate().rev() {
                    ui.horizontal(|ui| {
                        let root = &entry.doc.nodes[0];
                        let title = entry.local("title", &root.id).and_then(|v| v.as_str());
                        let label = Self::closed_entry_label(ui, root, title, entry.doc.nodes.len() - 1);
                        if list_row(ui, label).clicked() {
                            reopen_index = Some(index);
                        }
                    });
                }
            });

        let clicked_outside = clicked_outside_popup(ui, area_rect, Some(anchor_rect));

        if let Some(index) = reopen_index {
            let entry = self.closed.remove(index);
            self.closed_changed_at = Some(std::time::Instant::now());
            self.reopen_closed(entry);
            close = true;
        }
        if clicked_outside {
            close = true;
        }
        if close || self.closed.is_empty() {
            close_popup(&mut self.open_popup, AnchoredPopup::RecentlyClosed);
        }
    }

    /// Tab view's "Bookmarks (N)" dropdown -- every saved bookmark, click
    /// one to navigate the focused tab there, "x" to remove it. Grouped by
    /// `Bookmark::folder` (unfoldered entries first, then each named
    /// folder alphabetically) -- a single level, not a real tree, same
    /// scope limit `Bookmark::folder`'s own doc comment states. Same
    /// Area/Frame pattern as `build_recently_closed_popup`.
    pub(crate) fn build_bookmarks_popup(&mut self, ui: &mut egui::Ui, anchor_rect: egui::Rect, fade: f32) {
        let mut close = false;
        let mut navigate_url: Option<String> = None;
        let mut remove_index: Option<usize> = None;
        let mut folder_rename_start: Option<(String, String)> = None;
        let mut folder_rename_commit: Option<String> = None;
        let mut folder_rename_cancel = false;
        let open = self.open_popup == Some(AnchoredPopup::Bookmarks);
        let area_rect =
            build_list_popup_frame(ui.ctx(), "bookmarks_popup", anchor_rect, 220.0, 320.0, fade, open, |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.bookmarks_filter)
                        .desired_width(f32::INFINITY)
                        .hint_text("Filter..."),
                );
                let matches: Vec<(usize, &bookmarks::Bookmark)> = self
                    .bookmarks
                    .iter()
                    .enumerate()
                    .filter(|(_, b)| filter_matches(&self.bookmarks_filter, &[&b.title, &b.url]))
                    .collect();
                let mut folders: Vec<&str> = matches.iter().filter_map(|(_, b)| b.folder.as_deref()).collect();
                folders.sort_unstable();
                folders.dedup();
                // `None` (unfoldered) first, then each named folder
                // alphabetically -- `folders` is already sorted.
                let groups: Vec<Option<&str>> = std::iter::once(None).chain(folders.into_iter().map(Some)).collect();
                if matches.is_empty() {
                    ui.label(egui::RichText::new("No bookmarks yet.").weak());
                }
                egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                    for folder in groups {
                        let rows: Vec<&(usize, &bookmarks::Bookmark)> =
                            matches.iter().filter(|(_, b)| b.folder.as_deref() == folder).collect();
                        if rows.is_empty() {
                            continue;
                        }
                        if let Some(name) = folder {
                            ui.label(egui::RichText::new(name).small().strong().color(TEXT_PRIMARY));
                        }
                        for (index, bookmark) in rows {
                            let (index, bookmark) = (*index, *bookmark);
                            ui.horizontal(|ui| {
                                if list_row(ui, &bookmark.title).clicked() {
                                    navigate_url = Some(bookmark.url.clone());
                                }
                                if ui.small_button(egui_phosphor::light::X).clicked() {
                                    remove_index = Some(index);
                                }
                            });
                            let is_renaming =
                                self.renaming_bookmark_folder.as_ref().is_some_and(|(url, _)| url == &bookmark.url);
                            if is_renaming {
                                if let Some((_, draft)) = &mut self.renaming_bookmark_folder {
                                    let response = ui.add(
                                        egui::TextEdit::singleline(draft)
                                            .desired_width(140.0)
                                            .hint_text("Folder (blank for none)")
                                            .font(egui::TextStyle::Small),
                                    );
                                    if take_focus(&mut self.pending_focus, FocusTarget::BookmarkFolder) {
                                        response.request_focus();
                                    }
                                    if response.lost_focus() {
                                        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                                            folder_rename_cancel = true;
                                        } else {
                                            folder_rename_commit = Some(draft.clone());
                                        }
                                    }
                                }
                            } else {
                                let label = bookmark.folder.as_deref().unwrap_or("No folder");
                                if ui
                                    .add(
                                        egui::Label::new(egui::RichText::new(label).small().weak())
                                            .sense(egui::Sense::click()),
                                    )
                                    .on_hover_text("Click to set folder")
                                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                                    .clicked()
                                {
                                    folder_rename_start =
                                        Some((bookmark.url.clone(), bookmark.folder.clone().unwrap_or_default()));
                                }
                            }
                            ui.add_space(2.0);
                        }
                    }
                });
            });

        let clicked_outside = clicked_outside_popup(ui, area_rect, Some(anchor_rect));

        if let Some((url, draft)) = folder_rename_start {
            self.renaming_bookmark_folder = Some((url, draft));
            self.pending_focus = Some(FocusTarget::BookmarkFolder);
        }
        if folder_rename_cancel {
            self.renaming_bookmark_folder = None;
        }
        if let Some(draft) = folder_rename_commit {
            if let Some((url, _)) = self.renaming_bookmark_folder.take() {
                let trimmed = draft.trim();
                if let Some(bookmark) = self.bookmarks.iter_mut().find(|b| b.url == url) {
                    bookmark.folder = if trimmed.is_empty() { None } else { Some(trimmed.to_string()) };
                    bookmarks::save(&self.bookmarks);
                }
            }
        }
        if let Some(url) = navigate_url {
            if let Some(id) = self.focused.clone() {
                self.navigate_tab(&id, url);
            }
            close = true;
        }
        if let Some(index) = remove_index {
            self.bookmarks.remove(index);
            bookmarks::save(&self.bookmarks);
        }
        if clicked_outside && self.renaming_bookmark_folder.is_none() {
            close = true;
        }
        // Not `|| self.bookmarks.is_empty()` any more -- the Settings page's
        // own "Lists" section can open this popup with an empty list, and
        // that used to close it again the very next frame.
        if close {
            close_popup(&mut self.open_popup, AnchoredPopup::Bookmarks);
            self.renaming_bookmark_folder = None;
        }
    }

    /// Tab view's "History (N)" dropdown -- every entry in `self.history`,
    /// most-recently-visited first, click a title to navigate the focused
    /// tab there, "Clear" to empty the whole list. Same Area/Frame pattern
    /// as `build_bookmarks_popup`/`build_recently_closed_popup`, scrollable
    /// since history can grow much longer than either of those.
    pub(crate) fn build_history_popup(&mut self, ui: &mut egui::Ui, anchor_rect: egui::Rect, fade: f32) {
        let mut close = false;
        let mut navigate_url: Option<String> = None;
        let mut clear_requested = false;
        let open = self.open_popup == Some(AnchoredPopup::History);
        let area_rect =
            build_list_popup_frame(ui.ctx(), "history_popup", anchor_rect, 220.0, 320.0, fade, open, |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.history_filter)
                        .desired_width(f32::INFINITY)
                        .hint_text("Filter..."),
                );
                if self.history.is_empty() {
                    ui.label(egui::RichText::new("No history yet.").weak());
                }
                egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                    for entry in self.history.iter().rev() {
                        let label = if entry.title.is_empty() { &entry.url } else { &entry.title };
                        if !filter_matches(&self.history_filter, &[&entry.title, &entry.url]) {
                            continue;
                        }
                        if list_row(ui, label).clicked() {
                            navigate_url = Some(entry.url.clone());
                        }
                    }
                });
                ui.separator();
                if ui.button("Clear").clicked() {
                    clear_requested = true;
                }
            });

        let clicked_outside = clicked_outside_popup(ui, area_rect, Some(anchor_rect));

        if let Some(url) = navigate_url {
            if let Some(id) = self.focused.clone() {
                self.navigate_tab(&id, url);
            }
            close = true;
        }
        if clear_requested {
            self.history.clear();
            self.history_dirty = true;
            close = true;
        }
        if clicked_outside {
            close = true;
        }
        // Not `|| self.history.is_empty()` any more -- see the matching
        // comment in `build_bookmarks_popup`.
        if close {
            close_popup(&mut self.open_popup, AnchoredPopup::History);
        }
    }

    /// Tab view's "Downloads (N)" dropdown -- every entry in
    /// `self.downloads` (the same data the graph view's own download
    /// nodes/context menu show, see `App::node_menu`), most-
    /// recently-downloaded first, click a filename to open its containing
    /// folder (same as `reveal_node`), "x" to remove it from the list
    /// (same as `close_node_and_save`). Exists so a completed download can
    /// be found without first switching to Graph view.
    pub(crate) fn build_downloads_popup(&mut self, ui: &mut egui::Ui, anchor_rect: egui::Rect, fade: f32) {
        let mut close = false;
        let mut reveal_id: Option<String> = None;
        let mut remove_id: Option<String> = None;
        let open = self.open_popup == Some(AnchoredPopup::Downloads);
        let area_rect =
            build_list_popup_frame(ui.ctx(), "downloads_popup", anchor_rect, 220.0, 320.0, fade, open, |ui| {
                if !self.node_order.iter().any(|id| self.nodes.get(id).is_some_and(|n| n.as_download().is_some())) {
                    ui.label(egui::RichText::new("No downloads yet.").weak());
                }
                egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                    for id in self.node_order.iter().rev() {
                        let Some(node) = self.nodes.get(id).and_then(|n| n.as_download()) else { continue };
                        ui.horizontal(|ui| {
                            let filename = download_filename(&node.path);
                            if list_row(ui, filename).clicked() {
                                reveal_id = Some(id.clone());
                            }
                            if ui.small_button(egui_phosphor::light::X).clicked() {
                                remove_id = Some(id.clone());
                            }
                        });
                        // In-progress downloads get a live progress row below
                        // the filename; a completed one is left exactly as
                        // before this feature -- just the filename row above.
                        if !node.completed {
                            match node.total_bytes {
                                Some(total) if total > 0 => {
                                    let fraction = (node.bytes_received as f32 / total as f32).clamp(0.0, 1.0);
                                    ui.add(
                                        egui::ProgressBar::new(fraction)
                                            .text(format!(
                                                "{} / {}",
                                                human_bytes(node.bytes_received),
                                                human_bytes(total)
                                            ))
                                            .desired_width(f32::INFINITY),
                                    );
                                }
                                // No `Content-Length` to size a fraction
                                // against (chunked transfer) -- a byte count
                                // with no bar/percentage is honest; a bar stuck
                                // at some arbitrary fraction (or a fake
                                // "indeterminate" scroll egui has no built-in
                                // support for) would just be a guess.
                                _ => {
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(format!(
                                                "Downloading… {}",
                                                human_bytes(node.bytes_received)
                                            ))
                                            .small()
                                            .weak(),
                                        )
                                        .wrap(),
                                    );
                                }
                            }
                        }
                    }
                });
            });

        let clicked_outside = clicked_outside_popup(ui, area_rect, Some(anchor_rect));

        if let Some(id) = reveal_id {
            self.reveal_node(&id);
            close = true;
        }
        if let Some(id) = remove_id {
            self.close_node_and_save(&id);
        }
        if clicked_outside {
            close = true;
        }
        // Not `|| self.nodes.is_empty()` any more: that counted every graph
        // node (tabs, notes, images, ...), not just downloads, so it almost
        // never actually fired -- and per the same fix as
        // `build_bookmarks_popup`/`build_history_popup`, an empty list is no
        // longer a reason to self-close at all now that the "Lists" menu can
        // open this popup while empty.
        if close {
            close_popup(&mut self.open_popup, AnchoredPopup::Downloads);
        }
    }

    /// Tab view's "Tab actions" dropdown -- "Mute Others", "Close Others",
    /// and the "Pin All"/"Unpin All" toggle in one popup, folded from three
    /// always-visible toolbar buttons (a later session pass flagged the
    /// toolbar as cramped -- confirmed live, text touching the window edge
    /// even at 1300pt wide). Same anchored Area/Frame pattern
    /// `build_downloads_popup` already uses.
    pub(crate) fn build_tab_actions_popup(
        &mut self,
        ui: &mut egui::Ui,
        anchor_rect: egui::Rect,
        focused_id: &str,
        fade: f32,
    ) {
        let mut close = false;
        let mut mute_others = false;
        let mut close_others = false;
        let mut toggle_pin_all = false;
        // The focused tab's own actions, each with its shortcut shown at the row's edge.
        let mut tab_action: Option<fn(&mut App)> = None;
        let focused_pinned = self.nodes.get(focused_id).is_some_and(|n| n.payload.pinned());
        let tab_pin_states: Vec<bool> =
            self.nodes.values().filter(|n| n.payload.saved_tab().is_some()).map(|n| n.payload.pinned()).collect();
        let all_pinned = !tab_pin_states.is_empty() && tab_pin_states.iter().all(|&p| p);
        let area = egui::Area::new(egui::Id::new("tab_actions_popup"))
            .fixed_pos(anchor_rect.left_bottom())
            .order(egui::Order::Foreground)
            .interactable(self.open_popup == Some(AnchoredPopup::TabActions))
            .show(ui.ctx(), |ui| {
                ui.set_opacity(fade);
                popup_frame(ui).show(ui, |ui| {
                    menu_item_style(ui);
                    ui.set_min_width(140.0);
                    let rows: [TabActionRow; 5] = [
                        (if focused_pinned { "Unpin" } else { "Pin" }, "Ctrl+P", App::pin_focused_tab_shortcut),
                        ("Duplicate", "Ctrl+D", App::duplicate_focused_tab_shortcut),
                        ("Find in Page", "Ctrl+F", App::open_find_bar_shortcut),
                        ("Developer Tools", "F12", App::open_devtools_shortcut),
                        ("Close Tab", "Ctrl+W", App::close_focused_tab_shortcut),
                    ];
                    for (label, keys, action) in rows {
                        if ui.add(egui::Button::new(label).shortcut_text(keys)).clicked() {
                            tab_action = Some(action);
                            close = true;
                        }
                    }
                    ui.separator();
                    if ui.button("Mute Others").on_hover_text("Mute every other open tab").clicked() {
                        mute_others = true;
                        close = true;
                    }
                    if ui.button("Close Others").on_hover_text("Close every other open tab").clicked() {
                        close_others = true;
                        close = true;
                    }
                    let label = if all_pinned { "Unpin All" } else { "Pin All" };
                    let hover = if all_pinned { "Unpin every open tab" } else { "Pin every open tab" };
                    if ui.button(label).on_hover_text(hover).clicked() {
                        toggle_pin_all = true;
                        close = true;
                    }
                });
            });

        let clicked_outside = clicked_outside_popup(ui, area.response.rect, Some(anchor_rect));

        if let Some(action) = tab_action {
            action(self);
        }
        if mute_others {
            self.mute_all_except(focused_id);
        }
        if close_others {
            self.close_all_except(focused_id);
        }
        if toggle_pin_all {
            self.set_all_pinned(!all_pinned);
        }
        if clicked_outside {
            close = true;
        }
        if close {
            close_popup(&mut self.open_popup, AnchoredPopup::TabActions);
        }
    }

    /// Sends `FindStart` for `self.find.text` to the focused tab, or
    /// `FindStop` if it's empty (clearing any existing highlight) — shared
    /// by the find bar's own text-changed handling and `open_find_bar_shortcut`
    /// re-opening with a remembered term.
    pub(crate) fn send_find_start(&mut self) {
        self.find.result = None;
        // Tab view has no continuous redraw loop of its own (unlike Graph
        // view's physics) — without this, a headless `FINDTEXT` (or a
        // remembered-term re-open) can land with nothing scheduled to ever
        // paint the bar's updated count, the same gap `focus_address_bar_
        // shortcut` already has its own note about.
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        let Some(id) = self.focused.clone() else { return };
        let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) else { return };
        if self.find.text.is_empty() {
            node.tab.send(&HostToEngine::FindStop);
        } else {
            node.tab.send(&HostToEngine::FindStart { text: self.find.text.clone(), match_case: self.find.match_case });
        }
    }

    /// Closes the find bar: sends `FindStop` (clears highlighted matches)
    /// and resets its result and match case (`FindState::close`) — called on `Escape`, its own close
    /// button, headless `CLOSEFINDBAR`, and whenever focus leaves the tab
    /// it was searching (tab switch/close), so a stale search never keeps
    /// running silently in the background.
    pub(crate) fn close_find_bar(&mut self) {
        if !self.find.open {
            return;
        }
        self.find.close();
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        if let Some(id) = self.focused.clone() {
            if let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
                node.tab.send(&HostToEngine::FindStop);
            }
        }
    }

    /// `Enter`/`Shift+Enter` in the find bar, or its Prev/Next buttons.
    pub(crate) fn send_find_navigate(&mut self, forward: bool) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        let Some(id) = self.focused.clone() else { return };
        let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) else { return };
        node.tab.send(&if forward { HostToEngine::FindNext } else { HostToEngine::FindPrevious });
    }

    /// `Ctrl+F`'s floating find bar: a text field, prev/next, a match count,
    /// and a close button. Anchored top-right of the content area in
    /// screen space rather than off some toolbar button's rect — unlike
    /// Settings, there's no persistent visible button to anchor from (this
    /// only ever opens via `Ctrl+F`/headless `OPENFINDBAR`).
    pub(crate) fn build_find_bar(&mut self, ui: &mut egui::Ui) {
        let mut close = false;
        let mut text_changed = false;
        let mut go_next = false;
        let mut go_prev = false;
        egui::Area::new(egui::Id::new("find_bar"))
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-12.0, CHROME_HEIGHT_POINTS + 6.0))
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                popup_frame(ui).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let response = ui
                            .add(
                                egui::TextEdit::singleline(&mut self.find.text)
                                    .desired_width(160.0)
                                    .hint_text("Find in page"),
                            )
                            .on_hover_text("Enter: next match, Shift+Enter: previous, Esc: close");
                        if take_focus(&mut self.pending_focus, FocusTarget::FindBar) {
                            response.request_focus();
                        }
                        if response.changed() {
                            text_changed = true;
                        }
                        if rest_borderless(ui, |ui| ui.toggle_value(&mut self.find.match_case, "Aa"))
                            .on_hover_text("Match case")
                            .changed()
                        {
                            text_changed = true;
                        }
                        if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            if ui.input(|i| i.modifiers.shift) {
                                go_prev = true;
                            } else {
                                go_next = true;
                            }
                        }

                        match self.find.result {
                            Some((0, _)) => {
                                ui.label("No results");
                            }
                            // WebKit never reports `active_index` (see
                            // `EngineToHost::FindResult`'s doc comment)
                            // — show just the count, not "0 of N".
                            Some((count, 0)) => {
                                ui.label(format!("{count}"));
                            }
                            Some((count, active)) => {
                                ui.label(format!("{active}/{count}"));
                            }
                            None => {}
                        }

                        // Prev/Next stay plain ASCII labels, not Unicode
                        // arrow glyphs — confirmed live that `▲`/`▼`
                        // render as empty tofu boxes with this app's own
                        // font (unlike `◀`/`▶`/`⚙` elsewhere in this same
                        // toolbar, which do render). Not migrated to
                        // `egui_phosphor` alongside the close button
                        // below -- out of scope for the cosmetic-only
                        // "x"/"X" pass that touched everything else here.
                        let has_matches = matches!(self.find.result, Some((count, _)) if count > 0);
                        if ui.add_enabled(has_matches, egui::Button::new("Prev")).clicked() {
                            go_prev = true;
                        }
                        if ui.add_enabled(has_matches, egui::Button::new("Next")).clicked() {
                            go_next = true;
                        }
                        // Was a literal capital "X" -- confirmed live
                        // that `✕` tofu-boxed here (same finding the
                        // comment above already documents), so this one
                        // used the ASCII fallback rather than a Unicode
                        // glyph. Now the real `X` icon instead, once the
                        // icon font was already in place for the other
                        // close buttons in this app's chrome.
                        if ui.button(egui_phosphor::light::X).clicked() {
                            close = true;
                        }
                    });
                });
            });

        if text_changed {
            self.send_find_start();
        }
        if go_next {
            self.send_find_navigate(true);
        }
        if go_prev {
            self.send_find_navigate(false);
        }
        if close {
            self.close_find_bar();
        }
    }

    /// A tab list row's label for `id`: its title, or its url when the
    /// title is empty (live or unloaded alike, via `listed_tab`) -- factored
    /// out so `build_quick_switcher`'s row loop and its own test agree on
    /// exactly what a sleeping tab's row shows.
    pub(crate) fn quick_switcher_row_label(&self, id: &str) -> Option<String> {
        let (title, url) = self.nodes.get(id)?.payload.listed_tab()?;
        Some(if title.is_empty() { url.to_string() } else { title.to_string() })
    }

    /// `self.node_order`, filtered by a case-insensitive substring match
    /// against title-or-url — factored out so the quick-switcher's render
    /// function and its Enter-key jump always agree on the exact same list
    /// (computed once per frame, not separately in each place that needs
    /// it).
    pub(crate) fn quick_switcher_matches(&self, query: &str) -> Vec<String> {
        let query = query.to_lowercase();
        self.node_order
            .iter()
            .filter(|id| {
                self.nodes.get(id.as_str()).and_then(|n| n.payload.listed_tab()).is_some_and(|(title, url)| {
                    query.is_empty() || title.to_lowercase().contains(&query) || url.to_lowercase().contains(&query)
                })
            })
            .cloned()
            .collect()
    }

    /// Moves the quick-switcher's selection by `delta` (+1/-1), clamped to
    /// the current match list — shared by the real ArrowUp/ArrowDown
    /// handling in `build_quick_switcher` and the headless `QUICKSWITCHUP`/
    /// `QUICKSWITCHDOWN` debug commands (see `debug_control`'s own doc
    /// comment on why a real `KeyEvent` can't be synthesized).
    pub(crate) fn quick_switcher_move(&mut self, delta: i32) {
        let Some(query) = self.quick_switcher.as_ref().map(|s| s.query.clone()) else { return };
        let len = self.quick_switcher_matches(&query).len();
        let Some(switcher) = &mut self.quick_switcher else { return };
        if len == 0 {
            switcher.selected = 0;
        } else if delta > 0 {
            switcher.selected = (switcher.selected + 1).min(len - 1);
        } else {
            switcher.selected = switcher.selected.saturating_sub(1);
        }
    }

    /// Jumps to whichever match is currently selected and closes the
    /// switcher — shared by the real Enter handling in
    /// `build_quick_switcher` and headless `QUICKSWITCHENTER`. See
    /// `build_quick_switcher`'s own doc comment for why this calls
    /// `focus_tab` directly rather than the graph-node expand animation.
    pub(crate) fn quick_switcher_confirm(&mut self) {
        let Some(switcher) = &self.quick_switcher else { return };
        let matches = self.quick_switcher_matches(&switcher.query);
        if let Some(id) = matches.get(switcher.selected).cloned() {
            self.show_level_of(&id);
            self.focus_tab(&id);
        }
        self.quick_switcher = None;
    }

    /// `Ctrl+K`'s tab quick-switcher: a centered search box plus a
    /// scrollable list of matches (see `quick_switcher_matches`), arrow
    /// keys move the selection, Enter jumps to it, Escape closes. Called
    /// from `build_ui` itself rather than `build_item_ui`/`build_graph_ui`
    /// so it works the same from either view.
    pub(crate) fn build_quick_switcher(&mut self, ui: &mut egui::Ui) {
        let current = self.quick_switcher.clone();
        let is_live = current.is_some();
        let Some((switcher, fade)) =
            fade_popup_state(ui.ctx(), egui::Id::new("quick_switcher_fade"), &current, &mut self.quick_switcher_cache)
        else {
            return;
        };

        if is_live {
            // Read once, outside the `Area`, so arrow/Enter/Escape work
            // regardless of whether the search field inside currently has
            // egui focus — same reasoning the address-bar dropdown's own
            // arrow-key handling already documents. Gated on `is_live`:
            // once actually closing (rendering the frozen `cache` snapshot
            // below instead), there's no live switcher left to move a
            // selection in or confirm.
            let (arrow_down, arrow_up, enter, escape) = ui.input(|i| {
                (
                    i.key_pressed(egui::Key::ArrowDown),
                    i.key_pressed(egui::Key::ArrowUp),
                    i.key_pressed(egui::Key::Enter),
                    i.key_pressed(egui::Key::Escape),
                )
            });
            if arrow_down {
                self.quick_switcher_move(1);
            }
            if arrow_up {
                self.quick_switcher_move(-1);
            }
            if escape {
                self.quick_switcher = None;
            }
            if enter {
                self.quick_switcher_confirm();
            }
        }

        let query = switcher.query.clone();
        let selected_index = switcher.selected;
        let matches = self.quick_switcher_matches(&query);
        let mut jump_to: Option<String> = None;

        egui::Area::new(egui::Id::new("quick_switcher"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 80.0))
            .order(egui::Order::Foreground)
            .interactable(is_live)
            .show(ui.ctx(), |ui| {
                ui.set_opacity(fade);
                dialog_frame(ui).show(ui, |ui| {
                    ui.set_width(360.0);
                    if is_live {
                        if let Some(switcher) = &mut self.quick_switcher {
                            let response = ui.add(
                                egui::TextEdit::singleline(&mut switcher.query)
                                    .desired_width(f32::INFINITY)
                                    // Tab would otherwise walk focus out to the toolbar behind
                                    // the switcher; arrows and Enter pick a row.
                                    .lock_focus(true)
                                    .hint_text("Search open tabs..."),
                            );
                            if take_focus(&mut self.pending_focus, FocusTarget::QuickSwitcher) {
                                response.request_focus();
                            }
                        }
                    } else {
                        // Closing: a live `TextEdit` needs `&mut self.
                        // quick_switcher`, which no longer exists once
                        // it's actually `None` -- the fade past this
                        // point is purely a visual echo of the last
                        // real state, not something still editable.
                        let placeholder = if query.is_empty() { "Search open tabs..." } else { &query };
                        ui.label(placeholder);
                    }
                    ui.separator();
                    if matches.is_empty() {
                        ui.label("No matching tabs");
                    }
                    ui.horizontal(|ui| {
                        for (keys, what) in [("Up / Down", "move"), ("Enter", "jump"), ("Esc", "close")] {
                            kbd_keys(ui, keys);
                            ui.label(egui::RichText::new(what).small().weak());
                            ui.add_space(4.0);
                        }
                    });
                    egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                        for (i, id) in matches.iter().enumerate() {
                            let Some(label) = self.quick_switcher_row_label(id) else { continue };
                            if is_live {
                                if selectable_label(ui, i == selected_index, label).clicked() {
                                    jump_to = Some(id.clone());
                                }
                            } else {
                                // Non-interactive while closing --
                                // clicking a row already fading out
                                // would be a confusing "did that even
                                // do anything" moment.
                                ui.add_enabled(false, egui::Button::selectable(i == selected_index, label));
                            }
                        }
                    });
                });
            });

        if let Some(id) = jump_to {
            self.show_level_of(&id);
            self.focus_tab(&id);
            self.quick_switcher = None;
        }
    }

    /// `F1` / headless `TOGGLESHORTCUTSHELP` -- a centered reference popup
    /// listing every global keyboard shortcut `handle_global_shortcuts`
    /// handles, so a shortcut added to that function doesn't just live in a
    /// code comment nobody using the app ever sees. Static content (no
    /// state to stage/commit), so unlike the quick-switcher this is just a
    /// plain `Area`/`Frame` with a scrollable list and a Close button.
    pub(crate) fn build_shortcuts_help(&mut self, ui: &mut egui::Ui, anchor_rect: Option<egui::Rect>) {
        if !self.shortcuts_help_open {
            return;
        }
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.shortcuts_help_open = false;
            return;
        }

        // A key of "#" starts a section; its text is the heading.
        const SHORTCUTS: &[(&str, &str)] = &[
            ("#", "Everywhere"),
            ("Ctrl+T", "New tab"),
            ("Ctrl+W", "Close focused tab"),
            ("Ctrl+Shift+W", "Close all tabs at the current level"),
            ("Ctrl+Shift+T", "Reopen last closed tab"),
            ("Ctrl+D", "Duplicate focused tab"),
            ("Ctrl+Tab / Ctrl+Shift+Tab", "Next / previous tab"),
            ("Ctrl+K", "Quick switcher (jump to any open tab)"),
            ("Ctrl+G", "Toggle Tab / Graph view"),
            ("Ctrl+Shift+G", "New group"),
            ("F2", "Rename the group currently being viewed"),
            ("Ctrl+P", "Pin / unpin focused tab"),
            ("Ctrl+L", "Focus the address bar"),
            ("Ctrl+F", "Find in page"),
            ("Ctrl+R / F5", "Reload focused tab"),
            ("Ctrl+= / Ctrl+-", "Zoom in / out"),
            ("Ctrl+0", "Reset zoom"),
            ("F12", "Open devtools"),
            ("F1", "Toggle this shortcut reference"),
            ("#", "Graph canvas"),
            ("1 - 9", "Focus the card with that number (two digits for 10 and up); Enter then opens it"),
            ("Arrows", "Move focus to the nearest node"),
            ("Enter", "Open the focused node"),
            ("Escape", "Clear the selection, close a menu, dismiss a preview"),
            ("#", "Graph cards (Tab onto the canvas)"),
            ("Tab / Shift+Tab", "Next / previous card, in creation order (numbered badges)"),
            ("Enter", "Open the focused card; enters a group or JSON level"),
            ("Space", "Select / deselect a tab card"),
            ("Delete", "Close the focused card"),
            ("Shift+F10", "The card's menu: color, move to a group, ..."),
            ("Escape", "Close a menu or popup, back to its card"),
            ("#", "JSON level"),
            ("Up / Down", "Select the previous / next outline row"),
            ("Enter", "Expand / collapse the selected card"),
            ("Right / Left", "Open / close the row (Left on a closed row selects its parent)"),
            ("Ctrl+C", "Copy the selected value"),
            ("Ctrl+Shift+C", "Copy the selected path"),
            ("#", "Tab view and pages"),
            ("Escape", "Back to Graph (closes an open popup first)"),
            ("#", "Address bar, find bar, quick switcher"),
            ("Up / Down", "Address bar: move through the suggestions (wraps)"),
            ("Enter", "Address bar: take the highlighted suggestion, else go to the typed text"),
            ("Escape", "Address bar: dismiss the suggestions; again for Graph"),
            ("Enter", "Find bar: next match; Shift+Enter previous"),
            ("Escape", "Find bar / quick switcher: close"),
            ("#", "Lists and popups"),
            ("Up / Down", "Move the highlight (Recently Closed, Bookmarks, History, Downloads, quick switcher)"),
            ("Enter", "Pick the highlighted row"),
            ("Escape", "Close and return to the button that opened it"),
        ];

        let area = egui::Area::new(egui::Id::new("shortcuts_help"))
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                dialog_frame(ui).show(ui, |ui| {
                    ui.set_width(360.0);
                    ui.label(heading_text("Keyboard Shortcuts"));
                    ui.separator();
                    egui::ScrollArea::vertical().max_height(520.0).show(ui, |ui| {
                        egui::Grid::new("shortcuts_help_grid").num_columns(2).spacing(egui::vec2(16.0, 6.0)).show(
                            ui,
                            |ui| {
                                for (keys, desc) in SHORTCUTS {
                                    if *keys == "#" {
                                        ui.label(egui::RichText::new(*desc).strong().color(ACCENT));
                                        ui.label("");
                                    } else {
                                        ui.horizontal(|ui| kbd_keys(ui, keys));
                                        ui.label(*desc);
                                    }
                                    ui.end_row();
                                }
                            },
                        );
                    });
                    ui.separator();
                    if ui.button("Close").clicked() {
                        self.shortcuts_help_open = false;
                    }
                });
            });

        if clicked_outside_popup(ui, area.response.rect, anchor_rect) {
            self.shortcuts_help_open = false;
        }
    }

    /// "Import Session..." confirm popup -- shown whenever `pending_import`
    /// is staged (a real file pick, or headless `IMPORTSESSION`), comparing
    /// the picked file's tab count against the currently running session's
    /// own before actually replacing anything, so swapping in the wrong
    /// file is a deliberate second click rather than a silent one-shot
    /// dialog pick. Import applies it via the same `replace_running_session`
    /// `load_named_session` already uses; Cancel just discards the staged
    /// session, leaving the running one untouched.
    pub(crate) fn build_import_confirm_popup(&mut self, ui: &mut egui::Ui) {
        let Some(json) = &self.pending_import else { return };
        let incoming = json.nodes.iter().filter(|n| n.kind == "tab").count();
        let current = self.nodes.len();
        let mut confirm = false;
        let mut cancel = false;
        egui::Area::new(egui::Id::new("import_confirm_popup"))
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                dialog_frame(ui).show(ui, |ui| {
                    ui.set_min_width(240.0);
                    ui.label(heading_text("Import Session"));
                    ui.add_space(6.0);
                    ui.label(format!("Current session: {} tab{}", current, if current == 1 { "" } else { "s" }));
                    ui.label(format!("Importing: {} tab{}", incoming, if incoming == 1 { "" } else { "s" }));
                    ui.add_space(4.0);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new("Importing replaces every currently open tab.").small().weak(),
                        )
                        .wrap(),
                    );
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Import").clicked() {
                            confirm = true;
                        }
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                    });
                });
            });
        if confirm {
            if let Some(json) = self.pending_import.take() {
                self.replace_running_session(&json);
            }
        }
        if cancel {
            self.pending_import = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_a_popup_replaces_the_open_one() {
        let mut open = None;
        toggle_popup(&mut open, AnchoredPopup::Bookmarks);
        assert_eq!(open, Some(AnchoredPopup::Bookmarks));
        toggle_popup(&mut open, AnchoredPopup::History);
        assert_eq!(open, Some(AnchoredPopup::History));
        toggle_popup(&mut open, AnchoredPopup::History);
        assert_eq!(open, None);
    }

    #[test]
    fn a_fading_popup_cannot_close_its_replacement() {
        let mut open = Some(AnchoredPopup::History);
        // Bookmarks is still fading out and sees the click outside it.
        close_popup(&mut open, AnchoredPopup::Bookmarks);
        assert_eq!(open, Some(AnchoredPopup::History));
        close_popup(&mut open, AnchoredPopup::History);
        assert_eq!(open, None);
    }

    #[test]
    fn closing_find_keeps_the_text_and_resets_the_rest() {
        let mut find = FindState { open: true, text: "abc".into(), match_case: true, result: Some((3, 1)) };
        find.close();
        assert!(!find.open);
        assert_eq!(find.text, "abc");
        assert!(!find.match_case);
        assert_eq!(find.result, None);
    }

    #[test]
    fn a_focus_request_is_taken_once_by_its_own_widget() {
        let mut pending = Some(FocusTarget::FindBar);
        assert!(!take_focus(&mut pending, FocusTarget::AddressBar));
        assert!(take_focus(&mut pending, FocusTarget::FindBar));
        assert!(!take_focus(&mut pending, FocusTarget::FindBar));
    }

    /// A sleeping tab's Quick Switcher row shows its title, or its url when
    /// untitled -- same as a live tab's row, unlike the raw `as_tab` lookup
    /// this used to skip an unloaded match with (Fix 1, final-review).
    #[test]
    fn quick_switcher_label_uses_title_or_url_for_a_sleeping_tab() {
        use crate::session_sync::tests::{add, app_on, sleeping_tab};
        let mut app = app_on(&crate::store::testutil::temp_dir("qs-label").join("session.mnf"), 1);
        let titled = add(&mut app, sleeping_tab("https://a/", "Alpha"));
        let untitled = add(&mut app, sleeping_tab("https://b/", ""));
        assert_eq!(app.quick_switcher_row_label(&titled).as_deref(), Some("Alpha"));
        assert_eq!(app.quick_switcher_row_label(&untitled).as_deref(), Some("https://b/"));
    }
}

#[cfg(test)]
mod engine_picker_tests {
    use super::*;

    fn escape() -> egui::Event {
        let key = egui::Key::Escape;
        egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn frame(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 1000.0))),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| app.build_ui(ui));
        output.textures_delta.clear(); // nothing uploads the font atlas here
    }

    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    /// One frame of a three-row list popup; returns the row clicked.
    fn list_frame(ctx: &egui::Context, events: Vec<egui::Event>) -> Option<usize> {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 600.0))),
            events,
            ..Default::default()
        };
        let mut clicked = None;
        let mut output = ctx.run_ui(input, |ui| {
            let anchor = egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(60.0, 20.0));
            build_list_popup_frame(ui.ctx(), "test_list", anchor, 100.0, 300.0, 1.0, true, |ui| {
                for row in 0..3 {
                    if list_row(ui, format!("row {row}")).clicked() {
                        clicked = Some(row);
                    }
                }
            });
        });
        output.textures_delta.clear(); // nothing uploads the font atlas here
        clicked
    }

    /// A list popup opens on its first row; Down and Up move (wrapping) and Enter clicks the focused row.
    #[test]
    fn list_rows_take_arrows_and_enter() {
        let ctx = egui::Context::default();
        theme::apply_style(&ctx);
        for _ in 0..3 {
            assert_eq!(list_frame(&ctx, vec![]), None);
        }
        assert_eq!(list_frame(&ctx, vec![key(egui::Key::Enter)]), Some(0), "opens on the first row");
        list_frame(&ctx, vec![key(egui::Key::ArrowUp)]);
        list_frame(&ctx, vec![]);
        assert_eq!(list_frame(&ctx, vec![key(egui::Key::Enter)]), Some(2), "Up from the first row wraps to the last");
        list_frame(&ctx, vec![key(egui::Key::ArrowDown)]);
        list_frame(&ctx, vec![]);
        assert_eq!(list_frame(&ctx, vec![key(egui::Key::Enter)]), Some(0), "Down from the last row wraps to the first");
    }

    /// Escape closes an open engine picker and nothing else (the settings page stays), and focus goes back to
    /// the button that opened it; the next Escape leaves the page.
    #[test]
    fn escape_closes_the_picker_then_the_page_and_refocuses_the_button() {
        let ctx = egui::Context::default();
        theme::apply_fonts(&ctx);
        theme::apply_style(&ctx);
        let mut app = App { view_mode: ViewMode::Settings, ..App::default() };
        for _ in 0..3 {
            frame(&mut app, &ctx, vec![]);
        }
        app.open_popup = Some(AnchoredPopup::DefaultEngine);
        for _ in 0..3 {
            frame(&mut app, &ctx, vec![]);
        }
        assert_eq!(app.open_popup, Some(AnchoredPopup::DefaultEngine), "the picker should stay open");
        frame(&mut app, &ctx, vec![escape()]);
        assert_eq!(app.open_popup, None, "Escape should close the picker");
        assert_eq!(app.view_mode, ViewMode::Settings, "the same Escape must not leave the settings page");
        let opener = ctx.data(|d| d.get_temp::<egui::Id>(App::popup_opener_key(AnchoredPopup::DefaultEngine)));
        frame(&mut app, &ctx, vec![]);
        assert!(opener.is_some_and(|id| ctx.memory(|m| m.has_focus(id))), "focus should return to the opening button");
        frame(&mut app, &ctx, vec![escape()]);
        assert_ne!(app.view_mode, ViewMode::Settings, "a second Escape should leave the page");
    }
}
