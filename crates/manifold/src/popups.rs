//! The anchored toolbar popups (recently closed, bookmarks, history,
//! downloads, tab actions) and the floating windows: find bar, quick
//! switcher, shortcuts help, import confirm and the JSON viewer.

use super::*;

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
                ui.set_min_width(min_width);
                // An infinite cap makes egui lay out NaN rects; no cap
                // lets the area size to its contents.
                if max_width.is_finite() {
                    ui.set_max_width(max_width);
                }
                add_contents(ui);
            });
        })
        .response
        .rect
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
        if picked.is_some() || clicked_outside_popup(ui, area_rect, Some(rect)) {
            close_popup(&mut self.open_popup, which);
        }
        picked
    }

    /// The "Recently Closed" dropdown's row for one entry: an icon (drawn
    /// into `ui` as a side effect, same as the row's other icons) and the
    /// text -- `name` for a group (with its member count), `url` for a
    /// tab, `text` for a note, `path` for every file-backed kind.
    fn closed_entry_label(ui: &mut egui::Ui, node: &store::json::JsonNode, member_count: usize) -> String {
        let str_field = |name: &str| node.fields.get(name).and_then(|v| v.as_str()).unwrap_or_default().to_string();
        match node.kind.as_str() {
            "tab" => {
                let code = store::model::ENGINES.iter().position(|e| *e == str_field("engine")).unwrap_or(0) as u64 + 1;
                engine_dot(ui, session_sync::engine_of(code), 8.0);
                str_field("url")
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
                        let label = Self::closed_entry_label(ui, root, entry.doc.nodes.len() - 1);
                        if ui
                            .add(egui::Label::new(label).truncate().sense(egui::Sense::click()))
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .clicked()
                        {
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
                                if ui
                                    .add(egui::Label::new(&bookmark.title).truncate().sense(egui::Sense::click()))
                                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                                    .clicked()
                                {
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
                        if ui
                            .add(egui::Label::new(label).truncate().sense(egui::Sense::click()))
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .clicked()
                        {
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
                            if ui
                                .add(egui::Label::new(filename).truncate().sense(egui::Sense::click()))
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .clicked()
                            {
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
        let tab_pin_states: Vec<bool> = self.nodes.values().filter_map(|n| n.as_tab()).map(|t| t.pinned).collect();
        let all_pinned = !tab_pin_states.is_empty() && tab_pin_states.iter().all(|&p| p);
        let area = egui::Area::new(egui::Id::new("tab_actions_popup"))
            .fixed_pos(anchor_rect.left_bottom())
            .order(egui::Order::Foreground)
            .interactable(self.open_popup == Some(AnchoredPopup::TabActions))
            .show(ui.ctx(), |ui| {
                ui.set_opacity(fade);
                popup_frame(ui).show(ui, |ui| {
                    ui.set_min_width(140.0);
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
                        let response = ui.add(
                            egui::TextEdit::singleline(&mut self.find.text)
                                .desired_width(160.0)
                                .hint_text("Find in page"),
                        );
                        if take_focus(&mut self.pending_focus, FocusTarget::FindBar) {
                            response.request_focus();
                        }
                        if response.changed() {
                            text_changed = true;
                        }
                        if ui.toggle_value(&mut self.find.match_case, "Aa").on_hover_text("Match case").changed() {
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
                self.nodes.get(id.as_str()).and_then(|n| n.as_tab()).is_some_and(|n| {
                    query.is_empty() || n.title.to_lowercase().contains(&query) || n.url.to_lowercase().contains(&query)
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
                    egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                        for (i, id) in matches.iter().enumerate() {
                            let Some(node) = self.nodes.get(id).and_then(|n| n.as_tab()) else { continue };
                            let label = if node.title.is_empty() { node.url.clone() } else { node.title.clone() };
                            if is_live {
                                if ui.selectable_label(i == selected_index, label).clicked() {
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

        const SHORTCUTS: &[(&str, &str)] = &[
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
        ];

        let area = egui::Area::new(egui::Id::new("shortcuts_help"))
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                dialog_frame(ui).show(ui, |ui| {
                    ui.set_width(360.0);
                    ui.label(heading_text("Keyboard Shortcuts"));
                    ui.separator();
                    egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                        egui::Grid::new("shortcuts_help_grid").num_columns(2).spacing(egui::vec2(16.0, 6.0)).show(
                            ui,
                            |ui| {
                                for (keys, desc) in SHORTCUTS {
                                    ui.monospace(*keys);
                                    ui.label(*desc);
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

    /// Full-screen popup showing one node's JSON content as a real
    /// collapsible tree -- either a tab's own `TabPayload::json_body`
    /// (opened from Tab view's toolbar button or Graph view's tab context
    /// menu's "View as JSON") or a `JsonPayload::value` (opened by double-
    /// clicking a JSON node's card). Not tied to whichever view is
    /// currently active (see `json_viewer_node`'s own doc comment), so this
    /// renders every frame regardless of `view_mode`.
    pub(crate) fn build_json_viewer(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.json_viewer_node.clone() else { return };
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.json_viewer_node = None;
            return;
        }
        // A JSON node's own `value` is already parsed -- shown directly, no
        // fallback needed. A tab's `json_body` is raw text: the engine's
        // own `document.contentType` check already confirmed
        // `application/json` before this ever got set, but a server can lie
        // about its own header, so re-parsing here still needs a fallback.
        enum Source {
            Parsed(serde_json::Value),
            Raw(String),
        }
        let Some(source) = self.nodes.get(&id).and_then(|n| {
            if let Some(json) = n.as_json() {
                Some(Source::Parsed(json.value.clone()))
            } else {
                n.as_tab().and_then(|t| t.json_body.clone()).map(Source::Raw)
            }
        }) else {
            self.json_viewer_node = None;
            return;
        };
        // Only a tab's own live `json_body` is worth offering "Save as
        // Node" for -- a JSON node's own popup is already a saved node,
        // nothing to save. Cloned out before `source` is moved into the
        // render match below.
        let raw_body = match &source {
            Source::Raw(body) => Some(body.clone()),
            Source::Parsed(_) => None,
        };
        let mut close = false;
        let mut save_as_node = false;
        egui::Area::new(egui::Id::new("json_viewer"))
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                dialog_frame(ui).show(ui, |ui| {
                    ui.set_width(560.0);
                    ui.set_height(520.0);
                    ui.horizontal(|ui| {
                        ui.label(heading_text("JSON"));
                        // See `ipc_protocol::EngineToHost::
                        // RedirectableContent`'s own doc comment for
                        // why this is the *only* opt-in, user-
                        // triggered way a JSON tab ever becomes a
                        // node -- unlike image/etc., which redirect
                        // automatically, a JSON response is often a
                        // live, refetchable API call worth keeping as
                        // an ordinary tab.
                        if raw_body.is_some()
                            && ui
                                .button("Save as Node")
                                .on_hover_text(
                                    "Keep this JSON as a permanent graph node -- the tab keeps navigating normally",
                                )
                                .clicked()
                        {
                            save_as_node = true;
                        }
                        if ui.button("Close").clicked() {
                            close = true;
                        }
                    });
                    ui.separator();
                    egui::ScrollArea::vertical().show(ui, |ui| match source {
                        Source::Parsed(value) => draw_json_value(ui, "root", &value, true),
                        Source::Raw(body) => match serde_json::from_str::<serde_json::Value>(&body) {
                            Ok(value) => draw_json_value(ui, "root", &value, true),
                            Err(err) => {
                                ui.colored_label(DANGER_COLOR, format!("Invalid JSON: {err}"));
                                ui.add(egui::Label::new(&body).wrap());
                            }
                        },
                    });
                });
            });
        if save_as_node {
            if let Some(body) = raw_body {
                self.save_json_body_as_node(&body);
            }
        }
        if close {
            self.json_viewer_node = None;
        }
    }

    /// The JSON viewer popup's "Save as Node" button -- materializes the
    /// *currently showing* tab's own `json_body` as a real, permanent JSON
    /// node, without touching the tab itself (it keeps navigating/
    /// refreshing normally). Writes `body` to a real temp file first and
    /// reuses `insert_json_node_with_id` completely unchanged, same
    /// "downloaded bytes become a real file first" pattern `redirect_fetch`
    /// uses for the automatic kinds -- a node made this way is
    /// indistinguishable from one added via "+ JSON" from this point on.
    pub(crate) fn save_json_body_as_node(&mut self, body: &str) {
        let dir = std::env::temp_dir().join("manifold-redirects");
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let id = self.alloc_node_id();
        let path = dir.join(format!("{id}.bin"));
        if std::fs::write(&path, body).is_err() {
            return;
        }
        let pos = GRAPH_WORLD_CENTER + graph_layout::spawn_jitter(&id);
        let group = self.current_group.clone();
        if self.insert_json_node_with_id(id, path.to_string_lossy().into_owned(), pos, group).is_ok() {
            self.save_session();
            self.queue_toast("Saved as JSON node".to_string());
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
}
