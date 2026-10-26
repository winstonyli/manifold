//! The anchored toolbar dropdowns: Recently Closed, Bookmarks, History, Downloads, Tab actions, the engine pickers and the Back/Forward branch popup.

use super::*;

/// A Tab actions row: label, the key shown at its edge, and what it does.
type TabActionRow = (&'static str, &'static str, fn(&mut App));

/// Above this many matching entries, the History popup lays out only the
/// rows in view. Shorter lists keep the plain layout (and wrapping arrow keys).
const HISTORY_WINDOW_FROM: usize = 100;

/// Indexes into `history` of the entries `filter` matches, newest first.
pub(crate) fn history_matches(history: &[crate::history::HistoryEntry], filter: &str) -> Vec<usize> {
    (0..history.len()).rev().filter(|&i| filter_matches(filter, &[&history[i].title, &history[i].url])).collect()
}

/// A bookmark folder as committed from the folder field: the draft's clean form (name rules), or no folder when
/// nothing is left of it.
fn folder_from_draft(draft: &str) -> Option<String> {
    let clean = crate::names::clean_name(draft);
    (!clean.is_empty()).then(|| clean.into_owned())
}

/// A bookmark folder's name in the popup's small font and `color`, its hidden characters as placeholders (name
/// rules).
fn folder_job(ui: &egui::Ui, segs: &[crate::names::NameSeg], color: egui::Color32) -> egui::text::LayoutJob {
    let format = egui::TextFormat::simple(egui::TextStyle::Small.resolve(ui.style()), color);
    let mut job = egui::text::LayoutJob::default();
    crate::names::append_name(&mut job, segs, &format, ui.visuals().weak_text_color());
    job
}

/// One Recently Closed row: the closed root, its stored form, its kept title, a group's member count and, for a group
/// an ungroup or merge emptied, how many members it holds.
struct ClosedRow {
    id: store::model::NodeId,
    json: store::json::JsonNode,
    title: Option<String>,
    members: Option<usize>,
    held: Option<usize>,
}

/// A closed group's member count as its row shows it, after the name.
fn member_count_text(members: usize) -> String {
    format!("({members})")
}

/// Whether the branch popup should scroll its cursor page into view this frame: the first frame it is open, not every
/// frame (that would pull the view back whenever the user scrolls away). Closing re-arms it.
pub(crate) fn popup_centres_cursor(ctx: &egui::Context, which: AnchoredPopup, open: bool) -> bool {
    let key = egui::Id::new(("trail_popup_centred", which));
    let seen = ctx.data_mut(|d| {
        let seen = open && d.get_temp::<bool>(key).unwrap_or(false);
        d.insert_temp(key, open);
        seen
    });
    open && !seen
}

/// Canvas room around the trail's cards, and the popup viewport's least and most size.
const POPUP_PAD: f32 = 8.0;

const POPUP_MIN: egui::Vec2 = egui::vec2(160.0, 60.0);

const POPUP_MAX: egui::Vec2 = egui::vec2(480.0, 240.0);

/// The popup camera's rect on opening: `viewport` big, centred on the whole tree when it fits, else on the cursor page
/// (the tree's middle if there is none).
pub(crate) fn popup_start_rect(bounds: egui::Rect, cursor: Option<egui::Rect>, viewport: egui::Vec2) -> egui::Rect {
    let fits = bounds.width() <= viewport.x && bounds.height() <= viewport.y;
    let centre = if fits { bounds.center() } else { cursor.map_or(bounds.center(), |c| c.center()) };
    egui::Rect::from_center_size(centre, viewport)
}

impl App {
    /// One of the three engine pickers (Settings' default and current-tab
    /// engines, the Tab toolbar's Split): the tier/pros/cons cards of
    /// `build_view_picker_popup` below `anchor`, with `highlighted`
    /// marked. Closes on a pick or a click outside, and returns the engine
    /// picked; what picking does is the caller's.
    pub(crate) fn build_anchored_view_picker(
        &mut self,
        ui: &egui::Ui,
        which: AnchoredPopup,
        anchor: Option<egui::Rect>,
        highlighted: Option<ViewKind>,
    ) -> Option<ViewKind> {
        let (rect, fade) = self.anchored_popup_fade(ui.ctx(), which, anchor)?;
        let open = self.menus.open_popup == Some(which);
        let states: Vec<(ViewKind, crate::view_install::ViewStatus)> =
            ViewKind::picker_order().into_iter().map(|kind| (kind, self.view_status(kind))).collect();
        let state_of = |kind: ViewKind| {
            states
                .iter()
                .find(|(k, _)| *k == kind)
                .map(|(_, s)| s.clone())
                .unwrap_or(crate::view_install::ViewStatus::Installed)
        };
        let (area_rect, picked) =
            build_view_picker_popup(ui.ctx(), &format!("{which:?}_popup"), rect, highlighted, &state_of, fade, open);
        let escape = open && ui.input(|i| i.key_pressed(egui::Key::Escape));
        let outcome = picked.map(|kind| pick_outcome(kind, &state_of(kind)));
        // A click on an unavailable engine only explains: the picker stays open.
        let explains_only = matches!(outcome, Some(PickOutcome::Unavailable(..)));
        if (picked.is_some() && !explains_only) || escape || clicked_outside_popup(ui, area_rect, Some(rect)) {
            close_popup(&mut self.menus.open_popup, which);
        }
        // An engine that isn't installed stays selectable, but choosing it asks to install it: no
        // silent switch, and no silent fallback to another engine.
        match outcome? {
            PickOutcome::Switch(kind) => Some(kind),
            PickOutcome::AskInstall(kind) => {
                self.offer_confirm(Confirm::View(ViewConfirm::Download(kind)));
                None
            }
            PickOutcome::AlreadyInstalling(kind) => {
                self.queue_toast(format!("{} is already installing", view_label(kind)));
                None
            }
            PickOutcome::Unavailable(kind, reason) => {
                self.queue_toast(PickOutcome::unavailable_toast(kind, &reason));
                None
            }
        }
    }

    /// The "Recently Closed" dropdown's row for one entry: an icon (its kind's
    /// table glyph, `Caps::glyph`, or a tab's favicon; drawn
    /// into `ui` as a side effect, same as the row's other icons, and for a
    /// group nothing more: `build_recently_closed_popup` draws its member
    /// count after the text), the text, and its hover -- `name` for a
    /// group, `title` (its recorded title) or else `url` for a tab, the
    /// first non-blank line of `text` for a note, `path` for every
    /// file-backed kind. A group's row and a note's row are styled jobs by
    /// the display rule (name rules: a hidden character as a placeholder,
    /// "Untitled" when nothing is left, so a note with no text, or only
    /// blank lines, shows "Untitled"), with the hidden-character hint as
    /// the hover; other rows have none.
    fn closed_entry_label(
        ui: &mut egui::Ui,
        gpu: Option<&GpuState>,
        node: &store::json::JsonNode,
        title: Option<&str>,
    ) -> (egui::WidgetText, Option<String>) {
        let str_field = |name: &str| node.fields.get(name).and_then(|v| v.as_str()).unwrap_or_default().to_string();
        let kind = node.kind();
        // The kind's table glyph (`Caps::glyph`); a tab paints its favicon below, and a type this build doesn't know
        // reads the default.
        if kind != Some(Kind::Tab) {
            ui.label(kind_caps(kind.unwrap_or(Kind::Unknown)).glyph);
        }
        if kind == Some(Kind::Group) {
            let name = str_field("name");
            let segs = crate::names::display_name(&name, crate::names::UNTITLED);
            return (crate::names::name_job(ui, "", &segs, "").into(), crate::names::hidden_hint(&segs));
        }
        if kind == Some(Kind::Note) {
            // The note's text is never cleaned; its first non-blank line is shown by the display rule.
            let text = str_field("text");
            let first = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default();
            let segs = crate::names::display_name(first, crate::names::UNTITLED);
            return (crate::names::name_job(ui, "", &segs, "").into(), crate::names::hidden_hint(&segs));
        }
        // A type this build doesn't know (`JsonNode::kind` is `None`): its stored name, else its type string, as its
        // card's header shows them.
        if kind.is_none() {
            let name = str_field("name");
            if name.is_empty() {
                return (str_field("type").into(), None);
            }
            let segs = crate::names::display_name(&name, crate::names::UNTITLED);
            return (crate::names::name_job(ui, "", &segs, "").into(), crate::names::hidden_hint(&segs));
        }
        let text: String = match kind {
            Some(Kind::Tab) => {
                let url = str_field("url");
                let kind = ViewKind::from_name(&str_field("view_kind")).unwrap_or(ViewKind::Servo);
                let (slot, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                let identity = Identity::of_tab(false, App::favicon_in(gpu, &url), kind, &url);
                paint_row_icon(ui, slot, 0.0, "", false, identity);
                title.map_or(url, str::to_string)
            }
            // Image, Pdf, Json, a jar, a download: the stored path.
            _ => str_field("path"),
        };
        (text.into(), None)
    }

    /// The "Recently Closed" toolbar dropdown (Graph view, at any level):
    /// every entry, newest first; click one to reopen it where it was.
    pub(crate) fn build_recently_closed_popup(&mut self, ui: &mut egui::Ui, anchor_rect: egui::Rect, fade: f32) {
        let mut close = false;
        // Every closed root, any replica's, newest first (spec "Recently Closed rows"): label from its fields, title
        // from its kept personal state, a group's member count from its non-deleted descendants.
        let rows: Vec<ClosedRow> = self
            .doc
            .journal
            .as_ref()
            .map(|j| {
                let doc = j.doc();
                doc.closed_roots()
                    .into_iter()
                    .filter_map(|(id, _)| {
                        let node = doc.nodes.get(&id)?;
                        let json = store::json::node_json(
                            &id.to_string(),
                            node.ty(),
                            &session_sync::fields_of(node),
                            &store::tree::Trees::new(), // the label reads no props
                            None,
                        );
                        let title = self.doc.hidden_local.get(&id).map(|l| l.title.clone()).filter(|t| !t.is_empty());
                        let members = (node.kind() == Kind::Group).then(|| {
                            let under = doc.with_descendants(std::collections::BTreeSet::from([id]));
                            under.iter().filter(|d| **d != id && doc.nodes.get(d).is_some_and(|n| !n.deleted)).count()
                        });
                        // A group an ungroup or merge emptied: how many of the members it holds still exist.
                        let held =
                            (node.kind() == Kind::Group).then(|| node.get(store::model::field::HELD)).and_then(|v| {
                                let store::model::FieldValue::Held { members, .. } = v else { return None };
                                let alive =
                                    members.iter().filter(|m| doc.nodes.get(m).is_some_and(|n| !n.deleted)).count();
                                (!members.is_empty()).then_some(alive)
                            });
                        Some(ClosedRow { id, json, title, members, held })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut reopen: Option<store::model::NodeId> = None;
        let clicked_outside = list_popup(
            ui,
            self.menus.open_popup,
            AnchoredPopup::RecentlyClosed,
            "recently_closed_popup",
            anchor_rect,
            fade,
            |ui| {
                for ClosedRow { id, json: root, title, members, held } in &rows {
                    ui.horizontal(|ui| {
                        let mut row_ui = |ui: &mut egui::Ui| {
                            let (label, hint) = Self::closed_entry_label(ui, self.gpu.as_ref(), root, title.as_deref());
                            let mut row = list_row(ui, label);
                            if let Some(hint) = hint {
                                row = row.on_hover_text(hint);
                            }
                            if let Some(held) = held {
                                row = row.on_hover_text(format!("Emptied: reopening puts its {held} members back"));
                            }
                            if row.clicked() {
                                reopen = Some(*id);
                            }
                        };
                        if let Some(count) = members {
                            // The count is laid out first, at the right, so a name cut short keeps it.
                            let count =
                                if held.is_some() { "(emptied)".to_string() } else { member_count_text(*count) };
                            egui::Sides::new().shrink_left().truncate().show(ui, row_ui, |ui| ui.weak(count));
                        } else {
                            row_ui(ui);
                        }
                    });
                }
            },
        );

        if let Some(id) = reopen {
            self.reopen_node(id);
            close = true;
        }
        if clicked_outside {
            close = true;
        }
        if close || rows.is_empty() {
            close_popup(&mut self.menus.open_popup, AnchoredPopup::RecentlyClosed);
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
        let mut wanted: Vec<String> = Vec::new();
        let clicked_outside = list_popup(
            ui,
            self.menus.open_popup,
            AnchoredPopup::Bookmarks,
            "bookmarks_popup",
            anchor_rect,
            fade,
            |ui| {
                toolbar_field(ui, |ui| {
                    ui.add(
                        field_edit(&mut self.history_ui.bookmarks_filter)
                            .desired_width(f32::INFINITY)
                            .hint_text("Filter..."),
                    )
                });
                let matches: Vec<(usize, &bookmarks::Bookmark)> = self
                    .bookmarks
                    .iter()
                    .enumerate()
                    .filter(|(_, b)| filter_matches(&self.history_ui.bookmarks_filter, &[&b.title, &b.url]))
                    .collect();
                let mut folders: Vec<&str> = matches.iter().filter_map(|(_, b)| b.folder.as_deref()).collect();
                folders.sort_unstable();
                folders.dedup();
                // `None` (unfoldered) first, then each named folder
                // alphabetically -- `folders` is already sorted.
                let groups: Vec<Option<&str>> = std::iter::once(None).chain(folders.into_iter().map(Some)).collect();
                if matches.is_empty() {
                    ui.label(
                        egui::RichText::new("No bookmarks yet. Press the star in the toolbar on a page to add one.")
                            .weak(),
                    );
                }
                egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                    for folder in groups {
                        let rows: Vec<&(usize, &bookmarks::Bookmark)> =
                            matches.iter().filter(|(_, b)| b.folder.as_deref() == folder).collect();
                        if rows.is_empty() {
                            continue;
                        }
                        if let Some(name) = folder {
                            let segs = crate::names::display_name(name, crate::names::UNTITLED);
                            // One line, cut with "…" at the popup's width (egui's hover then shows it all).
                            let header =
                                ui.add(egui::Label::new(folder_job(ui, &segs, palette(ui.visuals()).text)).truncate());
                            if let Some(hint) = crate::names::hidden_hint(&segs) {
                                header.on_hover_text(hint);
                            }
                        }
                        for (index, bookmark) in rows {
                            let (index, bookmark) = (*index, *bookmark);
                            ui.horizontal(|ui| {
                                let identity = App::page_row_identity(self.gpu.as_ref(), &bookmark.url);
                                wanted.push(bookmark.url.clone());
                                let row = list_row(ui, row_label(ui, &bookmark.title, None, LIST_ROW_INSET));
                                paint_row_icon(
                                    ui,
                                    row.rect,
                                    LIST_ROW_INSET,
                                    &bookmark.title,
                                    row.has_focus(),
                                    identity,
                                );
                                if row.clicked() {
                                    navigate_url = Some(bookmark.url.clone());
                                }
                                if ui.small_button(egui_phosphor::light::X).named_tip("Remove bookmark").clicked() {
                                    remove_index = Some(index);
                                }
                            });
                            let is_renaming = self
                                .history_ui
                                .renaming_bookmark_folder
                                .as_ref()
                                .is_some_and(|(url, _)| url == &bookmark.url);
                            if is_renaming {
                                if let Some((_, draft)) = &mut self.history_ui.renaming_bookmark_folder {
                                    let response = toolbar_field(ui, |ui| {
                                        ui.add(
                                            field_edit(draft)
                                                .desired_width(140.0)
                                                .hint_text("Folder (blank for none)")
                                                .font(egui::TextStyle::Small),
                                        )
                                    });
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
                                let segs = match &bookmark.folder {
                                    Some(name) => crate::names::display_name(name, crate::names::UNTITLED),
                                    None => vec![crate::names::NameSeg::Plain("No folder")],
                                };
                                let job = folder_job(ui, &segs, ui.visuals().weak_text_color());
                                if ui
                                    .add(egui::Label::new(job).truncate().sense(egui::Sense::click()))
                                    .on_hover_text(crate::names::with_hint("Click to set folder", &segs))
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
            },
        );

        self.request_favicons(wanted.iter().map(String::as_str));

        if let Some((url, draft)) = folder_rename_start {
            self.history_ui.renaming_bookmark_folder = Some((url, draft));
            self.pending_focus = Some(FocusTarget::BookmarkFolder);
        }
        if folder_rename_cancel {
            self.history_ui.renaming_bookmark_folder = None;
        }
        if let Some(draft) = folder_rename_commit {
            if let Some((url, _)) = self.history_ui.renaming_bookmark_folder.take() {
                if let Some(bookmark) = self.bookmarks.iter_mut().find(|b| b.url == url) {
                    bookmark.folder = folder_from_draft(&draft);
                    bookmarks::save(&self.bookmarks);
                }
            }
        }
        if let Some(url) = navigate_url {
            if let Some(id) = self.focused {
                self.navigate_tab(id, url);
            }
            close = true;
        }
        if let Some(index) = remove_index {
            self.bookmarks.remove(index);
            bookmarks::save(&self.bookmarks);
        }
        if clicked_outside && self.history_ui.renaming_bookmark_folder.is_none() {
            close = true;
        }
        // Not `|| self.bookmarks.is_empty()` any more -- the Settings page's
        // own "Lists" section can open this popup with an empty list, and
        // that used to close it again the very next frame.
        if close {
            close_popup(&mut self.menus.open_popup, AnchoredPopup::Bookmarks);
            self.history_ui.renaming_bookmark_folder = None;
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
        let wanted = std::cell::RefCell::new(Vec::<String>::new());
        let clicked_outside =
            list_popup(ui, self.menus.open_popup, AnchoredPopup::History, "history_popup", anchor_rect, fade, |ui| {
                toolbar_field(ui, |ui| {
                    ui.add(
                        field_edit(&mut self.history_ui.history_filter)
                            .desired_width(f32::INFINITY)
                            .hint_text("Filter..."),
                    )
                });
                if self.history.is_empty() {
                    ui.label(egui::RichText::new("No history yet. Pages you visit appear here.").weak());
                }
                let key_ok = matches!(&self.history_ui.history_matches_cache,
                    Some((f, n, _)) if *f == self.history_ui.history_filter && *n == self.history.len());
                if !key_ok {
                    let matches = history_matches(&self.history, &self.history_ui.history_filter);
                    self.history_ui.history_matches_cache =
                        Some((self.history_ui.history_filter.clone(), self.history.len(), matches));
                }
                let matches = self.history_ui.history_matches_cache.as_ref().map_or(&[][..], |(_, _, m)| m.as_slice());
                let row = |ui: &mut egui::Ui, i: usize, navigate_url: &mut Option<String>| {
                    let entry = &self.history[i];
                    let label = if entry.title.is_empty() { &entry.url } else { &entry.title };
                    // Keyed by the entry, not by position: a windowed list starts at a different row each
                    // scroll, and positional ids would move keyboard focus to another entry. An explicit id,
                    // not `push_id`: a child's id also mixes in the parent's running widget count, which is
                    // positional. (An entry evicted at `MAX_HISTORY_ENTRIES` while the popup is open shifts
                    // every index by one, so focus slips to the neighbouring entry for that frame; rare
                    // enough to leave.)
                    let id = egui::Id::new(("history_row", i));
                    wanted.borrow_mut().push(entry.url.clone());
                    let widget_label = row_label(ui, label, None, LIST_ROW_INSET);
                    let row = ui.scope_builder(egui::UiBuilder::new().id(id), |ui| list_row(ui, widget_label)).inner;
                    let identity = App::page_row_identity(self.gpu.as_ref(), &entry.url);
                    paint_row_icon(ui, row.rect, LIST_ROW_INSET, label, row.has_focus(), identity);
                    if row.clicked() {
                        *navigate_url = Some(entry.url.clone());
                    }
                };
                let scroll = egui::ScrollArea::vertical().max_height(280.0);
                if matches.len() < HISTORY_WINDOW_FROM {
                    scroll.show(ui, |ui| matches.iter().for_each(|&i| row(ui, i, &mut navigate_url)));
                } else {
                    // The height a row really lays out at: the popup's own font (`menu_item_style` overrides
                    // the Body style), so the window is placed where the rows actually are.
                    let font_id = egui::FontSelection::Default.resolve(ui.style());
                    let spacing = ui.spacing().item_spacing.y;
                    let pitch = ui.ctx().fonts_mut(|f| f.row_height(&font_id)) + spacing;
                    scroll.show_viewport(ui, |ui, viewport| {
                        ui.set_height((pitch * matches.len() as f32 - spacing).max(0.0));
                        // `ScrollArea::show_rows`, but with one row of overlap above the view as well as
                        // below: `show_rows` leaves out the row just above a view whose top edge falls on a
                        // row boundary, and Up from the first laid-out row would then have nowhere to go.
                        // The extra row takes the focus, and `list_row`'s `scroll_to_me` brings it in.
                        let end = ((viewport.max.y / pitch).ceil() as usize + 1).min(matches.len());
                        let first = ((viewport.min.y / pitch).floor() as usize).saturating_sub(1).min(end);
                        let top = ui.max_rect().top();
                        let rect = egui::Rect::from_x_y_ranges(
                            ui.max_rect().x_range(),
                            top + first as f32 * pitch..=top + end as f32 * pitch,
                        );
                        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                            matches[first..end].iter().for_each(|&i| row(ui, i, &mut navigate_url));
                        });
                    });
                    mark_list_partial(ui);
                }
                menu_separator(ui);
                if ui.button("Clear").clicked() {
                    clear_requested = true;
                }
            });

        self.request_favicons(wanted.borrow().iter().map(String::as_str));

        if let Some(url) = navigate_url {
            if let Some(id) = self.focused {
                self.navigate_tab(id, url);
            }
            close = true;
        }
        if clear_requested {
            self.history.clear();
            self.history_ui.history_dirty = true;
            self.history_ui.history_matches_cache = None;
            close = true;
        }
        if clicked_outside {
            close = true;
        }
        // Not `|| self.history.is_empty()` any more -- see the matching
        // comment in `build_bookmarks_popup`.
        if close {
            close_popup(&mut self.menus.open_popup, AnchoredPopup::History);
        }
    }

    /// Tab view's "Downloads (N)" dropdown -- every entry in
    /// `self.downloads` (the same data the graph view's own download
    /// nodes/context menu show, see `App::node_menu`), most-
    /// recently-downloaded first, click a filename to open its containing
    /// folder (same as `reveal_node`), "x" to remove it from the list
    /// (same as `close_node`). Exists so a completed download can
    /// be found without first switching to Graph view.
    pub(crate) fn build_downloads_popup(&mut self, ui: &mut egui::Ui, anchor_rect: egui::Rect, fade: f32) {
        let mut close = false;
        let mut reveal_id: Option<NodeId> = None;
        let mut remove_id: Option<NodeId> = None;
        let clicked_outside = list_popup(
            ui,
            self.menus.open_popup,
            AnchoredPopup::Downloads,
            "downloads_popup",
            anchor_rect,
            fade,
            |ui| {
                if !self.node_order.iter().any(|id| self.nodes.get(id).is_some_and(|n| n.as_download().is_some())) {
                    ui.label(egui::RichText::new("No downloads yet. Files you download appear here.").weak());
                }
                egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                    for id in self.node_order.iter().rev() {
                        let Some(node) = self.nodes.get(id).and_then(|n| n.as_download()) else { continue };
                        ui.horizontal(|ui| {
                            let filename = download_filename(&node.path);
                            if list_row(ui, filename).clicked() {
                                reveal_id = Some(*id);
                            }
                            if ui.small_button(egui_phosphor::light::X).named_tip("Remove from list").clicked() {
                                remove_id = Some(*id);
                            }
                        });
                        // In-progress downloads get a live progress row below
                        // the filename; a completed one is left exactly as
                        // before this feature -- just the filename row above.
                        if node.failed {
                            ui.colored_label(danger(ui.visuals()), "Failed");
                        } else if !node.completed {
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
            },
        );

        if let Some(id) = reveal_id {
            self.reveal_node(id);
            close = true;
        }
        if let Some(id) = remove_id {
            self.close_node(id);
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
            close_popup(&mut self.menus.open_popup, AnchoredPopup::Downloads);
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
        focused_id: NodeId,
        fade: f32,
    ) {
        let mut close = false;
        let mut mute_others = false;
        let mut close_others = false;
        let mut toggle_pin_all = false;
        // The focused tab's own actions, each with its shortcut shown at the row's edge.
        let mut tab_action: Option<fn(&mut App)> = None;
        let focused_pinned = self.doc.mirror.pinned(focused_id);
        let tab_pin_states: Vec<bool> = self
            .nodes
            .iter()
            .filter(|(_, n)| n.runtime.saved_tab().is_some())
            .map(|(id, _)| self.doc.mirror.pinned(*id))
            .collect();
        let all_pinned = !tab_pin_states.is_empty() && tab_pin_states.iter().all(|&p| p);
        let area = egui::Area::new(egui::Id::new("tab_actions_popup"))
            .fixed_pos(anchor_rect.left_bottom())
            .interactable(self.menus.open_popup == Some(AnchoredPopup::TabActions));
        let area = menu_popup(ui.ctx(), area, fade, |ui| {
            ui.set_min_width(200.0);
            let rows: [TabActionRow; 5] = [
                (if focused_pinned { "Unpin" } else { "Pin" }, "Ctrl+P", App::pin_focused_tab_shortcut),
                ("Duplicate", "Ctrl+D", App::duplicate_focused_tab_shortcut),
                ("Find in Page", "Ctrl+F", App::open_find_bar_shortcut),
                ("Developer Tools", "F12", App::open_devtools_shortcut),
                ("Close Tab", "Ctrl+W", App::close_focused_tab_shortcut),
            ];
            for (label, keys, action) in rows {
                if ui
                    .add(egui::Button::new(label).shortcut_text(keys).min_size(egui::vec2(ui.min_rect().width(), 0.0)))
                    .clicked()
                {
                    tab_action = Some(action);
                    close = true;
                }
            }
            menu_separator(ui);
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

        let clicked_outside = clicked_outside_popup(ui, area.rect, Some(anchor_rect));

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
            close_popup(&mut self.menus.open_popup, AnchoredPopup::TabActions);
        }
    }

    /// The Back or Forward chip's branch list (trail spec decision 8): the strip's own page cards, edges and dot grid
    /// in a pannable, zoomable `Scene` (`2026-10-16-trail-popup-canvas-design.md`), recentred on every opening. A click
    /// on a page is a plain move there (`trail_pick`) and closes the popup, as does a click outside.
    pub(crate) fn build_trail_branches_popup(
        &mut self,
        ui: &mut egui::Ui,
        dir: Dir,
        anchor_rect: egui::Rect,
        tab: NodeId,
        fade: f32,
    ) {
        let which = match dir {
            Dir::Back => AnchoredPopup::BackBranches,
            Dir::Forward => AnchoredPopup::ForwardBranches,
        };
        let cards = crate::trail_strip::trail_cards(self, tab, egui::Pos2::ZERO);
        if cards.is_empty() {
            return;
        }
        let edges = crate::trail_strip::trail_edges(self, tab, &|id| cards.iter().find(|e| e.id == id).map(|e| e.rect));
        let bounds = cards.iter().fold(egui::Rect::NOTHING, |b, e| b.union(e.rect));
        let view = (bounds.size() + egui::Vec2::splat(2.0 * POPUP_PAD)).clamp(POPUP_MIN, POPUP_MAX);
        let cursor = self
            .nav_history
            .get(&tab)
            .and_then(|h| h.tree().iter().position(|p| p.cursor))
            .and_then(|i| cards.get(i))
            .map(|e| e.rect);
        let mut camera = match self.menus.trail_canvas.take() {
            Some((w, camera)) if w == which => camera,
            _ => crate::graph_ui::GraphCamera::new(),
        };
        // An eviction renumbers the pages under an open popup: the old framing means nothing, so recentre.
        let epoch = self.nav_history.get(&tab).map_or(0, |h| h.epoch());
        let epoch_key = egui::Id::new(("trail_popup_epoch", which));
        let seen_epoch = ui.ctx().data_mut(|d| {
            let seen = d.get_temp::<u32>(epoch_key);
            d.insert_temp(epoch_key, epoch);
            seen
        });
        let renumbered = seen_epoch.is_some_and(|seen| seen != epoch);
        let opened = popup_centres_cursor(ui.ctx(), which, self.menus.open_popup == Some(which));
        if opened || renumbered {
            camera.jump_to(popup_start_rect(bounds, cursor, view));
        }
        let order: Vec<CardId> = cards.iter().map(|e| e.id).collect();
        let mut clicked = false;
        let menu_was_open = self.menus.trail_menu.is_some();
        let area = egui::Area::new(egui::Id::new(("trail_branches_popup", which)))
            .fixed_pos(anchor_rect.left_bottom())
            .order(egui::Order::Foreground)
            .interactable(self.menus.open_popup == Some(which))
            .show(ui.ctx(), |ui| {
                ui.set_opacity(fade);
                popup_frame(ui).show(ui, |ui| {
                    let (outer, _) = ui.allocate_exact_size(view, egui::Sense::hover());
                    ui.painter().rect_filled(outer, 4.0, ui.visuals().panel_fill);
                    let (zoom_scale, _) = camera.begin_frame(ui.ctx(), outer);
                    let inv_zoom = 1.0 / zoom_scale;
                    let viewport = camera.rect;
                    let weak = ui.visuals().weak_text_color();
                    let mut rect = camera.rect;
                    let mut overlays: crate::graph_ui::DeferredOverlays = DeferredOverlays::default();
                    let mut actions = Vec::new();
                    let members = std::collections::HashMap::new();
                    let mut pass = crate::card::CardPass {
                        deferred_overlays: &mut overlays,
                        actions: &mut actions,
                        inv_zoom,
                        zoom_scale,
                        drop_preview: &None,
                        pick_targets: &[],
                        connect_target: None,
                        connect_ring: None,
                        show_order: false,
                        order: &order,
                        any_focused: false,
                        members: &members,
                        viewport,
                        pan_target: None,
                        carry: false,
                    };
                    ui.scope_builder(egui::UiBuilder::new().max_rect(outer), |ui| {
                        crate::graph_ui::show_scene(ui, &mut rect, true, |ui| {
                            crate::graph_ui::paint_dot_grid(ui.painter(), viewport, zoom_scale, inv_zoom, weak);
                            crate::graph_ui::paint_edges(ui, viewport, &edges, inv_zoom);
                            pass.pan_target = crate::graph_ui::card_tab_step(ui, &order, |id| {
                                cards.iter().find(|e| e.id == id).map(|e| e.rect)
                            });
                            for entry in &cards {
                                if let Some(response) = self.virtual_card_click(ui, &mut pass, entry, "trail", false) {
                                    clicked |= response.clicked();
                                    if response.secondary_clicked() {
                                        let at =
                                            ui.input(|i| i.pointer.interact_pos()).unwrap_or(response.rect.center());
                                        if let Some(source) = self.virtual_source("trail") {
                                            source.on_menu(self, entry, at);
                                        }
                                    }
                                }
                            }
                        });
                    });
                    camera.rect = rect;
                    if let Some(target) = pass.pan_target {
                        camera.pan_to(target);
                    }
                    // The pages' labels and icon letters, at screen size over the scene, clipped to the popup.
                    let (zoom_scale, translation) = camera.transform(outer);
                    let project = move |world: egui::Pos2| (translation + zoom_scale * world.to_vec2()).to_pos2();
                    let overlay_painter = ui.painter().with_clip_rect(outer);
                    overlays.play(&overlay_painter, &project);
                    debug_assert!(actions.is_empty(), "a trail page's click returns no GraphAction");
                });
            });
        self.menus.trail_canvas = Some((which, camera));
        // A fading or closed popup leaves no hover behind: its page ids are the strip's too.
        if self.menus.open_popup != Some(which) && self.interaction.hovered.is_some_and(crate::trail_strip::is_page_id)
        {
            self.interaction.hovered = None;
        }
        // A click that dismisses the page menu from outside is the menu's, not the popup's.
        let clicked_outside = clicked_outside_popup(ui, area.response.rect, Some(anchor_rect)) && !menu_was_open;
        if clicked || clicked_outside {
            close_popup(&mut self.menus.open_popup, which);
        }
        self.build_trail_menu(ui);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_matches_lists_filtered_entries_newest_first() {
        let entry = |title: &str, url: &str| crate::history::HistoryEntry { title: title.into(), url: url.into() };
        let history = vec![entry("Rust", "https://a"), entry("", "https://rust.example"), entry("Go", "https://b")];
        assert_eq!(history_matches(&history, ""), vec![2, 1, 0]);
        assert_eq!(history_matches(&history, "rust"), vec![1, 0]);
    }

    /// Name rules: a bookmark folder is committed under its clean name; nothing left of it means no folder.
    #[test]
    fn a_bookmark_folder_commits_its_clean_name() {
        assert_eq!(folder_from_draft("  Re\u{200B}ading\u{00A0}\u{00A0}list\n").as_deref(), Some("Reading list"));
        assert_eq!(folder_from_draft(" \u{200B}\u{202E} "), None);
        assert_eq!(folder_from_draft(""), None);
    }

    /// Name rules: a Recently Closed group row draws a hidden character in its name as a placeholder and gives the
    /// hidden-character hint for its hover; its member count reads "(n)" (`member_count_text`).
    #[test]
    fn a_closed_group_row_shows_a_hidden_character_as_a_placeholder() {
        let ctx = egui::Context::default();
        let mut fields = serde_json::Map::new();
        fields.insert("name".into(), "Re\u{200B}ading".into());
        fields.insert("type".into(), store::model::TYPE_GROUP.into());
        let node = store::json::JsonNode { id: "g".into(), fields };
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let (label, hint) = App::closed_entry_label(ui, None, &node, None);
            assert_eq!(label.text(), "Re⟦200B⟧ading");
            assert_eq!(hint.as_deref(), Some("Hidden characters: U+200B zero width space"));
        });
        output.textures_delta.clear();
        assert_eq!(member_count_text(3), "(3)");
    }

    /// A Recently Closed tab row has no name rule, so no hint.
    #[test]
    fn a_closed_tab_row_has_no_hint() {
        let ctx = egui::Context::default();
        let mut fields = serde_json::Map::new();
        fields.insert("url".into(), "https://example.org/".into());
        fields.insert("type".into(), store::model::TYPE_TAB.into());
        let node = store::json::JsonNode { id: "t".into(), fields };
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let (label, hint) = App::closed_entry_label(ui, None, &node, None);
            assert_eq!((label.text(), hint), ("https://example.org/", None));
        });
        output.textures_delta.clear();
    }

    /// The icon a Recently Closed row paints, for each kind (spec Decision 1): the table's glyph, one icon only.
    #[test]
    fn a_closed_row_paints_its_kinds_table_glyph() {
        let painted = |ty: &str, extra: &[(&str, &str)]| -> String {
            let ctx = egui::Context::default();
            let mut fields = serde_json::Map::new();
            fields.insert("type".into(), ty.into());
            for (k, v) in extra {
                fields.insert((*k).into(), (*v).into());
            }
            let node = store::json::JsonNode { id: "n".into(), fields };
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                App::closed_entry_label(ui, None, &node, None);
            });
            output.textures_delta.clear();
            output
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Text(t) => Some(t.galley.text().to_owned()),
                    _ => None,
                })
                .collect()
        };
        use egui_phosphor::light as i;
        assert_eq!(painted(store::model::TYPE_GROUP, &[("name", "G")]), i::FOLDER);
        assert_eq!(painted(store::model::TYPE_DOWNLOAD, &[("path", "f")]), i::DOWNLOAD_SIMPLE);
        assert_eq!(painted(store::model::TYPE_JAR, &[("path", "f")]), i::COOKIE);
        assert_eq!(painted(store::model::TYPE_PDF, &[("path", "f")]), i::FILE_PDF);
        assert_eq!(painted("application/x-from-the-future", &[]), i::QUESTION);
    }

    /// Name rules: a Recently Closed note row shows the note's first non-blank line by the display rule.
    #[test]
    fn a_closed_note_row_shows_its_first_line_by_the_display_rule() {
        let ctx = egui::Context::default();
        let mut fields = serde_json::Map::new();
        fields.insert("text".into(), "\n\tRe\u{200B}ading\u{00A0}\u{00A0}list \nbody".into());
        fields.insert("type".into(), store::model::TYPE_NOTE.into());
        let node = store::json::JsonNode { id: "n".into(), fields };
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let (label, hint) = App::closed_entry_label(ui, None, &node, None);
            assert_eq!(label.text(), "Re⟦200B⟧ading list");
            assert_eq!(hint.as_deref(), Some("Hidden characters: U+200B zero width space"));
        });
        output.textures_delta.clear();
    }

    /// Name rules: a long bookmark folder name is one line cut with "…" at the popup's width, not wrapped, both as
    /// a header and as the "click to set folder" label under a bookmark.
    #[test]
    fn a_long_bookmark_folder_header_is_one_cut_line() {
        let ctx = crate::theme::testutil::ctx(true);
        let mut app = App::default();
        let folder = format!("w{}", " Wardrobe".repeat(20));
        let bookmark = |title: &str| bookmarks::Bookmark {
            title: title.into(),
            url: format!("https://example.org/{title}"),
            folder: Some(folder.clone()),
        };
        app.bookmarks = vec![bookmark("a")];
        app.menus.open_popup = Some(AnchoredPopup::Bookmarks);
        let anchor = egui::Rect::from_min_size(egui::pos2(100.0, 40.0), egui::vec2(20.0, 20.0));
        let mut galleys = Vec::new();
        for _ in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0))),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| app.build_bookmarks_popup(ui, anchor, 1.0));
            output.textures_delta.clear();
            galleys = output
                .shapes
                .into_iter()
                .filter_map(|s| match s.shape {
                    egui::Shape::Text(t) if t.galley.job.text.starts_with(&folder[..9]) => Some(t.galley),
                    _ => None,
                })
                .collect();
        }
        let weak = ctx.global_style().visuals.weak_text_color();
        for (what, color) in [("header", palette(&ctx.global_style().visuals).text), ("label", weak)] {
            let galley = galleys.iter().find(|g| g.job.sections[0].format.color == color);
            let galley = galley.unwrap_or_else(|| panic!("the folder {what} is drawn"));
            assert_eq!(galley.rows.len(), 1, "{what}: one line: {:?}", galley.text());
            assert!(
                galley.elided || galley.job.text.ends_with('…'),
                "{what}: cut with an ellipsis: {:?}",
                galley.text()
            );
        }
    }

    /// Name rules: a bookmark folder's name is drawn in the popup's small font, a hidden character as a placeholder.
    #[test]
    fn a_bookmark_folder_shows_a_hidden_character_as_a_placeholder() {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let segs = crate::names::display_name("Re\u{200B}ading", crate::names::UNTITLED);
            let job = folder_job(ui, &segs, crate::theme::palette(&egui::Visuals::dark()).text);
            assert_eq!(job.text, "Re⟦200B⟧ading");
            assert_eq!(job.sections[0].format.font_id, egui::TextStyle::Small.resolve(ui.style()));
            assert_eq!(job.sections[0].format.color, crate::theme::palette(&egui::Visuals::dark()).text);
            assert_eq!(job.sections[1].format.font_id.family, egui::FontFamily::Monospace);
        });
        output.textures_delta.clear();
    }
}
