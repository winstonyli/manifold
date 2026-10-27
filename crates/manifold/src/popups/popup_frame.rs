//! The shell and row helpers every popup list shares: the `Area`/`Frame`, list rows and their arrow-key navigation.

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

/// The room `build_list_popup_frame` leaves above and below a popup that is as tall as the window.
const POPUP_WINDOW_MARGIN: f32 = 8.0;

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
#[allow(clippy::too_many_arguments, reason = "egui builder: each is a plain knob")]
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
    let area = egui::Area::new(egui::Id::new(id)).fixed_pos(anchor_rect.left_bottom()).interactable(interactable);
    menu_popup(ctx, area, fade, |ui| {
        ui.set_min_width(min_width);
        // An infinite cap makes egui lay out NaN rects; no cap
        // lets the area size to its contents.
        if max_width.is_finite() {
            ui.set_max_width(max_width);
        }
        // A list taller than the window scrolls instead of running off it.
        let fit = ctx.content_rect().height() - 2.0 * POPUP_WINDOW_MARGIN;
        // The area lays its contents out inside its last size: without this a list that grew since (another page
        // of the same menu) stays clipped to the old height.
        ui.set_max_height(fit.max(120.0));
        // A solid scrollbar, not the floating one that hides until hovered: it says there is more below.
        ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
        egui::ScrollArea::vertical().max_height(fit.max(120.0)).show(ui, add_contents);
        list_row_nav(ui, interactable);
    })
    .rect
}

/// How far a menu-like popup keeps from the window's edges when its anchor is near one.
const POPUP_EDGE_MARGIN: f32 = 6.0;

/// The shell of every menu-like popup: `area` (the caller sets its id, position and `interactable`) on the foreground
/// layer at opacity `fade`, in the popup frame with menu item styling. Returns the area's response, for its rect.
pub(crate) fn menu_popup(
    ctx: &egui::Context,
    area: egui::Area,
    fade: f32,
    add_contents: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    area.order(egui::Order::Foreground)
        .constrain_to(ctx.content_rect().shrink(POPUP_EDGE_MARGIN))
        .show(ctx, |ui| {
            ui.set_opacity(fade);
            popup_frame(ui).show(ui, |ui| {
                menu_item_style(ui);
                add_contents(ui)
            });
        })
        .response
}

fn list_rows_key(ui: &egui::Ui) -> egui::Id {
    ui.layer_id().id.with("list_rows")
}

/// Marks this frame's list rows as a window of a longer list: `list_row_nav`
/// then stops at the ends instead of wrapping to the wrong row.
pub(super) fn mark_list_partial(ui: &egui::Ui) {
    ui.data_mut(|d| d.insert_temp(list_rows_key(ui).with("partial"), true));
}

/// A toolbar list dropdown (`which`: Recently Closed, Bookmarks, History, Downloads) in `build_list_popup_frame`'s
/// shell, open while `open_popup` is `which`. Returns whether a click landed outside both it and its anchor; the
/// caller closes the popup then (`close_popup`), after applying what the click on a row asked for.
pub(crate) fn list_popup(
    ui: &egui::Ui,
    open_popup: Option<AnchoredPopup>,
    which: AnchoredPopup,
    id: &str,
    anchor_rect: egui::Rect,
    fade: f32,
    add_contents: impl FnOnce(&mut egui::Ui),
) -> bool {
    let area_rect =
        build_list_popup_frame(ui.ctx(), id, anchor_rect, 220.0, 320.0, fade, open_popup == Some(which), add_contents);
    clicked_outside_popup(ui, area_rect, Some(anchor_rect))
}

/// The end of an anchored popup's frame, whichever shell drew it: closes `which` when it is `settled` (a row applied
/// its action, Escape, nothing left to pick) or `clicked_outside` it (the caller folds in any reason to ignore that
/// click). Returns whether it closed.
pub(crate) fn settle_popup(
    open_popup: &mut Option<AnchoredPopup>,
    which: AnchoredPopup,
    settled: bool,
    clicked_outside: bool,
) -> bool {
    let close = settled || clicked_outside;
    if close {
        close_popup(open_popup, which);
    }
    close
}

/// A clickable text row in a list popup (`build_list_popup_frame`). Keyboard-focusable: a focused
/// row shows `focus_ring`, Enter clicks it, and the popup's Up/Down move between rows. A screen reader hears a button.
pub(crate) fn list_row(ui: &mut egui::Ui, text: impl Into<egui::WidgetText>) -> egui::Response {
    let text = text.into();
    let name = text.text().to_owned();
    let response = ui
        .add(egui::Label::new(text).truncate().sense(egui::Sense::click()))
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), &name));
    nav_row(ui, &response);
    response
}

/// Makes `response` a row of its list's keyboard navigation (`list_row_nav`, `step_focused_row`): a focused row shows
/// `focus_ring`, is scrolled into view and keeps Up/Down (a popup's also Tab). Call it for every row, in order.
pub(crate) fn nav_row(ui: &egui::Ui, response: &egui::Response) {
    if response.has_focus() {
        focus_ring(ui, response);
        response.scroll_to_me(None);
    }
    nav_stop(ui, std::slice::from_ref(&response.id));
}

/// One stop of the list's Up/Down navigation made of several widgets (a row of swatches): any of `ids` focused is at
/// the stop, Up/Down leave it, and entering it from a neighbour lands on its first widget. Left/Right inside it are
/// egui's own. The caller draws the focus rings.
pub(crate) fn nav_stop(ui: &egui::Ui, ids: &[egui::Id]) {
    lock_focused(ui, ids, egui::EventFilter { vertical_arrows: true, ..Default::default() });
    let key = list_rows_key(ui);
    ui.data_mut(|d| d.get_temp_mut_or_default::<Vec<Vec<egui::Id>>>(key).push(ids.to_vec()));
}

/// Names the page the popup shows now (call it before the rows): when it changes, the popup counts as just opened, so
/// `list_row_nav` focuses its first (or `nav_row_opens_on`) row. The row that had focus went away with the old page.
pub(crate) fn nav_page(ui: &egui::Ui, page: egui::Id) {
    ui.data_mut(|d| d.insert_temp(list_rows_key(ui).with("page"), Some(page)));
}

/// Makes `response` the row `list_row_nav` focuses when the popup opens, instead of the first.
pub(crate) fn nav_row_opens_on(ui: &egui::Ui, response: &egui::Response) {
    ui.data_mut(|d| d.insert_temp(list_rows_key(ui).with("opens_on"), Some(response.id)));
}

/// Whether a list popup drawn in pass `pass` was just opened, given the pass of its last interactable draw: unless that
/// was the pass just before, it was closed in between. A popup closed and reopened without a drawn non-interactable
/// pass between (no fade-out frame, as at a fast frame cadence) still counts.
fn reopened(last_open_pass: Option<u64>, pass: u64) -> bool {
    last_open_pass.is_none_or(|p| p + 1 < pass)
}

/// Whether the popup keyed `key` was just opened this pass (call once per pass, drawn or not): remembers the pass of its
/// last interactable draw in `ctx`'s temp data.
pub(crate) fn popup_just_opened(ctx: &egui::Context, key: egui::Id, interactable: bool) -> bool {
    let pass = ctx.cumulative_pass_nr();
    let just_opened = interactable && reopened(ctx.data(|d| d.get_temp::<u64>(key)), pass);
    ctx.data_mut(|d| {
        if interactable {
            d.insert_temp(key, pass);
        } else {
            d.remove_temp::<u64>(key);
        }
    });
    just_opened
}

/// The rows a layer's widgets queued this pass (`nav_row`, `nav_stop`), with the popup's marks.
pub(crate) struct ListRows {
    pub(crate) rows: Vec<Vec<egui::Id>>,
    opens_on: Option<egui::Id>,
    /// The rows are only a window of the list (`mark_list_partial`): stepping stops at the ends.
    pub(crate) partial: bool,
    page: Option<egui::Id>,
}

/// Reads and clears what this layer's rows queued this pass. Call once per pass, after the rows.
pub(crate) fn take_list_rows(ui: &egui::Ui) -> ListRows {
    let key = list_rows_key(ui);
    ListRows {
        rows: ui.data_mut(|d| d.remove_temp(key)).unwrap_or_default(),
        opens_on: ui.data_mut(|d| d.remove_temp::<Option<egui::Id>>(key.with("opens_on"))).flatten(),
        partial: ui.data_mut(|d| d.remove_temp::<bool>(key.with("partial"))).unwrap_or(false),
        page: ui.data_mut(|d| d.remove_temp::<Option<egui::Id>>(key.with("page"))).flatten(),
    }
}

/// Sets `filter` on whichever of `ids` has focus, so egui leaves those keys to the list.
fn lock_focused(ui: &egui::Ui, ids: &[egui::Id], filter: egui::EventFilter) {
    ui.memory_mut(|m| {
        for id in ids {
            if m.has_focus(*id) {
                m.set_focus_lock_filter(*id, filter);
            }
        }
    });
}

/// Which keys a list's focused row steps with.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum RowKeys {
    /// Up/Down; Tab is egui's, leaving the list (the card pane, spec R5).
    Arrows,
    /// Up/Down, and Tab/Shift+Tab as Down/Up, so Tab stays in the popup.
    ArrowsAndTab,
}

/// While one of `rows` has focus, `keys` move it (wrapping unless `partial`), every press counted; returns whether
/// one had focus. With none focused it leaves the keys alone: the card pane's rows (spec R5) use only this.
pub(crate) fn step_focused_row(ui: &egui::Ui, rows: &[Vec<egui::Id>], partial: bool, keys: RowKeys) -> bool {
    let Some(at) = rows.iter().position(|stop| stop.iter().any(|id| ui.memory(|m| m.has_focus(*id)))) else {
        return false;
    };
    let tab = keys == RowKeys::ArrowsAndTab;
    if tab {
        lock_focused(ui, &rows[at], egui::EventFilter { tab: true, vertical_arrows: true, ..Default::default() });
    }
    // Presses that land in one frame (a slow frame, a key repeat) are each a step. Shift+Tab is counted first: a plain
    // Tab pattern would match it too.
    let (downs, ups) = ui.input_mut(|i| {
        let (back, forth) = match tab {
            true => (
                i.count_and_consume_key(egui::Modifiers::SHIFT, egui::Key::Tab),
                i.count_and_consume_key(egui::Modifiers::NONE, egui::Key::Tab),
            ),
            false => (0, 0),
        };
        (
            forth + i.count_and_consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
            back + i.count_and_consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
        )
    });
    // Wraps at the ends, unless the rows are only a window of the list (`mark_list_partial`).
    let next = step_index(at, downs as isize - ups as isize, rows.len(), !partial);
    if next != at {
        ui.memory_mut(|m| {
            // A row focused last pass is not locked yet (`nav_stop` locks it once drawn focused), so egui would also
            // move it by its own arrow search at the end of this pass.
            m.move_focus(egui::FocusDirection::None);
            m.request_focus(rows[next][0]);
        });
    }
    true
}

/// After a list popup's rows are laid out: focuses the first row (or the `nav_row_opens_on` one) when the popup opens, and moves
/// focus between rows on Up/Down (wrapping, unless the rows are a window: `mark_list_partial`).
pub(crate) fn list_row_nav(ui: &mut egui::Ui, interactable: bool) {
    let key = list_rows_key(ui);
    let ListRows { rows, opens_on, partial, page } = take_list_rows(ui);
    let page_changed = ui.data_mut(|d| {
        let last = d.get_temp::<Option<egui::Id>>(key.with("last_page"));
        d.insert_temp(key.with("last_page"), page);
        interactable && last.is_some_and(|last| last != page)
    });
    let just_opened = popup_just_opened(ui.ctx(), key.with("was_open"), interactable) || page_changed;
    if !interactable || rows.is_empty() {
        return;
    }
    if just_opened {
        ui.memory_mut(|m| m.request_focus(opens_on.unwrap_or(rows[0][0])));
        return;
    }
    if step_focused_row(ui, &rows, partial, RowKeys::ArrowsAndTab) {
        return;
    }
    // Nothing focused (the focused row went away with a page swap, as in the node menu's Color page, or the popup
    // opened without focus): Down enters at the first row, Up at the last.
    let (down, up) = ui.input_mut(|i| {
        (
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
        )
    });
    if down || up {
        let entry = if down { &rows[0] } else { &rows[rows.len() - 1] };
        // egui's own arrow-key focus search would otherwise pick a widget at the end of the pass.
        ui.memory_mut(|m| {
            m.move_focus(egui::FocusDirection::None);
            m.request_focus(entry[0]);
        });
    }
}

/// Net Down presses minus Up presses this frame, counting every press: two that land in one frame (a slow frame, a
/// key repeat) are two steps. Doesn't consume the keys; the one-index lists (quick switcher, address bar) read this.
pub(crate) fn arrow_delta(input: &egui::InputState) -> isize {
    input.num_presses(egui::Key::ArrowDown) as isize - input.num_presses(egui::Key::ArrowUp) as isize
}

/// `at` moved by `delta` in a list of `len` (> 0): wrapping at the ends, or stopping there.
pub(crate) fn step_index(at: usize, delta: isize, len: usize, wrap: bool) -> usize {
    let to = at as isize + delta;
    if wrap {
        to.rem_euclid(len as isize) as usize
    } else {
        to.clamp(0, len as isize - 1) as usize
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_list_popup_counts_as_reopened_after_any_gap_in_its_passes() {
        assert!(super::reopened(None, 5), "never drawn");
        assert!(!super::reopened(Some(4), 5), "drawn in the pass just before");
        assert!(super::reopened(Some(3), 5), "skipped a pass: closed and reopened with no fade-out pass between");
    }
}
