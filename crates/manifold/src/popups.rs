//! The anchored toolbar popups (recently closed, bookmarks, history,
//! downloads, tab actions) and the floating windows: find bar, quick
//! switcher, shortcuts help and import confirm.

use super::*;
use crate::store::model::Kind;

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

/// Marks this frame's list rows as a window of a longer list: `list_row_nav`
/// then stops at the ends instead of wrapping to the wrong row.
fn mark_list_partial(ui: &egui::Ui) {
    ui.data_mut(|d| d.insert_temp(list_rows_key(ui).with("partial"), true));
}

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

/// A closed group's member count as its row shows it, after the name.
fn member_count_text(members: usize) -> String {
    format!("({members})")
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
/// focus between rows on Up/Down (wrapping, unless the rows are a window: `mark_list_partial`).
fn list_row_nav(ui: &mut egui::Ui, interactable: bool) {
    let key = list_rows_key(ui);
    let rows: Vec<egui::Id> = ui.data_mut(|d| d.remove_temp(key)).unwrap_or_default();
    let partial = ui.data_mut(|d| d.remove_temp::<bool>(key.with("partial"))).unwrap_or(false);
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
    // Wraps at the ends, unless the rows are only a window of the list (`mark_list_partial`).
    let next = if down {
        if at + 1 < rows.len() {
            Some(at + 1)
        } else if partial {
            None
        } else {
            Some(0)
        }
    } else if up {
        if at > 0 {
            Some(at - 1)
        } else if partial {
            None
        } else {
            Some(rows.len() - 1)
        }
    } else {
        None
    };
    if let Some(next) = next {
        ui.memory_mut(|m| m.request_focus(rows[next]));
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

/// A centred confirm dialog: `title`, then `detail` (the first line is the explanation, any later line a warning, as
/// `download_detail` writes), then `button` and Cancel. `Some(true)` on the button, `Some(false)` on Cancel or
/// Escape, `None` while it waits.
fn confirm_dialog(ui: &mut egui::Ui, area_id: &str, title: String, detail: &str, button: &str) -> Option<bool> {
    let (mut yes, mut no) = (false, false);
    egui::Area::new(egui::Id::new(area_id))
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            dialog_frame(ui).show(ui, |ui| {
                control_font(ui);
                ui.set_min_width(240.0);
                ui.label(heading_text(title));
                ui.add_space(4.0);
                for (i, line) in detail.lines().enumerate() {
                    let text = egui::RichText::new(line).small();
                    let text = if i == 0 { text.weak() } else { text.color(crate::theme::ACCENT) };
                    ui.add(egui::Label::new(text).wrap());
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    yes = ui.button(button).clicked();
                    no = ui.button("Cancel").clicked();
                });
            });
        });
    if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        no = true;
    }
    if yes {
        Some(true)
    } else if no {
        Some(false)
    } else {
        None
    }
}

/// Opens `confirm` in `slot` only while no confirm dialog is up, `slot`'s own or the other kind's (`other_open`):
/// `confirm_dialog` is non-modal and centred, so a second would replace the first unanswered or draw over it. A click
/// that would open one meanwhile is ignored. Returns whether it opened.
pub(crate) fn offer_confirm<T>(slot: &mut Option<T>, other_open: bool, confirm: T) -> bool {
    let free = slot.is_none() && !other_open;
    if free {
        *slot = Some(confirm);
    }
    free
}

/// The download dialog's detail text, with a warning line when system libraries the host needs are missing.
fn download_detail(missing: &[&str]) -> String {
    let mut detail = "Downloaded once, checked against a pinned hash, then kept in your profile.".to_string();
    if !missing.is_empty() {
        detail.push_str(&format!(
            "
Missing system libraries: {}. Install them with your package manager, or CEF will not start.",
            missing.join(", ")
        ));
    }
    detail
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
        let states: Vec<(EngineKind, crate::engine_install::EngineState)> =
            ENGINE_PICKER_ORDER.into_iter().map(|kind| (kind, self.engine_state(kind))).collect();
        let state_of = |kind: EngineKind| {
            states
                .iter()
                .find(|(k, _)| *k == kind)
                .map(|(_, s)| s.clone())
                .unwrap_or(crate::engine_install::EngineState::Installed)
        };
        let (area_rect, picked) =
            build_engine_picker_popup(ui.ctx(), &format!("{which:?}_popup"), rect, highlighted, &state_of, fade, open);
        let escape = open && ui.input(|i| i.key_pressed(egui::Key::Escape));
        let outcome = picked.map(|kind| pick_outcome(kind, &state_of(kind)));
        // A click on an unavailable engine only explains: the picker stays open.
        let explains_only = matches!(outcome, Some(PickOutcome::Unavailable(..)));
        if (picked.is_some() && !explains_only) || escape || clicked_outside_popup(ui, area_rect, Some(rect)) {
            close_popup(&mut self.open_popup, which);
        }
        // An engine that isn't installed stays selectable, but choosing it asks to install it: no
        // silent switch, and no silent fallback to another engine.
        match outcome? {
            PickOutcome::Switch(kind) => Some(kind),
            PickOutcome::AskInstall(kind) => {
                self.offer_engine_confirm(EngineConfirm::Download(kind));
                None
            }
            PickOutcome::AlreadyInstalling(kind) => {
                self.queue_toast(format!("{} is already installing", engine_label(kind)));
                None
            }
            PickOutcome::Unavailable(kind, reason) => {
                self.queue_toast(PickOutcome::unavailable_toast(kind, &reason));
                None
            }
        }
    }

    /// Whether an engine, jar or session-delete confirm dialog is up (`offer_confirm`).
    pub(crate) fn confirm_open(&self) -> bool {
        self.pending_engine_confirm.is_some()
            || self.pending_jar_confirm.is_some()
            || self.pending_session_delete.is_some()
    }

    /// Asks `confirm` unless a confirm is already up (`offer_confirm`).
    pub(crate) fn offer_engine_confirm(&mut self, confirm: EngineConfirm) -> bool {
        let other = self.pending_jar_confirm.is_some() || self.pending_session_delete.is_some();
        offer_confirm(&mut self.pending_engine_confirm, other, confirm)
    }

    /// Asks `confirm` unless a confirm is already up (`offer_confirm`).
    pub(crate) fn offer_jar_confirm(&mut self, confirm: crate::jar_defs::JarConfirm) -> bool {
        let other = self.pending_engine_confirm.is_some() || self.pending_session_delete.is_some();
        offer_confirm(&mut self.pending_jar_confirm, other, confirm)
    }

    /// Asks before deleting a named session (`offer_confirm`).
    pub(crate) fn offer_session_delete(&mut self, name: String) -> bool {
        let other = self.pending_engine_confirm.is_some() || self.pending_jar_confirm.is_some();
        offer_confirm(&mut self.pending_session_delete, other, name)
    }

    /// "Delete session <name>?" for the Settings session list's X.
    pub(crate) fn build_session_delete_popup(&mut self, ui: &mut egui::Ui) {
        let Some(name) = self.pending_session_delete.clone() else { return };
        let shown = crate::names::to_plain(&crate::names::display_name(&name, crate::names::UNTITLED));
        let title = format!("Delete session {shown}?");
        match confirm_dialog(
            ui,
            "session_delete_popup",
            title,
            "Deletes the saved session and its copies from disk.",
            "Delete",
        ) {
            Some(true) => {
                self.pending_session_delete = None;
                session::delete_named(&name);
                crate::settings_page::forget_disk_cache(ui.ctx());
            }
            Some(false) => self.pending_session_delete = None,
            None => {}
        }
    }

    /// The confirm before a large or destructive engine action: "Download CEF, 172 MB?" (always
    /// asked first, since it is a big network action) or removing the downloaded runtime.
    pub(crate) fn build_engine_confirm_popup(&mut self, ui: &mut egui::Ui) {
        let Some(confirm) = self.pending_engine_confirm else { return };
        let (title, detail, button) = match confirm {
            EngineConfirm::Download(kind) => {
                let size = crate::engine_info::megabytes(crate::cef_pin::pin_for_platform().map_or(0, |pin| pin.size));
                let verb = if matches!(self.engine_state(kind), crate::engine_install::EngineState::UpdateNeeded { .. })
                {
                    "Update"
                } else {
                    "Download"
                };
                let detail = download_detail(&crate::cef_platform::PLATFORM.missing_system_libs());
                (format!("{verb} {}, {size}?", engine_label(kind)), detail, verb)
            }
            EngineConfirm::Remove(kind)
                if !crate::process_manager::engine_hosts_dir().is_some_and(|d| d.join("cef-runtime").exists()) =>
            {
                (
                    format!("Clear {} leftovers?", engine_label(kind)),
                    "Deletes the partial download and install leftovers.".to_string(),
                    "Clear",
                )
            }
            EngineConfirm::Remove(kind) => (
                format!("Remove {}?", engine_label(kind)),
                "Deletes the downloaded runtime. You can download it again later.".to_string(),
                "Remove",
            ),
        };
        match confirm_dialog(ui, "engine_confirm_popup", title, &detail, button) {
            Some(true) => {
                self.pending_engine_confirm = None;
                match confirm {
                    EngineConfirm::Download(kind) => {
                        self.start_engine_install(kind);
                    }
                    EngineConfirm::Remove(kind) => self.remove_engine(kind),
                }
            }
            Some(false) => self.pending_engine_confirm = None,
            None => {}
        }
    }

    /// The confirm before a Settings jar action that signs tabs out or loses a jar or its saved logins: "Clear cookies
    /// in Work?", "Delete jar Work?" or "Make Work ephemeral?" (`jar_confirm_text`). A jar deleted (or made
    /// ephemeral) on another device meanwhile just drops the question.
    pub(crate) fn build_jar_confirm_popup(&mut self, ui: &mut egui::Ui) {
        let Some(confirm) = self.pending_jar_confirm else { return };
        let Some((title, detail, button)) = self.jar_confirm_text(confirm) else {
            self.pending_jar_confirm = None;
            return;
        };
        match confirm_dialog(ui, "jar_confirm_popup", title, &detail, button) {
            Some(true) => {
                self.pending_jar_confirm = None;
                match confirm {
                    crate::jar_defs::JarConfirm::Clear(key) => self.clear_jar_cookies(key),
                    crate::jar_defs::JarConfirm::Delete(id) => {
                        self.delete_jar(id);
                        self.save_session();
                    }
                    crate::jar_defs::JarConfirm::Ephemeral(id) => {
                        self.set_jar_ephemeral(id, true);
                        self.save_session();
                    }
                }
            }
            Some(false) => self.pending_jar_confirm = None,
            None => {}
        }
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
        let unavailable = matches!(self.engine_state(kind), crate::engine_install::EngineState::Unavailable(_));
        let color = if focused {
            BG
        } else if unavailable {
            TEXT_WEAK
        } else {
            engine_color(kind)
        };
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
        node: &store::json::JsonNode,
        title: Option<&str>,
    ) -> (egui::WidgetText, Option<String>) {
        let str_field = |name: &str| node.fields.get(name).and_then(|v| v.as_str()).unwrap_or_default().to_string();
        if node.kind() == Some(Kind::Group) {
            ui.label(egui_phosphor::light::FOLDER_SIMPLE);
            let name = str_field("name");
            let segs = crate::names::display_name(&name, crate::names::UNTITLED);
            return (crate::names::name_job(ui, "", &segs, "").into(), crate::names::hidden_hint(&segs));
        }
        if node.kind() == Some(Kind::Note) {
            ui.label(egui_phosphor::light::NOTE_PENCIL);
            // The note's text is never cleaned; its first non-blank line is shown by the display rule.
            let text = str_field("text");
            let first = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default();
            let segs = crate::names::display_name(first, crate::names::UNTITLED);
            return (crate::names::name_job(ui, "", &segs, "").into(), crate::names::hidden_hint(&segs));
        }
        let text: String = match node.kind() {
            Some(Kind::Tab) => {
                engine_dot(ui, EngineKind::from_name(&str_field("engine")).unwrap_or(EngineKind::Servo), 8.0);
                title.map_or_else(|| str_field("url"), str::to_string)
            }
            Some(Kind::Image) => {
                ui.label(egui_phosphor::light::IMAGE);
                str_field("path")
            }
            Some(Kind::Pdf) => {
                ui.label(egui_phosphor::light::FILE_PDF);
                str_field("path")
            }
            _ => {
                // Json / JsonLeaf, and anything a newer build added.
                ui.label(egui_phosphor::light::BRACKETS_CURLY);
                str_field("path")
            }
        };
        (text.into(), None)
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
                        let mut row_ui = |ui: &mut egui::Ui| {
                            let (label, hint) = Self::closed_entry_label(ui, root, title);
                            let mut row = list_row(ui, label);
                            if let Some(hint) = hint {
                                row = row.on_hover_text(hint);
                            }
                            if row.clicked() {
                                reopen_index = Some(index);
                            }
                        };
                        if root.kind() == Some(Kind::Group) {
                            // The count is laid out first, at the right, so a name cut short keeps it.
                            let count = member_count_text(entry.doc.nodes.len() - 1);
                            egui::Sides::new().shrink_left().truncate().show(ui, row_ui, |ui| ui.weak(count));
                        } else {
                            row_ui(ui);
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
                            let segs = crate::names::display_name(name, crate::names::UNTITLED);
                            // One line, cut with "…" at the popup's width (egui's hover then shows it all).
                            let header = ui.add(egui::Label::new(folder_job(ui, &segs, TEXT_PRIMARY)).truncate());
                            if let Some(hint) = crate::names::hidden_hint(&segs) {
                                header.on_hover_text(hint);
                            }
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
                if let Some(bookmark) = self.bookmarks.iter_mut().find(|b| b.url == url) {
                    bookmark.folder = folder_from_draft(&draft);
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
                let key_ok = matches!(&self.history_matches_cache,
                    Some((f, n, _)) if *f == self.history_filter && *n == self.history.len());
                if !key_ok {
                    let matches = history_matches(&self.history, &self.history_filter);
                    self.history_matches_cache = Some((self.history_filter.clone(), self.history.len(), matches));
                }
                let matches = self.history_matches_cache.as_ref().map_or(&[][..], |(_, _, m)| m.as_slice());
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
                    if ui.scope_builder(egui::UiBuilder::new().id(id), |ui| list_row(ui, label)).inner.clicked() {
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
            self.history_matches_cache = None;
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
                        if node.failed {
                            ui.colored_label(DANGER_COLOR, "Failed");
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
        let focused_pinned = self.mirror.pinned(focused_id);
        let tab_pin_states: Vec<bool> = self
            .nodes
            .iter()
            .filter(|(_, n)| n.runtime.saved_tab().is_some())
            .map(|(id, _)| self.mirror.pinned(id))
            .collect();
        let all_pinned = !tab_pin_states.is_empty() && tab_pin_states.iter().all(|&p| p);
        let area = egui::Area::new(egui::Id::new("tab_actions_popup"))
            .fixed_pos(anchor_rect.left_bottom())
            .order(egui::Order::Foreground)
            .interactable(self.open_popup == Some(AnchoredPopup::TabActions))
            .show(ui.ctx(), |ui| {
                ui.set_opacity(fade);
                popup_frame(ui).show(ui, |ui| {
                    menu_item_style(ui);
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
                            .add(
                                egui::Button::new(label)
                                    .shortcut_text(keys)
                                    .min_size(egui::vec2(ui.min_rect().width(), 0.0)),
                            )
                            .clicked()
                        {
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
                    control_font(ui);
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
        let (title, url) = self.nodes.get(id)?.runtime.listed_tab()?;
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
                self.nodes.get(id.as_str()).and_then(|n| n.runtime.listed_tab()).is_some_and(|(title, url)| {
                    query.is_empty() || title.to_lowercase().contains(&query) || url.to_lowercase().contains(&query)
                })
            })
            .cloned()
            .collect()
    }

    /// Opens the quick switcher, empty, with its field focused (Ctrl+K and `QUICKSWITCHOPEN`).
    pub(crate) fn open_quick_switcher(&mut self) {
        self.quick_switcher = Some(QuickSwitcher { query: String::new(), selected: 0 });
        self.pending_focus = Some(FocusTarget::QuickSwitcher);
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
        // Clamped: a query narrowed after moving down can leave `selected` past the end.
        if let Some(id) = matches.get(switcher.selected.min(matches.len().saturating_sub(1))).cloned() {
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
        let matches = self.quick_switcher_matches(&query);
        // The row Enter would pick (`quick_switcher_confirm` clamps the same way).
        let selected_index = switcher.selected.min(matches.len().saturating_sub(1));
        let mut jump_to: Option<String> = None;

        egui::Area::new(egui::Id::new("quick_switcher"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 80.0))
            .order(egui::Order::Foreground)
            .interactable(is_live)
            .show(ui.ctx(), |ui| {
                ui.set_opacity(fade);
                dialog_frame(ui).show(ui, |ui| {
                    control_font(ui);
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
                            if response.changed() {
                                switcher.selected = 0; // a new query starts at its first match
                            }
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
            ("Hold Ctrl", "Reveal the card numbers and the Ctrl shortcuts (after a short hold, with no other key)"),
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
                    control_font(ui);
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
        let incoming = json.nodes.iter().filter(|n| n.kind() == Some(Kind::Tab)).count();
        let current = self.nodes.len();
        let mut confirm = false;
        let mut cancel = false;
        egui::Area::new(egui::Id::new("import_confirm_popup"))
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                dialog_frame(ui).show(ui, |ui| {
                    control_font(ui);
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
                self.replace_running_session(&json, crate::store::json::Trust::Foreign);
            }
        }
        if cancel {
            self.pending_import = None;
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn download_detail_warns_only_when_libraries_are_missing() {
        assert!(!super::download_detail(&[]).contains("Missing"));
        let d = super::download_detail(&["libnss3.so", "libnspr4.so"]);
        assert!(d.contains("Missing system libraries: libnss3.so, libnspr4.so."), "{d}");
    }

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
            let (label, hint) = App::closed_entry_label(ui, &node, None);
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
            let (label, hint) = App::closed_entry_label(ui, &node, None);
            assert_eq!((label.text(), hint), ("https://example.org/", None));
        });
        output.textures_delta.clear();
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
            let (label, hint) = App::closed_entry_label(ui, &node, None);
            assert_eq!(label.text(), "Re⟦200B⟧ading list");
            assert_eq!(hint.as_deref(), Some("Hidden characters: U+200B zero width space"));
        });
        output.textures_delta.clear();
    }

    /// Name rules: a long bookmark folder name is one line cut with "…" at the popup's width, not wrapped, both as
    /// a header and as the "click to set folder" label under a bookmark.
    #[test]
    fn a_long_bookmark_folder_header_is_one_cut_line() {
        let ctx = egui::Context::default();
        theme::apply_fonts(&ctx);
        theme::apply_style(&ctx);
        let mut app = App::default();
        let folder = format!("w{}", " Wardrobe".repeat(7));
        let bookmark = |title: &str| bookmarks::Bookmark {
            title: title.into(),
            url: format!("https://example.org/{title}"),
            folder: Some(folder.clone()),
        };
        app.bookmarks = vec![bookmark("a")];
        app.open_popup = Some(AnchoredPopup::Bookmarks);
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
                    egui::Shape::Text(t) if t.galley.job.text == folder => Some(t.galley),
                    _ => None,
                })
                .collect();
        }
        let weak = ctx.global_style().visuals.weak_text_color();
        for (what, color) in [("header", TEXT_PRIMARY), ("label", weak)] {
            let galley = galleys.iter().find(|g| g.job.sections[0].format.color == color);
            let galley = galley.unwrap_or_else(|| panic!("the folder {what} is drawn"));
            assert_eq!(galley.rows.len(), 1, "{what}: one line: {:?}", galley.text());
            assert!(galley.elided, "{what}: cut with an ellipsis: {:?}", galley.text());
        }
    }

    /// Name rules: a bookmark folder's name is drawn in the popup's small font, a hidden character as a placeholder.
    #[test]
    fn a_bookmark_folder_shows_a_hidden_character_as_a_placeholder() {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let segs = crate::names::display_name("Re\u{200B}ading", crate::names::UNTITLED);
            let job = folder_job(ui, &segs, TEXT_PRIMARY);
            assert_eq!(job.text, "Re⟦200B⟧ading");
            assert_eq!(job.sections[0].format.font_id, egui::TextStyle::Small.resolve(ui.style()));
            assert_eq!(job.sections[0].format.color, TEXT_PRIMARY);
            assert_eq!(job.sections[1].format.font_id.family, egui::FontFamily::Monospace);
        });
        output.textures_delta.clear();
    }

    /// Fix batch B5: a confirm opens only into an empty slot with the other kind's closed; it never replaces one.
    #[test]
    fn a_confirm_opens_only_while_none_is_up() {
        let mut slot = None;
        assert!(!offer_confirm(&mut slot, true, 1), "the other kind is up");
        assert_eq!(slot, None);
        assert!(offer_confirm(&mut slot, false, 1));
        assert!(!offer_confirm(&mut slot, false, 2), "its own kind is up");
        assert_eq!(slot, Some(1));
    }

    #[test]
    fn a_session_delete_waits_for_a_free_confirm_slot() {
        let mut app = App {
            pending_jar_confirm: Some(crate::jar_defs::JarConfirm::Clear(crate::jar::JarKey::Default)),
            ..App::default()
        };
        assert!(!app.offer_session_delete("work".into()));
        app.pending_jar_confirm = None;
        assert!(app.offer_session_delete("work".into()));
        assert!(app.confirm_open());
    }

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
        let titled = add(&mut app, Kind::Tab, sleeping_tab("https://a/", "Alpha"));
        let untitled = add(&mut app, Kind::Tab, sleeping_tab("https://b/", ""));
        assert_eq!(app.quick_switcher_row_label(&titled).as_deref(), Some("Alpha"));
        assert_eq!(app.quick_switcher_row_label(&untitled).as_deref(), Some("https://b/"));
    }

    /// Review C-B2: a query narrowed after moving down confirms its first match, not nothing.
    #[test]
    fn a_narrowed_query_confirms_its_first_match() {
        use crate::session_sync::tests::{add, app_on, sleeping_tab};
        let mut app = app_on(&crate::store::testutil::temp_dir("qs-narrowed").join("session.mnf"), 1);
        let ids: Vec<String> = (0..6)
            .map(|i| add(&mut app, Kind::Tab, sleeping_tab(&format!("https://{i}/"), &format!("Tab {i}"))))
            .collect();
        app.open_quick_switcher();
        for _ in 0..5 {
            app.quick_switcher_move(1);
        }
        assert_eq!(app.quick_switcher.as_ref().unwrap().selected, 5);
        app.quick_switcher.as_mut().unwrap().query = "tab 2".into();
        app.quick_switcher_confirm();
        assert!(app.quick_switcher.is_none());
        assert_eq!(app.focused.as_deref(), Some(ids[2].as_str()), "jumped to the one match");

        app.open_quick_switcher();
        app.quick_switcher.as_mut().unwrap().query = "no such tab".into();
        app.quick_switcher_confirm(); // nothing matches: closes, no panic
        assert!(app.quick_switcher.is_none());
        assert_eq!(app.focused.as_deref(), Some(ids[2].as_str()), "focus unchanged");
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

    /// A long History popup lays out only the rows in view. Down walks one entry at a time past the first view
    /// (the list scrolls), and Up walks back one at a time to the newest entry (the list scrolls back), without
    /// wrapping at either end of the laid-out window.
    #[test]
    fn a_long_history_popup_lays_out_only_the_rows_in_view() {
        let ctx = egui::Context::default();
        theme::apply_fonts(&ctx);
        theme::apply_style(&ctx);
        let history = (0..300)
            .map(|i| crate::history::HistoryEntry { title: format!("entry {i}"), url: format!("https://e/{i}") })
            .collect();
        let mut app = App { history, open_popup: Some(AnchoredPopup::History), ..App::default() };
        let anchor = egui::Rect::from_min_size(egui::pos2(100.0, 40.0), egui::vec2(20.0, 20.0));
        // A second a frame, so `scroll_to_me`'s scroll animation finishes between frames.
        let mut time = 0.0;
        // One frame: the entry numbers painted, top to bottom, and the focused row's entry.
        let mut frame = |events: Vec<egui::Event>| -> (Vec<usize>, Option<usize>) {
            time += 1.0;
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0))),
                time: Some(time),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| app.build_history_popup(ui, anchor, 1.0));
            output.textures_delta.clear();
            let rows: Vec<(usize, egui::Pos2)> = output
                .shapes
                .into_iter()
                .filter_map(|s| match s.shape {
                    egui::Shape::Text(t) => {
                        t.galley.job.text.strip_prefix("entry ").and_then(|n| n.parse().ok()).map(|n| (n, t.pos))
                    }
                    _ => None,
                })
                .collect();
            let focused = ctx.memory(|m| m.focused()).and_then(|id| ctx.read_response(id)).map(|r| r.rect);
            let at = focused.and_then(|r| rows.iter().find(|(_, pos)| r.y_range().contains(pos.y + 1.0)).map(|p| p.0));
            (rows.into_iter().map(|(n, _)| n).collect(), at)
        };
        for _ in 0..3 {
            frame(vec![]);
        }
        let (rows, at) = frame(vec![]);
        // Painted rows alone prove nothing (a label out of view is laid out but not painted): count the widgets.
        let widgets = ctx.viewport(|v| v.this_pass.widgets.layers().map(|(_, w)| w.len()).sum::<usize>());
        assert!(widgets < 100, "{widgets} widgets laid out for 300 entries");
        assert_eq!((rows[0], at), (299, Some(299)), "newest first, and focused");
        frame(vec![key(egui::Key::ArrowUp)]);
        assert_eq!(frame(vec![]).1, Some(299), "Up on the first row does not wrap in a windowed list");
        let steps = rows.len() + 5;
        for k in 1..=steps {
            frame(vec![key(egui::Key::ArrowDown)]);
            frame(vec![]); // the focused row scrolls into view on the frame after it takes focus
            let (rows, at) = frame(vec![]);
            assert_eq!(at, Some(299 - k), "Down step {k}: {rows:?}");
        }
        let (rows, _) = frame(vec![]);
        assert!(!rows.contains(&299), "Down walked past the first view: {rows:?}");
        assert!(rows.windows(2).all(|w| w[0] == w[1] + 1), "a contiguous window, newest first: {rows:?}");
        for k in (0..steps).rev() {
            frame(vec![key(egui::Key::ArrowUp)]);
            frame(vec![]); // the focused row scrolls into view on the frame after it takes focus
            let (rows, at) = frame(vec![]);
            assert_eq!(at, Some(299 - k), "Up back to {}: {rows:?}", 299 - k);
        }
        let (rows, at) = frame(vec![]);
        assert_eq!((rows[0], at), (299, Some(299)), "back at the top, scrolled back: {rows:?}");
        assert_eq!(app.open_popup, Some(AnchoredPopup::History));
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

    /// Clicking an unavailable engine in the open picker toasts its reason, switches nothing and leaves the
    /// picker open.
    #[test]
    fn clicking_an_unavailable_engine_explains_and_keeps_the_picker_open() {
        let ctx = egui::Context::default();
        theme::apply_fonts(&ctx);
        theme::apply_style(&ctx);
        let mut app = App { view_mode: ViewMode::Settings, ..App::default() };
        let (first, reason) = (ENGINE_PICKER_ORDER[0], "needs Windows");
        // `build_ui` clears the per-frame state cache, so seed it inside each frame, past that clear.
        let frame = |app: &mut App, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 1000.0))),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                let state = crate::engine_install::EngineState::Unavailable(reason.to_string());
                app.engine_state_cache.borrow_mut().insert(first, state);
                app.build_ui_inner(ui);
            });
            output.textures_delta.clear();
        };
        let before = app.settings.default_starting_engine;
        app.open_popup = Some(AnchoredPopup::DefaultEngine);
        for _ in 0..4 {
            frame(&mut app, vec![]);
        }
        let area = ctx.memory(|m| m.area_rect(egui::Id::new("DefaultEngine_popup"))).expect("the picker is drawn");
        let at = area.min + egui::vec2(120.0, 30.0); // inside the first card
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&mut app, vec![egui::Event::PointerMoved(at)]);
        frame(&mut app, vec![button(true)]);
        frame(&mut app, vec![button(false)]);
        let toasted = |text: &str| {
            app.save_toast.iter().map(|(t, _)| t).chain(app.save_toast_queue.iter()).any(|t| t.contains(text))
        };
        assert!(toasted(reason), "the reason should be toasted");
        assert_eq!(app.open_popup, Some(AnchoredPopup::DefaultEngine), "the picker stays open");
        assert_eq!(app.settings.default_starting_engine, before, "nothing switched");
    }
}
